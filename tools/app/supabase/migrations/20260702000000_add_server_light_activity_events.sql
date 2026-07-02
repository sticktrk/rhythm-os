-- Server-origin light activity captured from Rhythm OS.
--
-- App interaction analytics stay in PostHog. This table is for raw,
-- light-focused server activity: API/app commands as applied by the server,
-- physical button/switch actions, scene/profile actions, and other explicit
-- user-triggered light mutations uploaded directly by the Rhythm OS device.

CREATE TABLE IF NOT EXISTS public.server_light_activity_events (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
  home_id UUID REFERENCES public.homes(id) ON DELETE CASCADE,
  hub_id UUID NOT NULL REFERENCES public.hubs(id) ON DELETE CASCADE,
  server_instance_id TEXT,
  event_id TEXT NOT NULL,
  node_id TEXT NOT NULL,
  action_id TEXT NOT NULL,
  source_kind TEXT NOT NULL,
  source_raw TEXT NOT NULL,
  source_control_id TEXT,
  marks_touched BOOLEAN NOT NULL DEFAULT TRUE,
  occurred_at TIMESTAMPTZ NOT NULL,
  epoch_ms BIGINT NOT NULL CHECK (epoch_ms >= 0),
  active_mode JSONB,
  target JSONB,
  change JSONB,
  payload JSONB,
  raw_activity JSONB NOT NULL DEFAULT '{}'::jsonb,
  brightness INTEGER CHECK (brightness IS NULL OR brightness BETWEEN 1 AND 100),
  kelvin INTEGER CHECK (kelvin IS NULL OR kelvin BETWEEN 500 AND 25000),
  correlation_id TEXT,
  fanout_of TEXT,
  sync_source TEXT NOT NULL DEFAULT 'device_server_direct'
    CHECK (sync_source IN (
      'app_server_history',
      'app_server_sse',
      'device_server_direct',
      'manual_import'
    )),
  first_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  UNIQUE (user_id, hub_id, event_id)
);

