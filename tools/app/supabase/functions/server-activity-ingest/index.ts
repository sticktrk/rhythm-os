import { createClient } from 'https://esm.sh/@supabase/supabase-js@2'

type JsonObject = Record<string, unknown>

type DeviceTokenRow = {
  id: string
  user_id: string
  home_id: string
  hub_id: string
  server_instance_id?: string | null
}

const corsHeaders = {
  'Access-Control-Allow-Origin': '*',
  'Access-Control-Allow-Methods': 'POST, OPTIONS',
  'Access-Control-Allow-Headers':
    'authorization, x-client-info, apikey, content-type',
}

Deno.serve(async (req) => {
  if (req.method === 'OPTIONS') {
    return new Response('ok', { headers: corsHeaders })
  }
  if (req.method !== 'POST') {
    return jsonResponse({ error: 'Method not allowed' }, 405)
  }

  try {
    const token = readBearerToken(req.headers.get('Authorization'))
    if (!token) return jsonResponse({ error: 'Missing device token' }, 401)

    const body = await readJson(req)
    const hubId = readString(body, 'hub_id')
    const homeId = readString(body, 'home_id')
    if (!hubId || !homeId) {
      return jsonResponse({ error: 'Missing home_id or hub_id' }, 400)
    }

    const adminClient = createClient(
      requireEnv('SUPABASE_URL'),
      readSupabaseSecretKey(),
    )
    const tokenRow = await lookupDeviceToken(adminClient, token)
    if (!tokenRow) return jsonResponse({ error: 'Invalid device token' }, 401)
    if (tokenRow.hub_id !== hubId || tokenRow.home_id !== homeId) {
      return jsonResponse({ error: 'Device token scope mismatch' }, 403)
    }
    const stillAuthorized = await tokenStillAuthorized(adminClient, tokenRow)
    if (!stillAuthorized) {
      await revokeDeviceToken(adminClient, tokenRow.id)
      return jsonResponse({ error: 'Device token no longer authorized' }, 403)
    }

    const serverInstanceId = readString(body, 'server_instance_id')
    if (
      tokenRow.server_instance_id &&
      tokenRow.server_instance_id !== serverInstanceId
    ) {
      return jsonResponse({ error: 'Server identity mismatch' }, 403)
    }

    const events = readEvents(body)
    if (events instanceof Response) return events
    if (events.length === 0) {
      return jsonResponse({ status: 'ok', inserted: 0 })
    }

    const rows = events
      .map((event) =>
        rowForActivity({
          userId: tokenRow.user_id,
          homeId,
          hubId,
          serverInstanceId: serverInstanceId ?? tokenRow.server_instance_id ??
            null,
          event,
        })
      )
      .filter((row): row is Record<string, unknown> => row != null)

    if (rows.length === 0) {
      return jsonResponse({ status: 'ok', inserted: 0 })
    }

    const uniqueRows = uniqueRowsByConflictKey(rows)
    const { error } = await adminClient
      .from('server_light_activity_events')
      .upsert(uniqueRows, { onConflict: 'user_id,hub_id,event_id' })
    if (error) throw new Error(error.message)

    await adminClient
      .from('server_light_activity_device_tokens')
      .update({ last_used_at: new Date().toISOString() })
      .eq('id', tokenRow.id)

    return jsonResponse({
      status: 'ok',
      inserted: uniqueRows.length,
      deduplicated: rows.length - uniqueRows.length,
    })
  } catch (error) {
    return jsonResponse({ error: errorMessage(error) }, 500)
  }
})

async function lookupDeviceToken(
  adminClient: any,
  token: string,
): Promise<DeviceTokenRow | null> {
  const tokenHash = await sha256Hex(token)
  const { data, error } = await adminClient
    .from('server_light_activity_device_tokens')
    .select('id,user_id,home_id,hub_id,server_instance_id')
    .eq('token_hash', tokenHash)
    .is('revoked_at', null)
    .maybeSingle()
  if (error) throw new Error(error.message)
  return data ?? null
}

async function tokenStillAuthorized(
  adminClient: any,
  tokenRow: DeviceTokenRow,
): Promise<boolean> {
  const { data: home, error: homeError } = await adminClient
    .from('homes')
    .select('id,member_ids')
    .eq('id', tokenRow.home_id)
    .maybeSingle()
  if (homeError) throw new Error(homeError.message)
  if (!home || !Array.isArray(home.member_ids)) return false
  if (!home.member_ids.includes(tokenRow.user_id)) return false

  const { data: hub, error: hubError } = await adminClient
    .from('hubs')
    .select('id,home_id,type,server_instance_id')
    .eq('id', tokenRow.hub_id)
    .maybeSingle()
  if (hubError) throw new Error(hubError.message)
  if (!hub || hub.home_id !== tokenRow.home_id || hub.type !== 'server') {
    return false
  }
  if (
    tokenRow.server_instance_id &&
    hub.server_instance_id &&
    hub.server_instance_id !== tokenRow.server_instance_id
  ) {
    return false
  }
  return true
}

async function revokeDeviceToken(
  adminClient: any,
  tokenId: string,
): Promise<void> {
  const { error } = await adminClient
    .from('server_light_activity_device_tokens')
    .update({ revoked_at: new Date().toISOString() })
    .eq('id', tokenId)
    .is('revoked_at', null)
  if (error) throw new Error(error.message)
}

