-- Staff/admin access for customer support tooling.
--
-- This adds a private allow-list of Rhythm staff users. Staff accounts can
-- read customer records across RLS-protected tables. Admin accounts can also
-- write operational customer records needed by support/admin tools. This
-- migration does not grant client-side write access to staff membership.
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

CREATE OR REPLACE FUNCTION public.is_rhythm_admin()
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
      AND role = 'admin'
  );
$$;

REVOKE ALL ON FUNCTION public.is_rhythm_admin() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.is_rhythm_admin() TO authenticated;

DROP POLICY IF EXISTS "Staff can read own staff status"
  ON public.rhythm_staff;
CREATE POLICY "Staff can read own staff status"
  ON public.rhythm_staff FOR SELECT TO authenticated
  USING (user_id = auth.uid() OR public.is_rhythm_staff());

-- No INSERT/UPDATE/DELETE staff policies are defined on purpose. Staff grants
-- must be managed with the Supabase service role or dashboard.

-- Read access for support and admin staff.

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

-- Write access for admin staff only.

DROP POLICY IF EXISTS "Admin staff can create homes"
  ON public.homes;
CREATE POLICY "Admin staff can create homes"
  ON public.homes FOR INSERT TO authenticated
  WITH CHECK (public.is_rhythm_admin());

DROP POLICY IF EXISTS "Admin staff can update homes"
  ON public.homes;
CREATE POLICY "Admin staff can update homes"
  ON public.homes FOR UPDATE TO authenticated
  USING (public.is_rhythm_admin())
  WITH CHECK (public.is_rhythm_admin());

DROP POLICY IF EXISTS "Admin staff can delete homes"
  ON public.homes;
CREATE POLICY "Admin staff can delete homes"
  ON public.homes FOR DELETE TO authenticated
  USING (public.is_rhythm_admin());

DROP POLICY IF EXISTS "Admin staff can create hubs"
  ON public.hubs;
CREATE POLICY "Admin staff can create hubs"
  ON public.hubs FOR INSERT TO authenticated
  WITH CHECK (public.is_rhythm_admin());

DROP POLICY IF EXISTS "Admin staff can update hubs"
  ON public.hubs;
CREATE POLICY "Admin staff can update hubs"
  ON public.hubs FOR UPDATE TO authenticated
  USING (public.is_rhythm_admin())
  WITH CHECK (public.is_rhythm_admin());

DROP POLICY IF EXISTS "Admin staff can delete hubs"
  ON public.hubs;
CREATE POLICY "Admin staff can delete hubs"
  ON public.hubs FOR DELETE TO authenticated
  USING (public.is_rhythm_admin());

DROP POLICY IF EXISTS "Admin staff can create remote access records"
  ON public.hub_remote_access;
CREATE POLICY "Admin staff can create remote access records"
  ON public.hub_remote_access FOR INSERT TO authenticated
  WITH CHECK (public.is_rhythm_admin());

DROP POLICY IF EXISTS "Admin staff can update remote access records"
  ON public.hub_remote_access;
CREATE POLICY "Admin staff can update remote access records"
  ON public.hub_remote_access FOR UPDATE TO authenticated
  USING (public.is_rhythm_admin())
  WITH CHECK (public.is_rhythm_admin());

DROP POLICY IF EXISTS "Admin staff can delete remote access records"
  ON public.hub_remote_access;
CREATE POLICY "Admin staff can delete remote access records"
  ON public.hub_remote_access FOR DELETE TO authenticated
  USING (public.is_rhythm_admin());

DROP POLICY IF EXISTS "Admin staff can create cloud snapshots"
  ON public.user_cloud_snapshots;
CREATE POLICY "Admin staff can create cloud snapshots"
  ON public.user_cloud_snapshots FOR INSERT TO authenticated
  WITH CHECK (public.is_rhythm_admin());

DROP POLICY IF EXISTS "Admin staff can update cloud snapshots"
  ON public.user_cloud_snapshots;
