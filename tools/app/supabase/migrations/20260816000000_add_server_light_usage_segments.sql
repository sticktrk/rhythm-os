-- Low-write, server-observed light usage aggregates.
--
-- App intent remains in PostHog and explicit light actions remain in
-- server_light_activity_events. This table stores absolute, revisioned
-- subject/day snapshots so polling frequency does not determine row growth.

CREATE TABLE IF NOT EXISTS public.server_light_usage_segments (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
  home_id UUID NOT NULL REFERENCES public.homes(id) ON DELETE CASCADE,
  hub_id UUID NOT NULL REFERENCES public.hubs(id) ON DELETE CASCADE,
  server_instance_id TEXT,
  segment_id TEXT NOT NULL CHECK (segment_id ~ '^[a-f0-9]{32}$'),
  subject_id TEXT NOT NULL CHECK (
    char_length(subject_id) BETWEEN 1 AND 160
    AND subject_id ~ '^[A-Za-z0-9_.:-]+$'
  ),
  subject_kind TEXT NOT NULL CHECK (subject_kind IN ('bulb', 'room_aggregate')),
  usage_date DATE NOT NULL,
  revision BIGINT NOT NULL CHECK (revision >= 1),
  on_ms BIGINT NOT NULL CHECK (on_ms >= 0),
  covered_ms BIGINT NOT NULL CHECK (covered_ms >= 0),
  transition_uncertainty_ms BIGINT NOT NULL CHECK (transition_uncertainty_ms >= 0),
  observation_count BIGINT NOT NULL CHECK (observation_count >= 0),
  transition_count BIGINT NOT NULL CHECK (
    transition_count >= 0 AND transition_count <= observation_count
  ),
  first_observed_at TIMESTAMPTZ NOT NULL,
  last_observed_at TIMESTAMPTZ NOT NULL,
  source_summary JSONB NOT NULL DEFAULT '{}'::jsonb,
  last_batch_id TEXT NOT NULL CHECK (last_batch_id ~ '^[a-f0-9]{32}$'),
  first_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  CONSTRAINT server_light_usage_duration_check CHECK (
    on_ms <= covered_ms
    AND transition_uncertainty_ms <= covered_ms
  ),
  CONSTRAINT server_light_usage_observed_time_check CHECK (
    first_observed_at <= last_observed_at
  ),
  UNIQUE (user_id, hub_id, segment_id)
);

CREATE INDEX IF NOT EXISTS server_light_usage_user_date_idx
  ON public.server_light_usage_segments (user_id, usage_date DESC);
CREATE INDEX IF NOT EXISTS server_light_usage_home_date_kind_idx
  ON public.server_light_usage_segments (home_id, usage_date DESC, subject_kind);
CREATE INDEX IF NOT EXISTS server_light_usage_server_date_idx
  ON public.server_light_usage_segments (server_instance_id, usage_date DESC)
  WHERE server_instance_id IS NOT NULL;

ALTER TABLE public.server_light_usage_segments ENABLE ROW LEVEL SECURITY;

REVOKE INSERT, UPDATE, DELETE ON public.server_light_usage_segments FROM anon;
REVOKE INSERT, UPDATE, DELETE ON public.server_light_usage_segments FROM authenticated;
GRANT SELECT ON public.server_light_usage_segments TO authenticated;

DROP POLICY IF EXISTS "Users can view their server light usage"
  ON public.server_light_usage_segments;
CREATE POLICY "Users can view their server light usage"
  ON public.server_light_usage_segments FOR SELECT TO authenticated
  USING (auth.uid() = user_id);

DROP POLICY IF EXISTS "Staff can view server light usage"
  ON public.server_light_usage_segments;
CREATE POLICY "Staff can view server light usage"
  ON public.server_light_usage_segments FOR SELECT TO authenticated
  USING (public.is_rhythm_staff());

DROP TRIGGER IF EXISTS server_light_usage_segments_updated_at
  ON public.server_light_usage_segments;
