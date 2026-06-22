import {
  errorMessage,
  jsonResponse,
  readJson,
  readString,
  type JsonObject,
  withAuthenticatedRequest,
} from '../_shared/auth.ts'

const DEFAULT_TTL_SECONDS = 60 * 60
const MIN_TTL_SECONDS = 60
const MAX_TTL_SECONDS = 4 * 60 * 60
const MANAGED_SUBSCRIPTION_TIERS = ['pro']
const DEFAULT_REMOTE_ACCESS_DOMAIN = 'rhythm.lighting'
const LEGACY_REMOTE_ACCESS_DOMAIN = 'devices.rhythm.lighting'
const defaultGitHubIssuesRepo = 'sticktrk/cross'
const legacyGitHubIssuesRepos = new Set([
  'sticktrk/rhythm-app',
  'sticktrk/rhythm-app-flutter',
  'sticktrk/rhythm-os',
])

type StaffContext = {
  userId: string
  email: string
}

type HubRow = {
  id: string
  home_id: string
  type: string
  name: string | null
}

type HomeRow = {
  id: string
  name: string | null
  owner_id: string
  support_access_consent_at: string | null
}

type RemoteAccessRow = {
  hostname: string
}

type SupportTokenRow = {
  home_id: string
  token: string
}

type DirectAccessPayload = {
  hostname: string
  token_id: string
  token: string
  expires_at: string
}

type GrantRow = {
  id: string
  staff_user_id: string
  staff_email: string
  hub_id: string
  home_id: string
  submission_id: string | null
  status: string
  reason: string | null
  hostname: string
  created_at: string
  expires_at: string
  revoked_at: string | null
  direct_token_id: string | null
}

type SubmissionRow = {
  id: string
  status: string
  server_hub_id: string | null
  github_issue_number: number | null
  github_issue_url: string | null
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

    const action = readString(body, 'action') ?? 'grant'
    try {
      if (action === 'grant') {
        return await grantAccess(adminClient, staff, body)
      }
      if (action === 'direct-session') {
        return await directSession(adminClient, staff, body)
      }
      if (action === 'refresh') {
        return await refreshGrant(adminClient, staff, body)
      }
      if (action === 'revoke') {
        return await revokeGrant(adminClient, staff, body)
      }
      return jsonResponse({ error: 'Invalid action' }, 400)
    } catch (error) {
      console.error('Support access error:', error)
      return jsonResponse({ error: errorMessage(error) }, 500)
    }
  })
)

