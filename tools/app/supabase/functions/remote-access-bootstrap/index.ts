import {
  errorMessage,
  jsonResponse,
  readJson,
  readString,
  type JsonObject,
  withAuthenticatedRequest,
} from '../_shared/auth.ts'

const CLOUDFLARE_API_BASE = 'https://api.cloudflare.com/client/v4'
const DEFAULT_REMOTE_ACCESS_DOMAIN = 'rhythm.lighting'
const LEGACY_REMOTE_ACCESS_DOMAIN = 'devices.rhythm.lighting'
const DEFAULT_ORIGIN_SERVICE = 'http://localhost:54448'

type CloudflareResponse<T> = {
  success: boolean
  errors?: Array<{ message?: string }>
  result: T
}

type TunnelResult = {
  id: string
  name: string
  token?: string
}

type ListedTunnelResult = TunnelResult & {
  deleted_at?: string | null
}

type DnsRecord = {
  id: string
  name: string
  type: string
  content: string
  proxied: boolean
}

type HomeRow = {
  id: string
  owner_id?: string
  member_ids?: string[]
}

type HubRow = {
  id: string
  home_id: string
  type: string
  server_instance_id?: string | null
  remote_endpoint?: unknown
  remote_access_disabled_at?: string | null
  last_connected?: string | null
  created_at?: string | null
  updated_at?: string | null
}

type RemoteAccessAction = 'enable' | 'disable'

type RemoteAccessEndpoint = {
  host: string
  port: number
  useSsl: boolean
}

type RemoteAccessMapping = {
  hub_id: string
  home_id: string
  hostname: string
  tunnel_id: string
  tunnel_name: string
  server_instance_id?: string | null
  created_at?: string | null
  updated_at?: string | null
}

Deno.serve((req) =>
  withAuthenticatedRequest(req, async ({ userId, adminClient }) => {
    if (req.method !== 'POST') {
      return jsonResponse({ error: 'Method not allowed' }, 405)
    }

    const body = await readJson(req)
    const hubId = readString(body, 'hub_id')
    if (!hubId) {
      return jsonResponse({ error: 'Missing hub_id' }, 400)
    }
    const actionResult = readRemoteAccessAction(body)
    if (actionResult instanceof Response) return actionResult
    const action = actionResult

    const serverInstanceId = readServerInstanceId(body)
    const serverEndpoint = readServerEndpoint(body)
    const ensured = await ensureServerHubRows(
      adminClient,
      userId,
      hubId,
      body,
      serverInstanceId,
    )
    if (ensured instanceof Response) return ensured
    const { hub } = ensured
    const explicitEnable = body['explicit_enable'] === true

    try {
      if (action === 'disable') {
        return await disableRemoteAccess({
          adminClient,
          userId,
          hubId: hub.id,
          homeId: hub.home_id,
          serverInstanceId,
          serverEndpoint,
        })
      }

      if (hub.remote_access_disabled_at && !explicitEnable) {
        return jsonResponse({
          status: 'disabled_by_user',
          hub_id: hub.id,
          requested_hub_id: hubId,
        })
      }
      if (explicitEnable && hub.remote_access_disabled_at) {
        await setRemoteAccessDisabled(adminClient, hub.id, false)
      }

      const accountId = requireEnv('CLOUDFLARE_ACCOUNT_ID')
      const zoneId = requireEnv('CLOUDFLARE_ZONE_ID')
      const apiToken = requireCloudflareApiToken()
      const domain = remoteAccessDomain()
      const originService =
        readEnv('RHYTHM_REMOTE_ACCESS_ORIGIN', DEFAULT_ORIGIN_SERVICE)

      const existing = await readExistingMapping(adminClient, {
        userId,
        hubId: hub.id,
        homeId: hub.home_id,
        serverInstanceId,
        serverEndpoint,
      })
      const tunnelName = existing?.tunnel_name ?? `rhythm-${hub.id}`
      const desiredHostname = `${hub.id}.${domain}`.toLowerCase()
      const hostname = hostnameForDomain(
        existing?.hostname,
        desiredHostname,
      )
      const tunnel = existing
        ? { id: existing.tunnel_id, name: existing.tunnel_name }
        : await createTunnel(accountId, apiToken, tunnelName)
      const connectorToken =
        tunnel.token ?? (await getTunnelToken(accountId, apiToken, tunnel.id))

      await putTunnelConfiguration(
        accountId,
        apiToken,
        tunnel.id,
        hostname,
        originService,
      )
      await upsertDnsRecord(
        zoneId,
        apiToken,
        hostname,
        `${tunnel.id}.cfargotunnel.com`,
      )

      const remoteEndpoint = endpointForHostname(hostname)
      await saveRemoteAccessMapping(adminClient, {
        hubId: hub.id,
        existingHubId: existing?.hub_id,
        homeId: hub.home_id,
        hostname,
        tunnelId: tunnel.id,
        tunnelName: tunnel.name,
        serverInstanceId,
      })
      const { data: endpointHub, error: endpointUpdateError } = await adminClient
        .from('hubs')
        .update({ remote_endpoint: remoteEndpoint })
        .eq('id', hub.id)
        .is('remote_access_disabled_at', null)
        .select('id')
        .maybeSingle()
      if (endpointUpdateError) throw new Error(endpointUpdateError.message)
      if (!endpointHub) {
        // A disable won the race while Cloudflare was provisioning. Remove
        // everything this request may have created instead of resurrecting
        // the route after the user turned it off.
        await deleteDnsRecordIfPresent(zoneId, apiToken, hostname)
        await deleteTunnelIfPresent(accountId, apiToken, tunnel.id)
        await deleteRemoteAccessMapping(adminClient, hub.id)
        return jsonResponse({
          status: 'disabled_by_user',
          hub_id: hub.id,
          requested_hub_id: hubId,
        })
      }
      if (existing && existing.hub_id !== hub.id) {
        await clearHubRemoteEndpoint(adminClient, existing.hub_id)
      }

      return jsonResponse({
        status: 'ok',
        hub_id: hub.id,
        requested_hub_id: hubId,
        hostname,
        remote_url: `https://${hostname}`,
        remote_endpoint: remoteEndpoint,
        tunnel_id: tunnel.id,
        tunnel_name: tunnel.name,
        server_instance_id: serverInstanceId,
        connector_token: connectorToken,
      })
    } catch (error) {
      return jsonResponse({ error: errorMessage(error) }, 500)
    }
  })
)

