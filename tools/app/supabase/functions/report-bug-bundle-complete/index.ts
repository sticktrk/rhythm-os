import { createClient } from 'https://esm.sh/@supabase/supabase-js@2'

import { createOrRefreshGitHubIssue } from '../report-bug/github_support_issue.ts'
import type { DebugBundleSubmission } from '../report-bug/support_issue_body.ts'

type JsonObject = Record<string, unknown>

const bucketName = 'support-debug-bundles'
const maxBundleBytes = 25 * 1024 * 1024
const allowedFailureStages = new Set([
  'build',
  'upload',
  'upload_expired',
  'job_expired',
])

Deno.serve(async (req) => {
  if (req.method !== 'POST') {
    return jsonResponse({ error: 'Method not allowed' }, 405)
  }

  try {
    const completionToken = readBearerToken(req.headers.get('Authorization'))
    if (!completionToken || completionToken.length > 200) {
      return jsonResponse({ error: 'Missing completion token' }, 401)
    }
    const payload = await readJson(req)
    const submissionId = readString(payload, 'submission_id')
    if (!submissionId || !isUuid(submissionId)) {
      return jsonResponse({ error: 'Invalid submission_id' }, 400)
    }

    const adminClient = createClient(
      requireEnv('SUPABASE_URL'),
      readSupabaseSecretKey(),
    )
    const { data, error } = await adminClient
      .from('support_debug_bundle_submissions')
      .select('*')
      .eq('id', submissionId)
      .single()
    if (error || !data) {
      return jsonResponse({ error: 'Submission not found' }, 404)
    }

    const submission = data as DebugBundleSubmission & {
      bundle_completion_token_hash?: string | null
    }
    const tokenHash = await sha256Hex(completionToken)
    if (
      !submission.bundle_completion_token_hash ||
      !constantTimeEqual(submission.bundle_completion_token_hash, tokenHash)
    ) {
      return jsonResponse({ error: 'Invalid completion token' }, 401)
    }

    const requestedStatus = readString(payload, 'status')
    if (requestedStatus !== 'uploaded' && requestedStatus !== 'failed') {
      return jsonResponse({ error: 'Invalid completion status' }, 400)
    }

    const uploadedObject = await findUploadedObject(
      adminClient,
      submission.bundle_storage_path,
    )
    if (requestedStatus === 'uploaded' && !uploadedObject) {
      // Storage object presence, not the appliance callback, owns bundle
      // availability. A short retry also covers object-listing propagation.
      return jsonResponse({ status: 'retry', bundle_status: 'collecting' }, 409)
    }
    const effectiveStatus = uploadedObject ? 'uploaded' : requestedStatus
    const summary = readObject(payload, 'summary')
    const durationMs = readBoundedInt(payload, 'duration_ms', 0, 86_400_000)
    const now = new Date().toISOString()

    const update = effectiveStatus === 'uploaded'
      ? uploadedUpdate({
          payload,
          summary,
          recoveredObject: uploadedObject,
          durationMs,
          now,
        })
      : failedUpdate({ payload, durationMs, now })

    // Never downgrade an already-authoritative uploaded object to failed on a
    // duplicate or delayed callback.
    if (submission.bundle_status !== 'uploaded' || effectiveStatus === 'uploaded') {
      const { error: updateError } = await adminClient
        .from('support_debug_bundle_submissions')
        .update(update)
        .eq('id', submissionId)
      if (updateError) throw new Error(updateError.message)
    }

    const { data: refreshed, error: refreshError } = await adminClient
      .from('support_debug_bundle_submissions')
      .select('*')
      .eq('id', submissionId)
      .single()
    if (refreshError || !refreshed) {
      throw new Error(refreshError?.message ?? 'Updated submission missing')
    }

    const issue = await createOrRefreshGitHubIssue(
      adminClient,
      refreshed as DebugBundleSubmission,
    )
    if (!issue.ok) {
      // Keep the server's durable job so this projection is retried even if the
      // app has already closed.
      return jsonResponse(
        { status: 'retry', bundle_status: effectiveStatus },
        502,
      )
    }

    return jsonResponse({
      status: 'ok',
      bundle_status: effectiveStatus,
      issue_url: issue.issueUrl,
      issue_number: issue.issueNumber,
    })
  } catch (error) {
    console.error('Support bundle completion error:', error)
    return jsonResponse({ error: errorMessage(error) }, 500)
  }
})