async function grantAccess(
  adminClient: any,
  staff: StaffContext,
  body: JsonObject,
): Promise<Response> {
  const hubId = readString(body, 'hub_id')
  if (!hubId) return jsonResponse({ error: 'Missing hub_id' }, 400)

  const context = await readGrantContext(adminClient, hubId)
  if (context instanceof Response) return context
  const { hub, home, remoteAccess, supportToken } = context

  const managed = await ensureManagedConsent(adminClient, home)
  if (managed instanceof Response) return managed

  if (supportToken.home_id !== hub.home_id) {
    return jsonResponse({ error: 'Support token home mismatch' }, 409)
  }

  const submissionId = readString(body, 'submission_id')
  const submission = submissionId
    ? await readSubmissionForHub(adminClient, submissionId, hub.id)
    : null
  if (submission instanceof Response) return submission

  const ttlSeconds = readTtlSeconds(body)
  const expiresAt = new Date(Date.now() + ttlSeconds * 1000).toISOString()
  const reason = readString(body, 'reason')

  const { data, error } = await adminClient
    .from('support_access_grants')
    .insert({
      staff_user_id: staff.userId,
      staff_email: staff.email,
      hub_id: hub.id,
      home_id: hub.home_id,
      submission_id: submission?.id ?? null,
      scope: 'full',
      status: 'active',
      reason,
      hostname: remoteAccess.hostname,
      expires_at: expiresAt,
    })
    .select('id,status,hostname,expires_at')
    .single()
  if (error) throw new Error(error.message)

  const grant: GrantRow = {
    id: data.id,
    staff_user_id: staff.userId,
    staff_email: staff.email,
    hub_id: hub.id,
    home_id: hub.home_id,
    submission_id: submission?.id ?? null,
    status: data.status,
    reason,
    hostname: data.hostname,
    created_at: new Date().toISOString(),
    expires_at: data.expires_at,
    revoked_at: null,
    direct_token_id: null,
  }
  const directAccess = await issueDirectSessionForGrant(
    adminClient,
    grant,
    supportToken,
    staff,
  )
  if (directAccess instanceof Response) {
    await revokeGrantAfterDirectSessionFailure(adminClient, grant, staff)
    return directAccess
  }

  await insertAudit(adminClient, {
    grantId: data.id,
    hubId: hub.id,
    staffEmail: staff.email,
    action: 'grant_created',
    detail: {
      ttl_seconds: ttlSeconds,
      reason,
      submission_id: submission?.id ?? null,
    },
  })

  if (submission && ['received', 'reported'].includes(submission.status)) {
    await updateSubmissionStatus(adminClient, {
      submissionId: submission.id,
      status: 'reviewing',
      grantId: data.id,
      hubId: hub.id,
      staffEmail: staff.email,
    })
  }

  const appLinkParams = employeeAppLinkParams({
    grantId: data.id,
    hostname: data.hostname,
    hubId: hub.id,
    homeName: home.name,
    hubName: hub.name,
    expiresAt: data.expires_at,
  })

  return jsonResponse({
    grant_id: data.id,
    status: data.status,
    hub_id: hub.id,
    home_id: hub.home_id,
    hub_name: hub.name,
    home_name: home.name,
    submission_id: submission?.id ?? null,
    hostname: data.hostname,
    expires_at: data.expires_at,
    app_link_params: appLinkParams,
    app_link_query: employeeAppLinkQuery(appLinkParams),
    direct_access: directAccess,
  })
}

async function directSession(
  adminClient: any,
  staff: StaffContext,
  body: JsonObject,
): Promise<Response> {
  const grantId = readString(body, 'grant_id')
  if (!grantId) return jsonResponse({ error: 'Missing grant_id' }, 400)

  const grant = await readOwnedGrant(adminClient, staff.userId, grantId)
  if (grant instanceof Response) return grant
  const active = await expireIfNeeded(adminClient, grant)
  if (active instanceof Response) return active

  const home = await fetchHome(adminClient, grant.home_id)
  if (!home) return jsonResponse({ error: 'Home not found' }, 404)
  const managed = await ensureManagedConsent(adminClient, home)
  if (managed instanceof Response) return managed

  const supportToken = await fetchSupportToken(adminClient, grant.hub_id)
  if (!supportToken) {
    return jsonResponse({ error: 'Support token not configured' }, 409)
  }
  if (supportToken.home_id !== grant.home_id) {
    return jsonResponse({ error: 'Support token home mismatch' }, 409)
  }

  const directAccess = await issueDirectSessionForGrant(
    adminClient,
    grant,
    supportToken,
    staff,
  )
  if (directAccess instanceof Response) return directAccess

  return jsonResponse({
    grant_id: grant.id,
    status: 'active',
    hub_id: grant.hub_id,
    home_id: grant.home_id,
    hostname: grant.hostname,
    expires_at: grant.expires_at,
    direct_access: directAccess,
  })
}

