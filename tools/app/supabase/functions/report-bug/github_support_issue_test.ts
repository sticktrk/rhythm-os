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
