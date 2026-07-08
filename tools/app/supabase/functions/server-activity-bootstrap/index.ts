import {
  errorMessage,
  jsonResponse,
  readJson,
  readString,
  type JsonObject,
  withAuthenticatedRequest,
} from '../_shared/auth.ts'

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
  last_connected?: string | null
  created_at?: string | null
  updated_at?: string | null
}

Deno.serve((req) =>
  withAuthenticatedRequest(req, async ({ userId, adminClient }) => {
    if (req.method !== 'POST') {
      return jsonResponse({ error: 'Method not allowed' }, 405)
    }

    try {
      const body = await readJson(req)
      const hubId = readString(body, 'hub_id')
      if (!hubId) return jsonResponse({ error: 'Missing hub_id' }, 400)

      const serverInstanceId = readServerInstanceId(body)
      const ensured = await ensureServerHubRows(
        adminClient,
        userId,
        hubId,
        body,
        serverInstanceId,
      )
      if (ensured instanceof Response) return ensured

      const token = randomToken()
      const tokenHash = await sha256Hex(token)
      const tokenPrefix = token.slice(0, 12)
      const now = new Date().toISOString()

      await revokeActiveTokens(adminClient, ensured.hub.id, now)
      const { data, error } = await adminClient
        .from('server_light_activity_device_tokens')
        .insert({
          user_id: userId,
          home_id: ensured.hub.home_id,
          hub_id: ensured.hub.id,
          server_instance_id: serverInstanceId,
          token_hash: tokenHash,
          token_prefix: tokenPrefix,
          label: 'rhythm-os-server-activity',
        })
        .select('id')
        .single()
      if (error) throw new Error(error.message)
      const tokenId = data?.id?.toString()
      if (!tokenId) throw new Error('Failed to create device upload token')

      const supabaseUrl = requireEnv('SUPABASE_URL').replace(/\/+$/, '')
      return jsonResponse({
        status: 'ok',
        hub_id: ensured.hub.id,
        requested_hub_id: hubId,
        home_id: ensured.hub.home_id,
        server_instance_id: serverInstanceId,
        token_id: tokenId,
        upload_token: token,
        ingest_url: `${supabaseUrl}/functions/v1/server-activity-ingest`,
      })
    } catch (error) {
      return jsonResponse({ error: errorMessage(error) }, 500)
    }
  })
)

