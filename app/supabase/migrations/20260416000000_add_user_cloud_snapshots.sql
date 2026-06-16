-- Cloud snapshots for the signed-in user's latest portable configuration
-- and full installation backup. Kept to one row per user for now.

CREATE TABLE IF NOT EXISTS user_cloud_snapshots (
  user_id UUID PRIMARY KEY REFERENCES auth.users(id) ON DELETE CASCADE,
  source_hub_id TEXT NOT NULL,
  source_hub_type TEXT NOT NULL,
  source_hub_name TEXT NOT NULL,
  source_hub_host TEXT NOT NULL,
  source_hub_port INTEGER NOT NULL,
  home_id TEXT,
  home_name TEXT,
  configuration_bundle JSONB NOT NULL,
  backup_bundle JSONB NOT NULL,
  captured_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS user_cloud_snapshots_captured_at_idx
  ON user_cloud_snapshots (captured_at DESC);

ALTER TABLE user_cloud_snapshots ENABLE ROW LEVEL SECURITY;

CREATE POLICY "Users can view their cloud snapshots"
  ON user_cloud_snapshots FOR SELECT
  USING (auth.uid() = user_id);

CREATE POLICY "Users can create their cloud snapshots"
  ON user_cloud_snapshots FOR INSERT
  WITH CHECK (auth.uid() = user_id);

CREATE POLICY "Users can update their cloud snapshots"
  ON user_cloud_snapshots FOR UPDATE
  USING (auth.uid() = user_id)
  WITH CHECK (auth.uid() = user_id);

CREATE POLICY "Users can delete their cloud snapshots"
  ON user_cloud_snapshots FOR DELETE
  USING (auth.uid() = user_id);

DROP TRIGGER IF EXISTS user_cloud_snapshots_updated_at ON user_cloud_snapshots;
CREATE TRIGGER user_cloud_snapshots_updated_at
  BEFORE UPDATE ON user_cloud_snapshots
  FOR EACH ROW
  EXECUTE FUNCTION update_updated_at_column();

COMMENT ON TABLE user_cloud_snapshots IS
  'Latest signed-in user snapshot of portable configuration and full backup';
COMMENT ON COLUMN user_cloud_snapshots.source_hub_id IS
  'Local hub ID for the server that produced the latest snapshot';
COMMENT ON COLUMN user_cloud_snapshots.configuration_bundle IS
  'Raw GET /api/configuration payload from the source server';
COMMENT ON COLUMN user_cloud_snapshots.backup_bundle IS
  'Raw GET /api/backup?include_secrets=true payload from the source server';