CREATE TRIGGER server_light_usage_segments_updated_at
  BEFORE UPDATE ON public.server_light_usage_segments
  FOR EACH ROW
  EXECUTE FUNCTION update_updated_at_column();

CREATE OR REPLACE FUNCTION public.ingest_server_light_usage_batch(
  p_user_id UUID,
  p_home_id UUID,
  p_hub_id UUID,
  p_server_instance_id TEXT,
  p_usage_schema INTEGER,
  p_batch_id TEXT,
  p_segments JSONB
)
RETURNS INTEGER
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
  affected_rows INTEGER := 0;
BEGIN
  IF p_usage_schema <> 1 THEN
    RAISE EXCEPTION 'unsupported light usage schema';
  END IF;
  IF p_batch_id IS NULL OR p_batch_id !~ '^[a-f0-9]{32}$' THEN
    RAISE EXCEPTION 'invalid light usage batch id';
  END IF;
  IF jsonb_typeof(p_segments) <> 'array'
     OR jsonb_array_length(p_segments) < 1
     OR jsonb_array_length(p_segments) > 512 THEN
    RAISE EXCEPTION 'invalid light usage segment batch';
  END IF;

  IF EXISTS (
    SELECT 1
    FROM jsonb_array_elements(p_segments) segment
    WHERE COALESCE(segment ->> 'segment_id', '') !~ '^[a-f0-9]{32}$'
       OR COALESCE(segment ->> 'subject_id', '') !~ '^[A-Za-z0-9_.:-]{1,160}$'
       OR segment ->> 'subject_kind' NOT IN ('bulb', 'room_aggregate')
       OR (segment ->> 'revision')::BIGINT < 1
       OR (segment ->> 'on_ms')::BIGINT < 0
       OR (segment ->> 'covered_ms')::BIGINT < (segment ->> 'on_ms')::BIGINT
       OR (segment ->> 'transition_uncertainty_ms')::BIGINT >
          (segment ->> 'covered_ms')::BIGINT
       OR (segment ->> 'transition_count')::BIGINT >
          (segment ->> 'observation_count')::BIGINT
       OR (segment ->> 'first_observed_at_epoch_ms')::BIGINT >
          (segment ->> 'last_observed_at_epoch_ms')::BIGINT
       OR jsonb_typeof(segment -> 'source_summary') <> 'object'
  ) THEN
    RAISE EXCEPTION 'invalid light usage segment';
  END IF;

  IF EXISTS (
    SELECT 1
    FROM jsonb_array_elements(p_segments) incoming
    JOIN public.server_light_usage_segments existing
      ON existing.user_id = p_user_id
     AND existing.hub_id = p_hub_id
     AND existing.segment_id = incoming ->> 'segment_id'
    WHERE existing.home_id <> p_home_id
       OR existing.subject_id <> incoming ->> 'subject_id'
       OR existing.subject_kind <> incoming ->> 'subject_kind'
       OR existing.usage_date <> (incoming ->> 'usage_date')::DATE
  ) THEN
    RAISE EXCEPTION 'light usage segment identity conflict';
  END IF;

  INSERT INTO public.server_light_usage_segments (
    user_id,
    home_id,
    hub_id,
    server_instance_id,
    segment_id,
    subject_id,
    subject_kind,
    usage_date,
    revision,
    on_ms,
    covered_ms,
    transition_uncertainty_ms,
    observation_count,
    transition_count,
    first_observed_at,
    last_observed_at,
    source_summary,
    last_batch_id,
    last_seen_at
  )
  SELECT
    p_user_id,
    p_home_id,
    p_hub_id,
    NULLIF(BTRIM(p_server_instance_id), ''),
    segment ->> 'segment_id',
    segment ->> 'subject_id',
    segment ->> 'subject_kind',
    (segment ->> 'usage_date')::DATE,
    (segment ->> 'revision')::BIGINT,
    (segment ->> 'on_ms')::BIGINT,
    (segment ->> 'covered_ms')::BIGINT,
    (segment ->> 'transition_uncertainty_ms')::BIGINT,
    (segment ->> 'observation_count')::BIGINT,
    (segment ->> 'transition_count')::BIGINT,
    TO_TIMESTAMP((segment ->> 'first_observed_at_epoch_ms')::DOUBLE PRECISION / 1000.0),
    TO_TIMESTAMP((segment ->> 'last_observed_at_epoch_ms')::DOUBLE PRECISION / 1000.0),
    segment -> 'source_summary',
    p_batch_id,
    NOW()
  FROM jsonb_array_elements(p_segments) segment
  ON CONFLICT (user_id, hub_id, segment_id) DO UPDATE
  SET
    server_instance_id = EXCLUDED.server_instance_id,
    revision = EXCLUDED.revision,
    on_ms = EXCLUDED.on_ms,
    covered_ms = EXCLUDED.covered_ms,
    transition_uncertainty_ms = EXCLUDED.transition_uncertainty_ms,
    observation_count = EXCLUDED.observation_count,
    transition_count = EXCLUDED.transition_count,
    first_observed_at = EXCLUDED.first_observed_at,
    last_observed_at = EXCLUDED.last_observed_at,
    source_summary = EXCLUDED.source_summary,
    last_batch_id = EXCLUDED.last_batch_id,
    last_seen_at = NOW()
  WHERE EXCLUDED.revision > server_light_usage_segments.revision;

  GET DIAGNOSTICS affected_rows = ROW_COUNT;
  RETURN affected_rows;