function readRemoteAccessAction(body: JsonObject): RemoteAccessAction | Response {
  const action = readString(body, 'action')
  if (!action || action === 'enable') return 'enable'
  if (action === 'disable') return 'disable'
  return jsonResponse({ error: 'Invalid action' }, 400)
}

function readServerInstanceId(body: JsonObject): string | null {
  const raw =
    readString(body, 'server_instance_id') ??
    readString(body, 'remote_access_device_key')
  const value = raw?.trim().toLowerCase()
  if (!value) return null
  return value.slice(0, 256)
}

function readServerEndpoint(body: JsonObject): RemoteAccessEndpoint | null {
  const hubSnapshot = readJsonObject(body, 'server_hub')
  if (!hubSnapshot) return null
  try {
    return normalizeEndpoint(hubSnapshot, 'endpoint')
  } catch (_) {
    return null
  }
}

async function disableRemoteAccess({
  adminClient,
  userId,
  hubId,
  homeId,
  serverInstanceId,
  serverEndpoint,
}: {
  adminClient: any
  userId: string
  hubId: string
  homeId: string
  serverInstanceId: string | null
  serverEndpoint: RemoteAccessEndpoint | null
}): Promise<Response> {
  // Persist intent first. Even if Cloudflare is temporarily unavailable, a
  // different client must not race this request and recreate the tunnel.
  await setRemoteAccessDisabled(adminClient, hubId, true)

  const existing = await readExistingMapping(adminClient, {
    userId,
    hubId,
    homeId,
    serverInstanceId,
    serverEndpoint,
  })

  if (existing) {
    const accountId = requireEnv('CLOUDFLARE_ACCOUNT_ID')
    const zoneId = requireEnv('CLOUDFLARE_ZONE_ID')
    const apiToken = requireCloudflareApiToken()

    await deleteDnsRecordIfPresent(zoneId, apiToken, existing.hostname)
    await deleteTunnelIfPresent(accountId, apiToken, existing.tunnel_id)
    await deleteRemoteAccessMapping(adminClient, existing.hub_id)
  } else {
    const accountId = readOptionalEnv('CLOUDFLARE_ACCOUNT_ID')
    const zoneId = readOptionalEnv('CLOUDFLARE_ZONE_ID')
    const apiToken = readCloudflareApiToken()

    if (accountId && zoneId && apiToken) {
      const domain = remoteAccessDomain()
      await deleteDnsRecordIfPresent(
        zoneId,
        apiToken,
        `${hubId}.${domain}`.toLowerCase(),
      )
      await deleteTunnelByNameIfPresent(accountId, apiToken, `rhythm-${hubId}`)
    }
  }

  await clearHubRemoteEndpoint(adminClient, hubId)
  if (existing && existing.hub_id !== hubId) {
    await clearHubRemoteEndpoint(adminClient, existing.hub_id)
  }

  return jsonResponse({
    status: 'ok',
    hub_id: hubId,
    remote_access_deleted: existing != null,
    server_instance_id: serverInstanceId,
  })
}