function uploadedUpdate({
  payload,
  summary,
  recoveredObject,
  durationMs,
  now,
}: {
  payload: JsonObject
  summary: JsonObject | null
  recoveredObject: { name: string; size: number | null } | null
  durationMs: number | null
  now: string
}): JsonObject {
  const fileName = recoveredObject?.name ?? readSafeFileName(payload, 'file_name')
  const sizeBytes = recoveredObject?.size ??
    readBoundedInt(payload, 'size_bytes', 1, maxBundleBytes)
  if (!fileName || sizeBytes == null) {
    throw new Error('Uploaded completion requires file_name and size_bytes')
  }

  return {
    bundle_status: 'uploaded',
    bundle_file_name: fileName,
    bundle_content_type: 'application/gzip',
    bundle_size_bytes: sizeBytes,
    bundle_collection_completed_at: now,
    bundle_collection_duration_ms: durationMs,
    bundle_schema_version: readBoundedInt(summary, 'schema_version', 1, 1000),
    bundle_captured_log_count: readBoundedInt(
      summary,
      'captured_log_count',
      0,
      100_000,
    ),
    bundle_captured_persisted_count: readBoundedInt(
      summary,
      'captured_persisted_count',
      0,
      100_000,
    ),
    bundle_generated_file_count: readBoundedInt(
      summary,
      'generated_file_count',
      0,
      100_000,
    ),
    bundle_missing_persisted_count: readBoundedInt(
      summary,
      'missing_persisted_count',
      0,
      100_000,
    ),
    bundle_file_error_count: readBoundedInt(
      summary,
      'file_error_count',
      0,
      100_000,
    ),
    bundle_app_log_included: readBoolean(summary, 'app_log_included'),
    bundle_failure_stage: null,
  }
}

function failedUpdate({
  payload,
  durationMs,
  now,
}: {
  payload: JsonObject
  durationMs: number | null
  now: string
}): JsonObject {
  const requestedStage = readString(payload, 'failure_stage')
  return {
    bundle_status: 'failed',
    bundle_collection_completed_at: now,
    bundle_collection_duration_ms: durationMs,
    bundle_failure_stage: requestedStage && allowedFailureStages.has(requestedStage)
      ? requestedStage
      : 'build',
  }
}

async function findUploadedObject(
  adminClient: any,
  storagePath: string | null,
): Promise<{ name: string; size: number | null } | null> {
  if (!storagePath) return null
  const segments = storagePath.split('/').filter((segment) => segment.length > 0)
  const name = segments.pop()
  if (!name) return null
  const directory = segments.join('/')
  const { data, error } = await adminClient.storage
    .from(bucketName)
    .list(directory, { limit: 10, search: name })
  if (error || !Array.isArray(data)) return null
  const object = data.find((entry: { name?: unknown }) => entry.name === name)
  if (!object) return null
  const rawSize = object.metadata?.size
  const size = typeof rawSize === 'number' && rawSize > 0 ? rawSize : null
  return { name, size }
}

async function readJson(req: Request): Promise<JsonObject> {
  const value = await req.json()
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('Invalid JSON body')
  }
  return value as JsonObject
}

function readObject(data: JsonObject, key: string): JsonObject | null {
  const value = data[key]
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as JsonObject
    : null
}

function readString(data: JsonObject | null, key: string): string | null {
  const value = data?.[key]
  if (typeof value !== 'string') return null
  const trimmed = value.trim()
  return trimmed.length > 0 ? trimmed : null
}

function readSafeFileName(data: JsonObject, key: string): string | null {
  const value = readString(data, key)
  if (!value || value.length > 200 || !/^[A-Za-z0-9._-]+$/.test(value)) {
    return null
  }
  return value
}

function readBoundedInt(
  data: JsonObject | null,
  key: string,
  min: number,
  max: number,
): number | null {
  const value = data?.[key]
  if (typeof value !== 'number' || !Number.isSafeInteger(value)) return null
  return value >= min && value <= max ? value : null
}

function readBoolean(data: JsonObject | null, key: string): boolean | null {
  const value = data?.[key]
  return typeof value === 'boolean' ? value : null
}

function readBearerToken(header: string | null): string | null {
  if (!header) return null
  const [scheme, token] = header.split(' ')
  return scheme === 'Bearer' && token?.trim() ? token.trim() : null
}

function isUuid(value: string): boolean {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i
    .test(value)
}

async function sha256Hex(value: string): Promise<string> {
  const digest = await crypto.subtle.digest(
    'SHA-256',
    new TextEncoder().encode(value),
  )
  return Array.from(new Uint8Array(digest))
    .map((byte) => byte.toString(16).padStart(2, '0'))
    .join('')
}

function constantTimeEqual(left: string, right: string): boolean {
  if (left.length !== right.length) return false
  let difference = 0
  for (let index = 0; index < left.length; index += 1) {
    difference |= left.charCodeAt(index) ^ right.charCodeAt(index)
  }
  return difference === 0
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

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
