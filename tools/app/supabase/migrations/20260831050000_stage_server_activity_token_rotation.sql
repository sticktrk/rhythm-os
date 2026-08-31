-- Make server-activity credential rotation an acknowledged handoff.
--
-- A bootstrap response crosses cloud -> app -> appliance before the new
-- credential can be proven. The previous single-active-token index forced the
-- cloud function to revoke the working token first, so an interrupted or
-- competing handoff left the appliance holding a revoked credential. Stage
-- replacements briefly and promote them only when ingest sees the secret.

ALTER TABLE public.server_light_activity_device_tokens
  ADD COLUMN IF NOT EXISTS activated_at TIMESTAMPTZ;

ALTER TABLE public.server_light_activity_device_tokens
  ADD COLUMN IF NOT EXISTS activation_expires_at TIMESTAMPTZ
    DEFAULT (NOW() + INTERVAL '15 minutes');

-- Every pre-migration unrevoked credential has already been the active token
-- under the old unique index, so preserve it as proven during the rollout.
UPDATE public.server_light_activity_device_tokens
SET
  activated_at = COALESCE(last_used_at, created_at, NOW()),
  activation_expires_at = NULL
WHERE revoked_at IS NULL
  AND activated_at IS NULL;

DROP INDEX IF EXISTS public.server_light_activity_device_tokens_active_hub_idx;

CREATE INDEX IF NOT EXISTS server_light_activity_device_tokens_pending_hub_idx
  ON public.server_light_activity_device_tokens (hub_id, activation_expires_at)
  WHERE revoked_at IS NULL AND activated_at IS NULL;

ALTER TABLE public.server_light_activity_device_tokens
  DROP CONSTRAINT IF EXISTS server_light_activity_device_tokens_activation_state_check;

ALTER TABLE public.server_light_activity_device_tokens
  ADD CONSTRAINT server_light_activity_device_tokens_activation_state_check
  CHECK (
    (activated_at IS NOT NULL AND activation_expires_at IS NULL)
    OR (activated_at IS NULL AND activation_expires_at IS NOT NULL)
  );

CREATE OR REPLACE FUNCTION public.activate_server_light_activity_device_token(
  token_uuid UUID
)
RETURNS BOOLEAN
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
  token_record public.server_light_activity_device_tokens%ROWTYPE;
  activation_time TIMESTAMPTZ := NOW();
BEGIN
  SELECT * INTO token_record
  FROM public.server_light_activity_device_tokens
  WHERE id = token_uuid
    AND revoked_at IS NULL
  FOR UPDATE;

  IF NOT FOUND THEN
    RETURN FALSE;
  END IF;

  IF token_record.activated_at IS NOT NULL THEN
    RETURN TRUE;
  END IF;

  IF token_record.activation_expires_at IS NULL
    OR token_record.activation_expires_at <= activation_time
  THEN
    UPDATE public.server_light_activity_device_tokens
    SET revoked_at = activation_time
    WHERE id = token_uuid
      AND revoked_at IS NULL;
    RETURN FALSE;
  END IF;

  UPDATE public.server_light_activity_device_tokens
  SET
    activated_at = activation_time,
    activation_expires_at = NULL,
    last_used_at = activation_time
  WHERE id = token_uuid
    AND revoked_at IS NULL
    AND activated_at IS NULL;

  IF NOT FOUND THEN
    RETURN FALSE;
  END IF;

  -- A proven replacement may retire only credentials that have been unused
  -- for a full month. Recent active tokens intentionally overlap so two valid
  -- recovery uploads cannot revoke each other when their requests reorder.
  UPDATE public.server_light_activity_device_tokens
  SET revoked_at = activation_time
  WHERE hub_id = token_record.hub_id
    AND id <> token_uuid
    AND revoked_at IS NULL
    AND activated_at IS NOT NULL
    AND COALESCE(last_used_at, activated_at, created_at)
      < activation_time - INTERVAL '30 days';

  UPDATE public.server_light_activity_device_tokens
  SET revoked_at = activation_time
  WHERE hub_id = token_record.hub_id
    AND id <> token_uuid
    AND revoked_at IS NULL
    AND activated_at IS NULL
    AND activation_expires_at <= activation_time;

  RETURN TRUE;
END;
$$;

REVOKE ALL ON FUNCTION public.activate_server_light_activity_device_token(UUID)
  FROM PUBLIC;
REVOKE ALL ON FUNCTION public.activate_server_light_activity_device_token(UUID)
  FROM anon;
REVOKE ALL ON FUNCTION public.activate_server_light_activity_device_token(UUID)
  FROM authenticated;
