-- Atomically reconcile a provisional Box identity after the appliance proves
-- possession of its active activity-upload credential.

CREATE TABLE IF NOT EXISTS public.server_identity_promotion_audit (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  home_id UUID NOT NULL,
  hub_id UUID NOT NULL,
  token_id UUID NOT NULL,
  actor_user_id UUID NOT NULL,
  old_identities JSONB NOT NULL,
  new_server_instance_id TEXT NOT NULL,
  reason TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS server_identity_promotion_audit_hub_time_idx
  ON public.server_identity_promotion_audit (hub_id, created_at DESC);

ALTER TABLE public.server_identity_promotion_audit ENABLE ROW LEVEL SECURITY;
REVOKE ALL ON public.server_identity_promotion_audit FROM PUBLIC;
REVOKE ALL ON public.server_identity_promotion_audit FROM anon;
REVOKE ALL ON public.server_identity_promotion_audit FROM authenticated;
GRANT SELECT ON public.server_identity_promotion_audit TO authenticated;

DROP POLICY IF EXISTS "Staff can view server identity promotion audit"
  ON public.server_identity_promotion_audit;
CREATE POLICY "Staff can view server identity promotion audit"
  ON public.server_identity_promotion_audit FOR SELECT TO authenticated
  USING (public.is_rhythm_staff());

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
SET search_path = public
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
    WHERE other.id <> token_uuid
      AND other.revoked_at IS NULL
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

COMMENT ON TABLE public.server_identity_promotion_audit IS
  'Staff-visible audit of provisional-to-durable Box identity promotions performed after signed local device proof';
COMMENT ON FUNCTION public.join_verified_device_home_with_identity(
  UUID, UUID, UUID, UUID, UUID, TEXT
) IS
  'Service-role-only atomic identity promotion and Home join after the Edge Function verifies signed local device proof';
