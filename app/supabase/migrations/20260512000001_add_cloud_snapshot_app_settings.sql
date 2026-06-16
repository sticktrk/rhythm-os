ALTER TABLE user_cloud_snapshots
  ADD COLUMN IF NOT EXISTS app_settings_bundle JSONB NOT NULL DEFAULT '{}'::jsonb;

COMMENT ON COLUMN user_cloud_snapshots.app_settings_bundle IS
  'App-local settings that should roam with the account, keyed by hub where applicable. Currently includes All Rooms page layout.';
