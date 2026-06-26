-- Staff/admin read access for customer support tooling.
--
-- This adds a private allow-list of Rhythm staff users. Staff accounts can
-- read customer records across RLS-protected tables, but this migration does
-- not grant client-side write access to customer data or staff membership.
--
-- Grant access manually with the Supabase service role, for example:
-- INSERT INTO public.rhythm_staff (user_id, role)
-- VALUES ('00000000-0000-0000-0000-000000000000', 'admin')
-- ON CONFLICT (user_id) DO UPDATE
-- SET role = EXCLUDED.role, enabled = TRUE;

CREATE TABLE IF NOT EXISTS public.rhythm_staff (
  user_id UUID PRIMARY KEY REFERENCES auth.users(id) ON DELETE CASCADE,
  role TEXT NOT NULL DEFAULT 'support'
    CHECK (role IN ('support', 'admin')),
  enabled BOOLEAN NOT NULL DEFAULT TRUE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS rhythm_staff_enabled_idx
  ON public.rhythm_staff (enabled)
  WHERE enabled = TRUE;

ALTER TABLE public.rhythm_staff ENABLE ROW LEVEL SECURITY;

GRANT SELECT ON public.rhythm_staff TO authenticated;

CREATE OR REPLACE FUNCTION public.is_rhythm_staff()
RETURNS BOOLEAN
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = public
AS $$
  SELECT EXISTS (
    SELECT 1
    FROM public.rhythm_staff
    WHERE user_id = auth.uid()
      AND enabled = TRUE
  );
$$;

REVOKE ALL ON FUNCTION public.is_rhythm_staff() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.is_rhythm_staff() TO authenticated;

DROP POLICY IF EXISTS "Staff can read own staff status"
  ON public.rhythm_staff;
CREATE POLICY "Staff can read own staff status"
  ON public.rhythm_staff FOR SELECT TO authenticated
  USING (user_id = auth.uid() OR public.is_rhythm_staff());

-- No INSERT/UPDATE/DELETE staff policies are defined on purpose. Staff grants
-- must be managed with the Supabase service role or dashboard.

DROP POLICY IF EXISTS "Staff can view all homes"
  ON public.homes;
CREATE POLICY "Staff can view all homes"
  ON public.homes FOR SELECT TO authenticated
  USING (public.is_rhythm_staff());

DROP POLICY IF EXISTS "Staff can view all hubs"
  ON public.hubs;
CREATE POLICY "Staff can view all hubs"
  ON public.hubs FOR SELECT TO authenticated
  USING (public.is_rhythm_staff());

DROP POLICY IF EXISTS "Staff can view all remote access records"
  ON public.hub_remote_access;
CREATE POLICY "Staff can view all remote access records"
  ON public.hub_remote_access FOR SELECT TO authenticated
  USING (public.is_rhythm_staff());

DROP POLICY IF EXISTS "Staff can view all cloud snapshots"
  ON public.user_cloud_snapshots;
CREATE POLICY "Staff can view all cloud snapshots"
  ON public.user_cloud_snapshots FOR SELECT TO authenticated
  USING (public.is_rhythm_staff());

DROP POLICY IF EXISTS "Staff can view all subscriptions"
  ON public.subscriptions;
CREATE POLICY "Staff can view all subscriptions"
  ON public.subscriptions FOR SELECT TO authenticated
  USING (public.is_rhythm_staff());

DROP POLICY IF EXISTS "Staff can view all debug bundle submissions"
  ON public.support_debug_bundle_submissions;
CREATE POLICY "Staff can view all debug bundle submissions"
  ON public.support_debug_bundle_submissions FOR SELECT TO authenticated
  USING (public.is_rhythm_staff());

DROP POLICY IF EXISTS "Staff can view all Matter bulb test reports"
  ON public.matter_bulb_test_reports;
CREATE POLICY "Staff can view all Matter bulb test reports"
  ON public.matter_bulb_test_reports FOR SELECT TO authenticated
  USING (public.is_rhythm_staff());

DROP POLICY IF EXISTS "Staff can view all Matter device profiles"
  ON public.published_matter_device_profiles;
CREATE POLICY "Staff can view all Matter device profiles"
  ON public.published_matter_device_profiles FOR SELECT TO authenticated
  USING (public.is_rhythm_staff());

DROP POLICY IF EXISTS "Staff can view support debug bundles"
  ON storage.objects;
CREATE POLICY "Staff can view support debug bundles"
  ON storage.objects FOR SELECT TO authenticated
  USING (
    bucket_id = 'support-debug-bundles'
    AND public.is_rhythm_staff()
  );

DROP TRIGGER IF EXISTS rhythm_staff_updated_at ON public.rhythm_staff;
CREATE TRIGGER rhythm_staff_updated_at
  BEFORE UPDATE ON public.rhythm_staff
  FOR EACH ROW
  EXECUTE FUNCTION public.update_updated_at_column();

COMMENT ON TABLE public.rhythm_staff IS
  'Allow-listed Rhythm staff accounts for support/admin tools';
COMMENT ON COLUMN public.rhythm_staff.role IS
  'support or admin. Both currently grant read-only support access through RLS.';
COMMENT ON FUNCTION public.is_rhythm_staff() IS
  'Returns true when auth.uid() is an enabled Rhythm staff account';
