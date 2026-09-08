-- Add only the low-cardinality commissioner boundary. Phone-provided setup
-- payloads, passcodes, and network addresses remain excluded from cloud rows.

ALTER TABLE public.server_device_lifecycle_events
  ADD COLUMN IF NOT EXISTS commissioner TEXT CHECK (
    commissioner IS NULL OR commissioner IN ('phone', 'server')
  );

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
  commissioner,
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
REVOKE ALL ON public.rhythm_support_server_device_lifecycle_events FROM anon;
REVOKE ALL ON public.rhythm_support_server_device_lifecycle_events FROM authenticated;
GRANT SELECT ON public.rhythm_support_server_device_lifecycle_events TO authenticated;

COMMENT ON COLUMN public.server_device_lifecycle_events.commissioner IS
  'Low-cardinality device commissioning boundary: phone or server; never handoff material';
COMMENT ON VIEW public.rhythm_support_server_device_lifecycle_events IS
  'Support-safe pair/unpair outcomes without device identity, names, addresses, setup payloads, passcodes, serials, warnings, or raw errors';
