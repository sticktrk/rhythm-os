-- The support service relies on the provisioning service's binding of a relay
-- to a hub, home, and durable installation. Apps configure tunnels through
-- remote-access-bootstrap. Household members retain their existing read view.
REVOKE INSERT, UPDATE, DELETE ON public.hub_remote_access FROM PUBLIC, anon, authenticated;
GRANT SELECT, INSERT, UPDATE, DELETE ON public.hub_remote_access TO service_role;
