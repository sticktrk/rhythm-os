import assert from 'node:assert/strict'
import test from 'node:test'

import {
  githubLabelsForSupportReport,
  normalizeSupportReportKind,
  supportReportIssueCopy,
} from './support_report_contract.ts'
import {
  buildIssueBody,
} from './support_issue_body.ts'
import type { DebugBundleSubmission } from './support_issue_body.ts'

test('legacy and invalid report kinds remain bug-compatible', () => {
  assert.equal(normalizeSupportReportKind(undefined), 'bug')
  assert.equal(normalizeSupportReportKind('unexpected'), 'bug')
})

test('bug reports receive only bug classification labels', () => {
  const labels = githubLabelsForSupportReport({
    kind: 'bug',
    fleet: false,
    configuredLabels: ['codex', 'enhancement'],
  })

  assert.deepEqual(labels, ['App Bug Report', 'bug', 'light-hub', 'codex'])
  assert.equal(labels.includes('feature'), false)
  assert.equal(labels.includes('enhancement'), false)
})

test('feature requests receive only feature classification labels', () => {
  const labels = githubLabelsForSupportReport({
    kind: 'feature',
    fleet: false,
    configuredLabels: ['codex', 'App Bug Report'],
  })

  assert.deepEqual(labels, ['enhancement', 'feature', 'light-hub', 'codex'])
  assert.equal(labels.includes('bug'), false)
  assert.equal(labels.includes('App Bug Report'), false)
  assert.deepEqual(supportReportIssueCopy('feature', false), {
    kind: 'feature',
    titlePrefix: 'app-feature: ',
    intro: 'Created automatically from the Rhythm app Report / Request flow.',
    summaryHeading: 'Requested Feature',
  })
})

test('fleet submissions remain bug-classified', () => {
  assert.deepEqual(
    githubLabelsForSupportReport({
      kind: 'feature',
      fleet: true,
      configuredLabels: [],
    }),
    ['App Bug Report', 'bug', 'light-hub', 'fleet-detected'],
  )
})

test('GitHub issue reports privacy-safe asynchronous bundle detail', () => {
  const submission = fixture({
    bundle_status: 'uploaded',
    bundle_file_name: 'rhythm-debug-bundle-rpiz-20260809T120000Z.tar.gz',
    bundle_content_type: 'application/gzip',
    bundle_size_bytes: 4_321_987,
    bundle_schema_version: 2,
    bundle_captured_log_count: 8,
    bundle_captured_persisted_count: 6,
    bundle_generated_file_count: 12,
    bundle_missing_persisted_count: 1,
    bundle_file_error_count: 0,
    bundle_app_log_included: true,
    bundle_collection_duration_ms: 18_420,
  })

  const body = buildIssueBody(submission)

  assert.match(body, /- Status: Available/)
  assert.match(body, /4\.12 MiB \(4321987 bytes\)/)
  assert.match(body, /- Bundle schema: v2/)
  assert.match(body, /8 log files, 6 persisted artifacts, 12 generated diagnostics/)
  assert.match(body, /1 missing persisted artifacts, 0 file errors/)
  assert.match(body, /- App log: included/)
  assert.match(body, /- Collection time: 18\.4 s/)
  assert.doesNotMatch(body, /bundle_storage_path/)
  assert.doesNotMatch(body, /signed-upload/)
  assert.doesNotMatch(body, /completion-token/)
})

test('collecting bundle copy tells support the app may close', () => {
  const body = buildIssueBody(fixture({ bundle_status: 'collecting' }))

  assert.match(body, /- Status: Collecting on RhythmServer/)
  assert.match(body, /continues after the app closes/)
})

function fixture(
  overrides: Partial<DebugBundleSubmission> = {},
): DebugBundleSubmission {
  return {
    id: '8f68c4ae-edf5-4e7d-9c9a-99cc2e86b1bc',
    user_id: 'user-id',
    user_email: null,
    is_anonymous: false,
    reference_code: 'DBG-ABC12345',
    status: 'received',
    report_kind: 'bug',
    summary: 'Lights stopped responding',
    app_version: '4.0.300',
    app_build: '300',
    app_platform: 'ios',
    server_hub_id: 'hub-id',
    server_name: 'Light Hub',
    server_host: '192.0.2.1',
    server_port: 54448,
    server_version: 'v0.6.500',
    server_platform_context: 'rpiz',
    bundle_storage_path: 'private/path/never-rendered.tar.gz',
    bundle_file_name: 'expected.tar.gz',
    bundle_content_type: 'application/gzip',
    bundle_size_bytes: null,
    bundle_status: 'collecting',
    created_at: '2026-08-09T12:00:00Z',
    github_issue_url: null,
    github_issue_number: null,
    ...overrides,
  }
}
