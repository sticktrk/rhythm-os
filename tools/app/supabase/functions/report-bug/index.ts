import { createClient } from 'https://esm.sh/@supabase/supabase-js@2'
import {
  errorMessage,
  jsonResponse,
  readJson,
  readString,
  withAuthenticatedRequest,
} from '../_shared/auth.ts'

// Required secrets:
// - GITHUB_ISSUES_TOKEN: fine-grained token with issue write access
// Optional secrets:
// - SB_PUBLISHABLE_KEY: preferred Supabase public key for Auth claims
// - SB_SECRET_KEY: preferred Supabase elevated key for admin operations
// - GITHUB_ISSUES_REPO: owner/repo, defaults to sticktrk/cross
// - GITHUB_ISSUES_LABELS: comma-separated labels
// - GITHUB_ISSUES_ASSIGNEES: comma-separated GitHub usernames
// - DEBUG_BUNDLE_SIGNED_URL_EXPIRES_SECONDS: default 604800, max 2592000

type DebugBundleSubmission = {
  id: string
  user_id: string
  user_email: string | null
  is_anonymous: boolean
  reference_code: string
  status: string
  summary: string | null
  app_version: string | null
  app_build: string | null
  app_platform: string | null
  server_hub_id: string | null
  server_name: string | null
  server_host: string | null
  server_port: number | null
  server_version: string | null
  server_platform_context: string | null
  bundle_storage_path: string | null
  bundle_file_name: string | null
  bundle_content_type: string | null
  bundle_size_bytes: number | null
  created_at: string
  github_issue_url: string | null
  github_issue_number: number | null
}

type SignedBundleLink = {
  signedUrl: string | null
  expiresAt: string | null
  error: string | null
}

const bucketName = 'support-debug-bundles'
const defaultGitHubIssuesRepo = 'sticktrk/cross'
const legacyGitHubIssuesRepos = new Set([
  'sticktrk/rhythm-app',
  'sticktrk/rhythm-app-flutter',
  'sticktrk/rhythm-os',
])
const defaultSignedUrlExpiresIn = 60 * 60 * 24 * 7

Deno.serve(async (req) => {
  try {
    return await withAuthenticatedRequest(req, ({ userId, adminClient }) => {
      return handleRequest(req, { userId, adminClient })
    })
  } catch (error) {
    console.error('Report bug error:', error)
    return jsonResponse({ error: errorMessage(error) }, 500)
  }
})

async function handleRequest(
  req: Request,
  {
    userId,
    adminClient,
  }: {
    userId: string
    adminClient: ReturnType<typeof createClient>
  },
): Promise<Response> {
  if (req.method !== 'POST') {
    return jsonResponse({ error: 'Method not allowed' }, 405)
  }

  let payload: Record<string, unknown>
  try {
    payload = await readJson(req)
  } catch (error) {
    return jsonResponse({ error: errorMessage(error) }, 400)
  }
  const submissionId =
    readString(payload, 'submission_id') ?? readString(payload, 'submissionId')
  if (!submissionId) {
    return jsonResponse({ error: 'Missing submission_id' }, 400)
  }

  const { data, error: submissionError } = await adminClient
    .from('support_debug_bundle_submissions')
    .select('*')
    .eq('id', submissionId)
    .eq('user_id', userId)
    .single()

  if (submissionError || !data) {
    return jsonResponse({ error: 'Debug bundle submission not found' }, 404)
  }

  const submission = data as DebugBundleSubmission
  if (submission.github_issue_url) {
    return jsonResponse({
      issue_created: false,
      already_exists: true,
      status: submission.status,
      issue_url: submission.github_issue_url,
      issue_number: submission.github_issue_number,
    })
  }

  const githubRepo = readGitHubIssuesRepo()
  const githubToken = Deno.env.get('GITHUB_ISSUES_TOKEN')?.trim()
  if (!githubToken) {
    const message =
      'GitHub issue creation is not configured. Set GITHUB_ISSUES_TOKEN.'
    await markGitHubIssueError(adminClient, submission.id, message, 'received')
    return jsonResponse({
      issue_created: false,
      status: 'received',
      error: message,
    })
  }

  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(githubRepo)) {
    const message = 'GITHUB_ISSUES_REPO must be in owner/repo format.'
    await markGitHubIssueError(adminClient, submission.id, message)
    return jsonResponse({ issue_created: false, error: message }, 500)
  }

  const signedBundleLink = submission.bundle_storage_path
    ? await createSignedBundleLink(
        adminClient,
        submission.bundle_storage_path,
      )
    : null

  try {
    const issue = await createGitHubIssue({
      repo: githubRepo,
      token: githubToken,
      title: buildIssueTitle(submission),
      body: buildIssueBody(submission, signedBundleLink),
      labels: dedupe([
        'App Bug Report',
        ...readCsvEnv('GITHUB_ISSUES_LABELS'),
      ]),
      assignees: readCsvEnv('GITHUB_ISSUES_ASSIGNEES'),
    })

    const issueUrl = issue.html_url
    const issueNumber = issue.number
    const { error: updateError } = await adminClient
      .from('support_debug_bundle_submissions')
      .update({
        status: 'reported',
        github_issue_url: issueUrl,
        github_issue_number: issueNumber,
        github_issue_created_at: new Date().toISOString(),
        github_issue_error: null,
      })
      .eq('id', submission.id)

    if (updateError) {
      console.error('Failed to store GitHub issue metadata:', updateError)
    }

    return jsonResponse({
      issue_created: true,
      status: 'reported',
      issue_url: issueUrl,
      issue_number: issueNumber,
    })
  } catch (error) {
    const message = errorMessage(error)
    await markGitHubIssueError(adminClient, submission.id, message)
    return jsonResponse(
      { issue_created: false, status: 'error', error: message },
      500,
    )
  }
}