CREATE POLICY "Admin staff can update cloud snapshots"
  ON public.user_cloud_snapshots FOR UPDATE TO authenticated
  USING (public.is_rhythm_admin())
  WITH CHECK (public.is_rhythm_admin());

DROP POLICY IF EXISTS "Admin staff can delete cloud snapshots"
  ON public.user_cloud_snapshots;
CREATE POLICY "Admin staff can delete cloud snapshots"
  ON public.user_cloud_snapshots FOR DELETE TO authenticated
  USING (public.is_rhythm_admin());

DROP POLICY IF EXISTS "Admin staff can create subscriptions"
  ON public.subscriptions;
CREATE POLICY "Admin staff can create subscriptions"
  ON public.subscriptions FOR INSERT TO authenticated
  WITH CHECK (public.is_rhythm_admin());

DROP POLICY IF EXISTS "Admin staff can update subscriptions"
  ON public.subscriptions;
CREATE POLICY "Admin staff can update subscriptions"
  ON public.subscriptions FOR UPDATE TO authenticated
  USING (public.is_rhythm_admin())
  WITH CHECK (public.is_rhythm_admin());

DROP POLICY IF EXISTS "Admin staff can delete subscriptions"
  ON public.subscriptions;
CREATE POLICY "Admin staff can delete subscriptions"
  ON public.subscriptions FOR DELETE TO authenticated
  USING (public.is_rhythm_admin());

DROP POLICY IF EXISTS "Admin staff can update debug bundle submissions"
  ON public.support_debug_bundle_submissions;
CREATE POLICY "Admin staff can update debug bundle submissions"
  ON public.support_debug_bundle_submissions FOR UPDATE TO authenticated
  USING (public.is_rhythm_admin())
  WITH CHECK (public.is_rhythm_admin());

DROP POLICY IF EXISTS "Admin staff can update Matter bulb test reports"
  ON public.matter_bulb_test_reports;
CREATE POLICY "Admin staff can update Matter bulb test reports"
  ON public.matter_bulb_test_reports FOR UPDATE TO authenticated
  USING (public.is_rhythm_admin())
  WITH CHECK (public.is_rhythm_admin());

DROP POLICY IF EXISTS "Admin staff can create Matter device profiles"
  ON public.published_matter_device_profiles;
CREATE POLICY "Admin staff can create Matter device profiles"
  ON public.published_matter_device_profiles FOR INSERT TO authenticated
  WITH CHECK (public.is_rhythm_admin());

DROP POLICY IF EXISTS "Admin staff can update Matter device profiles"
  ON public.published_matter_device_profiles;
CREATE POLICY "Admin staff can update Matter device profiles"
  ON public.published_matter_device_profiles FOR UPDATE TO authenticated
  USING (public.is_rhythm_admin())
  WITH CHECK (public.is_rhythm_admin());

DROP POLICY IF EXISTS "Admin staff can delete Matter device profiles"
  ON public.published_matter_device_profiles;
CREATE POLICY "Admin staff can delete Matter device profiles"
  ON public.published_matter_device_profiles FOR DELETE TO authenticated
  USING (public.is_rhythm_admin());

DROP TRIGGER IF EXISTS rhythm_staff_updated_at ON public.rhythm_staff;
CREATE TRIGGER rhythm_staff_updated_at
  BEFORE UPDATE ON public.rhythm_staff
  FOR EACH ROW
  EXECUTE FUNCTION public.update_updated_at_column();

COMMENT ON TABLE public.rhythm_staff IS
  'Allow-listed Rhythm staff accounts for support/admin tools';
COMMENT ON COLUMN public.rhythm_staff.role IS
  'support grants read-only support access; admin grants read/write support access through RLS.';
COMMENT ON FUNCTION public.is_rhythm_staff() IS
  'Returns true when auth.uid() is an enabled Rhythm staff account';
COMMENT ON FUNCTION public.is_rhythm_admin() IS
  'Returns true when auth.uid() is an enabled Rhythm admin account';
