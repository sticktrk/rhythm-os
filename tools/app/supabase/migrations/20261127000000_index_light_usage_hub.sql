-- pg-delta: transaction=false
-- Home removal cascades through hubs. PostgreSQL looks up usage segments
-- by hub_id alone, including when an installation has no recorded usage.
CREATE INDEX CONCURRENTLY server_light_usage_hub_idx
  ON public.server_light_usage_segments (hub_id);
