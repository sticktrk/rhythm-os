-- Store stable Rhythm OS appliance identity on server hubs.
--
-- The hub row id is an app/cloud record id. `server_instance_id` identifies the
-- physical Rhythm Box so local/direct-IP/mDNS connections can rejoin the right
-- Home even when the LAN endpoint changes.

ALTER TABLE hubs
  ADD COLUMN IF NOT EXISTS server_instance_id TEXT;

CREATE INDEX IF NOT EXISTS hubs_server_instance_id_idx
  ON hubs (server_instance_id)
  WHERE server_instance_id IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS hubs_home_server_instance_id_idx
  ON hubs (home_id, server_instance_id)
  WHERE server_instance_id IS NOT NULL;

COMMENT ON COLUMN hubs.server_instance_id IS
  'Stable Rhythm OS server_instance_id from /api/state for direct/local appliance identity';
