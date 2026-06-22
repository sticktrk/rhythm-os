-- Matter bulb tester reports submitted from the app for device database curation.

CREATE TABLE IF NOT EXISTS matter_bulb_test_reports (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
  user_email TEXT,
  is_anonymous BOOLEAN NOT NULL DEFAULT TRUE,
  app_version TEXT,
  app_build TEXT,
  app_platform TEXT,
  server_version TEXT,
  server_platform_context TEXT,
  client_report_id TEXT,
  canonical_device_id TEXT,
  native_device_id TEXT,
  device_name TEXT,
  manufacturer TEXT,
  model TEXT,
  inferred_quirks JSONB NOT NULL DEFAULT '[]'::JSONB,
  capability_hints JSONB NOT NULL DEFAULT '{}'::JSONB,
  observations JSONB NOT NULL DEFAULT '[]'::JSONB,
  report_payload JSONB NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS matter_bulb_test_reports_user_id_idx
  ON matter_bulb_test_reports (user_id);

CREATE INDEX IF NOT EXISTS matter_bulb_test_reports_created_at_idx
  ON matter_bulb_test_reports (created_at DESC);

CREATE INDEX IF NOT EXISTS matter_bulb_test_reports_device_idx
  ON matter_bulb_test_reports (manufacturer, model);

ALTER TABLE matter_bulb_test_reports ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS "Users can view their Matter bulb test reports"
  ON matter_bulb_test_reports;
CREATE POLICY "Users can view their Matter bulb test reports"
  ON matter_bulb_test_reports FOR SELECT
  USING (auth.uid() = user_id);

DROP POLICY IF EXISTS "Users can create their Matter bulb test reports"
  ON matter_bulb_test_reports;
CREATE POLICY "Users can create their Matter bulb test reports"
  ON matter_bulb_test_reports FOR INSERT
  WITH CHECK (auth.uid() = user_id);

DROP TRIGGER IF EXISTS matter_bulb_test_reports_updated_at
  ON matter_bulb_test_reports;
CREATE TRIGGER matter_bulb_test_reports_updated_at
  BEFORE UPDATE ON matter_bulb_test_reports
  FOR EACH ROW
  EXECUTE FUNCTION update_updated_at_column();

COMMENT ON TABLE matter_bulb_test_reports IS
  'App-submitted Matter bulb command behavior reports used to curate rhythm-devices quirks';
COMMENT ON COLUMN matter_bulb_test_reports.inferred_quirks IS
  'Quirks inferred by the app tester, serialized using rhythm-devices DeviceQuirk names';
COMMENT ON COLUMN matter_bulb_test_reports.capability_hints IS
  'Capability adjustments inferred by the app tester, such as minimum brightness and transition support';
COMMENT ON COLUMN matter_bulb_test_reports.report_payload IS
  'Full client and server Matter bulb tester report payload';
