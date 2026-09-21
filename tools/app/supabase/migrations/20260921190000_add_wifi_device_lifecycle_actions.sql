-- Provisioning outcomes reuse the authenticated, privacy-bounded lifecycle feed.
BEGIN;
ALTER TABLE public.server_device_lifecycle_events
  DROP CONSTRAINT server_device_lifecycle_events_action_check;
ALTER TABLE public.server_device_lifecycle_events
  ADD CONSTRAINT server_device_lifecycle_events_action_check
  CHECK (action IN ('pair', 'unpair', 'wifi_profile', 'wifi_change'));
COMMIT;
