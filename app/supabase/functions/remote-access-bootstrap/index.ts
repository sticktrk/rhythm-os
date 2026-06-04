import {
  errorMessage,
  jsonResponse,
  readJson,
  readString,
  type JsonObject,
  withAuthenticatedRequest,
} from '../_shared/auth.ts'

const CLOUDFLARE_API_BASE = 'https://api.cloudflare.com/client/v4'
const DEFAULT_REMOTE_ACCESS_DOMAIN = 'devices.rhythm.lighting'
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

    const ensured = await ensureServerHubRows(adminClient, userId, hubId, body)
    if (ensured instanceof Response) return ensured
    const { hub } = ensured

    try {
      const accountId = requireEnv('CLOUDFLARE_ACCOUNT_ID')
      const zoneId = requireEnv('CLOUDFLARE_ZONE_ID')
      const apiToken = requireCloudflareApiToken()
      const domain = readEnv('RHYTHM_REMOTE_ACCESS_DOMAIN', DEFAULT_REMOTE_ACCESS_DOMAIN)
        .replace(/^\.+|\.+$/g, '')
      const originService =
        readEnv('RHYTHM_REMOTE_ACCESS_ORIGIN', DEFAULT_ORIGIN_SERVICE)

      const existing = await readExistingMapping(adminClient, hubId)
      const tunnelName = existing?.tunnel_name ?? `rhythm-${hubId}`
      const hostname = existing?.hostname ?? `${hubId}.${domain}`.toLowerCase()
      const tunnel = existing
        ? { id: existing.tunnel_id, name: existing.tunnel_name }
        : await createTunnel(accountId, apiToken, tunnelName)
      const connectorToken =
        tunnel.token ?? (await getTunnelToken(accountId, apiToken, tunnel.id))

      await putTunnelConfiguration(accountId, apiToken, tunnel.id, hostname, originService)
      await upsertDnsRecord(zoneId, apiToken, hostname, `${tunnel.id}.cfargotunnel.com`)

      const remoteEndpoint = endpointForHostname(hostname)
      await adminClient.from('hub_remote_access').upsert({
        hub_id: hubId,
        home_id: hub.home_id,
        hostname,
        tunnel_id: tunnel.id,
        tunnel_name: tunnel.name,
        updated_at: new Date().toISOString(),
      })
      await adminClient
        .from('hubs')
        .update({ remote_endpoint: remoteEndpoint })
        .eq('id', hubId)

      return jsonResponse({
        status: 'ok',
        hub_id: hubId,
        hostname,
        remote_url: `https://${hostname}`,
        remote_endpoint: remoteEndpoint,
        tunnel_id: tunnel.id,
        tunnel_name: tunnel.name,
        connector_token: connectorToken,
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
): Promise<{ hub: HubRow; home: HomeRow } | Response> {
  try {
    const homeSnapshot = readJsonObject(body, 'home')
    const hubSnapshot = readJsonObject(body, 'server_hub')

    if (!homeSnapshot && !hubSnapshot) {
      return await readAuthorizedExistingRows(adminClient, userId, hubId)
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
      hub: { id: requestedHubId, home_id: requestedHomeId, type: 'server' },
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
): Promise<{ hub: HubRow; home: HomeRow } | Response> {
  const hub = await fetchHub(adminClient, hubId)
  if (!hub || hub.type !== 'server') {
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
    .select('id, home_id, type')
    .eq('id', hubId)
    .maybeSingle()

  if (error) throw new Error(error.message)
  return data ?? null
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
): Record<string, unknown> {
  const now = new Date().toISOString()
  const payload: Record<string, unknown> = {
    id: hubId,
    home_id: homeId,
    type: 'server',
    name: readString(hub, 'name') ?? 'Rhythm Server',
    endpoint: normalizeEndpoint(hub, 'endpoint'),
    enabled: readBool(hub, 'enabled') ?? true,
    remote_endpoint: normalizeOptionalEndpoint(hub, 'remote_endpoint'),
    created_at: readString(hub, 'created_at') ?? now,
    updated_at: readString(hub, 'updated_at') ?? now,
  }

  const lastConnected = readString(hub, 'last_connected')
  if (lastConnected) payload.last_connected = lastConnected
  return payload
}

function normalizeEndpoint(
  data: JsonObject,
  key: string,
): { host: string; port: number; useSsl: boolean } {
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
): { host: string; port: number; useSsl: boolean } | null {
  if (!(key in data) || data[key] == null) return null
  return normalizeEndpoint(data, key)
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

class RequestError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message)
  }
}

async function readExistingMapping(adminClient: any, hubId: string): Promise<{
  hostname: string
  tunnel_id: string
  tunnel_name: string
} | null> {
  const { data, error } = await adminClient
    .from('hub_remote_access')
    .select('hostname, tunnel_id, tunnel_name')
    .eq('hub_id', hubId)
    .maybeSingle()

  if (error) throw new Error(error.message)
  return data ?? null
}

async function createTunnel(
  accountId: string,
  apiToken: string,
  name: string,
): Promise<TunnelResult> {
  const response = await cloudflare<TunnelResult>(
    `/accounts/${accountId}/cfd_tunnel`,
    apiToken,
    {
      method: 'POST',
      body: JSON.stringify({ name, config_src: 'cloudflare' }),
    },
    'create Cloudflare tunnel',
  )
  return response
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

function endpointForHostname(hostname: string): { host: string; port: number; useSsl: boolean } {
  return {
    host: hostname,
    port: 443,
    useSsl: true,
  }
}

function requireEnv(name: string): string {
  const value = Deno.env.get(name)?.trim()
  if (!value) throw new Error(`Missing ${name}`)
  return value
}

function requireCloudflareApiToken(): string {
  return requireEnv('CLOUDFLARE_API_TOKEN').replace(/^Bearer\s+/i, '').trim()
}

function readEnv(name: string, fallback: string): string {
  return Deno.env.get(name)?.trim() || fallback
}
