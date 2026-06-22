-- Time-boxed employee support access to managed Rhythm appliances.

DO $$ BEGIN
  CREATE TYPE support_access_scope AS ENUM ('full');
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

DO $$ BEGIN
  CREATE TYPE support_access_status AS ENUM ('active', 'expired', 'revoked');
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

ALTER TABLE homes
  ADD COLUMN IF NOT EXISTS support_access_consent_at TIMESTAMPTZ;

CREATE OR REPLACE FUNCTION enforce_support_access_consent_owner()
RETURNS TRIGGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public
AS $$
BEGIN
  IF NEW.support_access_consent_at IS DISTINCT FROM OLD.support_access_consent_at
     AND auth.role() IS DISTINCT FROM 'service_role'
     AND auth.uid() IS DISTINCT FROM OLD.owner_id THEN
    RAISE EXCEPTION 'Only the home owner can update support access consent';
  END IF;

  RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS homes_support_access_consent_owner ON homes;
CREATE TRIGGER homes_support_access_consent_owner
  BEFORE UPDATE OF support_access_consent_at ON homes
  FOR EACH ROW
  EXECUTE FUNCTION enforce_support_access_consent_owner();

CREATE TABLE IF NOT EXISTS hub_support_tokens (
  hub_id UUID PRIMARY KEY REFERENCES hubs(id) ON DELETE CASCADE,
  home_id UUID NOT NULL REFERENCES homes(id) ON DELETE CASCADE,
  token TEXT NOT NULL CHECK (LENGTH(BTRIM(token)) > 0),
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS hub_support_tokens_home_id_idx
  ON hub_support_tokens (home_id);

ALTER TABLE hub_support_tokens ENABLE ROW LEVEL SECURITY;

CREATE TABLE IF NOT EXISTS support_access_grants (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  staff_user_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
  staff_email TEXT NOT NULL,
  hub_id UUID NOT NULL REFERENCES hubs(id) ON DELETE CASCADE,
  home_id UUID NOT NULL REFERENCES homes(id) ON DELETE CASCADE,
  submission_id UUID REFERENCES support_debug_bundle_submissions(id) ON DELETE SET NULL,
  scope support_access_scope NOT NULL DEFAULT 'full',
  status support_access_status NOT NULL DEFAULT 'active',
  reason TEXT,
  hostname TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  expires_at TIMESTAMPTZ NOT NULL,
  last_accessed_at TIMESTAMPTZ,
  revoked_at TIMESTAMPTZ,
  revoked_by UUID REFERENCES auth.users(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS support_access_grants_staff_user_id_idx
  ON support_access_grants (staff_user_id, created_at DESC);

CREATE INDEX IF NOT EXISTS support_access_grants_hub_id_idx
  ON support_access_grants (hub_id, created_at DESC);

CREATE INDEX IF NOT EXISTS support_access_grants_home_id_idx
  ON support_access_grants (home_id, created_at DESC);

CREATE INDEX IF NOT EXISTS support_access_grants_active_expires_idx
  ON support_access_grants (expires_at)
  WHERE status = 'active';

ALTER TABLE support_access_grants ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS "Home members can view support access grants"
  ON support_access_grants;
CREATE POLICY "Home members can view support access grants"
  ON support_access_grants FOR SELECT
  USING (
    EXISTS (
      SELECT 1 FROM homes
      WHERE homes.id = support_access_grants.home_id
        AND auth.uid() = ANY(homes.member_ids)
    )
  );

DROP POLICY IF EXISTS "Staff can view own support access grants"
  ON support_access_grants;
CREATE POLICY "Staff can view own support access grants"
  ON support_access_grants FOR SELECT
  USING (is_staff() AND staff_user_id = auth.uid());

CREATE TABLE IF NOT EXISTS support_access_audit (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  grant_id UUID NOT NULL REFERENCES support_access_grants(id) ON DELETE CASCADE,
  hub_id UUID NOT NULL REFERENCES hubs(id) ON DELETE CASCADE,
  staff_email TEXT NOT NULL,
  action TEXT NOT NULL,
  method TEXT,
  path TEXT,
  status_code INTEGER,
  detail JSONB NOT NULL DEFAULT '{}'::JSONB,
  source_ip TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS support_access_audit_grant_id_idx
  ON support_access_audit (grant_id, created_at DESC);

CREATE INDEX IF NOT EXISTS support_access_audit_hub_id_idx
  ON support_access_audit (hub_id, created_at DESC);

ALTER TABLE support_access_audit ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS "Home members can view support access audit"
  ON support_access_audit;
CREATE POLICY "Home members can view support access audit"
  ON support_access_audit FOR SELECT
  USING (
    EXISTS (
      SELECT 1
      FROM support_access_grants grants
      JOIN homes ON homes.id = grants.home_id
      WHERE grants.id = support_access_audit.grant_id
        AND auth.uid() = ANY(homes.member_ids)
    )
  );

DROP POLICY IF EXISTS "Staff can view own support access audit"
  ON support_access_audit;
CREATE POLICY "Staff can view own support access audit"
  ON support_access_audit FOR SELECT
  USING (
    EXISTS (
      SELECT 1
      FROM support_access_grants grants
      WHERE grants.id = support_access_audit.grant_id
        AND grants.staff_user_id = auth.uid()
        AND is_staff()
    )
  );

DROP TRIGGER IF EXISTS hub_support_tokens_updated_at ON hub_support_tokens;
CREATE TRIGGER hub_support_tokens_updated_at
  BEFORE UPDATE ON hub_support_tokens
  FOR EACH ROW
  EXECUTE FUNCTION update_updated_at_column();

COMMENT ON COLUMN homes.support_access_consent_at IS
  'Timestamp when the owner accepted managed-service blanket support access terms';
COMMENT ON FUNCTION enforce_support_access_consent_owner() IS
  'Prevents non-owner home members from changing managed support access consent';
COMMENT ON TABLE hub_support_tokens IS
  'Service-role-only vault for per-hub support bearer tokens used by support-proxy';
COMMENT ON TABLE support_access_grants IS
  'Time-boxed employee support sessions granted against a managed hub';
COMMENT ON TABLE support_access_audit IS
  'Request-level support access audit log written by support-access and support-proxy';
