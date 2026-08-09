import {
  githubLabelsForSupportReport,
} from './support_report_contract.ts'
import {
  buildIssueBody,
  buildIssueTitle,
  isFleetSubmission,
} from './support_issue_body.ts'
import type { DebugBundleSubmission } from './support_issue_body.ts'

const defaultGitHubIssuesRepo = 'sticktrk/cross'
const legacyGitHubIssuesRepos = new Set([
  'sticktrk/rhythm-app',
  'sticktrk/rhythm-app-flutter',
  'sticktrk/rhythm-os',
])

export type GitHubIssueReport = {
  ok: boolean
  created: boolean
  alreadyExists: boolean
  status: string
  issueUrl?: string
  issueNumber?: number
  error?: string
}

export async function createOrRefreshGitHubIssue(
  adminClient: any,
  submission: DebugBundleSubmission,
): Promise<GitHubIssueReport> {
  const githubRepo = readGitHubIssuesRepo()
  const githubToken = Deno.env.get('GITHUB_ISSUES_TOKEN')?.trim()
  if (!githubToken) {
    return await failedReport(
      adminClient,
      submission.id,
      'GitHub issue creation is not configured. Set GITHUB_ISSUES_TOKEN.',
    )
  }
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(githubRepo)) {
    return await failedReport(
      adminClient,
      submission.id,
      'GITHUB_ISSUES_REPO must be in owner/repo format.',
    )
  }

  try {
    const storedNumber = submission.github_issue_number
    const recoveredIssue = storedNumber == null && !isFleetSubmission(submission)
      ? await findExistingIssue(githubRepo, githubToken, submission.id)
      : null
    const existingNumber = storedNumber ?? recoveredIssue?.number ?? null
    const created = existingNumber == null
    const issue = created
      ? await createGitHubIssue({
          repo: githubRepo,
          token: githubToken,
          title: buildIssueTitle(submission),
          body: buildIssueBody(submission),
          labels: githubLabelsForSupportReport({
            kind: submission.report_kind,
            fleet: isFleetSubmission(submission),
            configuredLabels: readCsvEnv('GITHUB_ISSUES_LABELS'),
          }),
          assignees: readCsvEnv('GITHUB_ISSUES_ASSIGNEES'),
        })
      : await refreshGitHubIssue({
          repo: githubRepo,
          token: githubToken,
          issueNumber: existingNumber,
          title: buildIssueTitle(submission),
          body: buildIssueBody(submission),
        })

    const metadataUpdate: Record<string, unknown> = {
      status: 'reported',
      github_issue_url: issue.html_url,
      github_issue_number: issue.number,
      github_issue_error: null,
    }
    if (existingNumber == null) {
      metadataUpdate.github_issue_created_at = new Date().toISOString()
    }

    const { error: updateError } = await adminClient
      .from('support_debug_bundle_submissions')
      .update(metadataUpdate)
      .eq('id', submission.id)

    if (updateError) {
      console.error('Failed to store GitHub issue metadata:', updateError)
      return {
        ok: false,
        created,
        alreadyExists: !created,
        status: submission.status,
        issueUrl: issue.html_url,
        issueNumber: issue.number,
        error: updateError.message,
      }
    }

    return {
      ok: true,
      created,
      alreadyExists: !created,
      status: 'reported',
      issueUrl: issue.html_url,
      issueNumber: issue.number,
    }
  } catch (error) {
    const message = errorMessage(error)
    console.error('GitHub support issue projection failed:', error)
    return await failedReport(adminClient, submission.id, message)
  }
}

async function findExistingIssue(
  repo: string,
  token: string,
  submissionId: string,
): Promise<{ html_url: string; number: number } | null> {
  const url = new URL(`https://api.github.com/repos/${repo}/issues`)
  url.searchParams.set('state', 'all')
  url.searchParams.set('sort', 'created')
  url.searchParams.set('direction', 'desc')
  url.searchParams.set('per_page', '100')
  const response = await fetch(url, {
    headers: githubHeaders(token),
  })
  const responseBody = await response.json().catch(() => [])
  if (!response.ok) {
    throw new Error(formatGitHubError(repo, response.status, responseBody))
  }
  if (!Array.isArray(responseBody)) {
    throw new Error('GitHub issue list response was invalid.')
  }

  const marker = `- Submission ID: ${submissionId}`
  const match = responseBody.find((candidate) =>
    candidate &&
    typeof candidate === 'object' &&
    typeof candidate.body === 'string' &&
    candidate.body.split('\n').includes(marker) &&
    typeof candidate.html_url === 'string' &&
    typeof candidate.number === 'number'
  )
  return match
    ? { html_url: match.html_url, number: match.number }
    : null
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
  return await callGitHubIssueApi({
    url: `https://api.github.com/repos/${repo}/issues`,
    method: 'POST',
    token,
    repo,
    body: {
      title,
      body,
      ...(labels.length > 0 ? { labels } : {}),
      ...(assignees.length > 0 ? { assignees } : {}),
    },
  })
}

async function refreshGitHubIssue({
  repo,
  token,
  issueNumber,
  title,
  body,
}: {
  repo: string
  token: string
  issueNumber: number
  title: string
  body: string
}): Promise<{ html_url: string; number: number }> {
  return await callGitHubIssueApi({
    url: `https://api.github.com/repos/${repo}/issues/${issueNumber}`,
    method: 'PATCH',
    token,
    repo,
    body: { title, body },
  })
}

async function callGitHubIssueApi({
  url,
  method,
  token,
  repo,
  body,
}: {
  url: string
  method: 'POST' | 'PATCH'
  token: string
  repo: string
  body: Record<string, unknown>
}): Promise<{ html_url: string; number: number }> {
  const response = await fetch(url, {
    method,
    headers: {
      ...githubHeaders(token),
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(body),
  })

  const responseBody = await response.json().catch(() => ({}))
  if (!response.ok) {
    throw new Error(formatGitHubError(repo, response.status, responseBody))
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

function githubHeaders(token: string): Record<string, string> {
  return {
    Accept: 'application/vnd.github+json',
    Authorization: `Bearer ${token}`,
    'User-Agent': 'rhythm-report-bug-edge-function',
    'X-GitHub-Api-Version': '2022-11-28',
  }
}

async function failedReport(
  adminClient: any,
  submissionId: string,
  message: string,
): Promise<GitHubIssueReport> {
  const { error } = await adminClient
    .from('support_debug_bundle_submissions')
    .update({
      github_issue_error: truncate(message, 1000),
    })
    .eq('id', submissionId)
  if (error) console.error('Failed to store GitHub issue error:', error)
  return {
    ok: false,
    created: false,
    alreadyExists: false,
    status: 'received',
    error: message,
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

function readCsvEnv(name: string): string[] {
  return (Deno.env.get(name) ?? '')
    .split(',')
    .map((value) => value.trim())
    .filter((value) => value.length > 0)
}

function formatGitHubError(
  repo: string,
  status: number,
  responseBody: unknown,
): string {
  if (
    responseBody &&
    typeof responseBody === 'object' &&
    'message' in responseBody &&
    typeof responseBody.message === 'string'
  ) {
    return `GitHub issue creation failed for ${repo} (${status}): ${responseBody.message}`
  }
  return `GitHub issue creation failed for ${repo} with HTTP ${status}.`
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function truncate(value: string, maxLength: number): string {
  if (value.length <= maxLength) return value
  return value.substring(0, maxLength)
}
