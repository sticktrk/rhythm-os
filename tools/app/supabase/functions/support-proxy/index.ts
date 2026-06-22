import {
  corsHeaders,
  errorMessage,
  jsonResponse,
  readJson,
  readString,
  type JsonObject,
  withAuthenticatedRequest,
} from '../_shared/auth.ts'

const ALLOWED_METHODS = new Set(['GET', 'POST', 'PUT', 'PATCH', 'DELETE'])
const SUPPORT_OWNER_ONLY_REQUESTS = new Set([
  'POST /api/auth/claim',
  'POST /api/auth/support-token',
  'PUT /api/auth/settings',
  'POST /api/factory-reset',
  'PUT /api/hub/credentials',
  'DELETE /api/hub/credentials',
  'PUT /api/remote-access/config',
  'DELETE /api/remote-access/config',
  'PUT /api/wifi',
  'DELETE /api/wifi',
  'POST /api/diag/reset-matter-fabric',
  'POST /api/ota/upload',
  'POST /api/ota/update',
  'PUT /api/backup',
])
const DEFAULT_REMOTE_ACCESS_DOMAIN = 'rhythm.lighting'
const LEGACY_REMOTE_ACCESS_DOMAIN = 'devices.rhythm.lighting'
const MANAGED_SUBSCRIPTION_TIERS = ['pro']

type StaffContext = {
  userId: string
  email: string
}

type GrantRow = {
  id: string
  staff_user_id: string
  staff_email: string
  hub_id: string
  home_id: string
  status: string
  hostname: string
  expires_at: string
}

type HomeRow = {
  id: string
  owner_id: string
  support_access_consent_at: string | null
}

type SupportTokenRow = {
  token: string
}

Deno.serve((req) =>
  withAuthenticatedRequest(req, async ({ userId, claims, adminClient }) => {
    if (req.method !== 'POST') {
      return jsonResponse({ error: 'Method not allowed' }, 405)
    }

    let body: JsonObject
    try {
      body = await readJson(req)
    } catch (error) {
      return jsonResponse({ error: errorMessage(error) }, 400)
    }

    const staff = await requireActiveStaff(adminClient, userId, claims)
    if (staff instanceof Response) return staff

    try {
      return await proxyRequest(adminClient, staff, req, body)
    } catch (error) {
      console.error('Support proxy error:', error)
      return jsonResponse({ error: errorMessage(error) }, 500)
    }
  })
)

