import {
  errorMessage,
  jsonResponse,
  readJson,
  readString,
  withAuthenticatedRequest,
} from '../_shared/auth.ts'
import { createOrRefreshGitHubIssue } from './github_support_issue.ts'
import type { DebugBundleSubmission } from './support_issue_body.ts'

// Required secrets:
// - GITHUB_ISSUES_TOKEN: fine-grained token with metadata read and issue write access
// - GITHUB_ISSUES_REPO: private owner/repo; explicitly configured, no default
// Optional secrets:
// - SB_PUBLISHABLE_KEY: preferred Supabase public key for Auth claims
// - SB_SECRET_KEY: preferred Supabase elevated key for admin operations
// - GITHUB_ISSUES_LABELS: comma-separated labels
// - GITHUB_ISSUES_ASSIGNEES: comma-separated GitHub usernames

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
    adminClient: any
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

  if (readString(payload, 'action') === 'mark_queue_failed') {
    const { error: updateError } = await adminClient
      .from('support_debug_bundle_submissions')
      .update({
        bundle_status: 'failed',
        bundle_collection_completed_at: new Date().toISOString(),
        bundle_failure_stage: 'queue',
      })
      .eq('id', submissionId)
      .eq('user_id', userId)
      .neq('bundle_status', 'uploaded')
    if (updateError) {
      return jsonResponse({ error: updateError.message }, 500)
    }
    const { data: refreshed, error: refreshError } = await adminClient
      .from('support_debug_bundle_submissions')
      .select('*')
      .eq('id', submissionId)
      .eq('user_id', userId)
      .single()
    if (refreshError || !refreshed) {
      return jsonResponse({ error: 'Updated submission not found' }, 500)
    }
    const failedReport = await createOrRefreshGitHubIssue(
      adminClient,
      refreshed as DebugBundleSubmission,
    )
    return jsonResponse({
      marked_failed: true,
      issue_created: failedReport.created,
      issue_url: failedReport.issueUrl,
      issue_number: failedReport.issueNumber,
    })
  }

  const report = await createOrRefreshGitHubIssue(
    adminClient,
    data as DebugBundleSubmission,
  )
  return jsonResponse({
    issue_created: report.created,
    already_exists: report.alreadyExists,
    status: report.status,
    issue_url: report.issueUrl,
    issue_number: report.issueNumber,
    error: report.error,
  })
}
