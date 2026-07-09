ALTER TABLE public.hubs
  ADD COLUMN IF NOT EXISTS remote_access_disabled_at TIMESTAMPTZ;

COMMENT ON COLUMN public.hubs.remote_access_disabled_at IS
  'Set by an explicit remote-access disable so background clients do not recreate the tunnel.';