async function ensureServerHubRows(
  adminClient: any,
  userId: string,
  hubId: string,
  body: JsonObject,
  serverInstanceId: string | null,
): Promise<{ hub: HubRow; home: HomeRow } | Response> {
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
    return jsonResponse({
      error: 'Both home and server_hub are required when creating activity credentials',
    }, 400)
  }

  const requestedHomeId = requireBodyString(homeSnapshot, 'id')
  const requestedHubId = requireBodyString(hubSnapshot, 'id')
  if (requestedHubId !== hubId) {
    return jsonResponse({ error: 'server_hub.id must match hub_id' }, 400)
  }
  if (readString(hubSnapshot, 'type') !== 'server') {
    return jsonResponse({ error: 'server_hub.type must be server' }, 400)
  }

  const requestedHubHomeId = readString(hubSnapshot, 'home_id')
  if (requestedHubHomeId && requestedHubHomeId !== requestedHomeId) {
    return jsonResponse({ error: 'server_hub.home_id must match home.id' }, 400)
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
    const { error: hubUpdateError } = await adminClient
      .from('hubs')
      .upsert(
        normalizeServerHubSnapshot(
          hubSnapshot,
          hub.id,
          home.id,
          serverInstanceId,
        ),
        { onConflict: 'id' },
      )
    if (hubUpdateError) throw new Error(hubUpdateError.message)
    return { hub, home }
  }

  const existingHub = await fetchHub(adminClient, requestedHubId)
  if (existingHub) {
    if (existingHub.type !== 'server') {
      return jsonResponse({ error: 'Server hub not found' }, 404)
    }
    if (existingHub.home_id !== requestedHomeId) {
      return jsonResponse({
        error: 'Existing server hub belongs to a different home',
      }, 409)
    }
  }

  const existingHome = await fetchHome(adminClient, requestedHomeId)
  if (existingHome && !isHomeMember(existingHome, userId)) {
    return jsonResponse({ error: 'Not authorized for this home' }, 403)
  }

  const memberIds = memberIdsForUpsert(existingHome, userId)
  const ownerId = existingHome?.owner_id ?? userId
  const { error: homeError } = await adminClient
    .from('homes')
    .upsert(normalizeHomeSnapshot(homeSnapshot, requestedHomeId, ownerId, memberIds), {
      onConflict: 'id',
    })
  if (homeError) throw new Error(homeError.message)

  const { error: hubError } = await adminClient
    .from('hubs')
    .upsert(
      normalizeServerHubSnapshot(
        hubSnapshot,
        requestedHubId,
        requestedHomeId,
        serverInstanceId,
      ),
      { onConflict: 'id' },
    )
  if (hubError) throw new Error(hubError.message)

  return {
    hub: { id: requestedHubId, home_id: requestedHomeId, type: 'server' },
    home: { id: requestedHomeId, owner_id: ownerId, member_ids: memberIds },
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
      'id, home_id, type, server_instance_id, remote_endpoint, last_connected, created_at, updated_at',
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
      'id, home_id, type, server_instance_id, remote_endpoint, last_connected, created_at, updated_at',
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

async function revokeActiveTokens(
  adminClient: any,
  hubId: string,
  revokedAt: string,
): Promise<void> {
  const { error } = await adminClient
    .from('server_light_activity_device_tokens')
    .update({ revoked_at: revokedAt })
    .eq('hub_id', hubId)
    .is('revoked_at', null)
  if (error) throw new Error(error.message)
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
  const serverInstanceId =
    readServerInstanceId(hub) ?? serverInstanceIdFallback
  if (serverInstanceId) payload.server_instance_id = serverInstanceId
  const lastConnected = readString(hub, 'last_connected')
  if (lastConnected) payload.last_connected = lastConnected
  return payload
}

function normalizeEndpoint(data: JsonObject, key: string): Record<string, unknown> {
  const endpoint = readJsonObject(data, key)
  if (!endpoint) throw new Error(`Missing ${key}`)
  const host = readString(endpoint, 'host')
  const port = readInt(endpoint, 'port')
  if (!host || port == null) throw new Error(`Invalid ${key}`)
  return {
    host,
    port,
    useSsl: endpoint.useSsl === true || endpoint.use_ssl === true,
  }
}

function normalizeOptionalEndpoint(
  data: JsonObject,
  key: string,
): Record<string, unknown> | null {
  if (!(key in data) || data[key] == null) return null
  return normalizeEndpoint(data, key)
}

function readServerInstanceId(body: JsonObject): string | null {
  const raw = readString(body, 'server_instance_id')
  const value = raw?.trim().toLowerCase()
  if (!value) return null
  return value.slice(0, 256)
}

function readJsonObject(data: JsonObject, key: string): JsonObject | null {
  const value = data[key]
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  return value as JsonObject
}

function requireBodyString(data: JsonObject, key: string): string {
  const value = readString(data, key)
  if (!value) throw new Error(`Missing ${key}`)
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

function randomToken(): string {
  const bytes = new Uint8Array(32)
  crypto.getRandomValues(bytes)
  return btoa(String.fromCharCode(...bytes))
    .replaceAll('+', '-')
    .replaceAll('/', '_')
    .replaceAll('=', '')
}

async function sha256Hex(value: string): Promise<string> {
  const data = new TextEncoder().encode(value)
  const digest = await crypto.subtle.digest('SHA-256', data)
  return Array.from(new Uint8Array(digest))
    .map((byte) => byte.toString(16).padStart(2, '0'))
    .join('')
}

function requireEnv(name: string): string {
  const value = Deno.env.get(name)?.trim()
  if (!value) throw new Error(`Missing ${name}`)
  return value
}