async function ensureServerHubRows(
  adminClient: any,
  userId: string,
  hubId: string,
  body: JsonObject,
  serverInstanceId: string | null,
): Promise<{ hub: HubRow; home: HomeRow } | Response> {
  try {
    const homeSnapshot = readJsonObject(body, 'home')
    const hubSnapshot = readJsonObject(body, 'server_hub')

    if (!homeSnapshot && !hubSnapshot) {
      return await readAuthorizedExistingRows(
        adminClient,
        userId,
        hubId,
        serverInstanceId,
      )
    }
    if (!homeSnapshot || !hubSnapshot) {
      throw new RequestError(
        400,
        'Both home and server_hub are required when repairing remote access rows',
      )
    }

    const requestedHomeId = requireBodyString(homeSnapshot, 'id')
    const requestedHubId = requireBodyString(hubSnapshot, 'id')
    if (requestedHubId !== hubId) {
      throw new RequestError(400, 'server_hub.id must match hub_id')
    }

    const requestedHubHomeId = readString(hubSnapshot, 'home_id')
    if (requestedHubHomeId && requestedHubHomeId !== requestedHomeId) {
      throw new RequestError(400, 'server_hub.home_id must match home.id')
    }

    const requestedHubType = readString(hubSnapshot, 'type')
    if (requestedHubType !== 'server') {
      throw new RequestError(400, 'server_hub.type must be server')
    }

    const existingByServerInstanceId = await readAuthorizedServerHubByInstanceId(
      adminClient,
      userId,
      serverInstanceId,
      true,
    )
    if (existingByServerInstanceId instanceof Response) {
      return existingByServerInstanceId
    }
    if (
      existingByServerInstanceId &&
      (existingByServerInstanceId.hub.id !== requestedHubId ||
        existingByServerInstanceId.home.id !== requestedHomeId)
    ) {
      const { hub, home } = existingByServerInstanceId
      const hubPayload = normalizeServerHubSnapshot(
        hubSnapshot,
        hub.id,
        home.id,
        serverInstanceId,
      )
      const { error: hubUpdateError } = await adminClient
        .from('hubs')
        .upsert(hubPayload, { onConflict: 'id' })
      if (hubUpdateError) throw new Error(hubUpdateError.message)
      return { hub, home }
    }

    const existingHub = await fetchHub(adminClient, requestedHubId)
    if (existingHub) {
      if (existingHub.type !== 'server') {
        return jsonResponse({ error: 'Server hub not found' }, 404)
      }
      if (existingHub.home_id !== requestedHomeId) {
        throw new RequestError(
          409,
          'Existing server hub belongs to a different home',
        )
      }
    }

    const existingHome = await fetchHome(adminClient, requestedHomeId)
    if (existingHome && !isHomeMember(existingHome, userId)) {
      return jsonResponse({ error: 'Not authorized for this home' }, 403)
    }

    const memberIds = memberIdsForUpsert(existingHome, userId)
    const ownerId = existingHome?.owner_id ?? userId
    const homePayload = normalizeHomeSnapshot(
      homeSnapshot,
      requestedHomeId,
      ownerId,
      memberIds,
    )
    const hubPayload = normalizeServerHubSnapshot(
      hubSnapshot,
      requestedHubId,
      requestedHomeId,
      serverInstanceId,
    )

    const { error: homeUpsertError } = await adminClient
      .from('homes')
      .upsert(homePayload, { onConflict: 'id' })
    if (homeUpsertError) throw new Error(homeUpsertError.message)

    const { error: hubUpsertError } = await adminClient
      .from('hubs')
      .upsert(hubPayload, { onConflict: 'id' })
    if (hubUpsertError) throw new Error(hubUpsertError.message)

    return {
      hub: existingHub ?? {
        id: requestedHubId,
        home_id: requestedHomeId,
        type: 'server',
      },
      home: {
        id: requestedHomeId,
        owner_id: ownerId,
        member_ids: memberIds,
      },
    }
  } catch (error) {
    if (error instanceof RequestError) {
      return jsonResponse({ error: error.message }, error.status)
    }
    return jsonResponse({ error: errorMessage(error) }, 500)
  }
}

