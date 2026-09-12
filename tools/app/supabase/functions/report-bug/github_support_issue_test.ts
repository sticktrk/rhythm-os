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
          return Promise.resolve(Response.json({ private: true, full_name: 'community/support' }))
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
      github_issue_url: 'https://github.com/community/support/issues/15',
    } as DebugBundleSubmission, (name) => ({
      GITHUB_ISSUES_TOKEN: 'fixture-token', GITHUB_ISSUES_REPO: 'community/support',
    })[name])
    deepStrictEqual(result.ok, true)
    deepStrictEqual(requests.map((r) => r.method), ['GET', 'PATCH'])
    deepStrictEqual(updates[0].status, 'reported')
    deepStrictEqual(updates[0].github_issue_error, null)
  } finally { globalThis.fetch = originalFetch }
})

Deno.test('support requests refuse redirects before forwarding diagnostics to another repository', async () => {
  const originalFetch = globalThis.fetch
  try {
    for (const status of [301, 302, 307, 308]) for (const existing of [false, true]) {
      const writes: string[] = []
      globalThis.fetch = ((input: URL | RequestInfo, init?: RequestInit) => {
        const request = new Request(input, init)
        if (request.method === 'GET') {
          deepStrictEqual(request.redirect, 'error', 'metadata and lookup redirects must also be refused')
          return Promise.resolve(Response.json(request.url.includes('/issues')
            ? [] : { private: true, full_name: 'community/support' }))
        }
        // Model fetch's redirect behavior for an issue transferred after the
        // submission was stored. Following the redirect sends this PATCH body
        // to a public issue before the caller can inspect the final response.
        if (request.redirect === 'follow') {
          writes.push('public')
          return Promise.resolve(Response.json({
            html_url: 'https://github.com/community/public/issues/15', number: 15,
          }))
        }
        if (request.redirect === 'error') return Promise.reject(new TypeError('Redirect refused'))
        return Promise.resolve(new Response(null, {
          status, headers: { Location: 'https://api.github.com/repos/community/public/issues/15' },
        }))
      }) as typeof fetch
      const updates: Record<string, unknown>[] = []
      const admin = { from: () => ({ update: (values: Record<string, unknown>) => {
        updates.push(values)
        return { eq: async () => ({ error: null }) }
      } }) }
      const result = await createOrRefreshGitHubIssue(admin, {
        id: 'fixture-submission', summary: 'Synthetic private diagnostics',
        created_at: '2026-01-01T00:00:00Z', bundle_status: 'uploaded',
        github_issue_number: existing ? 15 : null,
        github_issue_url: existing ? 'https://github.com/community/support/issues/15' : null,
      } as DebugBundleSubmission, (name) => ({
        GITHUB_ISSUES_TOKEN: 'fixture-token', GITHUB_ISSUES_REPO: 'community/support',
      })[name])
      deepStrictEqual(writes, [], 'a redirected issue must receive no private payload')
      deepStrictEqual(result.ok, false)
      deepStrictEqual(updates.length, 1)
      deepStrictEqual(Object.keys(updates[0]), ['github_issue_error'])
    }
  } finally { globalThis.fetch = originalFetch }
})

Deno.test('private metadata must identify the configured repository exactly', async () => {
  const originalFetch = globalThis.fetch
  try {
    for (const metadata of [
      { private: true },
      { private: true, full_name: 'community/other' },
      { private: true, full_name: null },
    ]) {
      let requests = 0
      globalThis.fetch = ((_input: URL | RequestInfo, init?: RequestInit) => {
        requests++
        deepStrictEqual(init?.body, undefined)
        return Promise.resolve(Response.json(metadata))
      }) as typeof fetch
      const admin = { from: () => ({ update: (values: Record<string, unknown>) => {
        deepStrictEqual(Object.keys(values), ['github_issue_error'])
        return { eq: async () => ({ error: null }) }
      } }) }
      const result = await createOrRefreshGitHubIssue(admin, {
        id: 'fixture-submission', summary: 'Synthetic private diagnostics',
      } as DebugBundleSubmission, (name) => ({
        GITHUB_ISSUES_TOKEN: 'fixture-token', GITHUB_ISSUES_REPO: 'community/support',
      })[name])
      deepStrictEqual(result.ok, false)
      deepStrictEqual(requests, 1)
      match(result.error!, /identity/)
    }
  } finally { globalThis.fetch = originalFetch }
})

Deno.test('stored and recovered issue associations are validated before any write', async () => {
  const originalFetch = globalThis.fetch
  try {
    for (const recovered of [false, true]) for (const issueUrl of [
      null, 'invalid', 'https://github.com/community/other/issues/15',
      'https://github.com/community/support/issues/16',
      'https://example.test/community/support/issues/15',
      'http://github.com/community/support/issues/15',
      'https://github.com/community/support/issues/15?redirect=other',
    ]) {
      let requests = 0
      globalThis.fetch = ((_input: URL | RequestInfo, init?: RequestInit) => {
        requests++
        deepStrictEqual(init?.body, undefined, 'an invalid association must not receive diagnostics')
        return Promise.resolve(Response.json(requests === 1
          ? { private: true, full_name: 'community/support' }
          : [{ body: '- Submission ID: fixture-submission', number: 15, html_url: issueUrl }]))
      }) as typeof fetch
      const admin = { from: () => ({ update: (values: Record<string, unknown>) => {
        deepStrictEqual(Object.keys(values), ['github_issue_error'])
        return { eq: async () => ({ error: null }) }
      } }) }
      const result = await createOrRefreshGitHubIssue(admin, {
        id: 'fixture-submission', summary: 'Synthetic report', created_at: '2026-01-01T00:00:00Z',
        github_issue_number: recovered ? null : 15, github_issue_url: recovered ? null : issueUrl,
      } as DebugBundleSubmission, (name) => ({
        GITHUB_ISSUES_TOKEN: 'fixture-token', GITHUB_ISSUES_REPO: 'community/support',
      })[name])
      deepStrictEqual(result.ok, false)
      deepStrictEqual(requests, recovered ? 2 : 1)
    }
  } finally { globalThis.fetch = originalFetch }
})

Deno.test('changing support routing cannot overwrite the same issue number in another repository', async () => {
  const originalFetch = globalThis.fetch
  const requests: string[] = []
  const updates: Record<string, unknown>[] = []
  try {
    globalThis.fetch = ((_input: URL | RequestInfo, init?: RequestInit) => {
      requests.push(init?.method ?? 'GET')
      return Promise.resolve(Response.json(requests.length === 1
        ? { private: true, full_name: 'community/new-support' }
        : { html_url: 'https://github.com/community/new-support/issues/15', number: 15 }))
    }) as typeof fetch
    const admin = { from: () => ({ update: (values: Record<string, unknown>) => {
      updates.push(values)
      return { eq: async () => ({ error: null }) }
    } }) }
    const result = await createOrRefreshGitHubIssue(admin, {
      id: 'fixture-submission', summary: 'Synthetic report', created_at: '2026-01-01T00:00:00Z',
      bundle_status: 'uploaded', github_issue_number: 15,
      github_issue_url: 'https://github.com/community/old-support/issues/15',
    } as DebugBundleSubmission, (name) => ({
      GITHUB_ISSUES_TOKEN: 'fixture-token', GITHUB_ISSUES_REPO: 'community/new-support',
    })[name])
    deepStrictEqual(result.ok, false)
    deepStrictEqual(requests, ['GET'])
    deepStrictEqual(Object.keys(updates[0]), ['github_issue_error'])
  } finally { globalThis.fetch = originalFetch }
})