async function proxyRequest(
  adminClient: any,
  staff: StaffContext,
  req: Request,
  body: JsonObject,
): Promise<Response> {
  const grantId = readString(body, 'grant_id')
  if (!grantId) return jsonResponse({ error: 'Missing grant_id' }, 400)

  const grant = await readOwnedGrant(adminClient, staff.userId, grantId)
  if (grant instanceof Response) return grant

  const active = await ensureActiveGrant(adminClient, grant, req)
  if (active instanceof Response) return active

  const managed = await ensureManagedConsent(adminClient, grant, req)
  if (managed instanceof Response) return managed

  const method = (readString(body, 'method') ?? 'GET').toUpperCase()
  if (!ALLOWED_METHODS.has(method)) {
    await insertAudit(adminClient, grant, req, {
      action: 'proxy_denied',
      method,
      path: readString(body, 'path') ?? null,
      statusCode: 400,
      detail: { error: 'invalid_method' },
    })
    return jsonResponse({ error: 'Invalid method' }, 400)
  }

  const pathResult = buildProxyUrl(grant.hostname, body)
  if (pathResult instanceof Response) {
    await insertAudit(adminClient, grant, req, {
      action: 'proxy_denied',
      method,
      path: readString(body, 'path') ?? null,
      statusCode: pathResult.status,
      detail: { error: 'invalid_path' },
    })
    return pathResult
  }
  const { url, path } = pathResult

  if (isOwnerOnlyRequestForSupport(method, path)) {
    await insertAudit(adminClient, grant, req, {
      action: 'proxy_denied',
      method,
      path,
      statusCode: 403,
      detail: { error: 'owner_only_path' },
    })
    return jsonResponse({
      error: 'Support access is not allowed for this endpoint',
    }, 403)
  }

  const supportToken = await fetchSupportToken(adminClient, grant.hub_id)
  if (!supportToken) {
    await insertAudit(adminClient, grant, req, {
      action: 'proxy_denied',
      method,
      path,
      statusCode: 409,
      detail: { error: 'support_token_missing' },
    })
    return jsonResponse({ error: 'Support token not configured' }, 409)
  }

  let response: Response
  try {
    response = await fetch(url, {
      method,
      headers: proxyRequestHeaders(body, supportToken.token),
      body: method === 'GET' || body.body == null
        ? undefined
        : JSON.stringify(body.body),
    })
  } catch (error) {
    await insertAudit(adminClient, grant, req, {
      action: 'proxy_error',
      method,
      path,
      statusCode: 502,
      detail: { error: errorMessage(error) },
    })
    return jsonResponse({ error: 'Pi request failed' }, 502)
  }

  if (isEventStream(response)) {
    await insertAudit(adminClient, grant, req, {
      action: 'proxy_stream_start',
      method,
      path,
      statusCode: response.status,
    })
    await touchGrant(adminClient, grant.id)
    return streamResponse(response)
  }

  await insertAudit(adminClient, grant, req, {
    action: 'proxy_request',
    method,
    path,
    statusCode: response.status,
  })

  await touchGrant(adminClient, grant.id)

  return new Response(await response.arrayBuffer(), {
    status: response.status,
    headers: responseHeaders(response),
  })
}

async function touchGrant(adminClient: any, grantId: string): Promise<void> {
  const { error } = await adminClient
    .from('support_access_grants')
    .update({ last_accessed_at: new Date().toISOString() })
    .eq('id', grantId)
  if (error) console.error('Failed to update support grant access time:', error)
}

function isEventStream(response: Response): boolean {
  return response.headers.get('content-type')?.toLowerCase().includes(
    'text/event-stream',
  ) === true
}

function streamResponse(response: Response): Response {
  const headers = responseHeaders(response)
  headers.set('cache-control', 'no-cache')
  return new Response(response.body, {
    status: response.status,
    headers,
  })
}

function responseHeaders(response: Response): Headers {
  const headers = new Headers(corsHeaders)
  for (const name of [
    'content-type',
    'content-disposition',
    'cache-control',
  ]) {
    const value = response.headers.get(name)
    if (value) headers.set(name, value)
  }
  return headers
}

function proxyRequestHeaders(body: JsonObject, supportToken: string): Headers {
  const headers = new Headers()
  headers.set('authorization', `Bearer ${supportToken}`)

  const forwarded = body.headers
  if (forwarded && typeof forwarded === 'object' && !Array.isArray(forwarded)) {
    for (const [name, value] of Object.entries(forwarded)) {
      const headerName = name.trim().toLowerCase()
      if (!canForwardRequestHeader(headerName)) continue
      if (typeof value !== 'string') continue
      const headerValue = value.trim()
      if (!headerValue || /[\r\n]/.test(headerValue)) continue
      headers.set(headerName, headerValue)
    }
  }

  if (!headers.has('accept')) headers.set('accept', '*/*')
  if (body.body != null) {
    headers.set('content-type', 'application/json')
  }
  return headers
}

function canForwardRequestHeader(name: string): boolean {
  return name === 'accept' ||
    name === 'content-type' ||
    name === 'cache-control'
}

function isOwnerOnlyRequestForSupport(method: string, path: string): boolean {
  const pathname = path.split('?', 1)[0]
  return SUPPORT_OWNER_ONLY_REQUESTS.has(`${method} ${pathname}`)
}