async function readAuthorizedExistingRows(
  adminClient: any,
  userId: string,
  hubId: string,
  serverInstanceId: string | null,
): Promise<{ hub: HubRow; home: HomeRow } | Response> {
  const hub = await fetchHub(adminClient, hubId)
  if (!hub || hub.type !== 'server') {
    const existingByServerInstanceId = await readAuthorizedServerHubByInstanceId(
      adminClient,
      userId,
      serverInstanceId,
      false,
    )
    if (existingByServerInstanceId) return existingByServerInstanceId
    return jsonResponse({ error: 'Server hub not found' }, 404)
  }

  const home = await fetchHome(adminClient, hub.home_id)
  if (!home || !isHomeMember(home, userId)) {
    return jsonResponse({ error: 'Not authorized for this home' }, 403)
  }

  return { hub, home }
}

async function fetchHub(adminClient: any, hubId: string): Promise<HubRow | null> {
  const { data, error } = await adminClient
    .from('hubs')
    .select(
      'id, home_id, type, server_instance_id, remote_endpoint, remote_access_disabled_at, last_connected, created_at, updated_at',
    )
    .eq('id', hubId)
    .maybeSingle()

  if (error) throw new Error(error.message)
  return data ?? null
}

async function readAuthorizedServerHubByInstanceId(
  adminClient: any,
  userId: string,
  serverInstanceId: string | null,
  allowAutoJoin: boolean,
): Promise<{ hub: HubRow; home: HomeRow } | Response | null> {
  if (!serverInstanceId) return null

  const { data, error } = await adminClient
    .from('hubs')
    .select(
      'id, home_id, type, server_instance_id, remote_endpoint, remote_access_disabled_at, last_connected, created_at, updated_at',
    )
    .eq('type', 'server')
    .eq('server_instance_id', serverInstanceId)
    .limit(20)

  if (error) throw new Error(error.message)
  const rows = ((data as HubRow[] | null) ?? []).filter(
    (hub) => hub.type === 'server',
  )
  if (rows.length === 0) return null

  const candidates: Array<{ hub: HubRow; home: HomeRow }> = []
  for (const hub of rows) {
    const home = await fetchHome(adminClient, hub.home_id)
    if (home) candidates.push({ hub, home })
  }
  if (candidates.length === 0) return null

  candidates.sort((left, right) => compareCanonicalServerHubs(left.hub, right.hub))
  const canonical = candidates[0]

  if (isHomeMember(canonical.home, userId)) return canonical

  if (allowAutoJoin) {
    const memberIds = memberIdsForUpsert(canonical.home, userId)
    const { error } = await adminClient
      .from('homes')
      .update({ member_ids: memberIds, updated_at: new Date().toISOString() })
      .eq('id', canonical.home.id)
    if (error) throw new Error(error.message)

    return {
      hub: canonical.hub,
      home: {
        ...canonical.home,
        member_ids: memberIds,
      },
    }
  }

  const authorized = candidates.filter((candidate) =>
    isHomeMember(candidate.home, userId)
  )
  if (authorized.length === 0) {
    return jsonResponse({ error: 'Not authorized for this home' }, 403)
  }

  authorized.sort((left, right) => compareCanonicalServerHubs(left.hub, right.hub))
  return authorized[0]
}

async function fetchHome(
  adminClient: any,
  homeId: string,
): Promise<HomeRow | null> {
  const { data, error } = await adminClient
    .from('homes')
    .select('id, owner_id, member_ids')
    .eq('id', homeId)
    .maybeSingle()

  if (error) throw new Error(error.message)
  return data ?? null
}

function normalizeHomeSnapshot(
  home: JsonObject,
  homeId: string,
  ownerId: string,
  memberIds: string[],
): Record<string, unknown> {
  const now = new Date().toISOString()
  const payload: Record<string, unknown> = {
    id: homeId,
    name: readString(home, 'name') ?? 'My Home',
    owner_id: ownerId,
    member_ids: memberIds,
    sleep_schedule: readJsonObject(home, 'sleep_schedule') ?? {
      bedtime: 22.0,
      wakeTime: 6.5,
      enabled: true,
    },
    created_at: readString(home, 'created_at') ?? now,
    updated_at: readString(home, 'updated_at') ?? now,
  }

  const location = readJsonObject(home, 'location')
  if (location) payload.location = location
  const curveConfig = readJsonObject(home, 'curve_config')
  if (curveConfig) payload.curve_config = curveConfig
  const timezone = readString(home, 'timezone')
  if (timezone) payload.timezone = timezone

  return payload
}

