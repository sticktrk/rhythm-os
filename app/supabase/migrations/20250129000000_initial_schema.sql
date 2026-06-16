-- Rhythm Lighting Supabase Database Schema
-- Initial migration: homes and hubs tables with RLS

-- ============================================================
-- HOMES TABLE
-- ============================================================

CREATE TABLE IF NOT EXISTS homes (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  name TEXT NOT NULL,
  owner_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
  member_ids UUID[] NOT NULL DEFAULT '{}',
  location JSONB,
  sleep_schedule JSONB NOT NULL DEFAULT '{"bedtime": 22.0, "wakeTime": 6.5, "enabled": true}',
  curve_config JSONB,
  timezone TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Index for member lookup (used by RLS and queries)
CREATE INDEX IF NOT EXISTS homes_member_ids_idx ON homes USING GIN (member_ids);

-- Index for owner lookup
CREATE INDEX IF NOT EXISTS homes_owner_id_idx ON homes (owner_id);

-- ============================================================
-- HUBS TABLE
-- ============================================================

CREATE TABLE IF NOT EXISTS hubs (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  home_id UUID NOT NULL REFERENCES homes(id) ON DELETE CASCADE,
  type TEXT NOT NULL CHECK (type IN ('homeAssistant', 'hue', 'esp32')),
  name TEXT NOT NULL,
  endpoint JSONB NOT NULL,
  enabled BOOLEAN NOT NULL DEFAULT TRUE,
  token TEXT,
  last_connected TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Index for home lookup
CREATE INDEX IF NOT EXISTS hubs_home_id_idx ON hubs (home_id);

-- ============================================================
-- ROW LEVEL SECURITY (RLS) POLICIES
-- ============================================================

-- Enable RLS on both tables
ALTER TABLE homes ENABLE ROW LEVEL SECURITY;
ALTER TABLE hubs ENABLE ROW LEVEL SECURITY;

-- Homes policies: Users can access homes where they are a member
CREATE POLICY "Users can view homes they are members of"
  ON homes FOR SELECT
  USING (auth.uid() = ANY(member_ids));

CREATE POLICY "Users can create homes"
  ON homes FOR INSERT
  WITH CHECK (auth.uid() = owner_id);

CREATE POLICY "Home members can update homes"
  ON homes FOR UPDATE
  USING (auth.uid() = ANY(member_ids))
  WITH CHECK (auth.uid() = ANY(member_ids));

CREATE POLICY "Only owner can delete homes"
  ON homes FOR DELETE
  USING (auth.uid() = owner_id);

-- Hubs policies: Users can access hubs in homes they are members of
CREATE POLICY "Users can view hubs in their homes"
  ON hubs FOR SELECT
  USING (
    EXISTS (
      SELECT 1 FROM homes
      WHERE homes.id = hubs.home_id
      AND auth.uid() = ANY(homes.member_ids)
    )
  );

CREATE POLICY "Users can create hubs in their homes"
  ON hubs FOR INSERT
  WITH CHECK (
    EXISTS (
      SELECT 1 FROM homes
      WHERE homes.id = hubs.home_id
      AND auth.uid() = ANY(homes.member_ids)
    )
  );

CREATE POLICY "Users can update hubs in their homes"
  ON hubs FOR UPDATE
  USING (
    EXISTS (
      SELECT 1 FROM homes
      WHERE homes.id = hubs.home_id
      AND auth.uid() = ANY(homes.member_ids)
    )
  )
  WITH CHECK (
    EXISTS (
      SELECT 1 FROM homes
      WHERE homes.id = hubs.home_id
      AND auth.uid() = ANY(homes.member_ids)
    )
  );

CREATE POLICY "Users can delete hubs in their homes"
  ON hubs FOR DELETE
  USING (
    EXISTS (
      SELECT 1 FROM homes
      WHERE homes.id = hubs.home_id
      AND auth.uid() = ANY(homes.member_ids)
    )
  );

-- ============================================================
-- UPDATED_AT TRIGGER
-- ============================================================

-- Function to automatically update updated_at timestamp
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
  NEW.updated_at = NOW();
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Triggers for both tables
DROP TRIGGER IF EXISTS homes_updated_at ON homes;
CREATE TRIGGER homes_updated_at
  BEFORE UPDATE ON homes
  FOR EACH ROW
  EXECUTE FUNCTION update_updated_at_column();

DROP TRIGGER IF EXISTS hubs_updated_at ON hubs;
CREATE TRIGGER hubs_updated_at
  BEFORE UPDATE ON hubs
  FOR EACH ROW
  EXECUTE FUNCTION update_updated_at_column();

-- ============================================================
-- ENABLE REALTIME
-- ============================================================

-- Enable realtime for both tables (required for Supabase Realtime subscriptions)
ALTER PUBLICATION supabase_realtime ADD TABLE homes;
ALTER PUBLICATION supabase_realtime ADD TABLE hubs;

-- ============================================================
-- HELPER FUNCTIONS
-- ============================================================

-- Function to add a member to a home
CREATE OR REPLACE FUNCTION add_home_member(home_uuid UUID, user_uuid UUID)
RETURNS VOID AS $$
BEGIN
  UPDATE homes
  SET member_ids = array_append(member_ids, user_uuid)
  WHERE id = home_uuid
  AND NOT (user_uuid = ANY(member_ids));
END;
$$ LANGUAGE plpgsql SECURITY DEFINER;

-- Function to remove a member from a home
CREATE OR REPLACE FUNCTION remove_home_member(home_uuid UUID, user_uuid UUID)
RETURNS VOID AS $$
BEGIN
  UPDATE homes
  SET member_ids = array_remove(member_ids, user_uuid)
  WHERE id = home_uuid;
END;
$$ LANGUAGE plpgsql SECURITY DEFINER;

-- ============================================================
-- COMMENTS
-- ============================================================

COMMENT ON TABLE homes IS 'User homes containing lighting configurations';
COMMENT ON COLUMN homes.member_ids IS 'Array of user UUIDs who have access to this home';
COMMENT ON COLUMN homes.location IS 'JSON: {latitude, longitude, cityName}';
COMMENT ON COLUMN homes.sleep_schedule IS 'JSON: {bedtime, wakeTime, enabled}';
COMMENT ON COLUMN homes.curve_config IS 'JSON: Lighting curve configuration parameters';

COMMENT ON TABLE hubs IS 'Lighting controllers (Home Assistant, Hue, ESP32)';
COMMENT ON COLUMN hubs.type IS 'Controller type: homeAssistant, hue, or esp32';
COMMENT ON COLUMN hubs.endpoint IS 'JSON: {host, port, useSsl}';
COMMENT ON COLUMN hubs.token IS 'Authentication token (HA long-lived token or Hue app key)';
