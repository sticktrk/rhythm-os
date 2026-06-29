-- Customer-granted support/admin access for Rhythm Server hubs.
--
-- Raw token material is never exposed through RLS-backed client views. The
-- support-access Edge Function writes encrypted token envelopes with the
-- service role; admin-api reads and decrypts them server-side.

CREATE TABLE IF NOT EXISTS public.hub_support_access_grants (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  hub_id UUID NOT NULL REFERENCES public.hubs(id) ON DELETE CASCADE,
  home_id UUID NOT NULL REFERENCES public.homes(id) ON DELETE CASCADE,
  token_id TEXT NOT NULL,
  label TEXT,
  scope TEXT NOT NULL DEFAULT 'beta_admin'
    CHECK (scope IN ('beta_admin')),
  encrypted_token JSONB NOT NULL,
  key_id TEXT,
  granted_by UUID REFERENCES auth.users(id) ON DELETE SET NULL,
  expires_at TIMESTAMPTZ,
  revoked_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  CHECK (expires_at IS NULL OR expires_at > created_at)
);

CREATE INDEX IF NOT EXISTS hub_support_access_grants_home_id_idx
  ON public.hub_support_access_grants (home_id);

CREATE INDEX IF NOT EXISTS hub_support_access_grants_active_hub_idx
  ON public.hub_support_access_grants (hub_id)
  WHERE revoked_at IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS hub_support_access_grants_one_active_scope_idx
  ON public.hub_support_access_grants (hub_id, scope)
  WHERE revoked_at IS NULL;

DROP TRIGGER IF EXISTS hub_support_access_grants_updated_at
  ON public.hub_support_access_grants;
CREATE TRIGGER hub_support_access_grants_updated_at
  BEFORE UPDATE ON public.hub_support_access_grants
  FOR EACH ROW
  EXECUTE FUNCTION public.update_updated_at_column();

ALTER TABLE public.hub_support_access_grants ENABLE ROW LEVEL SECURITY;

REVOKE ALL ON public.hub_support_access_grants FROM PUBLIC;
REVOKE ALL ON public.hub_support_access_grants FROM anon;
REVOKE ALL ON public.hub_support_access_grants FROM authenticated;

DROP VIEW IF EXISTS public.rhythm_support_access_grants;
CREATE VIEW public.rhythm_support_access_grants
WITH (security_barrier = true)
AS
SELECT
  id,
  hub_id,
  home_id,
  token_id,
  label,
  scope,
  key_id,
  granted_by,
  expires_at,
  revoked_at,
  created_at,
  updated_at,
  encrypted_token IS NOT NULL AS has_encrypted_token
FROM public.hub_support_access_grants
WHERE public.is_rhythm_staff();

REVOKE ALL ON public.rhythm_support_access_grants FROM PUBLIC;
GRANT SELECT ON public.rhythm_support_access_grants TO authenticated;

DROP VIEW IF EXISTS public.user_support_access_grants;
CREATE VIEW public.user_support_access_grants
WITH (security_barrier = true)
AS
SELECT
  grants.id,
  grants.hub_id,
  grants.home_id,
  grants.token_id,
  grants.label,
  grants.scope,
  grants.expires_at,
  grants.revoked_at,
  grants.created_at,
  grants.updated_at
FROM public.hub_support_access_grants grants
WHERE EXISTS (
  SELECT 1
  FROM public.homes homes
  WHERE homes.id = grants.home_id
    AND auth.uid() = ANY(homes.member_ids)
);

REVOKE ALL ON public.user_support_access_grants FROM PUBLIC;
GRANT SELECT ON public.user_support_access_grants TO authenticated;

COMMENT ON TABLE public.hub_support_access_grants IS
  'Encrypted customer-granted support/admin tokens for Rhythm Server hubs';
COMMENT ON COLUMN public.hub_support_access_grants.scope IS
  'beta_admin grants broad beta support/admin permissions enforced by the device support role';
COMMENT ON COLUMN public.hub_support_access_grants.encrypted_token IS
  'AES-GCM encrypted support token envelope for server-side admin-api use only';
COMMENT ON VIEW public.rhythm_support_access_grants IS
  'Sanitized staff view of support access grants without encrypted token material';
COMMENT ON VIEW public.user_support_access_grants IS
  'Sanitized owner/member view of support access grants without encrypted token material';
