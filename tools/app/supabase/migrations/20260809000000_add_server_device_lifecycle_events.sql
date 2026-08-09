-- Privacy-bounded pair/unpair outcomes uploaded by Rhythm OS through the
-- existing scoped server-activity device token. Raw device names, IDs, bridge
-- addresses, serials, warnings, and errors remain on the appliance.

CREATE TABLE IF NOT EXISTS public.server_device_lifecycle_events (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
  home_id UUID NOT NULL REFERENCES public.homes(id) ON DELETE CASCADE,
  hub_id UUID NOT NULL REFERENCES public.hubs(id) ON DELETE CASCADE,
  server_instance_id TEXT,
  event_id TEXT NOT NULL CHECK (char_length(event_id) BETWEEN 1 AND 96),
  occurred_at TIMESTAMPTZ NOT NULL,
  epoch_ms BIGINT NOT NULL CHECK (epoch_ms >= 0),
  action TEXT NOT NULL CHECK (action IN ('pair', 'unpair')),
  hub_type TEXT NOT NULL CHECK (char_length(hub_type) BETWEEN 1 AND 64),
  device_type TEXT CHECK (
    device_type IS NULL OR device_type IN ('light', 'button', 'motion', 'contact')
  ),
  outcome TEXT NOT NULL CHECK (char_length(outcome) BETWEEN 1 AND 32),
  failure_stage TEXT CHECK (
    failure_stage IS NULL OR char_length(failure_stage) BETWEEN 1 AND 64
  ),
  force BOOLEAN NOT NULL DEFAULT FALSE,
  correlation_id TEXT CHECK (
    correlation_id IS NULL OR char_length(correlation_id) BETWEEN 1 AND 96
  ),
  sync_source TEXT NOT NULL DEFAULT 'device_server_direct'
    CHECK (sync_source = 'device_server_direct'),
  first_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  UNIQUE (user_id, hub_id, event_id)
);

CREATE INDEX IF NOT EXISTS server_device_lifecycle_user_time_idx
  ON public.server_device_lifecycle_events (user_id, occurred_at DESC);

CREATE INDEX IF NOT EXISTS server_device_lifecycle_hub_time_idx
  ON public.server_device_lifecycle_events (hub_id, occurred_at DESC);

CREATE INDEX IF NOT EXISTS server_device_lifecycle_kind_time_idx
  ON public.server_device_lifecycle_events
  (hub_type, device_type, action, occurred_at DESC);

ALTER TABLE public.server_device_lifecycle_events ENABLE ROW LEVEL SECURITY;

REVOKE INSERT, UPDATE, DELETE ON public.server_device_lifecycle_events FROM anon;
REVOKE INSERT, UPDATE, DELETE ON public.server_device_lifecycle_events FROM authenticated;

DROP POLICY IF EXISTS "Users can view their server device lifecycle"
  ON public.server_device_lifecycle_events;
CREATE POLICY "Users can view their server device lifecycle"
  ON public.server_device_lifecycle_events FOR SELECT TO authenticated
  USING (
    auth.uid() = user_id OR EXISTS (
      SELECT 1
      FROM public.homes
      WHERE homes.id = server_device_lifecycle_events.home_id
        AND auth.uid() = ANY(homes.member_ids)
    )
  );

DROP POLICY IF EXISTS "Staff can view server device lifecycle"
  ON public.server_device_lifecycle_events;
CREATE POLICY "Staff can view server device lifecycle"
  ON public.server_device_lifecycle_events FOR SELECT TO authenticated
  USING (public.is_rhythm_staff());

DROP TRIGGER IF EXISTS server_device_lifecycle_events_updated_at
  ON public.server_device_lifecycle_events;
CREATE TRIGGER server_device_lifecycle_events_updated_at
  BEFORE UPDATE ON public.server_device_lifecycle_events
  FOR EACH ROW
  EXECUTE FUNCTION update_updated_at_column();

DROP VIEW IF EXISTS public.rhythm_support_server_device_lifecycle_events;
CREATE VIEW public.rhythm_support_server_device_lifecycle_events
WITH (security_barrier = true)
AS
SELECT
  user_id,
  home_id,
  hub_id,
  server_instance_id,
  event_id,
  occurred_at,
  epoch_ms,
  action,
  hub_type,
  device_type,
  outcome,
  failure_stage,
  force,
  correlation_id,
  sync_source,
  first_seen_at,
  last_seen_at,
  created_at,
  updated_at
FROM public.server_device_lifecycle_events
WHERE public.is_rhythm_staff();

REVOKE ALL ON public.rhythm_support_server_device_lifecycle_events FROM PUBLIC;
GRANT SELECT ON public.rhythm_support_server_device_lifecycle_events TO authenticated;

COMMENT ON TABLE public.server_device_lifecycle_events IS
  'Privacy-bounded server-origin pair/unpair outcomes uploaded directly by Rhythm OS';
COMMENT ON COLUMN public.server_device_lifecycle_events.event_id IS
  'Stable hash-derived event id; it does not expose the local device identity';
COMMENT ON VIEW public.rhythm_support_server_device_lifecycle_events IS
  'Support-safe pair/unpair outcomes without device identity, names, addresses, serials, warnings, or raw errors';