END;
$$;

REVOKE ALL ON FUNCTION public.ingest_server_light_usage_batch(
  UUID, UUID, UUID, TEXT, INTEGER, TEXT, JSONB
) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.ingest_server_light_usage_batch(
  UUID, UUID, UUID, TEXT, INTEGER, TEXT, JSONB
) FROM anon;
REVOKE ALL ON FUNCTION public.ingest_server_light_usage_batch(
  UUID, UUID, UUID, TEXT, INTEGER, TEXT, JSONB
) FROM authenticated;
GRANT EXECUTE ON FUNCTION public.ingest_server_light_usage_batch(
  UUID, UUID, UUID, TEXT, INTEGER, TEXT, JSONB
) TO service_role;

DROP VIEW IF EXISTS public.rhythm_support_server_light_usage_daily;
CREATE VIEW public.rhythm_support_server_light_usage_daily
WITH (security_barrier = true)
AS
SELECT
  user_id,
  home_id,
  hub_id,
  server_instance_id,
  usage_date,
  subject_kind,
  SUM(on_ms)::BIGINT AS on_ms,
  SUM(covered_ms)::BIGINT AS covered_ms,
  SUM(transition_uncertainty_ms)::BIGINT AS transition_uncertainty_ms,
  SUM(observation_count)::BIGINT AS observation_count,
  COUNT(*)::BIGINT AS segment_count,
  COUNT(*) FILTER (
    WHERE last_seen_at < NOW() - INTERVAL '12 hours'
  )::BIGINT AS stale_segment_count,
  MAX(last_seen_at) AS last_upload_at
FROM public.server_light_usage_segments
WHERE public.is_rhythm_staff()
GROUP BY user_id, home_id, hub_id, server_instance_id, usage_date, subject_kind;

REVOKE ALL ON public.rhythm_support_server_light_usage_daily FROM PUBLIC;
GRANT SELECT ON public.rhythm_support_server_light_usage_daily TO authenticated;

COMMENT ON TABLE public.server_light_usage_segments IS
  'Revisioned subject/day observed-power aggregates uploaded by Rhythm OS; no command intent or per-poll rows';
COMMENT ON FUNCTION public.ingest_server_light_usage_batch(
  UUID, UUID, UUID, TEXT, INTEGER, TEXT, JSONB
) IS 'Service-role-only transactional ingest for one scoped absolute light usage batch';
COMMENT ON VIEW public.rhythm_support_server_light_usage_daily IS
  'Staff-safe daily light usage totals by observation scope without canonical subject identifiers';

NOTIFY pgrst, 'reload schema';