function normalizeServerHubSnapshot(
  hub: JsonObject,
  hubId: string,
  homeId: string,
  serverInstanceIdFallback: string | null = null,
): Record<string, unknown> {
  const now = new Date().toISOString()
  const payload: Record<string, unknown> = {
    id: hubId,
    home_id: homeId,
    type: 'server',
    name: readString(hub, 'name') ?? 'Rhythm Server',
    endpoint: normalizeEndpoint(hub, 'endpoint'),
    enabled: readBool(hub, 'enabled') ?? true,
    created_at: readString(hub, 'created_at') ?? now,
    updated_at: readString(hub, 'updated_at') ?? now,
  }

  if ('remote_endpoint' in hub) {
    payload.remote_endpoint = normalizeOptionalEndpoint(hub, 'remote_endpoint')
  }
  const lastConnected = readString(hub, 'last_connected')
  if (lastConnected) payload.last_connected = lastConnected
  const serverInstanceId =
    readServerInstanceId(hub) ?? serverInstanceIdFallback
  if (serverInstanceId) payload.server_instance_id = serverInstanceId
  return payload
}

function normalizeEndpoint(
  data: JsonObject,
  key: string,
): RemoteAccessEndpoint {
  const endpoint = readJsonObject(data, key)
  if (!endpoint) throw new RequestError(400, `Missing ${key}`)

  const host = readString(endpoint, 'host')
  const port = readInt(endpoint, 'port')
  if (!host || port == null) {
    throw new RequestError(400, `Invalid ${key}`)
  }

  return {
    host,
    port,
    useSsl: endpoint.useSsl === true || endpoint.use_ssl === true,
  }
}

function normalizeOptionalEndpoint(
  data: JsonObject,
  key: string,
): RemoteAccessEndpoint | null {
  if (!(key in data) || data[key] == null) return null
  return normalizeEndpoint(data, key)
}

function endpointsMatch(
  storedEndpoint: unknown,
  requestedEndpoint: RemoteAccessEndpoint,
): boolean {
  const parsed = parseEndpointValue(storedEndpoint)
  if (!parsed) return false
  return (
    parsed.host.trim().toLowerCase() ===
      requestedEndpoint.host.trim().toLowerCase() &&
    parsed.port === requestedEndpoint.port &&
    parsed.useSsl === requestedEndpoint.useSsl
  )
}

function parseEndpointValue(value: unknown): RemoteAccessEndpoint | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    return null
  }

  const endpoint = value as JsonObject
  const host = readString(endpoint, 'host')
  const port = readInt(endpoint, 'port')
  if (!host || port == null) return null

  return {
    host,
    port,
    useSsl: endpoint.useSsl === true || endpoint.use_ssl === true,
  }
}

function readJsonObject(data: JsonObject, key: string): JsonObject | null {
  const value = data[key]
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    return null
  }
  return value as JsonObject
}

function requireBodyString(data: JsonObject, key: string): string {
  const value = readString(data, key)
  if (!value) throw new RequestError(400, `Missing ${key}`)
  return value
}

function readBool(data: JsonObject, key: string): boolean | null {
  const value = data[key]
  return typeof value === 'boolean' ? value : null
}

function readInt(data: JsonObject, key: string): number | null {
  const value = data[key]
  if (typeof value === 'number' && Number.isInteger(value)) return value
  if (typeof value === 'string' && /^\d+$/.test(value)) {
    return Number.parseInt(value, 10)
  }
  return null
}

function isHomeMember(home: HomeRow, userId: string): boolean {
  return Array.isArray(home.member_ids) && home.member_ids.includes(userId)
}

function memberIdsForUpsert(home: HomeRow | null, userId: string): string[] {
  const members = Array.isArray(home?.member_ids) ? home!.member_ids : []
  return Array.from(new Set([...members, userId]))
}

function compareCanonicalServerHubs(left: HubRow, right: HubRow): number {
  const leftHasRemote = left.remote_endpoint == null ? 0 : 1
  const rightHasRemote = right.remote_endpoint == null ? 0 : 1
  if (leftHasRemote !== rightHasRemote) return rightHasRemote - leftHasRemote

  return compareNullableIsoAsc(left.created_at, right.created_at) ||
    compareNullableIsoDesc(left.updated_at, right.updated_at) ||
    compareNullableIsoDesc(left.last_connected, right.last_connected) ||
    left.id.localeCompare(right.id)
}

function compareRemoteAccessMappings(
  left: RemoteAccessMapping,
  right: RemoteAccessMapping,
): number {
  return compareNullableIsoDesc(left.updated_at, right.updated_at) ||
    compareNullableIsoDesc(left.created_at, right.created_at) ||
    right.hub_id.localeCompare(left.hub_id)
}

