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
  name: string
  owner_id: string
  member_ids?: string[]
  location?: unknown
  sleep_schedule?: unknown
  curve_config?: unknown
  timezone?: string | null
  created_at: string
  updated_at: string
}

type HubRow = {
  id: string
  home_id: string
  type: string
  name: string
  endpoint: unknown
  enabled?: boolean
  remote_endpoint?: unknown
  server_instance_id?: string | null
  last_connected?: string | null
  created_at: string
  updated_at: string
}

const HOME_COLUMNS =
  'id,name,owner_id,member_ids,location,sleep_schedule,curve_config,timezone,created_at,updated_at'

const HUB_COLUMNS =
  'id,home_id,type,name,endpoint,enabled,remote_endpoint,server_instance_id,last_connected,created_at,updated_at'

Deno.serve((req) =>
  withAuthenticatedRequest(req, async ({ userId, adminClient }) => {
    if (req.method !== 'POST') {
      return jsonResponse({ error: 'Method not allowed' }, 405)
    }

    try {
      const body = await readJson(req)
      const serverInstanceId = readServerInstanceId(body)
      if (!serverInstanceId) {
        return jsonResponse({ error: 'Missing server_instance_id' }, 400)
      }

      const joined = await joinExistingHomeByServerInstanceId(
        adminClient,
        userId,
        serverInstanceId,
      )
      if (!joined) {
        return jsonResponse({ error: 'Home not found for server' }, 404)
      }

      return jsonResponse({
        status: 'ok',
        joined: joined.joined,
        home: joined.home,
        server_hub: joined.hub,
        home_id: joined.home.id,
        hub_id: joined.hub.id,
        server_instance_id: serverInstanceId,
      })
    } catch (error) {
      return jsonResponse({ error: errorMessage(error) }, 500)
    }
  })
)

async function joinExistingHomeByServerInstanceId(
  adminClient: any,
  userId: string,
  serverInstanceId: string,
): Promise<{ home: HomeRow; hub: HubRow; joined: boolean } | null> {
  const { data, error } = await adminClient
    .from('hubs')
    .select(HUB_COLUMNS)
    .eq('type', 'server')
    .eq('server_instance_id', serverInstanceId)
    .limit(20)

  if (error) throw new Error(error.message)
  const hubs = ((data as HubRow[] | null) ?? []).filter(
    (hub) => hub.type === 'server',
  )
  if (hubs.length === 0) return null

  const candidates: Array<{ hub: HubRow; home: HomeRow }> = []
  for (const hub of hubs) {
    const home = await fetchHome(adminClient, hub.home_id)
    if (home) candidates.push({ hub, home })
  }
  if (candidates.length === 0) return null

  candidates.sort((left, right) =>
    compareCanonicalServerHubs(left.hub, right.hub)
  )
  const canonical = candidates[0]
  const memberIds = memberIdsForJoin(canonical.home, userId)
  const alreadyMember = isHomeMember(canonical.home, userId)

  if (!alreadyMember) {
    const { error: updateError } = await adminClient
      .from('homes')
      .update({ member_ids: memberIds, updated_at: new Date().toISOString() })
      .eq('id', canonical.home.id)
    if (updateError) throw new Error(updateError.message)
  }

  const home = await fetchHome(adminClient, canonical.home.id)
  if (!home) return null

  return {
    hub: canonical.hub,
    home,
    joined: !alreadyMember,
  }
}

async function fetchHome(
  adminClient: any,
  homeId: string,
): Promise<HomeRow | null> {
  const { data, error } = await adminClient
    .from('homes')
    .select(HOME_COLUMNS)
    .eq('id', homeId)
    .maybeSingle()

  if (error) throw new Error(error.message)
  return data ?? null
}

function readServerInstanceId(body: JsonObject): string | null {
  const raw = readString(body, 'server_instance_id')
  const value = raw?.trim().toLowerCase()
  if (!value) return null
  return value.slice(0, 256)
}

function isHomeMember(home: HomeRow, userId: string): boolean {
  return Array.isArray(home.member_ids) && home.member_ids.includes(userId)
}

function memberIdsForJoin(home: HomeRow, userId: string): string[] {
  const members = Array.isArray(home.member_ids) ? home.member_ids : []
  return Array.from(new Set([...members, home.owner_id, userId]))
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
  return -compareNullableIsoAsc(left, right)
}