function rowForActivity({
  userId,
  homeId,
  hubId,
  serverInstanceId,
  event,
}: {
  userId: string
  homeId: string
  hubId: string
  serverInstanceId: string | null
  event: JsonObject
}): Record<string, unknown> | null {
  const eventId = readString(event, 'id')
  const nodeId = readString(event, 'node_id')
  if (!eventId || !nodeId) return null

  const source = readJsonObject(event, 'source') ?? {}
  const target = readJsonObject(event, 'target')
  const change = readJsonObject(event, 'change')
  const payload = event.payload
  const epochMs = readInt(event, 'epoch_ms') ?? Date.now()
  const actionId = readString(event, 'action_id') ?? 'unknown'
  const sourceKind = readString(source, 'kind') ?? 'unknown'
  const sourceRaw = readString(source, 'raw') ?? sourceKind
  const activityServerInstanceId = readString(event, 'server_instance_id') ??
    serverInstanceId
  const brightness = clampInt(
    readInt(event, 'brightness') ?? readInt(target ?? {}, 'brightness'),
    1,
    100,
  )
  const kelvin = clampInt(
    readInt(event, 'kelvin') ?? readInt(target ?? {}, 'kelvin'),
    500,
    25000,
  )

  return {
    user_id: userId,
    home_id: homeId,
    hub_id: hubId,
    ...(activityServerInstanceId
      ? { server_instance_id: activityServerInstanceId }
      : {}),
    event_id: eventId,
    node_id: nodeId,
    action_id: actionId,
    source_kind: sourceKind,
    source_raw: sourceRaw,
    ...(readString(source, 'control_id')
      ? { source_control_id: readString(source, 'control_id') }
      : {}),
    marks_touched: source.marks_touched === true,
    occurred_at: new Date(epochMs).toISOString(),
    epoch_ms: epochMs,
    ...(event.active_mode ? { active_mode: event.active_mode } : {}),
    ...(target ? { target } : {}),
    ...(change ? { change } : {}),
    ...(payload != null ? { payload } : {}),
    raw_activity: event,
    ...(brightness != null ? { brightness } : {}),
    ...(kelvin != null ? { kelvin } : {}),
    ...(readString(event, 'correlation_id')
      ? { correlation_id: readString(event, 'correlation_id') }
      : {}),
    ...(readString(event, 'fanout_of')
      ? { fanout_of: readString(event, 'fanout_of') }
      : {}),
    sync_source: 'device_server_direct',
    last_seen_at: new Date().toISOString(),
  }
}

function readEvents(body: JsonObject): JsonObject[] | Response {
  const value = body.events
  if (!Array.isArray(value)) {
    return jsonResponse({ error: 'events must be an array' }, 400)
  }
  const events = value.filter(
    (event) => event && typeof event === 'object' && !Array.isArray(event),
  ) as JsonObject[]
  if (events.length !== value.length) {
    return jsonResponse({ error: 'events must contain objects' }, 400)
  }
  return events.slice(0, 2000)
}

function uniqueRowsByConflictKey(
  rows: Record<string, unknown>[],
): Record<string, unknown>[] {
  const rowsByKey = new Map<string, Record<string, unknown>>()
  for (const row of rows) {
    const key = [
      String(row.user_id ?? ''),
      String(row.hub_id ?? ''),
      String(row.event_id ?? ''),
    ].join('\0')
    const current = rowsByKey.get(key)
    if (!current || rowEpochMs(row) > rowEpochMs(current)) {
      rowsByKey.set(key, row)
    }
  }
  return Array.from(rowsByKey.values())
}

function rowEpochMs(row: Record<string, unknown>): number {
  return typeof row.epoch_ms === 'number' ? row.epoch_ms : 0
}

async function readJson(req: Request): Promise<JsonObject> {
  const value = await req.json()
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('Invalid JSON body')
  }
  return value as JsonObject
}

function readJsonObject(data: JsonObject, key: string): JsonObject | null {
  const value = data[key]
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  return value as JsonObject
}

function readString(data: JsonObject, key: string): string | null {
  const value = data[key]
  if (typeof value !== 'string') return null
  const trimmed = value.trim()
  return trimmed.length > 0 ? trimmed : null
}

function readInt(data: JsonObject, key: string): number | null {
  const value = data[key]
  if (typeof value === 'number' && Number.isFinite(value)) {
    return Math.trunc(value)
  }
  if (typeof value === 'string' && /^-?\d+$/.test(value)) {
    return Number.parseInt(value, 10)
  }
  return null
}

function clampInt(
  value: number | null,
  min: number,
  max: number,
): number | null {
  if (value == null) return null
  return Math.min(max, Math.max(min, value))
}

function readBearerToken(authHeader: string | null): string | null {
  if (!authHeader) return null
  const [scheme, token] = authHeader.split(' ')
  if (scheme !== 'Bearer' || !token) return null
  const trimmed = token.trim()
  return trimmed.length > 0 ? trimmed : null
}

async function sha256Hex(value: string): Promise<string> {
  const data = new TextEncoder().encode(value)
  const digest = await crypto.subtle.digest('SHA-256', data)
  return Array.from(new Uint8Array(digest))
    .map((byte) => byte.toString(16).padStart(2, '0'))
    .join('')
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      ...corsHeaders,
      'Content-Type': 'application/json',
    },
  })
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function requireEnv(name: string): string {
  const value = Deno.env.get(name)?.trim()
  if (!value) throw new Error(`Missing ${name}`)
  return value
}

function readSupabaseSecretKey(): string {
  return Deno.env.get('SB_SECRET_KEY')?.trim() ??
    requireEnv('SUPABASE_SERVICE_ROLE_KEY')
}
