BEGIN;

DO $$
DECLARE
  owner_id uuid := gen_random_uuid();
  other_owner_id uuid := gen_random_uuid();
  removed_home uuid := gen_random_uuid();
  retained_home uuid := gen_random_uuid();
  removed_hub uuid := gen_random_uuid();
  retained_hub uuid := gen_random_uuid();
  deleted_count integer;
BEGIN
  -- A trailing hub_id in the (user_id, hub_id, segment_id) unique constraint
  -- cannot bound the cascade's search across the entire usage history.
  IF NOT EXISTS (
    SELECT 1
    FROM pg_index i
    JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = i.indkey[0]
    JOIN pg_class c ON c.oid = i.indexrelid
    JOIN pg_am am ON am.oid = c.relam
    WHERE i.indrelid = 'public.server_light_usage_segments'::regclass
      AND a.attname = 'hub_id' AND am.amname = 'btree'
      AND i.indisvalid AND i.indisready AND i.indpred IS NULL
  ) THEN
    RAISE EXCEPTION 'Usage segment cascades need an index starting with hub_id';
  END IF;

  INSERT INTO auth.users (id) VALUES (owner_id), (other_owner_id);
  INSERT INTO public.homes (id, name, owner_id, member_ids) VALUES
    (removed_home, 'Removed home', owner_id, ARRAY[owner_id]),
    (retained_home, 'Retained home', other_owner_id, ARRAY[other_owner_id]);
  INSERT INTO public.hubs (id, home_id, type, name, endpoint) VALUES
    (removed_hub, removed_home, 'server', 'Removed hub', '{}'),
    (retained_hub, retained_home, 'server', 'Retained hub', '{}');
  INSERT INTO public.server_light_usage_segments (
    user_id, home_id, hub_id, segment_id, subject_id, subject_kind, usage_date,
    revision, on_ms, covered_ms, transition_uncertainty_ms, observation_count,
    transition_count, first_observed_at, last_observed_at, last_batch_id
  )
  SELECT user_id, home_id, hub_id, md5('segment'), 'fixture', 'bulb', CURRENT_DATE,
    1, 0, 1, 0, 1, 0, now(), now(), md5('batch')
  FROM (VALUES
    (owner_id, removed_home, removed_hub),
    (other_owner_id, retained_home, retained_hub)
  ) AS samples(user_id, home_id, hub_id);

  PERFORM set_config('request.jwt.claim.sub', owner_id::text, true);
  SET LOCAL ROLE authenticated;
  DELETE FROM public.homes WHERE id = removed_home;
  GET DIAGNOSTICS deleted_count = ROW_COUNT;
  RESET ROLE;

  IF deleted_count <> 1
      OR EXISTS (SELECT 1 FROM public.hubs WHERE id = removed_hub)
      OR EXISTS (SELECT 1 FROM public.server_light_usage_segments WHERE hub_id = removed_hub)
      OR NOT EXISTS (SELECT 1 FROM public.homes WHERE id = retained_home)
      OR NOT EXISTS (SELECT 1 FROM public.hubs WHERE id = retained_hub)
      OR NOT EXISTS (SELECT 1 FROM public.server_light_usage_segments WHERE hub_id = retained_hub) THEN
    RAISE EXCEPTION 'Removing the owner home must delete only its hubs and usage segments';
  END IF;
END $$;

ROLLBACK;