function compareNullableIsoAsc(
  left: string | null | undefined,
  right: string | null | undefined,
): number {
  const leftTime = left ? Date.parse(left) : Number.NaN
  const rightTime = right ? Date.parse(right) : Number.NaN
  const leftValid = Number.isFinite(leftTime)
  const rightValid = Number.isFinite(rightTime)
  if (leftValid && rightValid && leftTime !== rightTime) {
    return leftTime - rightTime
  }
  if (leftValid !== rightValid) return leftValid ? -1 : 1
  return 0
}

function compareNullableIsoDesc(
  left: string | null | undefined,
  right: string | null | undefined,
): number {
  const leftTime = left ? Date.parse(left) : Number.NaN
  const rightTime = right ? Date.parse(right) : Number.NaN
  const leftValid = Number.isFinite(leftTime)
  const rightValid = Number.isFinite(rightTime)
  if (leftValid && rightValid && leftTime !== rightTime) {
    return rightTime - leftTime
  }
  if (leftValid !== rightValid) return leftValid ? -1 : 1
  return 0
}

class RequestError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message)
  }
}

const REMOTE_ACCESS_MAPPING_SELECT =
  'hub_id, home_id, hostname, tunnel_id, tunnel_name, server_instance_id, created_at, updated_at'

async function readExistingMapping(
  adminClient: any,
  params: {
    userId: string
    hubId: string
    homeId: string
    serverInstanceId: string | null
    serverEndpoint: RemoteAccessEndpoint | null
  },
): Promise<RemoteAccessMapping | null> {
  const byHub = await readExistingMappingByHubId(adminClient, params.hubId)
  if (byHub) return byHub

  if (params.serverInstanceId) {
    const byServerInstanceId = await readExistingMappingByServerInstanceId(
      adminClient,
      params.userId,
      params.serverInstanceId,
    )
    if (byServerInstanceId) return byServerInstanceId
  }

  if (params.serverEndpoint) {
    return await readExistingMappingByHomeEndpoint(
      adminClient,
      params.homeId,
      params.serverEndpoint,
    )
  }

  return null
}

async function readExistingMappingByServerInstanceId(
  adminClient: any,
  userId: string,
  serverInstanceId: string,
): Promise<RemoteAccessMapping | null> {
  const { data, error } = await adminClient
    .from('hub_remote_access')
    .select(REMOTE_ACCESS_MAPPING_SELECT)
    .eq('server_instance_id', serverInstanceId)
    .limit(20)

  if (error) throw new Error(error.message)
  const mappings = (data as RemoteAccessMapping[] | null) ?? []
  mappings.sort(compareRemoteAccessMappings)

  for (const mapping of mappings) {
    const home = await fetchHome(adminClient, mapping.home_id)
    if (home && isHomeMember(home, userId)) return mapping
  }

  return null
}

async function readExistingMappingByHubId(
  adminClient: any,
  hubId: string,
): Promise<RemoteAccessMapping | null> {
  const { data, error } = await adminClient
    .from('hub_remote_access')
    .select(REMOTE_ACCESS_MAPPING_SELECT)
    .eq('hub_id', hubId)
    .maybeSingle()

  if (error) throw new Error(error.message)
  return (data as RemoteAccessMapping | null) ?? null
}

async function readExistingMappingByHomeEndpoint(
  adminClient: any,
  homeId: string,
  serverEndpoint: RemoteAccessEndpoint,
): Promise<RemoteAccessMapping | null> {
  const { data: hubs, error: hubsError } = await adminClient
    .from('hubs')
    .select('id, endpoint')
    .eq('home_id', homeId)
    .eq('type', 'server')

  if (hubsError) throw new Error(hubsError.message)

  const hubRows = (hubs as Array<{ id?: string; endpoint?: unknown }> | null) ??
    []
  const matchingHubIds = hubRows
    .filter((hub) => hub.id && endpointsMatch(hub.endpoint, serverEndpoint))
    .map((hub) => hub.id as string)

  if (matchingHubIds.length === 0) return null

  const { data, error } = await adminClient
    .from('hub_remote_access')
    .select(REMOTE_ACCESS_MAPPING_SELECT)
    .in('hub_id', matchingHubIds)
    .limit(1)

  if (error) throw new Error(error.message)
  const mappings = (data as RemoteAccessMapping[] | null) ?? []
  return mappings[0] ?? null
}