CREATE TABLE IF NOT EXISTS public.server_light_activity_device_tokens (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
  home_id UUID NOT NULL REFERENCES public.homes(id) ON DELETE CASCADE,
  hub_id UUID NOT NULL REFERENCES public.hubs(id) ON DELETE CASCADE,
  server_instance_id TEXT,
  token_hash TEXT NOT NULL,
  token_prefix TEXT NOT NULL,
  label TEXT,
  revoked_at TIMESTAMPTZ,
  last_used_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS server_light_activity_device_tokens_home_idx
  ON public.server_light_activity_device_tokens (home_id);

CREATE UNIQUE INDEX IF NOT EXISTS server_light_activity_device_tokens_hash_idx
  ON public.server_light_activity_device_tokens (token_hash);

CREATE UNIQUE INDEX IF NOT EXISTS server_light_activity_device_tokens_active_hub_idx
  ON public.server_light_activity_device_tokens (hub_id)
  WHERE revoked_at IS NULL;

CREATE INDEX IF NOT EXISTS server_light_activity_events_user_time_idx
  ON public.server_light_activity_events (user_id, occurred_at DESC);

CREATE INDEX IF NOT EXISTS server_light_activity_events_server_time_idx
  ON public.server_light_activity_events (server_instance_id, occurred_at DESC)
  WHERE server_instance_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS server_light_activity_events_user_node_time_idx
  ON public.server_light_activity_events (user_id, node_id, occurred_at DESC);

CREATE INDEX IF NOT EXISTS server_light_activity_events_user_action_time_idx
  ON public.server_light_activity_events (user_id, action_id, occurred_at DESC);

CREATE INDEX IF NOT EXISTS server_light_activity_events_user_source_time_idx
  ON public.server_light_activity_events (user_id, source_kind, occurred_at DESC);

CREATE INDEX IF NOT EXISTS server_light_activity_events_target_gin_idx
  ON public.server_light_activity_events USING GIN (target jsonb_path_ops);

CREATE INDEX IF NOT EXISTS server_light_activity_events_payload_gin_idx
  ON public.server_light_activity_events USING GIN (payload jsonb_path_ops);

ALTER TABLE public.server_light_activity_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.server_light_activity_device_tokens ENABLE ROW LEVEL SECURITY;

REVOKE INSERT, UPDATE, DELETE ON public.server_light_activity_events FROM anon;
REVOKE INSERT, UPDATE, DELETE ON public.server_light_activity_events FROM authenticated;

REVOKE ALL ON public.server_light_activity_device_tokens FROM PUBLIC;
REVOKE ALL ON public.server_light_activity_device_tokens FROM anon;
REVOKE ALL ON public.server_light_activity_device_tokens FROM authenticated;

DROP POLICY IF EXISTS "Users can view their server light activity"
  ON public.server_light_activity_events;
CREATE POLICY "Users can view their server light activity"
  ON public.server_light_activity_events FOR SELECT TO authenticated
  USING (auth.uid() = user_id);

DROP POLICY IF EXISTS "Users can create their server light activity"
  ON public.server_light_activity_events;
DROP POLICY IF EXISTS "Users can update their server light activity"
  ON public.server_light_activity_events;
DROP POLICY IF EXISTS "Users can delete their server light activity"
  ON public.server_light_activity_events;

DROP POLICY IF EXISTS "Staff can view server light activity"
  ON public.server_light_activity_events;
CREATE POLICY "Staff can view server light activity"
  ON public.server_light_activity_events FOR SELECT TO authenticated
  USING (public.is_rhythm_staff());

DROP TRIGGER IF EXISTS server_light_activity_events_updated_at
  ON public.server_light_activity_events;
CREATE TRIGGER server_light_activity_events_updated_at
  BEFORE UPDATE ON public.server_light_activity_events
  FOR EACH ROW
  EXECUTE FUNCTION update_updated_at_column();

DROP TRIGGER IF EXISTS server_light_activity_device_tokens_updated_at
  ON public.server_light_activity_device_tokens;
CREATE TRIGGER server_light_activity_device_tokens_updated_at
  BEFORE UPDATE ON public.server_light_activity_device_tokens
  FOR EACH ROW
  EXECUTE FUNCTION update_updated_at_column();

DROP VIEW IF EXISTS public.rhythm_support_server_light_activity_events;
CREATE VIEW public.rhythm_support_server_light_activity_events
WITH (security_barrier = true)
AS
SELECT
  user_id,
  home_id,
  hub_id,
  server_instance_id,
  event_id,
  node_id,
  action_id,
  source_kind,
  source_raw,
  source_control_id,
  marks_touched,
  occurred_at,
  epoch_ms,
  brightness,
  kelvin,
  correlation_id,
  fanout_of,
  sync_source,
  target ->> 'name' AS target_name,
  target ->> 'kind' AS target_kind,
  target -> 'hub_types' AS target_hub_types,
  octet_length(COALESCE(target, '{}'::jsonb)::TEXT) AS target_bytes,
  octet_length(COALESCE(payload, '{}'::jsonb)::TEXT) AS payload_bytes,
  octet_length(raw_activity::TEXT) AS raw_activity_bytes,
  first_seen_at,
  last_seen_at,
  created_at,
  updated_at
FROM public.server_light_activity_events
WHERE public.is_rhythm_staff();

REVOKE ALL ON public.rhythm_support_server_light_activity_events FROM PUBLIC;
GRANT SELECT ON public.rhythm_support_server_light_activity_events TO authenticated;

DROP VIEW IF EXISTS public.user_server_light_activity_device_tokens;
CREATE VIEW public.user_server_light_activity_device_tokens
WITH (security_barrier = true)
AS
SELECT
  tokens.id,
  tokens.home_id,
  tokens.hub_id,
  tokens.server_instance_id,
  tokens.token_prefix,
  tokens.label,
  tokens.revoked_at,
  tokens.last_used_at,
  tokens.created_at,
  tokens.updated_at
FROM public.server_light_activity_device_tokens tokens
WHERE EXISTS (
  SELECT 1
  FROM public.homes homes
  WHERE homes.id = tokens.home_id
    AND auth.uid() = ANY(homes.member_ids)
);

REVOKE ALL ON public.user_server_light_activity_device_tokens FROM PUBLIC;
GRANT SELECT ON public.user_server_light_activity_device_tokens TO authenticated;

COMMENT ON TABLE public.server_light_activity_events IS
  'Server-origin, light-focused user activity uploaded directly by Rhythm OS with scoped device tokens; app analytics remain in PostHog';
COMMENT ON TABLE public.server_light_activity_device_tokens IS
  'Hashed scoped device upload tokens for direct Rhythm OS server light activity ingest';
COMMENT ON COLUMN public.server_light_activity_events.event_id IS
  'Stable event id generated by the Rhythm OS server activity feed';
COMMENT ON COLUMN public.server_light_activity_events.target IS
  'Light target snapshot at event time: name, kind, parent, hub types, current output, profile state';
COMMENT ON COLUMN public.server_light_activity_events.raw_activity IS
  'Raw /api/history or activity_appended payload for forward-compatible reprocessing';
COMMENT ON VIEW public.rhythm_support_server_light_activity_events IS
  'Support-safe metadata view of uploaded server light activity without full raw payload bodies';
COMMENT ON VIEW public.user_server_light_activity_device_tokens IS
  'Owner/member view of device activity upload token metadata without token hashes or raw secrets';
