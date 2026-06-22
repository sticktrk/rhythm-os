-- Staff identities for employee-only support workflows.

DO $$ BEGIN
  CREATE TYPE staff_role AS ENUM ('support', 'engineer', 'admin');
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

CREATE TABLE IF NOT EXISTS staff_members (
  user_id UUID PRIMARY KEY REFERENCES auth.users(id) ON DELETE CASCADE,
  email TEXT,
  role staff_role NOT NULL DEFAULT 'support',
  active BOOLEAN NOT NULL DEFAULT TRUE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS staff_members_active_idx
  ON staff_members (active)
  WHERE active = TRUE;

ALTER TABLE staff_members ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS "Staff can read own staff row" ON staff_members;
CREATE POLICY "Staff can read own staff row"
  ON staff_members FOR SELECT
  USING (auth.uid() = user_id);

CREATE OR REPLACE FUNCTION is_staff(uid UUID DEFAULT auth.uid())
RETURNS BOOLEAN
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = public
AS $$
  SELECT EXISTS (
    SELECT 1
    FROM public.staff_members
    WHERE user_id = uid
      AND active = TRUE
  );
$$;

DROP TRIGGER IF EXISTS staff_members_updated_at ON staff_members;
CREATE TRIGGER staff_members_updated_at
  BEFORE UPDATE ON staff_members
  FOR EACH ROW
  EXECUTE FUNCTION update_updated_at_column();

DROP POLICY IF EXISTS "Staff can view debug bundle submissions"
  ON support_debug_bundle_submissions;
CREATE POLICY "Staff can view debug bundle submissions"
  ON support_debug_bundle_submissions FOR SELECT
  USING (is_staff());

DROP POLICY IF EXISTS "Staff can view submitted debug bundles"
  ON storage.objects;
CREATE POLICY "Staff can view submitted debug bundles"
  ON storage.objects FOR SELECT TO authenticated
  USING (
    bucket_id = 'support-debug-bundles'
    AND is_staff()
    AND EXISTS (
      SELECT 1
      FROM support_debug_bundle_submissions submissions
      WHERE submissions.bundle_storage_path = storage.objects.name
    )
  );

COMMENT ON TABLE staff_members IS
  'Service-role managed staff identity allowlist for support workflows';
COMMENT ON FUNCTION is_staff(UUID) IS
  'True when the given Supabase auth user is an active staff member';
