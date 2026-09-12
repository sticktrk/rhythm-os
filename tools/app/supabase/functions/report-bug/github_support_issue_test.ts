import { deepStrictEqual, match } from 'node:assert/strict'
import { createOrRefreshGitHubIssue } from './github_support_issue.ts'
import type { DebugBundleSubmission } from './support_issue_body.ts'

Deno.test('missing or invalid support configuration stores an error without making GitHub requests', async () => {
  for (const config of [
    { GITHUB_ISSUES_TOKEN: 'fixture-token' },
    { GITHUB_ISSUES_TOKEN: 'fixture-token', GITHUB_ISSUES_REPO: 'https://example.test' },
    { GITHUB_ISSUES_REPO: 'community/support' },
  ]) {
    const updates: unknown[] = []
    const admin = { from: (table: string) => {
      deepStrictEqual(table, 'support_debug_bundle_submissions')
      return { update: (values: unknown) => {
        updates.push(values)
        return { eq: async (column: string, id: string) => {
          deepStrictEqual([column, id], ['id', 'fixture-submission'])
          return { error: null }
        } }
      } }
    } }
    const result = await createOrRefreshGitHubIssue(
      admin,
      { id: 'fixture-submission' } as DebugBundleSubmission,
      (name) => (config as Record<string, string>)[name],
    )
    deepStrictEqual(result.ok, false)
    deepStrictEqual(result.created, false)
    deepStrictEqual(result.status, 'received')
    deepStrictEqual(updates.length, 1)
    match(result.error!, /GITHUB_ISSUES_(REPO|TOKEN)/)
  }
})

Deno.test('public or unverified repositories receive no private support payload', async () => {
  const originalFetch = globalThis.fetch
  try {
    for (const [metadata, status] of [
      [{ private: false }, 200], [{}, 200], [null, 200],
      [{ message: 'Forbidden' }, 403], [{ message: 'Not Found' }, 404],
    ] as const) {
      const requests: { method: string; body: unknown }[] = []
      globalThis.fetch = ((_input: unknown, init?: RequestInit) => {
        requests.push({ method: init?.method ?? 'GET', body: init?.body })
        return Promise.resolve(Response.json(metadata, { status }))
      }) as typeof fetch
      const updates: Record<string, unknown>[] = []
      const admin = { from: () => ({ update: (values: Record<string, unknown>) => {
        updates.push(values)
        return { eq: async () => ({ error: null }) }
      } }) }
      const result = await createOrRefreshGitHubIssue(admin, {
        id: 'fixture-submission', summary: 'Private customer report',
        github_issue_number: 15,
      } as DebugBundleSubmission, (name) => ({
        GITHUB_ISSUES_TOKEN: 'fixture-token', GITHUB_ISSUES_REPO: 'community/support',
      })[name])
      deepStrictEqual(result.ok, false)
      match(result.error!, status === 200 ? /private repository/i : /GitHub/)
      deepStrictEqual(requests, [{ method: 'GET', body: undefined }])
      deepStrictEqual(updates.length, 1)
    }
  } finally { globalThis.fetch = originalFetch }
})

Deno.test('private report retries create once and recover a previously created issue', async () => {
  const originalFetch = globalThis.fetch
  const updates: Record<string, unknown>[] = []
  const admin = { from: () => ({ update: (values: Record<string, unknown>) => {
    updates.push(values)
    return { eq: async () => ({ error: null }) }
  } }) }
  try {
    for (const recovered of [false, true]) {
      const requests: string[] = []
      globalThis.fetch = ((input: URL | RequestInfo, init?: RequestInit) => {
        requests.push(init?.method ?? 'GET')
        if (requests.length === 1) {
          deepStrictEqual(String(input), 'https://api.github.com/repos/community/support')
          return Promise.resolve(Response.json({ private: true }))
        }
        if (requests.length === 2) {
          return Promise.resolve(Response.json(recovered ? [{
            body: '- Submission ID: fixture-submission',
            html_url: 'https://github.com/community/support/issues/15', number: 15,
          }] : []))
        }
        return Promise.resolve(Response.json({
          html_url: 'https://github.com/community/support/issues/15', number: 15,
        }))
      }) as typeof fetch
      const result = await createOrRefreshGitHubIssue(admin, {
        id: 'fixture-submission', summary: 'Synthetic report', created_at: '2026-01-01T00:00:00Z',
        bundle_status: 'uploaded', github_issue_number: null,
      } as DebugBundleSubmission, (name) => ({
        GITHUB_ISSUES_TOKEN: 'fixture-token', GITHUB_ISSUES_REPO: 'community/support',
      })[name])
      deepStrictEqual(result.ok, true)
      deepStrictEqual(result.created, !recovered)
      deepStrictEqual(requests, ['GET', 'GET', recovered ? 'PATCH' : 'POST'])
      deepStrictEqual(updates.at(-1)?.status, 'reported')
    }
  } finally { globalThis.fetch = originalFetch }
})

Deno.test('private repository validation permits an uploaded report to finish', async () => {
  const originalFetch = globalThis.fetch
  const requests: { method: string; url: string }[] = []
  const updates: Record<string, unknown>[] = []
  try {
    globalThis.fetch = ((input: URL | RequestInfo, init?: RequestInit) => {
      requests.push({ method: init?.method ?? 'GET', url: String(input) })
      return Promise.resolve(Response.json(requests.length === 1
        ? { private: true, full_name: 'community/support' }
        : { html_url: 'https://github.com/community/support/issues/15', number: 15 }))
    }) as typeof fetch
    const admin = { from: () => ({ update: (values: Record<string, unknown>) => {
      updates.push(values)
      return { eq: async () => ({ error: null }) }
    } }) }
    const result = await createOrRefreshGitHubIssue(admin, {
      id: 'fixture-submission', summary: 'Synthetic report', created_at: '2026-01-01T00:00:00Z',
      bundle_status: 'uploaded', github_issue_number: 15,
    } as DebugBundleSubmission, (name) => ({
      GITHUB_ISSUES_TOKEN: 'fixture-token', GITHUB_ISSUES_REPO: 'community/support',
    })[name])
    deepStrictEqual(result.ok, true)
    deepStrictEqual(requests.map((r) => r.method), ['GET', 'PATCH'])
    deepStrictEqual(updates[0].status, 'reported')
    deepStrictEqual(updates[0].github_issue_error, null)
  } finally { globalThis.fetch = originalFetch }
})