async function requireActiveStaff(
  adminClient: any,
  userId: string,
  claims: JsonObject,
): Promise<StaffContext | Response> {
  const { data, error } = await adminClient
    .from('staff_members')
    .select('email,active')
    .eq('user_id', userId)
    .maybeSingle()
  if (error) throw new Error(error.message)
  if (!data?.active) {
    return jsonResponse({ error: 'Staff access required' }, 403)
  }

  return {
    userId,
    email: data.email ?? readClaimString(claims, 'email') ?? userId,
  }
}

async function readOwnedGrant(
  adminClient: any,
  userId: string,
  grantId: string,
): Promise<GrantRow | Response> {
  const { data, error } = await adminClient
    .from('support_access_grants')
    .select(
      'id,staff_user_id,staff_email,hub_id,home_id,status,hostname,expires_at',
    )
    .eq('id', grantId)
    .eq('staff_user_id', userId)
    .maybeSingle()
  if (error) throw new Error(error.message)
  if (!data) return jsonResponse({ error: 'Support access grant not found' }, 404)
  return data as GrantRow
}

async function ensureActiveGrant(
  adminClient: any,
  grant: GrantRow,
  req: Request,
): Promise<true | Response> {
  if (grant.status !== 'active') {
    await insertAudit(adminClient, grant, req, {
      action: 'proxy_denied',
      statusCode: 403,
      detail: { error: 'grant_not_active', status: grant.status },
    })
    return jsonResponse({ error: 'Support access grant is not active' }, 403)
  }

  if (new Date(grant.expires_at).getTime() > Date.now()) return true

  const { error } = await adminClient
    .from('support_access_grants')
    .update({ status: 'expired' })
    .eq('id', grant.id)
    .eq('status', 'active')
  if (error) throw new Error(error.message)

  await insertAudit(adminClient, grant, req, {
    action: 'grant_expired',
    statusCode: 403,
  })
  return jsonResponse({ error: 'Support access grant expired' }, 403)
}

async function ensureManagedConsent(
  adminClient: any,
  grant: GrantRow,
  req: Request,
): Promise<true | Response> {
  const home = await fetchHome(adminClient, grant.home_id)
  if (!home) {
    await insertAudit(adminClient, grant, req, {
      action: 'proxy_denied',
      statusCode: 404,
      detail: { error: 'home_not_found' },
    })
    return jsonResponse({ error: 'Home not found' }, 404)
  }

  if (!home.support_access_consent_at) {
    await insertAudit(adminClient, grant, req, {
      action: 'proxy_denied',
      statusCode: 403,
      detail: { error: 'support_access_consent_missing' },
    })
    return jsonResponse({ error: 'Support access consent is required' }, 403)
  }

  const hasSubscription = await ownerHasManagedSubscription(
    adminClient,
    home.owner_id,
  )
  if (!hasSubscription) {
    await insertAudit(adminClient, grant, req, {
      action: 'proxy_denied',
      statusCode: 403,
      detail: { error: 'managed_subscription_missing' },
    })
    return jsonResponse({ error: 'Active managed subscription required' }, 403)
  }

  return true
}

async function fetchHome(
  adminClient: any,
  homeId: string,
): Promise<HomeRow | null> {
  const { data, error } = await adminClient
    .from('homes')
    .select('id,owner_id,support_access_consent_at')
    .eq('id', homeId)
    .maybeSingle()
  if (error) throw new Error(error.message)
  return (data as HomeRow | null) ?? null
}

async function ownerHasManagedSubscription(
  adminClient: any,
  ownerId: string,
): Promise<boolean> {
  const { data, error } = await adminClient
    .from('subscriptions')
    .select('id')
    .eq('user_id', ownerId)
    .eq('status', 'active')
    .in('tier', MANAGED_SUBSCRIPTION_TIERS)
    .limit(1)
    .maybeSingle()
  if (error) throw new Error(error.message)
  return data != null
}

