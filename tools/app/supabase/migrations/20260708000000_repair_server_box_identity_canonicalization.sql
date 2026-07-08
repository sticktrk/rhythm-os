-- Follow-up hardening for physical Rhythm Box identity canonicalization.
--
-- Some environments may have run an older cleanup that selected the newest
-- server-hub row instead of the customer/support canonical row. Prefer active
-- support-granted hubs when two server rows share the same physical endpoint,
-- then re-run duplicate cleanup and enforce global uniqueness.

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

DROP TABLE IF EXISTS pg_temp.server_identity_transfers;
CREATE TEMP TABLE server_identity_transfers AS
WITH active_support AS (
  SELECT DISTINCT hub_id
  FROM public.hub_support_access_grants
  WHERE revoked_at IS NULL
    AND (expires_at IS NULL OR expires_at > NOW())
),
ranked AS (
  SELECT
    canonical.id AS canonical_hub_id,
    duplicate.id AS duplicate_hub_id,
    lower(btrim(duplicate.server_instance_id)) AS server_instance_id,
    ROW_NUMBER() OVER (
      PARTITION BY lower(btrim(duplicate.server_instance_id))
      ORDER BY
        canonical.created_at ASC,
        canonical.updated_at DESC,
        canonical.id ASC
    ) AS transfer_rank
  FROM public.hubs canonical
  JOIN active_support support
    ON support.hub_id = canonical.id
  JOIN public.hubs duplicate
    ON duplicate.type = 'server'
   AND duplicate.id <> canonical.id
   AND duplicate.endpoint = canonical.endpoint
  WHERE canonical.type = 'server'
    AND duplicate.server_instance_id IS NOT NULL
    AND btrim(duplicate.server_instance_id) <> ''
    AND (
      canonical.server_instance_id IS NULL
      OR lower(btrim(canonical.server_instance_id)) =
        lower(btrim(duplicate.server_instance_id))
    )
)
SELECT canonical_hub_id, duplicate_hub_id, server_instance_id
FROM ranked
WHERE transfer_rank = 1;

UPDATE public.hubs duplicate
SET
  server_instance_id = NULL,
  remote_endpoint = NULL
FROM server_identity_transfers transfers
WHERE duplicate.id = transfers.duplicate_hub_id;

UPDATE public.hubs canonical
SET
  server_instance_id = transfers.server_instance_id,
  remote_endpoint = COALESCE(
    canonical.remote_endpoint,
    (
      SELECT jsonb_build_object(
        'host', remote.hostname,
        'port', 443,
        'useSsl', true
      )
      FROM public.hub_remote_access remote
      WHERE remote.hub_id = canonical.id
      LIMIT 1
    )
  )
FROM server_identity_transfers transfers
WHERE canonical.id = transfers.canonical_hub_id;

UPDATE public.hub_remote_access remote
SET
  server_instance_id = transfers.server_instance_id,
  updated_at = NOW()
FROM server_identity_transfers transfers
WHERE remote.hub_id = transfers.canonical_hub_id;

DELETE FROM public.hub_remote_access remote
USING server_identity_transfers transfers
WHERE remote.hub_id = transfers.duplicate_hub_id;

DROP TABLE IF EXISTS pg_temp.server_identity_keep;
CREATE TEMP TABLE server_identity_keep AS
WITH active_support AS (
  SELECT DISTINCT hub_id
  FROM public.hub_support_access_grants
  WHERE revoked_at IS NULL
    AND (expires_at IS NULL OR expires_at > NOW())
),
ranked AS (
  SELECT
    lower(btrim(hubs.server_instance_id)) AS server_instance_id,
    hubs.id AS hub_id,
    ROW_NUMBER() OVER (
      PARTITION BY lower(btrim(hubs.server_instance_id))
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
  LEFT JOIN active_support support
    ON support.hub_id = hubs.id
  LEFT JOIN public.hub_remote_access remote
    ON remote.hub_id = hubs.id
   AND remote.server_instance_id = hubs.server_instance_id
  WHERE hubs.type = 'server'
    AND hubs.server_instance_id IS NOT NULL
    AND btrim(hubs.server_instance_id) <> ''
)
SELECT server_instance_id, hub_id
FROM ranked
WHERE duplicate_rank = 1;

UPDATE public.hubs hubs
SET
  server_instance_id = NULL,
  remote_endpoint = NULL
FROM server_identity_keep keep
WHERE lower(btrim(hubs.server_instance_id)) = keep.server_instance_id
  AND hubs.id <> keep.hub_id;

DROP TABLE IF EXISTS pg_temp.remote_identity_keep;
CREATE TEMP TABLE remote_identity_keep AS
WITH active_support AS (
  SELECT DISTINCT hub_id
  FROM public.hub_support_access_grants
  WHERE revoked_at IS NULL
    AND (expires_at IS NULL OR expires_at > NOW())
),
ranked AS (
  SELECT
    lower(btrim(remote.server_instance_id)) AS server_instance_id,
    remote.hub_id,
    ROW_NUMBER() OVER (
      PARTITION BY lower(btrim(remote.server_instance_id))
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
  LEFT JOIN active_support support
    ON support.hub_id = remote.hub_id
  WHERE remote.server_instance_id IS NOT NULL
    AND btrim(remote.server_instance_id) <> ''
)
SELECT server_instance_id, hub_id
FROM ranked
WHERE duplicate_rank = 1;

UPDATE public.hubs hubs
SET remote_endpoint = NULL
FROM remote_identity_keep keep
JOIN public.hub_remote_access remote
  ON lower(btrim(remote.server_instance_id)) = keep.server_instance_id
 AND remote.hub_id <> keep.hub_id
WHERE hubs.id = remote.hub_id;

UPDATE public.hub_remote_access remote
SET
  server_instance_id = NULL,
  updated_at = NOW()
FROM remote_identity_keep keep
WHERE lower(btrim(remote.server_instance_id)) = keep.server_instance_id
  AND remote.hub_id <> keep.hub_id;

CREATE UNIQUE INDEX IF NOT EXISTS hubs_server_instance_id_unique_idx
  ON public.hubs ((lower(btrim(server_instance_id))))
  WHERE type = 'server'
    AND server_instance_id IS NOT NULL
    AND btrim(server_instance_id) <> '';

CREATE UNIQUE INDEX IF NOT EXISTS hub_remote_access_server_instance_id_unique_idx
  ON public.hub_remote_access ((lower(btrim(server_instance_id))))
  WHERE server_instance_id IS NOT NULL
    AND btrim(server_instance_id) <> '';

DROP TABLE IF EXISTS pg_temp.remote_identity_keep;
DROP TABLE IF EXISTS pg_temp.server_identity_keep;
DROP TABLE IF EXISTS pg_temp.server_identity_transfers;

NOTIFY pgrst, 'reload schema';
