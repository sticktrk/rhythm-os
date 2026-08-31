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