async function fetchSupportToken(
  adminClient: any,
  hubId: string,
): Promise<SupportTokenRow | null> {
  const { data, error } = await adminClient
    .from('hub_support_tokens')
    .select('token')
    .eq('hub_id', hubId)
    .maybeSingle()
  if (error) throw new Error(error.message)
  return (data as SupportTokenRow | null) ?? null
}

function buildProxyUrl(
  hostname: string,
  body: JsonObject,
): { url: string; path: string } | Response {
  const safeHostname = normalizeAllowedRemoteAccessHostname(hostname)
  if (!safeHostname) {
    return jsonResponse({ error: 'Remote access hostname is not allowed' }, 409)
  }

  const rawPath = readString(body, 'path')
  if (!rawPath || !rawPath.startsWith('/') || rawPath.startsWith('//')) {
    return jsonResponse({ error: 'Invalid path' }, 400)
  }
  if (/[\r\n]/.test(rawPath)) {
    return jsonResponse({ error: 'Invalid path' }, 400)
  }

  const [pathname, existingSearch = ''] = rawPath.split('?', 2)
  const url = new URL(`https://${safeHostname}${pathname}`)
  if (existingSearch) url.search = existingSearch

  const query = body.query
  if (query && typeof query === 'object' && !Array.isArray(query)) {
    for (const [key, value] of Object.entries(query)) {
      if (value == null) continue
      if (Array.isArray(value)) {
        for (const item of value) {
          if (item != null) url.searchParams.append(key, String(item))
        }
      } else {
        url.searchParams.set(key, String(value))
      }
    }
  }

  return { url: url.toString(), path: `${url.pathname}${url.search}` }
}

function normalizeAllowedRemoteAccessHostname(hostname: string): string | null {
  const clean = hostname.trim().toLowerCase().replace(/\.$/, '')
  if (!/^[a-z0-9-]+(\.[a-z0-9-]+)+$/.test(clean)) return null
  return allowedRemoteAccessDomains().some((domain) =>
    clean.endsWith(`.${domain}`) && clean.length > domain.length + 1
  )
    ? clean
    : null
}

function allowedRemoteAccessDomains(): string[] {
  const configured = Deno.env.get('RHYTHM_REMOTE_ACCESS_DOMAIN')?.trim() ??
    DEFAULT_REMOTE_ACCESS_DOMAIN
  const domains = [
    configured,
    DEFAULT_REMOTE_ACCESS_DOMAIN,
    LEGACY_REMOTE_ACCESS_DOMAIN,
  ]
  return Array.from(
    new Set(
      domains
        .map((domain) => domain.replace(/^\.+|\.+$/g, '').toLowerCase())
        .filter(Boolean),
    ),
  )
}

async function insertAudit(
  adminClient: any,
  grant: GrantRow,
  req: Request,
  {
    action,
    method = null,
    path = null,
    statusCode = null,
    detail = {},
  }: {
    action: string
    method?: string | null
    path?: string | null
    statusCode?: number | null
    detail?: JsonObject
  },
): Promise<void> {
  const { error } = await adminClient.from('support_access_audit').insert({
    grant_id: grant.id,
    hub_id: grant.hub_id,
    staff_email: grant.staff_email,
    action,
    method,
    path,
    status_code: statusCode,
    detail,
    source_ip: sourceIp(req),
  })
  if (error) console.error('Failed to write support proxy audit:', error)
}

function sourceIp(req: Request): string | null {
  const forwarded = req.headers.get('x-forwarded-for')?.trim()
  if (forwarded) return forwarded.split(',')[0].trim()
  return req.headers.get('cf-connecting-ip')?.trim() ?? null
}

function readClaimString(claims: JsonObject, key: string): string | null {
  const value = claims[key]
  return typeof value === 'string' && value.trim() ? value.trim() : null
}
