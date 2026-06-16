-- Dedupe remote-access tunnels for the same Rhythm server across app installs.

ALTER TABLE hub_remote_access
  ADD COLUMN IF NOT EXISTS server_instance_id TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS hub_remote_access_home_server_instance_id_idx
  ON hub_remote_access (home_id, server_instance_id)
  WHERE server_instance_id IS NOT NULL;
