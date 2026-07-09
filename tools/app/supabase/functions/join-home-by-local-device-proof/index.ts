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

type DeviceTokenRow = {
  id: string
  user_id: string
  home_id: string
  hub_id: string
  server_instance_id?: string | null
  token_hash: string
}

type JoinProof = {
  proof_version: string
  algorithm: string
  server_instance_id: string
  home_id: string
  hub_id: string
  token_id?: string | null
  issued_at_epoch_ms: number
  expires_at_epoch_ms: number
  nonce: string
  signature: string
}

const JOIN_PROOF_VERSION = 'activity-token-hmac-v1'
const JOIN_PROOF_ALGORITHM = 'hmac-sha256'
const MAX_CLOCK_SKEW_MS = 5 * 60 * 1000

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

      const proof = readJoinProof(body, serverInstanceId)
      if (proof instanceof Response) return proof

      const tokenRow = await verifyJoinProof(adminClient, proof)
      if (!tokenRow) {
        return jsonResponse({ error: 'Invalid local device proof' }, 403)
      }

      const stillAuthorized = await deviceTokenStillAuthorized(
        adminClient,
        tokenRow,
        proof,
      )
      if (!stillAuthorized) {
        return jsonResponse({ error: 'Device proof is no longer authorized' }, 403)
      }

      const joined = await joinHomeFromVerifiedDeviceProof(
        adminClient,
        userId,
        tokenRow,
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

async function verifyJoinProof(
  adminClient: any,
  proof: JoinProof,
): Promise<DeviceTokenRow | null> {
  const rows = await fetchCandidateDeviceTokens(adminClient, proof)
  for (const row of rows) {
    if (row.server_instance_id !== proof.server_instance_id) continue
    if (row.home_id !== proof.home_id || row.hub_id !== proof.hub_id) continue

    const expected = await joinProofSignature(row.token_hash, proof)
    if (constantTimeEqualHex(expected, proof.signature)) return row
  }
  return null
}

async function fetchCandidateDeviceTokens(
  adminClient: any,
  proof: JoinProof,
): Promise<DeviceTokenRow[]> {
  let query = adminClient
    .from('server_light_activity_device_tokens')
    .select('id,user_id,home_id,hub_id,server_instance_id,token_hash')
    .is('revoked_at', null)

  if (proof.token_id) {
    query = query.eq('id', proof.token_id)
  } else {
    query = query.eq('server_instance_id', proof.server_instance_id)
  }

  const { data, error } = await query.limit(20)
  if (error) throw new Error(error.message)
  return ((data as DeviceTokenRow[] | null) ?? []).filter((row) =>
    typeof row.token_hash === 'string' && row.token_hash.length > 0
  )
}

async function deviceTokenStillAuthorized(
  adminClient: any,
  tokenRow: DeviceTokenRow,
  proof: JoinProof,
): Promise<boolean> {
  const home = await fetchHome(adminClient, tokenRow.home_id)
  if (!home || !isHomeMember(home, tokenRow.user_id)) return false

  const hub = await fetchHub(adminClient, tokenRow.hub_id)
  if (!hub || hub.home_id !== tokenRow.home_id || hub.type !== 'server') {
    return false
  }
  if (hub.server_instance_id !== proof.server_instance_id) return false
  return true
}

async function joinHomeFromVerifiedDeviceProof(
  adminClient: any,
  userId: string,
  tokenRow: DeviceTokenRow,
): Promise<{ home: HomeRow; hub: HubRow; joined: boolean } | null> {
  const before = await fetchHome(adminClient, tokenRow.home_id)
  if (!before) return null
  const alreadyMember = isHomeMember(before, userId)

  if (!alreadyMember) {
    const { error } = await adminClient.rpc(
      'add_verified_device_home_member',
      {
        home_uuid: tokenRow.home_id,
        user_uuid: userId,
      },
    )
    if (error) throw new Error(error.message)
  }

  const home = await fetchHome(adminClient, tokenRow.home_id)
  const hub = await fetchHub(adminClient, tokenRow.hub_id)
  if (!home || !hub) return null

  return {
    home,
    hub,
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

async function fetchHub(
  adminClient: any,
  hubId: string,
): Promise<HubRow | null> {
  const { data, error } = await adminClient
    .from('hubs')
    .select(HUB_COLUMNS)
    .eq('id', hubId)
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

function readJoinProof(
  body: JsonObject,
  serverInstanceId: string,
): JoinProof | Response {
  const proof = readJsonObject(body, 'join_proof')
  if (!proof) return jsonResponse({ error: 'Missing join_proof' }, 400)

  const parsed: JoinProof = {
    proof_version: readString(proof, 'proof_version') ?? '',
    algorithm: readString(proof, 'algorithm') ?? '',
    server_instance_id: readServerInstanceId(proof) ?? '',
    home_id: readString(proof, 'home_id') ?? '',
    hub_id: readString(proof, 'hub_id') ?? '',
    token_id: readString(proof, 'token_id'),
    issued_at_epoch_ms: readEpochMs(proof, 'issued_at_epoch_ms'),
    expires_at_epoch_ms: readEpochMs(proof, 'expires_at_epoch_ms'),
    nonce: readString(proof, 'nonce') ?? '',
    signature: readString(proof, 'signature')?.toLowerCase() ?? '',
  }

  if (parsed.proof_version !== JOIN_PROOF_VERSION) {
    return jsonResponse({ error: 'Unsupported join proof version' }, 400)
  }
  if (parsed.algorithm !== JOIN_PROOF_ALGORITHM) {
    return jsonResponse({ error: 'Unsupported join proof algorithm' }, 400)
  }
  if (parsed.server_instance_id !== serverInstanceId) {
    return jsonResponse({ error: 'Join proof server identity mismatch' }, 403)
  }
  if (!parsed.home_id || !parsed.hub_id || parsed.nonce.length < 16) {
    return jsonResponse({ error: 'Malformed join proof' }, 400)
  }
  if (!/^[0-9a-f]{64}$/.test(parsed.signature)) {
    return jsonResponse({ error: 'Malformed join proof signature' }, 400)
  }

  const now = Date.now()
  if (parsed.expires_at_epoch_ms <= now) {
    return jsonResponse({ error: 'Join proof expired' }, 403)
  }
  if (parsed.issued_at_epoch_ms > now + MAX_CLOCK_SKEW_MS) {
    return jsonResponse({ error: 'Join proof issued in the future' }, 403)
  }

  return parsed
}

function readJsonObject(data: JsonObject, key: string): JsonObject | null {
  const value = data[key]
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  return value as JsonObject
}

function readEpochMs(data: JsonObject, key: string): number {
  const value = data[key]
  return typeof value === 'number' && Number.isFinite(value)
    ? Math.trunc(value)
    : 0
}

function isHomeMember(home: HomeRow, userId: string): boolean {
  return Array.isArray(home.member_ids) && home.member_ids.includes(userId)
}

async function joinProofSignature(
  tokenHash: string,
  proof: JoinProof,
): Promise<string> {
  const canonical = [
    JOIN_PROOF_VERSION,
    proof.server_instance_id,
    proof.home_id,
    proof.hub_id,
    String(proof.issued_at_epoch_ms),
    String(proof.expires_at_epoch_ms),
    proof.nonce,
  ].join('\n')
  return await hmacSha256Hex(tokenHash, canonical)
}

async function hmacSha256Hex(key: string, message: string): Promise<string> {
  const encoder = new TextEncoder()
  const cryptoKey = await crypto.subtle.importKey(
    'raw',
    encoder.encode(key),
    { name: 'HMAC', hash: 'SHA-256' },
    false,
    ['sign'],
  )
  const signature = await crypto.subtle.sign(
    'HMAC',
    cryptoKey,
    encoder.encode(message),
  )
  return hexLower(new Uint8Array(signature))
}

function constantTimeEqualHex(left: string, right: string): boolean {
  if (left.length !== right.length) return false
  let diff = 0
  for (let index = 0; index < left.length; index += 1) {
    diff |= left.charCodeAt(index) ^ right.charCodeAt(index)
  }
  return diff === 0
}

function hexLower(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map((byte) => byte.toString(16).padStart(2, '0'))
    .join('')
}
