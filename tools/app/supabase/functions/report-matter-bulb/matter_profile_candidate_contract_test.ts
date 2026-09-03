import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'

import { matterProfileCandidateIngestionPlan } from './matter_profile_candidate_contract.ts'

test('a new report creates pending evidence without revoking an approved profile', () => {
  const plan = matterProfileCandidateIngestionPlan(
    'matter:123:456',
    { profile_key: 'matter:123:456', color_route: 'xy' },
    'report-1',
    { profile_version: 7, report_count: 3, approved: true },
    6,
  )

  assert.deepEqual(plan.candidateInsert, {
    profile_key: 'matter:123:456',
    candidate_version: 8,
    status: 'pending',
    profile_payload: { profile_key: 'matter:123:456', color_route: 'xy' },
    source_report_id: 'report-1',
  })
  assert.deepEqual(plan.publishedCountUpdate, { report_count: 4 })
  assert.equal('approved' in plan.publishedCountUpdate!, false)
  assert.equal(plan.approvedProfilePreserved, true)
})

test('the first report does not create a published fleet profile', () => {
  const plan = matterProfileCandidateIngestionPlan(
    'model:vendor:bulb',
    { profile_key: 'model:vendor:bulb' },
    null,
    null,
  )

  assert.equal(plan.candidateInsert.status, 'pending')
  assert.equal(plan.candidateInsert.candidate_version, 1)
  assert.equal(plan.publishedCountUpdate, null)
})

test('candidate versions advance beside multiple reports awaiting review', () => {
  const plan = matterProfileCandidateIngestionPlan(
    'matter:123:456',
    { profile_key: 'matter:123:456' },
    'report-2',
    { profile_version: 7, report_count: 4, approved: true },
    9,
  )

  assert.equal(plan.candidateInsert.candidate_version, 10)
})

test('the admin approval RPC atomically promotes pending candidate evidence', () => {
  const migration = readFileSync(
    new URL(
      '../../migrations/20260903000000_add_matter_device_profile_candidates.sql',
      import.meta.url,
    ),
    'utf8',
  )

  assert.match(
    migration,
    /FUNCTION public\.approve_matter_profile_candidate\(candidate_id UUID\)/,
  )
  assert.match(migration, /SECURITY DEFINER/)
  assert.match(migration, /IF NOT public\.is_rhythm_admin\(\)/)
  assert.match(migration, /ON CONFLICT \(profile_key\) DO UPDATE/)
  assert.match(migration, /profile_version = EXCLUDED\.profile_version/)
  assert.match(migration, /approved_by = EXCLUDED\.approved_by/)
  assert.match(migration, /status = 'approved'/)
  assert.match(migration, /reviewed_by = reviewer_id/)
  assert.match(
    migration,
    /GRANT EXECUTE ON FUNCTION public\.approve_matter_profile_candidate\(UUID\) TO authenticated/,
  )
})
