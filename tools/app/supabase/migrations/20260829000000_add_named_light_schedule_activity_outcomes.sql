-- Expose bounded named-schedule outcomes through the existing staff-only
-- support view. Names, stable IDs, request payloads, and raw activity remain
-- excluded from the projection.
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
  CASE
    WHEN action_id IN (
      'room_schedule_config_updated',
      'room_schedule_boundary',
      'room_schedule_test_wake',
      'room_schedule_test_sleep',
      'light_schedule_config_updated',
      'light_schedule_assignment_updated',
      'light_schedule_boundary'
    ) AND payload ->> 'status' IN ('accepted', 'applied', 'rejected', 'failed')
    THEN payload ->> 'status'
  END AS bounded_outcome,
  CASE
    WHEN action_id IN (
      'room_schedule_config_updated',
      'room_schedule_boundary',
      'room_schedule_test_wake',
      'room_schedule_test_sleep',
      'light_schedule_boundary'
    ) AND payload ->> 'failure_stage' IN ('persistence', 'output_apply')
    THEN payload ->> 'failure_stage'
  END AS bounded_failure_stage,
  CASE
    WHEN action_id IN ('room_schedule_config_updated', 'room_schedule_boundary')
      AND payload ->> 'source' IN ('wake_sleep_presets', 'follow_time')
    THEN payload ->> 'source'
  END AS bounded_schedule_source,
  CASE
    WHEN action_id = 'light_schedule_assignment_updated'
      AND payload ->> 'binding_kind' IN ('legacy', 'named', 'unscheduled')
    THEN payload ->> 'binding_kind'
  END AS bounded_schedule_binding_kind,
  CASE
    WHEN action_id = 'light_schedule_boundary'
      AND payload ->> 'trigger_kind' IN ('manual', 'solar', 'scheduled')
    THEN payload ->> 'trigger_kind'
  END AS bounded_schedule_trigger_kind,
  CASE
    WHEN action_id = 'light_schedule_boundary'
      AND (payload ->> 'target_count') ~ '^[0-9]+$'
    THEN LEAST((payload ->> 'target_count')::INTEGER, 10000)
  END AS bounded_schedule_target_count,
  CASE
    WHEN action_id = 'light_schedule_config_updated'
      AND (payload ->> 'schedule_count') ~ '^[0-9]+$'
    THEN LEAST((payload ->> 'schedule_count')::INTEGER, 1000)
  END AS bounded_schedule_count,
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
REVOKE ALL ON public.rhythm_support_server_light_activity_events FROM anon;
REVOKE ALL ON public.rhythm_support_server_light_activity_events FROM authenticated;
GRANT SELECT ON public.rhythm_support_server_light_activity_events TO authenticated;

COMMENT ON VIEW public.rhythm_support_server_light_activity_events IS
  'Support-safe light activity metadata with bounded room and named light schedule outcomes; raw payload bodies remain excluded';
