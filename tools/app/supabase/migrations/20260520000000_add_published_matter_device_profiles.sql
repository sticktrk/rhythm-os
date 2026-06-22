-- Curated Matter device profiles derived from bulb tester reports.
--
-- Rows are auto-created/overwritten by report-matter-bulb, but only rows with
-- approved=true are returned to hubs by the public profile feed.

CREATE TABLE IF NOT EXISTS published_matter_device_profiles (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  profile_key TEXT NOT NULL UNIQUE,
  schema_version INTEGER NOT NULL DEFAULT 1,
  profile_version BIGINT NOT NULL DEFAULT 1,
  approved BOOLEAN NOT NULL DEFAULT FALSE,
  approved_at TIMESTAMPTZ,
  approved_by UUID REFERENCES auth.users(id) ON DELETE SET NULL,
  manufacturer TEXT,
  model TEXT,
  device_name TEXT,
  matter_vendor_id INTEGER,
  matter_product_id INTEGER,
  firmware_version TEXT,
  cluster_fingerprint JSONB NOT NULL DEFAULT '{}'::JSONB,
  capabilities JSONB NOT NULL DEFAULT '{}'::JSONB,
  quirks JSONB NOT NULL DEFAULT '{}'::JSONB,
  recommended_control_strategy JSONB NOT NULL DEFAULT '{}'::JSONB,
  evidence JSONB NOT NULL DEFAULT '{}'::JSONB,
  source_report_id UUID REFERENCES matter_bulb_test_reports(id) ON DELETE SET NULL,
  client_report_id TEXT,
  report_count INTEGER NOT NULL DEFAULT 1,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS published_matter_device_profiles_approved_idx
  ON published_matter_device_profiles (approved, profile_version DESC);

CREATE INDEX IF NOT EXISTS published_matter_device_profiles_matter_ids_idx
  ON published_matter_device_profiles (matter_vendor_id, matter_product_id);

CREATE INDEX IF NOT EXISTS published_matter_device_profiles_model_idx
  ON published_matter_device_profiles (manufacturer, model);

ALTER TABLE published_matter_device_profiles ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS "Approved Matter device profiles are public"
  ON published_matter_device_profiles;
CREATE POLICY "Approved Matter device profiles are public"
  ON published_matter_device_profiles FOR SELECT
  USING (approved = TRUE);

DROP TRIGGER IF EXISTS published_matter_device_profiles_updated_at
  ON published_matter_device_profiles;
CREATE TRIGGER published_matter_device_profiles_updated_at
  BEFORE UPDATE ON published_matter_device_profiles
  FOR EACH ROW
  EXECUTE FUNCTION update_updated_at_column();

COMMENT ON TABLE published_matter_device_profiles IS
  'Curated Matter bulb/device runtime profiles generated from tester reports and served to hubs after manual approval';
COMMENT ON COLUMN published_matter_device_profiles.approved IS
  'Only approved profiles are served to hubs; report submissions reset this to false for manual review';
COMMENT ON COLUMN published_matter_device_profiles.profile_key IS
  'Stable model key, preferring Matter vendor/product IDs and falling back to manufacturer/model';
COMMENT ON COLUMN published_matter_device_profiles.capabilities IS
  'Runtime capability overlay such as min brightness, transition support, and preferred command families';
COMMENT ON COLUMN published_matter_device_profiles.quirks IS
  'Runtime quirk overlay inferred from visible behavior and command telemetry';
