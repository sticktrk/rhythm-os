-- Ensure server activity upserts have the conflict key they declare.
--
-- The original table definition includes UNIQUE (user_id, hub_id, event_id),
-- but CREATE TABLE IF NOT EXISTS will not add that constraint to an already
-- existing table. Repair existing deployments before server-activity-ingest
-- calls .upsert(..., { onConflict: 'user_id,hub_id,event_id' }).

WITH ranked AS (
  SELECT
    id,
    ROW_NUMBER() OVER (
      PARTITION BY user_id, hub_id, event_id
      ORDER BY occurred_at DESC, last_seen_at DESC, created_at DESC, id DESC
    ) AS duplicate_rank
  FROM public.server_light_activity_events
)
DELETE FROM public.server_light_activity_events events
USING ranked
WHERE events.id = ranked.id
  AND ranked.duplicate_rank > 1;

DO $$
BEGIN
  IF NOT EXISTS (
    SELECT 1
    FROM pg_index indexes
    JOIN pg_class tables
      ON tables.oid = indexes.indrelid
    JOIN pg_namespace namespaces
      ON namespaces.oid = tables.relnamespace
    WHERE namespaces.nspname = 'public'
      AND tables.relname = 'server_light_activity_events'
      AND indexes.indisunique
      AND indexes.indpred IS NULL
      AND indexes.indkey::TEXT = (
        SELECT STRING_AGG(attributes.attnum::TEXT, ' ' ORDER BY expected.ordinality)
        FROM (
          VALUES
            ('user_id', 1),
            ('hub_id', 2),
            ('event_id', 3)
        ) AS expected(attname, ordinality)
        JOIN pg_attribute attributes
          ON attributes.attrelid = indexes.indrelid
         AND attributes.attname = expected.attname
      )
  ) THEN
    CREATE UNIQUE INDEX server_light_activity_events_user_hub_event_idx
      ON public.server_light_activity_events (user_id, hub_id, event_id);
  END IF;
END $$;

NOTIFY pgrst, 'reload schema';
