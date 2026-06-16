ALTER TABLE hubs
  ADD COLUMN IF NOT EXISTS encrypted_token JSONB;

COMMENT ON COLUMN hubs.encrypted_token IS
  'Client-side encrypted hub token envelope for signed-in account restore';
