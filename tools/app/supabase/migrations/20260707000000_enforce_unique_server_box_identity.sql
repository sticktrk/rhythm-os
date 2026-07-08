-- A physical Rhythm Box must have a single cloud server-hub identity.
--
-- Previous constraints only deduped server_instance_id inside one Home, which
-- allowed the same Box to be represented by two Homes and then receive two
-- remote-access tunnels. Keep the most useful existing row for each identity,
-- clear duplicate identity/tunnel metadata, then enforce uniqueness globally.

UPDATE public.hubs
SET server_instance_id = NULL
WHERE server_instance_id IS NOT NULL
  AND btrim(server_instance_id) = '';

UPDATE public.hub_remote_access
SET server_instance_id = NULL
WHERE server_instance_id IS NOT NULL
  AND btrim(server_instance_id) = '';

UPDATE public.hubs
SET server_instance_id = lower(btrim(server_instance_id))
WHERE server_instance_id IS NOT NULL
  AND server_instance_id <> lower(btrim(server_instance_id));

UPDATE public.hub_remote_access
SET server_instance_id = lower(btrim(server_instance_id))
WHERE server_instance_id IS NOT NULL
  AND server_instance_id <> lower(btrim(server_instance_id));

WITH ranked_hubs AS (
  SELECT
    hubs.id,
    ROW_NUMBER() OVER (
      PARTITION BY hubs.server_instance_id
      ORDER BY
        (support.hub_id IS NOT NULL) DESC,
        (remote.hub_id IS NOT NULL) DESC,
        (hubs.remote_endpoint IS NOT NULL) DESC,
        hubs.created_at ASC,
        hubs.updated_at DESC,
        hubs.last_connected DESC NULLS LAST,
        hubs.id ASC
    ) AS duplicate_rank
  FROM public.hubs hubs
  LEFT JOIN (
    SELECT DISTINCT hub_id
    FROM public.hub_support_access_grants
    WHERE revoked_at IS NULL
      AND (expires_at IS NULL OR expires_at > NOW())
  ) support
    ON support.hub_id = hubs.id
  LEFT JOIN public.hub_remote_access remote
    ON remote.hub_id = hubs.id
   AND remote.server_instance_id = hubs.server_instance_id
  WHERE hubs.type = 'server'
    AND hubs.server_instance_id IS NOT NULL
),
duplicate_hubs AS (
  SELECT id
  FROM ranked_hubs
  WHERE duplicate_rank > 1
)
UPDATE public.hubs hubs
SET
  server_instance_id = NULL,
  remote_endpoint = NULL
FROM duplicate_hubs
WHERE hubs.id = duplicate_hubs.id;

WITH ranked_remote_access AS (
  SELECT
    remote.hub_id,
    ROW_NUMBER() OVER (
      PARTITION BY remote.server_instance_id
      ORDER BY
        (support.hub_id IS NOT NULL) DESC,
        (hubs.server_instance_id = remote.server_instance_id) DESC,
        hubs.created_at ASC NULLS LAST,
        remote.updated_at DESC,
        remote.created_at DESC,
        remote.hub_id ASC
    ) AS duplicate_rank
  FROM public.hub_remote_access remote
  LEFT JOIN public.hubs hubs
    ON hubs.id = remote.hub_id
  LEFT JOIN (
    SELECT DISTINCT hub_id
    FROM public.hub_support_access_grants
    WHERE revoked_at IS NULL
      AND (expires_at IS NULL OR expires_at > NOW())
  ) support
    ON support.hub_id = remote.hub_id
  WHERE remote.server_instance_id IS NOT NULL
),
duplicate_remote_access AS (
  SELECT hub_id
  FROM ranked_remote_access
  WHERE duplicate_rank > 1
)
UPDATE public.hubs hubs
SET remote_endpoint = NULL
FROM duplicate_remote_access
WHERE hubs.id = duplicate_remote_access.hub_id;

WITH ranked_remote_access AS (
  SELECT
    remote.hub_id,
    ROW_NUMBER() OVER (
      PARTITION BY remote.server_instance_id
      ORDER BY
        (support.hub_id IS NOT NULL) DESC,
        (hubs.server_instance_id = remote.server_instance_id) DESC,
        hubs.created_at ASC NULLS LAST,
        remote.updated_at DESC,
        remote.created_at DESC,
        remote.hub_id ASC
    ) AS duplicate_rank
  FROM public.hub_remote_access remote
  LEFT JOIN public.hubs hubs
    ON hubs.id = remote.hub_id
  LEFT JOIN (
    SELECT DISTINCT hub_id
    FROM public.hub_support_access_grants
    WHERE revoked_at IS NULL
      AND (expires_at IS NULL OR expires_at > NOW())
  ) support
    ON support.hub_id = remote.hub_id
  WHERE remote.server_instance_id IS NOT NULL
),
duplicate_remote_access AS (
  SELECT hub_id
  FROM ranked_remote_access
  WHERE duplicate_rank > 1
)
UPDATE public.hub_remote_access remote
SET server_instance_id = NULL
FROM duplicate_remote_access
WHERE remote.hub_id = duplicate_remote_access.hub_id;

CREATE UNIQUE INDEX IF NOT EXISTS hubs_server_instance_id_unique_idx
  ON public.hubs ((lower(btrim(server_instance_id))))
  WHERE type = 'server'
    AND server_instance_id IS NOT NULL
    AND btrim(server_instance_id) <> '';

CREATE UNIQUE INDEX IF NOT EXISTS hub_remote_access_server_instance_id_unique_idx
  ON public.hub_remote_access ((lower(btrim(server_instance_id))))
  WHERE server_instance_id IS NOT NULL
    AND btrim(server_instance_id) <> '';

NOTIFY pgrst, 'reload schema';
