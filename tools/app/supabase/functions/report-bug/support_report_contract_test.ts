import assert from 'node:assert/strict'
import test from 'node:test'

import {
  githubLabelsForSupportReport,
  normalizeSupportReportKind,
  supportReportIssueCopy,
} from './support_report_contract.ts'

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