function readGitHubIssuesRepo(): string {
  const configured = Deno.env.get('GITHUB_ISSUES_REPO')?.trim()
  if (!configured) return defaultGitHubIssuesRepo

  if (legacyGitHubIssuesRepos.has(configured.toLowerCase())) {
    return defaultGitHubIssuesRepo
  }

  return configured
}

async function createSignedBundleLink(
  adminClient: ReturnType<typeof createClient>,
  storagePath: string,
): Promise<SignedBundleLink> {
  const expiresIn = readSignedUrlExpiresIn()
  const expiresAt = new Date(Date.now() + expiresIn * 1000).toISOString()
  const { data, error } = await adminClient.storage
    .from(bucketName)
    .createSignedUrl(storagePath, expiresIn)

  if (error) {
    console.error('Failed to create signed bundle URL:', error)
    return {
      signedUrl: null,
      expiresAt: null,
      error: error.message,
    }
  }

  return {
    signedUrl: data?.signedUrl ?? null,
    expiresAt,
    error: null,
  }
}

function readSignedUrlExpiresIn(): number {
  const rawValue = Deno.env.get('DEBUG_BUNDLE_SIGNED_URL_EXPIRES_SECONDS')
  if (!rawValue) return defaultSignedUrlExpiresIn

  const parsed = Number.parseInt(rawValue, 10)
  if (!Number.isFinite(parsed) || parsed <= 0) return defaultSignedUrlExpiresIn

  return Math.min(parsed, 60 * 60 * 24 * 30)
}

async function createGitHubIssue({
  repo,
  token,
  title,
  body,
  labels,
  assignees,
}: {
  repo: string
  token: string
  title: string
  body: string
  labels: string[]
  assignees: string[]
}): Promise<{ html_url: string; number: number }> {
  const response = await fetch(`https://api.github.com/repos/${repo}/issues`, {
    method: 'POST',
    headers: {
      Accept: 'application/vnd.github+json',
      Authorization: `Bearer ${token}`,
      'Content-Type': 'application/json',
      'User-Agent': 'rhythm-report-bug-edge-function',
      'X-GitHub-Api-Version': '2022-11-28',
    },
    body: JSON.stringify({
      title,
      body,
      ...(labels.length > 0 ? { labels } : {}),
      ...(assignees.length > 0 ? { assignees } : {}),
    }),
  })

  const responseBody = await response.json().catch(() => ({}))
  if (!response.ok) {
    throw new Error(formatGitHubError(response.status, responseBody))
  }

  if (
    typeof responseBody.html_url !== 'string' ||
    typeof responseBody.number !== 'number'
  ) {
    throw new Error('GitHub response did not include an issue URL and number.')
  }

  return {
    html_url: responseBody.html_url,
    number: responseBody.number,
  }
}

function buildIssueTitle(submission: DebugBundleSubmission): string {
  const prefix = 'app-report: '
  const suffix = ` (${submission.reference_code})`
  const maxDetailLength = 256 - prefix.length - suffix.length
  const titleDetail =
    summaryTitleDetail(submission.summary) ??
    fallbackTitleDetail(submission)

  return `${prefix}${truncate(titleDetail, maxDetailLength)}${suffix}`
}

function summaryTitleDetail(summary: string | null): string | null {
  const firstLine = summary
    ?.trim()
    .split(/\r?\n/)
    .map((line) => compactWhitespace(line))
    .find((line) => line.length > 0)

  if (!firstLine) return null
  const titleDetail = stripTrailingSentencePunctuation(firstLine)
  return titleDetail.length > 0 ? titleDetail : null
}

function fallbackTitleDetail(submission: DebugBundleSubmission): string {
  const serverName = submission.server_name?.trim()
  if (serverName) return `${serverName} report`

  const appPlatform = submission.app_platform?.trim()
  if (appPlatform) return `${appPlatform} app report`

  return 'App report'
}