async function refreshGrant(
  adminClient: any,
  staff: StaffContext,
  body: JsonObject,
): Promise<Response> {
  const grantId = readString(body, 'grant_id')
  if (!grantId) return jsonResponse({ error: 'Missing grant_id' }, 400)

  const grant = await readOwnedGrant(adminClient, staff.userId, grantId)
  if (grant instanceof Response) return grant
  const expiry = await expireIfNeeded(adminClient, grant)
  if (expiry instanceof Response) return expiry

  const home = await fetchHome(adminClient, grant.home_id)
  if (!home) return jsonResponse({ error: 'Home not found' }, 404)
  const managed = await ensureManagedConsent(adminClient, home)
  if (managed instanceof Response) return managed

  const ttlSeconds = readTtlSeconds(body)
  const expiresAt = new Date(Date.now() + ttlSeconds * 1000).toISOString()
  const { error } = await adminClient
    .from('support_access_grants')
    .update({ expires_at: expiresAt })
    .eq('id', grant.id)
  if (error) throw new Error(error.message)

  await insertAudit(adminClient, {
    grantId: grant.id,
    hubId: grant.hub_id,
    staffEmail: staff.email,
    action: 'grant_refreshed',
    detail: { ttl_seconds: ttlSeconds },
  })

  let directAccess: DirectAccessPayload | null = null
  const supportToken = await fetchSupportToken(adminClient, grant.hub_id)
  if (supportToken && supportToken.home_id === grant.home_id) {
    const directGrant = { ...grant, expires_at: expiresAt }
    const issued = await issueDirectSessionForGrant(
      adminClient,
      directGrant,
      supportToken,
      staff,
    )
    if (!(issued instanceof Response)) {
      directAccess = issued
    }
  }

  return jsonResponse({
    grant_id: grant.id,
    status: 'active',
    hostname: grant.hostname,
    expires_at: expiresAt,
    ...(directAccess ? { direct_access: directAccess } : {}),
  })
}

