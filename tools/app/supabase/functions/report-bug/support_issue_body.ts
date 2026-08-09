import { supportReportIssueCopy } from './support_report_contract.ts'

export type DebugBundleSubmission = {
  id: string
  user_id: string
  user_email: string | null
  is_anonymous: boolean
  reference_code: string
  status: string
  report_kind: string | null
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
  bundle_status?: string | null
  bundle_collection_started_at?: string | null
  bundle_collection_completed_at?: string | null
  bundle_collection_duration_ms?: number | null
  bundle_schema_version?: number | null
  bundle_captured_log_count?: number | null
  bundle_captured_persisted_count?: number | null
  bundle_generated_file_count?: number | null
  bundle_missing_persisted_count?: number | null
  bundle_file_error_count?: number | null
  bundle_app_log_included?: boolean | null
  bundle_failure_stage?: string | null
  created_at: string
  github_issue_url: string | null
  github_issue_number: number | null
}

export function buildIssueTitle(submission: DebugBundleSubmission): string {
  const fleet = isFleetSubmission(submission)
  const prefix = supportReportIssueCopy(
    submission.report_kind,
    fleet,
  ).titlePrefix
  const suffix = fleet ? '' : ` (${submission.reference_code})`
  const maxDetailLength = 256 - prefix.length - suffix.length
  const titleDetail =
    summaryTitleDetail(submission.summary) ?? fallbackTitleDetail(submission)

  return `${prefix}${truncate(titleDetail, maxDetailLength)}${suffix}`
}

export function buildIssueBody(submission: DebugBundleSubmission): string {
  const fleet = isFleetSubmission(submission)
  const reportCopy = supportReportIssueCopy(submission.report_kind, fleet)
  const summary = submission.summary?.trim()

  return [
    reportCopy.intro,
    '',
    `## ${reportCopy.summaryHeading}`,
    summary ? quoteBlock(summary) : '_No summary provided._',
    '',
    '## Submission',
    ...(fleet
      ? []
      : [
          `- Reference: ${submission.reference_code}`,
          `- Submission ID: ${submission.id}`,
        ]),
    `- Kind: ${reportCopy.kind}`,
    `- Submitted at: ${submission.created_at}`,
    `- Auth mode: ${submission.is_anonymous ? 'anonymous' : 'signed in'}`,
    '',
    fleet ? '## Automation' : '## App',
    `- Version: ${valueOrUnknown(submission.app_version)}`,
    `- Build: ${valueOrUnknown(submission.app_build)}`,
    `- Platform: ${valueOrUnknown(submission.app_platform)}`,
    '',
    '## RhythmServer',
    `- Hub: ${fleet ? 'Redacted representative hub' : valueOrUnknown(submission.server_name)}`,
    `- Hub ID: ${fleet ? 'Redacted' : valueOrUnknown(submission.server_hub_id)}`,
    `- Endpoint: ${fleet ? 'Redacted' : formatEndpoint(submission)}`,
    `- Version: ${valueOrUnknown(submission.server_version)}`,
    `- Platform context: ${valueOrUnknown(submission.server_platform_context)}`,
    '',
    ...bundleSection(submission),
  ].join('\n')
}

export function isFleetSubmission(
  submission: DebugBundleSubmission,
): boolean {
  return submission.app_platform?.trim().toLowerCase() === 'admin-fleet'
}

function bundleSection(submission: DebugBundleSubmission): string[] {
  const status = effectiveBundleStatus(submission)
  if (status === 'none') {
    return ['## Debug Bundle', '_No debug bundle attached (text-only report)._']
  }

  const lines = [
    '## Debug Bundle',
    `- Status: ${bundleStatusLabel(status)}`,
  ]
  if (submission.bundle_file_name) {
    lines.push(`- Archive: ${submission.bundle_file_name}`)
  }
  if (submission.bundle_content_type) {
    lines.push(`- Content type: ${submission.bundle_content_type}`)
  }
  if (submission.bundle_size_bytes != null) {
    lines.push(`- Size: ${formatBytes(submission.bundle_size_bytes)}`)
  }
  if (submission.bundle_schema_version != null) {
    lines.push(`- Bundle schema: v${submission.bundle_schema_version}`)
  }
  if (submission.bundle_app_log_included != null) {
    lines.push(
      `- App log: ${submission.bundle_app_log_included ? 'included' : 'not included'}`,
    )
  }

  const captured = compactCounts([
    ['log files', submission.bundle_captured_log_count],
    ['persisted artifacts', submission.bundle_captured_persisted_count],
    ['generated diagnostics', submission.bundle_generated_file_count],
  ])
  if (captured) lines.push(`- Captured: ${captured}`)

  const incomplete = compactCounts([
    ['missing persisted artifacts', submission.bundle_missing_persisted_count],
    ['file errors', submission.bundle_file_error_count],
  ])
  if (incomplete) lines.push(`- Incomplete sources: ${incomplete}`)

  if (submission.bundle_collection_duration_ms != null) {
    lines.push(
      `- Collection time: ${formatDuration(submission.bundle_collection_duration_ms)}`,
    )
  }
  if (status === 'failed' && submission.bundle_failure_stage) {
    lines.push(`- Failure stage: ${submission.bundle_failure_stage}`)
  }

  lines.push(
    '',
    status === 'collecting'
      ? '_Collection is owned by RhythmServer and continues after the app closes._'
      : '_Private support bundle. Staff triage tooling resolves it by GitHub issue number; storage coordinates and credentials are intentionally omitted._',
  )
  return lines
}

function effectiveBundleStatus(submission: DebugBundleSubmission): string {
  const status = submission.bundle_status?.trim().toLowerCase()
  if (['none', 'collecting', 'uploaded', 'failed'].includes(status ?? '')) {
    return status!
  }
  return submission.bundle_storage_path ? 'uploaded' : 'none'
}

function bundleStatusLabel(status: string): string {
  switch (status) {
    case 'collecting':
      return 'Collecting on RhythmServer'
    case 'uploaded':
      return 'Available'
    case 'failed':
      return 'Collection failed'
    default:
      return 'Not requested'
  }
}

function compactCounts(
  values: Array<[string, number | null | undefined]>,
): string | null {
  const parts = values
    .filter((entry): entry is [string, number] => entry[1] != null)
    .map(([label, count]) => `${count} ${label}`)
  return parts.length > 0 ? parts.join(', ') : null
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return 'Unknown'
  if (bytes < 1024) return `${bytes} bytes`
  const units = ['KiB', 'MiB', 'GiB']
  let value = bytes / 1024
  let unit = units[0]
  for (let index = 1; index < units.length && value >= 1024; index += 1) {
    value /= 1024
    unit = units[index]
  }
  return `${value.toFixed(value >= 10 ? 1 : 2)} ${unit} (${bytes} bytes)`
}

function formatDuration(milliseconds: number): string {
  if (!Number.isFinite(milliseconds) || milliseconds < 0) return 'Unknown'
  if (milliseconds < 1000) return `${milliseconds} ms`
  return `${(milliseconds / 1000).toFixed(milliseconds >= 10_000 ? 1 : 2)} s`
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
  if (isFleetSubmission(submission)) return 'detected fleet finding'

  const serverName = submission.server_name?.trim()
  if (serverName) return `${serverName} report`

  const appPlatform = submission.app_platform?.trim()
  if (appPlatform) return `${appPlatform} app report`

  return 'App report'
}

function formatEndpoint(submission: DebugBundleSubmission): string {
  const host = submission.server_host?.trim()
  if (!host) return 'Unknown'
  return submission.server_port ? `${host}:${submission.server_port}` : host
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
