import assert from 'node:assert/strict'
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