function buildIssueBody(
  submission: DebugBundleSubmission,
  signedBundleLink: SignedBundleLink | null,
): string {
  const summary = submission.summary?.trim()
  const hasBundle = !!submission.bundle_storage_path
  const bundleSection = hasBundle
    ? [
        '## Debug Bundle',
        `- Storage path: \`${bucketName}/${submission.bundle_storage_path}\``,
        `- File name: ${valueOrUnknown(submission.bundle_file_name)}`,
        `- Content type: ${valueOrUnknown(submission.bundle_content_type)}`,
        `- Size: ${formatBytes(submission.bundle_size_bytes)}`,
        ...(signedBundleLink?.signedUrl
          ? [
              `- Signed download URL: ${signedBundleLink.signedUrl}`,
              `- Signed URL expires: ${signedBundleLink.expiresAt}`,
            ]
          : [
              '- Signed download URL: unavailable',
              `- Signed URL error: ${signedBundleLink?.error ?? 'Unknown error'}`,
            ]),
        '',
        'The bundle is private in Supabase Storage. The signed URL is intended for support triage and expires automatically.',
      ]
    : [
        '## Debug Bundle',
        '_No debug bundle attached (text-only report)._',
      ]

  return [
    'Created automatically from the Rhythm app Report Bug flow.',
    '',
    '## User Summary',
    summary ? quoteBlock(summary) : '_No summary provided._',
    '',
    '## Submission',
    `- Reference: ${submission.reference_code}`,
    `- Submission ID: ${submission.id}`,
    `- Submitted at: ${submission.created_at}`,
    `- Auth mode: ${submission.is_anonymous ? 'anonymous' : 'signed in'}`,
    '',
    '## App',
    `- Version: ${valueOrUnknown(submission.app_version)}`,
    `- Build: ${valueOrUnknown(submission.app_build)}`,
    `- Platform: ${valueOrUnknown(submission.app_platform)}`,
    '',
    '## RhythmServer',
    `- Hub: ${valueOrUnknown(submission.server_name)}`,
    `- Hub ID: ${valueOrUnknown(submission.server_hub_id)}`,
    `- Endpoint: ${formatEndpoint(submission)}`,
    `- Version: ${valueOrUnknown(submission.server_version)}`,
    `- Platform context: ${valueOrUnknown(submission.server_platform_context)}`,
    '',
    ...bundleSection,
  ].join('\n')
}

async function markGitHubIssueError(
  adminClient: ReturnType<typeof createClient>,
  submissionId: string,
  message: string,
  status = 'error',
): Promise<void> {
  const { error } = await adminClient
    .from('support_debug_bundle_submissions')
    .update({
      status,
      github_issue_error: truncate(message, 1000),
    })
    .eq('id', submissionId)

  if (error) {
    console.error('Failed to store GitHub issue error:', error)
  }
}

function readCsvEnv(name: string): string[] {
  return (Deno.env.get(name) ?? '')
    .split(',')
    .map((value) => value.trim())
    .filter((value) => value.length > 0)
}

function dedupe(values: string[]): string[] {
  return Array.from(new Set(values))
}

function formatEndpoint(submission: DebugBundleSubmission): string {
  const host = submission.server_host?.trim()
  if (!host) return 'Unknown'
  return submission.server_port ? `${host}:${submission.server_port}` : host
}

function formatBytes(size: number | null): string {
  if (size === null || !Number.isFinite(size) || size <= 0) return 'Unknown'
  if (size < 1024) return `${size} B`
  const kib = size / 1024
  if (kib < 1024) return `${kib.toFixed(1)} KiB`
  return `${(kib / 1024).toFixed(1)} MiB`
}

function formatGitHubError(status: number, responseBody: unknown): string {
  if (
    responseBody &&
    typeof responseBody === 'object' &&
    'message' in responseBody &&
    typeof responseBody.message === 'string'
  ) {
    return `GitHub issue creation failed (${status}): ${responseBody.message}`
  }
  return `GitHub issue creation failed with HTTP ${status}.`
}

function quoteBlock(value: string): string {
  return value
    .split('\n')
    .map((line) => `> ${line}`)
    .join('\n')
}

function compactWhitespace(value: string): string {
  return value.replace(/\s+/g, ' ').trim()
}

function stripTrailingSentencePunctuation(value: string): string {
  return value.replace(/[.!?]+$/, '').trim()
}

function valueOrUnknown(value: string | null): string {
  const trimmed = value?.trim()
  return trimmed && trimmed.length > 0 ? trimmed : 'Unknown'
}

function truncate(value: string, maxLength: number): string {
  if (value.length <= maxLength) return value
  if (maxLength <= 3) return value.substring(0, maxLength)
  return `${value.substring(0, maxLength - 3)}...`
}