GRANT EXECUTE ON FUNCTION public.activate_server_light_activity_device_token(UUID)
  TO service_role;

COMMENT ON COLUMN public.server_light_activity_device_tokens.activated_at IS
  'First successful ingest proving that a staged credential reached its appliance';
COMMENT ON COLUMN public.server_light_activity_device_tokens.activation_expires_at IS
  'Deadline for a staged replacement to prove possession without revoking the previous credential';
COMMENT ON FUNCTION public.activate_server_light_activity_device_token(UUID) IS
  'Service-role-only promotion of a staged server-activity credential after ingest validates its scope and payload';

-- Home join uses the same device credential as activity ingest. Preserve the
-- existing identity-promotion transaction while recognizing that multiple
-- recent credentials may now legitimately belong to one hub. Revalidate the
-- staged-token deadline under the RPC row lock so the Edge Function check
-- cannot race credential expiry.
CREATE OR REPLACE FUNCTION public.join_verified_device_home_with_identity(
  home_uuid UUID,
  hub_uuid UUID,
  token_uuid UUID,
  user_uuid UUID,
  actor_uuid UUID,
  durable_server_instance_id TEXT
)
RETURNS JSONB
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
  durable_identity TEXT := lower(btrim(durable_server_instance_id));
  home_record public.homes%ROWTYPE;
  hub_record public.hubs%ROWTYPE;
  token_record public.server_light_activity_device_tokens%ROWTYPE;
  remote_identity TEXT;
  joined_home BOOLEAN;
  promoted_identity BOOLEAN;
  old_identity_snapshot JSONB;
