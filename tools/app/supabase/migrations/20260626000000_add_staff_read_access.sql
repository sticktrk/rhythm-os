-- Staff/admin access for customer support tooling.
--
-- This adds a private allow-list of Rhythm staff users. Staff accounts can
-- read customer support records across RLS-protected tables. Secret-bearing
-- tables use sanitized support views instead of raw staff SELECT policies.
-- Admin accounts can also write operational customer records needed by
-- support/admin tools. This migration does not grant client-side write access
-- to staff membership.
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
  USING (user_id = auth.uid() OR public.is_rhythm_admin());

-- No INSERT/UPDATE/DELETE staff policies are defined on purpose. Staff grants
-- must be managed with the Supabase service role or dashboard.

-- Read access for support and admin staff.

DROP POLICY IF EXISTS "Staff can view all homes"
  ON public.homes;
CREATE POLICY "Staff can view all homes"
  ON public.homes FOR SELECT TO authenticated
  USING (public.is_rhythm_staff());

DROP VIEW IF EXISTS public.rhythm_support_hubs;
CREATE VIEW public.rhythm_support_hubs
WITH (security_barrier = true)
AS
SELECT
  id,
  home_id,
  type,
  name,
  endpoint,
  enabled,
  remote_endpoint,
  server_instance_id,
  last_connected,
  created_at,
  updated_at,
  token IS NOT NULL AND token <> '' AS has_legacy_token,
  encrypted_token IS NOT NULL AS has_encrypted_token
FROM public.hubs
WHERE public.is_rhythm_staff();

REVOKE ALL ON public.rhythm_support_hubs FROM PUBLIC;
GRANT SELECT ON public.rhythm_support_hubs TO authenticated;

DROP POLICY IF EXISTS "Staff can view all hubs"
  ON public.hubs;

DROP POLICY IF EXISTS "Staff can view all remote access records"
  ON public.hub_remote_access;
CREATE POLICY "Staff can view all remote access records"
  ON public.hub_remote_access FOR SELECT TO authenticated
  USING (public.is_rhythm_staff());

DROP VIEW IF EXISTS public.rhythm_support_cloud_snapshots;
CREATE VIEW public.rhythm_support_cloud_snapshots
WITH (security_barrier = true)
AS
SELECT
  user_id,
  source_hub_id,
  source_hub_type,
  source_hub_name,
  source_hub_host,
  source_hub_port,
  home_id,
  home_name,
  captured_at,
  created_at,
  updated_at,
  octet_length(configuration_bundle::TEXT) AS configuration_bundle_bytes,
  octet_length(backup_bundle::TEXT) AS backup_bundle_bytes,
  octet_length(app_settings_bundle::TEXT) AS app_settings_bundle_bytes
FROM public.user_cloud_snapshots
WHERE public.is_rhythm_staff();

REVOKE ALL ON public.rhythm_support_cloud_snapshots FROM PUBLIC;
GRANT SELECT ON public.rhythm_support_cloud_snapshots TO authenticated;

DROP POLICY IF EXISTS "Staff can view all cloud snapshots"
  ON public.user_cloud_snapshots;

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
DROP POLICY IF EXISTS "Admin staff can update cloud snapshots"
  ON public.user_cloud_snapshots;
DROP POLICY IF EXISTS "Admin staff can delete cloud snapshots"
  ON public.user_cloud_snapshots;

DROP POLICY IF EXISTS "Admin staff can create subscriptions"
  ON public.subscriptions;
DROP POLICY IF EXISTS "Admin staff can update subscriptions"
  ON public.subscriptions;
DROP POLICY IF EXISTS "Admin staff can delete subscriptions"
  ON public.subscriptions;

CREATE OR REPLACE FUNCTION public.set_rhythm_subscription_tier(
  target_user_id UUID,
  target_tier public.subscription_tier
)
RETURNS public.subscriptions
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public
AS $$
DECLARE
  new_subscription public.subscriptions;
BEGIN
  IF NOT public.is_rhythm_admin() THEN
    RAISE EXCEPTION 'not authorized'
      USING ERRCODE = '42501';
  END IF;

  UPDATE public.subscriptions
  SET
    status = 'canceled',
    ended_at = NOW()
  WHERE user_id = target_user_id
    AND status = 'active';

  INSERT INTO public.subscriptions (
    user_id,
    tier,
    status,
    source
  )
  VALUES (
    target_user_id,
    target_tier,
    'active',
    'manual'
  )
  RETURNING * INTO new_subscription;

  RETURN new_subscription;
END;
$$;

REVOKE ALL ON FUNCTION public.set_rhythm_subscription_tier(UUID, public.subscription_tier)
  FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.set_rhythm_subscription_tier(UUID, public.subscription_tier)
  TO authenticated;

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
COMMENT ON VIEW public.rhythm_support_hubs IS
  'Sanitized support view of hubs that omits token and encrypted_token values';
COMMENT ON VIEW public.rhythm_support_cloud_snapshots IS
  'Sanitized support view of cloud snapshot metadata that omits backup payloads';
COMMENT ON FUNCTION public.set_rhythm_subscription_tier(UUID, public.subscription_tier) IS
  'Admin-only subscription tier change that cancels prior active rows before inserting the new active row';