async function revokeGrant(
  adminClient: any,
  staff: StaffContext,
  body: JsonObject,
): Promise<Response> {
  const grantId = readString(body, 'grant_id')
  if (!grantId) return jsonResponse({ error: 'Missing grant_id' }, 400)

  const grant = await readOwnedGrant(adminClient, staff.userId, grantId)
  if (grant instanceof Response) return grant
  const resolveSubmission = readBool(body, 'resolve_submission') === true

  if (grant.status === 'revoked') {
    if (resolveSubmission && grant.submission_id) {
      await updateSubmissionStatus(adminClient, {
        submissionId: grant.submission_id,
        status: 'resolved',
        grantId: grant.id,
        hubId: grant.hub_id,
        staffEmail: staff.email,
      })
      await insertAudit(adminClient, {
        grantId: grant.id,
        hubId: grant.hub_id,
        staffEmail: staff.email,
        action: 'grant_revoke_replayed',
        detail: { resolve_submission: true },
      })
    }

    return jsonResponse({
      grant_id: grant.id,
      status: 'revoked',
      revoked_at: grant.revoked_at,
    })
  }
  if (grant.status !== 'active' && grant.status !== 'expired') {
    return jsonResponse({ error: 'Support access grant is not active' }, 403)
  }

  if (grant.status === 'active' && isExpired(grant)) {
    await markGrantExpired(adminClient, grant)
    grant.status = 'expired'
  }

  const revokedAt = new Date().toISOString()
  const { error } = await adminClient
    .from('support_access_grants')
    .update({
      status: 'revoked',
      revoked_at: revokedAt,
      revoked_by: staff.userId,
    })
    .eq('id', grant.id)
  if (error) throw new Error(error.message)

  await revokeDirectSessionForGrant(adminClient, grant, staff)

  let submission: SubmissionRow | null = null
  if (grant.submission_id) {
    submission = await fetchSubmission(adminClient, grant.submission_id)
    if (resolveSubmission) {
      await updateSubmissionStatus(adminClient, {
        submissionId: grant.submission_id,
        status: 'resolved',
        grantId: grant.id,
        hubId: grant.hub_id,
        staffEmail: staff.email,
      })
    }
  }

  await insertAudit(adminClient, {
    grantId: grant.id,
    hubId: grant.hub_id,
    staffEmail: staff.email,
    action: 'grant_revoked',
    detail: { resolve_submission: resolveSubmission },
  })

  if (resolveSubmission && submission?.github_issue_number) {
    await commentOnGitHubIssueIfConfigured(staff, grant, submission, revokedAt)
  }

  return jsonResponse({
    grant_id: grant.id,
    status: 'revoked',
    revoked_at: revokedAt,
  })
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

async function readGrantContext(
  adminClient: any,
  hubId: string,
): Promise<{
  hub: HubRow
  home: HomeRow
  remoteAccess: RemoteAccessRow
  supportToken: SupportTokenRow
} | Response> {
  const hub = await fetchHub(adminClient, hubId)
  if (!hub || hub.type !== 'server') {
    return jsonResponse({ error: 'Server hub not found' }, 404)
  }

  const home = await fetchHome(adminClient, hub.home_id)
  if (!home) return jsonResponse({ error: 'Home not found' }, 404)

  const remoteAccess = await fetchRemoteAccess(adminClient, hub.id)
  if (!remoteAccess) {
    return jsonResponse({ error: 'Remote access tunnel not configured' }, 409)
  }
  const hostname = normalizeAllowedRemoteAccessHostname(remoteAccess.hostname)
  if (!hostname) {
    return jsonResponse({ error: 'Remote access hostname is not allowed' }, 409)
  }
  remoteAccess.hostname = hostname

  const supportToken = await fetchSupportToken(adminClient, hub.id)
  if (!supportToken) {
    return jsonResponse({ error: 'Support token not configured' }, 409)
  }

  return { hub, home, remoteAccess, supportToken }
}

async function ensureManagedConsent(
  adminClient: any,
  home: HomeRow,
): Promise<true | Response> {
  if (!home.support_access_consent_at) {
    return jsonResponse({ error: 'Support access consent is required' }, 403)
  }

  const hasSubscription = await ownerHasManagedSubscription(
    adminClient,
    home.owner_id,
  )
  if (!hasSubscription) {
    return jsonResponse({ error: 'Active managed subscription required' }, 403)
  }
  return true
}

async function fetchHub(adminClient: any, hubId: string): Promise<HubRow | null> {
  const { data, error } = await adminClient
    .from('hubs')
    .select('id,home_id,type,name')
    .eq('id', hubId)
    .maybeSingle()
  if (error) throw new Error(error.message)
  return (data as HubRow | null) ?? null
}

async function fetchHome(
  adminClient: any,
  homeId: string,
): Promise<HomeRow | null> {
  const { data, error } = await adminClient
    .from('homes')
    .select('id,name,owner_id,support_access_consent_at')
    .eq('id', homeId)
    .maybeSingle()
  if (error) throw new Error(error.message)
  return (data as HomeRow | null) ?? null
}

async function fetchRemoteAccess(
  adminClient: any,
  hubId: string,
): Promise<RemoteAccessRow | null> {
  const { data, error } = await adminClient
    .from('hub_remote_access')
    .select('hostname')
    .eq('hub_id', hubId)
    .maybeSingle()
  if (error) throw new Error(error.message)
  return (data as RemoteAccessRow | null) ?? null
}

async function fetchSupportToken(
  adminClient: any,
  hubId: string,
): Promise<SupportTokenRow | null> {
  const { data, error } = await adminClient
    .from('hub_support_tokens')
    .select('home_id,token')
    .eq('hub_id', hubId)
    .maybeSingle()
  if (error) throw new Error(error.message)
  return (data as SupportTokenRow | null) ?? null
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

async function readSubmissionForHub(
  adminClient: any,
  submissionId: string,
  hubId: string,
): Promise<SubmissionRow | null | Response> {
  const submission = await fetchSubmission(adminClient, submissionId)
  if (!submission) {
    return jsonResponse({ error: 'Debug bundle submission not found' }, 404)
  }
  if (submission.server_hub_id !== hubId) {
    return jsonResponse({
      error: 'Debug bundle submission is not linked to this hub',
    }, 400)
  }
  return submission
}

async function fetchSubmission(
  adminClient: any,
  submissionId: string,
): Promise<SubmissionRow | null> {
  const { data, error } = await adminClient
    .from('support_debug_bundle_submissions')
    .select('id,status,server_hub_id,github_issue_number,github_issue_url')
    .eq('id', submissionId)
    .maybeSingle()
  if (error) throw new Error(error.message)
  return (data as SubmissionRow | null) ?? null
}

async function updateSubmissionStatus(
  adminClient: any,
  {
    submissionId,
    status,
    grantId,
    hubId,
    staffEmail,
  }: {
    submissionId: string
    status: string
    grantId: string
    hubId: string
    staffEmail: string
  },
): Promise<void> {
  const { error } = await adminClient
    .from('support_debug_bundle_submissions')
    .update({ status })
    .eq('id', submissionId)
  if (!error) return

  console.error('Failed to update debug bundle submission status:', error)
  await insertAudit(adminClient, {
    grantId,
    hubId,
    staffEmail,
    action: 'submission_status_update_failed',
    detail: {
      submission_id: submissionId,
      status,
      error: error.message ?? String(error),
    },
  })
}

async function readOwnedGrant(
  adminClient: any,
  userId: string,
  grantId: string,
): Promise<GrantRow | Response> {
  const { data, error } = await adminClient
    .from('support_access_grants')
    .select(
      'id,staff_user_id,staff_email,hub_id,home_id,submission_id,status,reason,hostname,created_at,expires_at,revoked_at,direct_token_id',
    )
    .eq('id', grantId)
    .eq('staff_user_id', userId)
    .maybeSingle()
  if (error) throw new Error(error.message)
  if (!data) return jsonResponse({ error: 'Support access grant not found' }, 404)
  return data as GrantRow
}

function isExpired(grant: GrantRow): boolean {
  return new Date(grant.expires_at).getTime() <= Date.now()
}

async function expireIfNeeded(
  adminClient: any,
  grant: GrantRow,
): Promise<true | Response> {
  if (grant.status !== 'active') {
    return jsonResponse({ error: 'Support access grant is not active' }, 403)
  }

  if (!isExpired(grant)) return true

  await markGrantExpired(adminClient, grant)

  return jsonResponse({ error: 'Support access grant expired' }, 403)
}

async function markGrantExpired(
  adminClient: any,
  grant: GrantRow,
): Promise<void> {
  const { error } = await adminClient
    .from('support_access_grants')
    .update({ status: 'expired' })
    .eq('id', grant.id)
    .eq('status', 'active')
  if (error) throw new Error(error.message)

  await insertAudit(adminClient, {
    grantId: grant.id,
    hubId: grant.hub_id,
    staffEmail: grant.staff_email,
    action: 'grant_expired',
  })
}

async function insertAudit(
  adminClient: any,
  {
    grantId,
    hubId,
    staffEmail,
    action,
    detail = {},
  }: {
    grantId: string
    hubId: string
    staffEmail: string
    action: string
    detail?: JsonObject
  },
): Promise<void> {
  const { error } = await adminClient.from('support_access_audit').insert({
    grant_id: grantId,
    hub_id: hubId,
    staff_email: staffEmail,
    action,
    detail,
  })
  if (error) console.error('Failed to write support access audit:', error)
}

async function issueDirectSessionForGrant(
  adminClient: any,
  grant: GrantRow,
  supportToken: SupportTokenRow,
  staff: StaffContext,
): Promise<DirectAccessPayload | Response> {
  if (grant.direct_token_id) {
    await revokeDirectSessionToken({
      hostname: grant.hostname,
      supportToken: supportToken.token,
      tokenId: grant.direct_token_id,
    })
  }

  const ttlSeconds = Math.max(
    1,
    Math.floor((new Date(grant.expires_at).getTime() - Date.now()) / 1000),
  )
  const response = await fetch(
    `https://${grant.hostname}/api/auth/support-session-token`,
    {
      method: 'POST',
      headers: {
        'Authorization': `Bearer ${supportToken.token}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({
        ttl_seconds: ttlSeconds,
        label: `support:${grant.id}:${staff.email}`,
      }),
    },
  ).catch((error) => error as Error)

  if (response instanceof Error) {
    await insertAudit(adminClient, {
      grantId: grant.id,
      hubId: grant.hub_id,
      staffEmail: staff.email,
      action: 'direct_session_failed',
      detail: { error: errorMessage(response) },
    })
    return jsonResponse({ error: 'Could not open direct support session' }, 502)
  }

  const data = await readResponseJson(response)
  if (!response.ok) {
    await insertAudit(adminClient, {
      grantId: grant.id,
      hubId: grant.hub_id,
      staffEmail: staff.email,
      action: 'direct_session_failed',
      detail: {
        status_code: response.status,
        error: readErrorFromResponseData(data),
      },
    })
    return jsonResponse({ error: 'Could not open direct support session' }, 502)
  }

  const token = readString(data, 'token')
  const tokenId = readString(data, 'token_id')
  const expiresAtEpochMs = readNumber(data, 'expires_at_epoch_ms')
  if (!token || !tokenId || !expiresAtEpochMs) {
    await insertAudit(adminClient, {
      grantId: grant.id,
      hubId: grant.hub_id,
      staffEmail: staff.email,
      action: 'direct_session_failed',
      detail: { error: 'invalid_device_response' },
    })
    return jsonResponse({ error: 'Invalid direct support session response' }, 502)
  }

  const directExpiresAt = new Date(expiresAtEpochMs).toISOString()
  const { error } = await adminClient
    .from('support_access_grants')
    .update({
      direct_token_id: tokenId,
      last_accessed_at: new Date().toISOString(),
    })
    .eq('id', grant.id)
  if (error) throw new Error(error.message)

  await insertAudit(adminClient, {
    grantId: grant.id,
    hubId: grant.hub_id,
    staffEmail: staff.email,
    action: 'direct_session_issued',
    detail: {
      token_id: tokenId,
      expires_at: directExpiresAt,
    },
  })

  grant.direct_token_id = tokenId
  return {
    hostname: grant.hostname,
    token_id: tokenId,
    token,
    expires_at: directExpiresAt,
  }
}

async function revokeGrantAfterDirectSessionFailure(
  adminClient: any,
  grant: GrantRow,
  staff: StaffContext,
): Promise<void> {
  const revokedAt = new Date().toISOString()
  const { error } = await adminClient
    .from('support_access_grants')
    .update({
      status: 'revoked',
      revoked_at: revokedAt,
      revoked_by: staff.userId,
    })
    .eq('id', grant.id)
    .eq('status', 'active')
  if (error) {
    console.error('Failed to revoke grant after direct session failure:', error)
  }
  await insertAudit(adminClient, {
    grantId: grant.id,
    hubId: grant.hub_id,
    staffEmail: staff.email,
    action: 'grant_revoked_direct_session_failed',
  })
}

async function revokeDirectSessionForGrant(
  adminClient: any,
  grant: GrantRow,
  staff: StaffContext,
): Promise<void> {
  if (!grant.direct_token_id) return
  const supportToken = await fetchSupportToken(adminClient, grant.hub_id)
  if (!supportToken) return
  const revoked = await revokeDirectSessionToken({
    hostname: grant.hostname,
    supportToken: supportToken.token,
    tokenId: grant.direct_token_id,
  })
  await insertAudit(adminClient, {
    grantId: grant.id,
    hubId: grant.hub_id,
    staffEmail: staff.email,
    action: revoked ? 'direct_session_revoked' : 'direct_session_revoke_failed',
    detail: { token_id: grant.direct_token_id },
  })
}

async function revokeDirectSessionToken({
  hostname,
  supportToken,
  tokenId,
}: {
  hostname: string
  supportToken: string
  tokenId: string
}): Promise<boolean> {
  try {
    const response = await fetch(
      `https://${hostname}/api/auth/support-session-token/revoke`,
      {
        method: 'POST',
        headers: {
          'Authorization': `Bearer ${supportToken}`,
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({ token_id: tokenId }),
      },
    )
    return response.ok
  } catch (error) {
    console.error('Failed to revoke direct support session token:', error)
    return false
  }
}

async function readResponseJson(response: Response): Promise<JsonObject> {
  try {
    const data = await response.json()
    return data && typeof data === 'object'
      ? data as JsonObject
      : {}
  } catch (_) {
    return {}
  }
}

function readErrorFromResponseData(data: JsonObject): string | null {
  return readString(data, 'error') ?? readString(data, 'message')
}

async function commentOnGitHubIssueIfConfigured(
  staff: StaffContext,
  grant: GrantRow,
  submission: SubmissionRow,
  revokedAt: string,
): Promise<void> {
  const token = Deno.env.get('GITHUB_ISSUES_TOKEN')?.trim()
  if (!token || !submission.github_issue_number) return

  const repo = readGitHubIssuesRepo()
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repo)) return

  const durationMinutes = Math.max(
    0,
    Math.round(
      (new Date(revokedAt).getTime() - new Date(grant.created_at).getTime()) /
        60000,
    ),
  )
  const body = [
    `Support access session \`${grant.id}\` was revoked by ${staff.email}.`,
    `Duration: ${durationMinutes} minute${durationMinutes === 1 ? '' : 's'}.`,
  ].join('\n')

  try {
    const response = await fetch(
      `https://api.github.com/repos/${repo}/issues/${submission.github_issue_number}/comments`,
      {
        method: 'POST',
        headers: {
          Authorization: `Bearer ${token}`,
          Accept: 'application/vnd.github+json',
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({ body }),
      },
    )
    if (!response.ok) {
      console.error('GitHub support access comment failed:', response.status)
    }
  } catch (error) {
    console.error('GitHub support access comment error:', error)
  }
}

function readTtlSeconds(body: JsonObject): number {
  const raw = readNumber(body, 'ttl_seconds')
  if (raw == null) return DEFAULT_TTL_SECONDS
  return Math.max(MIN_TTL_SECONDS, Math.min(MAX_TTL_SECONDS, Math.floor(raw)))
}

function readNumber(data: JsonObject, key: string): number | null {
  const value = data[key]
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value === 'string' && /^-?\d+(\.\d+)?$/.test(value.trim())) {
    return Number.parseFloat(value)
  }
  return null
}

function readBool(data: JsonObject, key: string): boolean | null {
  const value = data[key]
  if (typeof value === 'boolean') return value
  if (typeof value === 'string') {
    if (value.toLowerCase() === 'true') return true
    if (value.toLowerCase() === 'false') return false
  }
  return null
}

function employeeAppLinkParams({
  grantId,
  hostname,
  hubId,
  homeName,
  hubName,
  expiresAt,
}: {
  grantId: string
  hostname: string
  hubId: string
  homeName: string | null
  hubName: string | null
  expiresAt: string
}): Record<string, string> {
  const params: Record<string, string> = {
    employee_mode: '1',
    employee_grant_id: grantId,
    employee_hostname: hostname,
    employee_hub_id: hubId,
    employee_expires_at: expiresAt,
  }
  if (homeName?.trim()) params.employee_home_name = homeName.trim()
  if (hubName?.trim()) params.employee_hub_name = hubName.trim()
  return params
}

function employeeAppLinkQuery(params: Record<string, string>): string {
  const search = new URLSearchParams()
  for (const [key, value] of Object.entries(params)) {
    search.set(key, value)
  }
  return search.toString()
}

function readClaimString(claims: JsonObject, key: string): string | null {
  const value = claims[key]
  return typeof value === 'string' && value.trim() ? value.trim() : null
}

function readGitHubIssuesRepo(): string {
  const configured = Deno.env.get('GITHUB_ISSUES_REPO')?.trim()
  if (!configured) return defaultGitHubIssuesRepo

  if (legacyGitHubIssuesRepos.has(configured.toLowerCase())) {
    return defaultGitHubIssuesRepo
  }

  return configured
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