BEGIN
  IF actor_uuid IS DISTINCT FROM user_uuid THEN
    RETURN jsonb_build_object(
      'status', 'identity_conflict',
      'reason', 'actor_mismatch'
    );
  END IF;
  IF durable_identity IS NULL OR durable_identity !~ '^srv-[a-z0-9._:-]+$' THEN
    RETURN jsonb_build_object(
      'status', 'identity_conflict',
      'reason', 'candidate_not_durable'
    );
  END IF;

  -- Serialize all promotions of one durable physical identity before taking
  -- row locks, including attempts that currently have no canonical row.
  PERFORM pg_advisory_xact_lock(hashtextextended(durable_identity, 0));

  SELECT * INTO home_record
  FROM public.homes
  WHERE id = home_uuid
  FOR UPDATE;
  IF NOT FOUND THEN
    RETURN jsonb_build_object(
      'status', 'identity_conflict',
      'reason', 'home_missing'
    );
  END IF;

  SELECT * INTO hub_record
  FROM public.hubs
  WHERE id = hub_uuid
    AND home_id = home_uuid
    AND type = 'server'
  FOR UPDATE;
  IF NOT FOUND THEN
    RETURN jsonb_build_object(
      'status', 'identity_conflict',
      'reason', 'hub_missing_or_moved'
    );
  END IF;

  SELECT * INTO token_record
  FROM public.server_light_activity_device_tokens
  WHERE id = token_uuid
    AND home_id = home_uuid
    AND hub_id = hub_uuid
    AND revoked_at IS NULL
    AND (
      activated_at IS NOT NULL
      OR activation_expires_at > NOW()
    )
  FOR UPDATE;
  IF NOT FOUND THEN
    RETURN jsonb_build_object(
      'status', 'identity_conflict',
      'reason', 'active_token_missing_or_moved'
    );
  END IF;
  IF NOT (
    token_record.user_id = ANY(
      COALESCE(home_record.member_ids, ARRAY[]::UUID[])
    )
  ) THEN
    RETURN jsonb_build_object(
      'status', 'identity_conflict',
      'reason', 'token_owner_not_authorized'
    );
  END IF;

  SELECT lower(btrim(server_instance_id)) INTO remote_identity
  FROM public.hub_remote_access
  WHERE hub_id = hub_uuid
  FOR UPDATE;

  IF hub_record.server_instance_id IS NOT NULL
    AND btrim(hub_record.server_instance_id) <> ''
    AND lower(btrim(hub_record.server_instance_id)) <> durable_identity
    AND lower(btrim(hub_record.server_instance_id)) NOT LIKE 'endpoint:%'
  THEN
    RETURN jsonb_build_object(
      'status', 'identity_conflict',
      'reason', 'hub_has_different_durable_identity'
    );
  END IF;
  IF token_record.server_instance_id IS NOT NULL
    AND btrim(token_record.server_instance_id) <> ''
    AND lower(btrim(token_record.server_instance_id)) <> durable_identity
    AND lower(btrim(token_record.server_instance_id)) NOT LIKE 'endpoint:%'
  THEN
    RETURN jsonb_build_object(
      'status', 'identity_conflict',
      'reason', 'token_has_different_durable_identity'
    );
  END IF;
  IF remote_identity IS NOT NULL
    AND remote_identity <> ''
    AND remote_identity <> durable_identity
    AND remote_identity NOT LIKE 'endpoint:%'
  THEN
    RETURN jsonb_build_object(
      'status', 'identity_conflict',
      'reason', 'remote_mapping_has_different_durable_identity'
    );
  END IF;

  IF EXISTS (
    SELECT 1 FROM public.hubs other
    WHERE other.id <> hub_uuid
      AND lower(btrim(other.server_instance_id)) = durable_identity
  ) OR EXISTS (
    SELECT 1 FROM public.hub_remote_access other
    WHERE other.hub_id <> hub_uuid
      AND lower(btrim(other.server_instance_id)) = durable_identity
  ) OR EXISTS (
    SELECT 1 FROM public.server_light_activity_device_tokens other
    WHERE other.hub_id <> hub_uuid
      AND other.revoked_at IS NULL
      AND (
        other.activated_at IS NOT NULL
        OR other.activation_expires_at > NOW()
      )
      AND lower(btrim(other.server_instance_id)) = durable_identity
  ) THEN
    RETURN jsonb_build_object(
      'status', 'identity_conflict',
      'reason', 'durable_identity_owned_elsewhere'
    );
  END IF;

  old_identity_snapshot := jsonb_build_object(
    'hub', hub_record.server_instance_id,
    'remote_access', remote_identity,
    'activity_token', token_record.server_instance_id
  );
  promoted_identity :=
    lower(btrim(COALESCE(hub_record.server_instance_id, ''))) <>
      durable_identity
    OR lower(btrim(COALESCE(token_record.server_instance_id, ''))) <>
      durable_identity
    OR (
      remote_identity IS NOT NULL
      AND remote_identity <> durable_identity
    );
  joined_home := NOT (
    user_uuid = ANY(COALESCE(home_record.member_ids, ARRAY[]::UUID[]))
  );

  UPDATE public.hubs
  SET server_instance_id = durable_identity, updated_at = NOW()
  WHERE id = hub_uuid;

  UPDATE public.hub_remote_access
  SET server_instance_id = durable_identity, updated_at = NOW()
  WHERE hub_id = hub_uuid;

  UPDATE public.server_light_activity_device_tokens
  SET server_instance_id = durable_identity, updated_at = NOW()
  WHERE id = token_uuid;

  UPDATE public.homes
  SET
    member_ids = CASE
      WHEN user_uuid = ANY(COALESCE(member_ids, ARRAY[]::UUID[]))
        THEN member_ids
      ELSE array_append(COALESCE(member_ids, ARRAY[]::UUID[]), user_uuid)
    END,
    updated_at = NOW()
  WHERE id = home_uuid;

  IF promoted_identity THEN
    INSERT INTO public.server_identity_promotion_audit (
      home_id,
      hub_id,
      token_id,
      actor_user_id,
      old_identities,
      new_server_instance_id,
      reason
    ) VALUES (
      home_uuid,
      hub_uuid,
      token_uuid,
      actor_uuid,
      old_identity_snapshot,
      durable_identity,
      'verified_local_device_proof'
    );
  END IF;

  RETURN jsonb_build_object(
    'status', 'ok',
    'home_id', home_uuid,
    'hub_id', hub_uuid,
    'joined', joined_home,
    'promoted', promoted_identity,
    'server_instance_id', durable_identity
  );
END;
$$;

REVOKE ALL ON FUNCTION public.join_verified_device_home_with_identity(
  UUID, UUID, UUID, UUID, UUID, TEXT
) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.join_verified_device_home_with_identity(
  UUID, UUID, UUID, UUID, UUID, TEXT
) FROM anon;
REVOKE ALL ON FUNCTION public.join_verified_device_home_with_identity(
  UUID, UUID, UUID, UUID, UUID, TEXT
) FROM authenticated;
GRANT EXECUTE ON FUNCTION public.join_verified_device_home_with_identity(
  UUID, UUID, UUID, UUID, UUID, TEXT
) TO service_role;

COMMENT ON FUNCTION public.join_verified_device_home_with_identity(
  UUID, UUID, UUID, UUID, UUID, TEXT
) IS
  'Service-role-only Home join and identity promotion using an active or unexpired staged server-activity credential';
