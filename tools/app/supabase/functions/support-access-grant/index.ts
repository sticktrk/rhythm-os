import {
  errorMessage,
  jsonResponse,
  readJson,
  readString,
  type JsonObject,
  withAuthenticatedRequest,
} from '../_shared/auth.ts'

const ENVELOPE_VERSION = 'support_access_token_v1'
const ENVELOPE_ALGORITHM = 'aes-gcm-256'
const KEY_DERIVATION = 'sha256-env-v1'
const DEFAULT_SCOPE = 'beta_admin'
const SUPPORT_TOKEN_PREFIX = 'rhythm_support_'
const SUPPORT_SESSION_TOKEN_PREFIX = 'rhythm_support_session_'

type HomeRow = {
  id: string
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

    const authorized = await readAuthorizedHub(adminClient, userId, hubId)
    if (authorized instanceof Response) return authorized

    const action = readString(body, 'action') ?? 'grant'
    if (action === 'check') {
      const { data, error } = await adminClient.from('hub_support_review_policy')
        .select('hub_id').eq('hub_id', authorized.hub.id).maybeSingle()
      if (error) return jsonResponse({ error: 'Could not check automatic support access' }, 503)
      return jsonResponse({ allowed: !data })
    }
    if (action === 'revoke') {
      return await storeGrant(adminClient, userId, authorized.hub, 'revoke', body, {})
    }
    if (action !== 'grant') {
      return jsonResponse({ error: 'Invalid action' }, 400)
    }

    return await createGrant(adminClient, userId, authorized.hub, body)
  })
)

async function createGrant(
  adminClient: any,
  userId: string,
  hub: HubRow,
  body: JsonObject,
): Promise<Response> {
  try {
    const tokenId = readString(body, 'token_id')
    const token = readString(body, 'token')
    const scopeResult = readScope(body)
    if (scopeResult instanceof Response) return scopeResult
    const scope = scopeResult
    const label = readString(body, 'label')
    const expiresAt = readString(body, 'expires_at')

    if (!tokenId) return jsonResponse({ error: 'Missing token_id' }, 400)
    if (!token) return jsonResponse({ error: 'Missing token' }, 400)
    if (!isDurableSupportToken(token)) {
      return jsonResponse({ error: 'Expected durable support token' }, 400)
    }

    const encryptedToken = await encryptSupportToken(token, {
      hubId: hub.id,
      tokenId,
      scope,
    })
    return await storeGrant(adminClient, userId, hub, 'grant', body, {
      token_id: tokenId, label, scope, encrypted_token: encryptedToken,
      key_id: encryptedToken.key_id, expires_at: expiresAt,
    })
  } catch (error) {
    return jsonResponse({ error: errorMessage(error) }, 500)
  }
}

async function storeGrant(
  adminClient: any,
  userId: string,
  hub: HubRow,
  action: 'grant' | 'revoke',
  body: JsonObject,
  payload: JsonObject,
): Promise<Response> {
  const scope = readScope(body)
  if (scope instanceof Response) return scope
  const { data, error } = await adminClient.rpc('store_customer_support_grant', {
    p_actor: userId, p_hub: hub.id, p_action: action, p_payload: payload,
    p_review: readString(body, 'assistant_plan_id'),
  })
  if (error?.code === '42501') return jsonResponse({ error: 'Support access requires resident review' }, 403)
  if (error?.code === 'P0001') return jsonResponse({ error: 'Review support access again' }, 409)
  if (error) return jsonResponse({ error: 'Could not update support access' }, 500)
  return jsonResponse(data)
}

async function readAuthorizedHub(
  adminClient: any,
  userId: string,
  hubId: string,
): Promise<{ hub: HubRow; home: HomeRow } | Response> {
  const { data: hub, error: hubError } = await adminClient
    .from('hubs')
    .select('id, home_id, type')
    .eq('id', hubId)
    .maybeSingle()
  if (hubError) throw new Error(hubError.message)
  if (!hub || hub.type !== 'server') {
    return jsonResponse({ error: 'Server hub not found' }, 404)
  }

  const { data: home, error: homeError } = await adminClient
    .from('homes')
    .select('id, member_ids')
    .eq('id', hub.home_id)
    .maybeSingle()
  if (homeError) throw new Error(homeError.message)
  if (!home || !isHomeMember(home, userId)) {
    return jsonResponse({ error: 'Not authorized for this home' }, 403)
  }

  return { hub, home }
}

function readScope(body: JsonObject): string | Response {
  const scope = readString(body, 'scope') ?? DEFAULT_SCOPE
  if (scope !== DEFAULT_SCOPE) {
    return jsonResponse({ error: `Unsupported support access scope: ${scope}` }, 400)
  }
  return scope
}

function isHomeMember(home: HomeRow, userId: string): boolean {
  return Array.isArray(home.member_ids) && home.member_ids.includes(userId)
}

function isDurableSupportToken(token: string): boolean {
  return token.startsWith(SUPPORT_TOKEN_PREFIX) &&
    !token.startsWith(SUPPORT_SESSION_TOKEN_PREFIX)
}

async function encryptSupportToken(
  token: string,
  context: { hubId: string; tokenId: string; scope: string },
): Promise<Record<string, string>> {
  const keyId = Deno.env.get('SUPPORT_ACCESS_KEY_ID')?.trim() || 'default'
  const key = await supportAccessKey()
  const nonce = crypto.getRandomValues(new Uint8Array(12))
  const aad = supportAccessAad(context)
  const encoded = new TextEncoder().encode(token)
  const encrypted = new Uint8Array(
    await crypto.subtle.encrypt(
      {
        name: 'AES-GCM',
        iv: nonce,
        additionalData: new TextEncoder().encode(aad),
      },
      key,
      encoded,
    ),
  )
  const macStart = encrypted.length - 16
  const ciphertext = encrypted.slice(0, macStart)
  const mac = encrypted.slice(macStart)

  return {
    version: ENVELOPE_VERSION,
    algorithm: ENVELOPE_ALGORITHM,
    key_derivation: KEY_DERIVATION,
    key_id: keyId,
    aad,
    nonce: base64Url(nonce),
    ciphertext: base64Url(ciphertext),
    mac: base64Url(mac),
  }
}

async function supportAccessKey(): Promise<CryptoKey> {
  const secret = Deno.env.get('SUPPORT_ACCESS_ENCRYPTION_KEY')?.trim()
  if (!secret) throw new Error('Missing SUPPORT_ACCESS_ENCRYPTION_KEY')
  const digest = await crypto.subtle.digest(
    'SHA-256',
    new TextEncoder().encode(secret),
  )
  return await crypto.subtle.importKey(
    'raw',
    digest,
    { name: 'AES-GCM' },
    false,
    ['encrypt'],
  )
}

function supportAccessAad(context: {
  hubId: string
  tokenId: string
  scope: string
}): string {
  return `hub:${context.hubId}:token:${context.tokenId}:scope:${context.scope}`
}

function base64Url(bytes: Uint8Array): string {
  let binary = ''
  for (const byte of bytes) {
    binary += String.fromCharCode(byte)
  }
  return btoa(binary)
    .replaceAll('+', '-')
    .replaceAll('/', '_')
    .replaceAll('=', '')
}