async function saveRemoteAccessMapping(
  adminClient: any,
  params: {
    hubId: string
    existingHubId?: string
    homeId: string
    hostname: string
    tunnelId: string
    tunnelName: string
    serverInstanceId: string | null
  },
): Promise<void> {
  const payload = {
    hub_id: params.hubId,
    home_id: params.homeId,
    hostname: params.hostname,
    tunnel_id: params.tunnelId,
    tunnel_name: params.tunnelName,
    server_instance_id: params.serverInstanceId,
    updated_at: new Date().toISOString(),
  }

  if (params.existingHubId) {
    const { error } = await adminClient
      .from('hub_remote_access')
      .update(payload)
      .eq('hub_id', params.existingHubId)
    if (error) throw new Error(error.message)
    return
  }

  const { error } = await adminClient.from('hub_remote_access').insert(payload)
  if (error) throw new Error(error.message)
}

async function deleteRemoteAccessMapping(
  adminClient: any,
  hubId: string,
): Promise<void> {
  const { error } = await adminClient
    .from('hub_remote_access')
    .delete()
    .eq('hub_id', hubId)
  if (error) throw new Error(error.message)
}

async function setRemoteAccessDisabled(
  adminClient: any,
  hubId: string,
  disabled: boolean,
): Promise<void> {
  const { error } = await adminClient
    .from('hubs')
    .update({
      remote_access_disabled_at: disabled ? new Date().toISOString() : null,
    })
    .eq('id', hubId)
  if (error) throw new Error(error.message)
}

async function clearHubRemoteEndpoint(
  adminClient: any,
  hubId: string,
): Promise<void> {
  const { error } = await adminClient
    .from('hubs')
    .update({ remote_endpoint: null })
    .eq('id', hubId)
  if (error) throw new Error(error.message)
}

async function createTunnel(
  accountId: string,
  apiToken: string,
  name: string,
): Promise<TunnelResult> {
  try {
    return await cloudflare<TunnelResult>(
      `/accounts/${accountId}/cfd_tunnel`,
      apiToken,
      {
        method: 'POST',
        body: JSON.stringify({ name, config_src: 'cloudflare' }),
      },
      'create Cloudflare tunnel',
    )
  } catch (error) {
    if (!isCloudflareConflict(error)) throw error

    const existing = await readTunnelByName(accountId, apiToken, name)
    if (existing) return existing
    throw error
  }
}

async function getTunnelToken(
  accountId: string,
  apiToken: string,
  tunnelId: string,
): Promise<string> {
  return await cloudflare<string>(
    `/accounts/${accountId}/cfd_tunnel/${tunnelId}/token`,
    apiToken,
    undefined,
    'read Cloudflare tunnel token',
  )
}

async function putTunnelConfiguration(
  accountId: string,
  apiToken: string,
  tunnelId: string,
  hostname: string,
  service: string,
): Promise<void> {
  await cloudflare(
    `/accounts/${accountId}/cfd_tunnel/${tunnelId}/configurations`,
    apiToken,
    {
      method: 'PUT',
      body: JSON.stringify({
        config: {
          ingress: [
            {
              hostname,
              service,
              originRequest: {},
            },
            {
              service: 'http_status:404',
            },
          ],
        },
      }),
    },
    'write Cloudflare tunnel configuration',
  )
}

async function upsertDnsRecord(
  zoneId: string,
  apiToken: string,
  hostname: string,
  content: string,
): Promise<void> {
  const query = new URLSearchParams({ type: 'CNAME', name: hostname })
  const records = await cloudflare<DnsRecord[]>(
    `/zones/${zoneId}/dns_records?${query.toString()}`,
    apiToken,
    undefined,
    'list Cloudflare DNS records',
  )
  const existing = records[0]
  const payload = {
    type: 'CNAME',
    proxied: true,
    name: hostname,
    content,
  }

  if (existing) {
    await cloudflare(`/zones/${zoneId}/dns_records/${existing.id}`, apiToken, {
      method: 'PUT',
      body: JSON.stringify(payload),
    }, 'update Cloudflare DNS record')
    return
  }

  await cloudflare(`/zones/${zoneId}/dns_records`, apiToken, {
    method: 'POST',
    body: JSON.stringify(payload),
  }, 'create Cloudflare DNS record')
}

async function deleteDnsRecordIfPresent(
  zoneId: string,
  apiToken: string,
  hostname: string,
): Promise<void> {
  const query = new URLSearchParams({ type: 'CNAME', name: hostname })
  const records = await cloudflare<DnsRecord[]>(
    `/zones/${zoneId}/dns_records?${query.toString()}`,
    apiToken,
    undefined,
    'list Cloudflare DNS records',
  )

  for (const record of records) {
    await cloudflareDeleteIfPresent(
      `/zones/${zoneId}/dns_records/${record.id}`,
      apiToken,
      'delete Cloudflare DNS record',
    )
  }
}

