-- Remote access tunnel metadata.

ALTER TABLE hubs
  ADD COLUMN IF NOT EXISTS remote_endpoint JSONB;

CREATE TABLE IF NOT EXISTS hub_remote_access (
  hub_id UUID PRIMARY KEY REFERENCES hubs(id) ON DELETE CASCADE,
  home_id UUID NOT NULL REFERENCES homes(id) ON DELETE CASCADE,
  hostname TEXT NOT NULL UNIQUE,
  tunnel_id TEXT NOT NULL,
  tunnel_name TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS hub_remote_access_home_id_idx
  ON hub_remote_access (home_id);

ALTER TABLE hub_remote_access ENABLE ROW LEVEL SECURITY;

CREATE POLICY "Users can view remote access in their homes"
  ON hub_remote_access FOR SELECT
  USING (
    EXISTS (
      SELECT 1 FROM homes
      WHERE homes.id = hub_remote_access.home_id
      AND auth.uid() = ANY(homes.member_ids)
    )
  );

CREATE POLICY "Users can create remote access in their homes"
  ON hub_remote_access FOR INSERT
  WITH CHECK (
    EXISTS (
      SELECT 1 FROM homes
      WHERE homes.id = hub_remote_access.home_id
      AND auth.uid() = ANY(homes.member_ids)
    )
  );

CREATE POLICY "Users can update remote access in their homes"
  ON hub_remote_access FOR UPDATE
  USING (
    EXISTS (
      SELECT 1 FROM homes
      WHERE homes.id = hub_remote_access.home_id
      AND auth.uid() = ANY(homes.member_ids)
    )
  )
  WITH CHECK (
    EXISTS (
      SELECT 1 FROM homes
      WHERE homes.id = hub_remote_access.home_id
      AND auth.uid() = ANY(homes.member_ids)
    )
  );

CREATE POLICY "Users can delete remote access in their homes"
  ON hub_remote_access FOR DELETE
  USING (
    EXISTS (
      SELECT 1 FROM homes
      WHERE homes.id = hub_remote_access.home_id
      AND auth.uid() = ANY(homes.member_ids)
    )
  );

DROP TRIGGER IF EXISTS hub_remote_access_updated_at ON hub_remote_access;
CREATE TRIGGER hub_remote_access_updated_at
  BEFORE UPDATE ON hub_remote_access
  FOR EACH ROW
  EXECUTE FUNCTION update_updated_at_column();
