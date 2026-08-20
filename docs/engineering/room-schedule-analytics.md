# Room Schedule analytics proof

This contract instruments future room-schedule journeys without room, home,
hub, device, or customer identity in PostHog. One opaque `journey_id` is reused
as the server `request_id` across retries of the same save or test. Exact wake
and sleep times, physical light state, raw errors, and unrestricted payloads
are prohibited.

## PostHog funnel

Insight definition: `Room Schedule — opened to terminal outcome`.

- Query type: funnel, unique users, ordered steps, 60-minute conversion window.
- Step 1: `room_schedule_opened`, filtered to `source = room_settings`.
- Step 2: either `room_schedule_save_completed` or
  `room_schedule_test_completed`.
- Breakdown: Step 2 `source`, then `outcome`.
- Save diagnostics: `change_kind`, `input_method`, and bounded
  `failure_stage`.
- Test diagnostics: `action`, `input_method`, and bounded `failure_stage`.
- Retry analysis: group by `journey_id` and inspect numeric `attempt_number`;
  attempts for one journey must share the same opaque ID.

Success is `outcome = succeeded`; failure is `outcome = failed`. Analytics is
non-blocking and an unconfigured PostHog backend remains a no-op. This insight
is forward-only; no historical events are synthesized.

The release-channel join check selects one test journey and verifies that the
same ID appears in the matching attempt and terminal events, with no forbidden
properties:

```sql
SELECT
  properties.journey_id AS journey_id,
  groupArray(event) AS events,
  max(toInt(properties.attempt_number)) AS attempts
FROM events
WHERE event IN (
  'room_schedule_save_attempted',
  'room_schedule_save_completed',
  'room_schedule_test_completed'
)
  AND timestamp >= now() - INTERVAL 24 HOUR
  AND properties.journey_id IS NOT NULL
GROUP BY properties.journey_id
HAVING length(events) >= 2
ORDER BY max(timestamp) DESC
LIMIT 20
```

Inspect the selected events to confirm they contain none of `room_id`,
`room_name`, `home_id`, `device_id`, `wake_time`, `sleep_time`, or `error`.

## Staff-safe server outcome query

The existing `server_light_activity_events` ingest, authentication, RLS,
deduplication, retention, and deletion behavior is unchanged. Migration
`20260819000000_add_room_schedule_activity_outcomes.sql` extends only the
staff-only support view with allowlisted outcome categories; it does not expose
the raw payload.

```sql
SELECT
  action_id,
  bounded_schedule_source,
  bounded_outcome,
  bounded_failure_stage,
  count(*) AS event_count,
  count(*) FILTER (WHERE correlation_id IS NOT NULL) AS correlated_count
FROM public.rhythm_support_server_light_activity_events
WHERE occurred_at >= now() - interval '24 hours'
  AND action_id IN (
    'room_schedule_config_updated',
    'room_schedule_boundary',
    'room_schedule_test_wake',
    'room_schedule_test_sleep'
  )
GROUP BY
  action_id,
  bounded_schedule_source,
  bounded_outcome,
  bounded_failure_stage
ORDER BY action_id, bounded_outcome
LIMIT 100;
```

For an app-initiated save or test, verify one PostHog `journey_id` equals the
server row's `correlation_id`. Autonomous boundary evaluations use their own
stable evaluation identity and intentionally have no PostHog event.