async function deleteTunnelIfPresent(
  accountId: string,
  apiToken: string,
  tunnelId: string,
): Promise<void> {
  await cloudflareDeleteIfPresent(
    `/accounts/${accountId}/cfd_tunnel/${tunnelId}`,
    apiToken,
    'delete Cloudflare tunnel',
  )
}

async function deleteTunnelByNameIfPresent(
  accountId: string,
  apiToken: string,
  tunnelName: string,
): Promise<void> {
  const existing = await readTunnelByName(accountId, apiToken, tunnelName)
  if (!existing) return
  await deleteTunnelIfPresent(accountId, apiToken, existing.id)
}

async function readTunnelByName(
  accountId: string,
  apiToken: string,
  tunnelName: string,
): Promise<TunnelResult | null> {
  const query = new URLSearchParams({ name: tunnelName, per_page: '50' })
  const tunnels = await cloudflare<ListedTunnelResult[]>(
    `/accounts/${accountId}/cfd_tunnel?${query.toString()}`,
    apiToken,
    undefined,
    'list Cloudflare tunnels',
  )
  return tunnels.find((tunnel) =>
    tunnel.name === tunnelName && !tunnel.deleted_at
  ) ?? null
}

function isCloudflareConflict(error: unknown): boolean {
  return error instanceof Error && error.message.includes('failed (409):')
}

async function cloudflare<T = unknown>(
  path: string,
  apiToken: string,
  init: RequestInit = {},
  operation = path,
): Promise<T> {
  const response = await fetch(`${CLOUDFLARE_API_BASE}${path}`, {
    ...init,
    headers: {
      Authorization: `Bearer ${apiToken}`,
      'Content-Type': 'application/json',
      ...(init.headers ?? {}),
    },
  })

  const data = (await response.json()) as CloudflareResponse<T>
  if (!response.ok || !data.success) {
    const message = data.errors?.map((error) => error.message).filter(Boolean).join('; ')
    throw new Error(
      `Cloudflare ${operation} failed (${response.status}): ${
        message || 'request failed'
      }`,
    )
  }
  return data.result
}

async function cloudflareDeleteIfPresent(
  path: string,
  apiToken: string,
  operation: string,
): Promise<void> {
  const response = await fetch(`${CLOUDFLARE_API_BASE}${path}`, {
    method: 'DELETE',
    headers: {
      Authorization: `Bearer ${apiToken}`,
      'Content-Type': 'application/json',
    },
  })

  if (response.status === 404) return

  const data = (await response.json()) as CloudflareResponse<unknown>
  if (!response.ok || !data.success) {
    const message = data.errors?.map((error) => error.message).filter(Boolean).join('; ')
    throw new Error(
      `Cloudflare ${operation} failed (${response.status}): ${
        message || 'request failed'
      }`,
    )
  }
}

function endpointForHostname(hostname: string): { host: string; port: number; useSsl: boolean } {
  return {
    host: hostname,
    port: 443,
    useSsl: true,
  }
}

function remoteAccessDomain(): string {
  const domain = readEnv(
    'RHYTHM_REMOTE_ACCESS_DOMAIN',
    DEFAULT_REMOTE_ACCESS_DOMAIN,
  )
    .replace(/^\.+|\.+$/g, '')
    .toLowerCase()
  return domain === LEGACY_REMOTE_ACCESS_DOMAIN
    ? DEFAULT_REMOTE_ACCESS_DOMAIN
    : domain
}

function hostnameForDomain(
  existingHostname: string | undefined,
  desiredHostname: string,
): string {
  if (!existingHostname) return desiredHostname
  return existingHostname.toLowerCase()
}

function requireEnv(name: string): string {
  const value = Deno.env.get(name)?.trim()
  if (!value) throw new Error(`Missing ${name}`)
  return value
}

function requireCloudflareApiToken(): string {
  return requireEnv('CLOUDFLARE_API_TOKEN').replace(/^Bearer\s+/i, '').trim()
}

function readCloudflareApiToken(): string | null {
  const value = readOptionalEnv('CLOUDFLARE_API_TOKEN')
    ?.replace(/^Bearer\s+/i, '')
    .trim()
  return value || null
}

function readEnv(name: string, fallback: string): string {
  return Deno.env.get(name)?.trim() || fallback
}

function readOptionalEnv(name: string): string | null {
  return Deno.env.get(name)?.trim() || null
}
