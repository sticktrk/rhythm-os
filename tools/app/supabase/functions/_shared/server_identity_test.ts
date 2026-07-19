import assert from 'node:assert/strict'
import test from 'node:test'

import {
  isDurableRhythmServerIdentity,
  resolveServerIdentity,
  serverIdentityKind,
} from './server_identity.ts'

test('a provisional candidate cannot downgrade a durable identity', () => {
  assert.deepEqual(
    resolveServerIdentity(
      'srv-known',
      'endpoint:http://192.168.5.123:54448',
    ),
    { status: 'ok', value: 'srv-known', changed: false },
  )
})

test('a durable candidate promotes a provisional identity', () => {
  assert.deepEqual(
    resolveServerIdentity(
      'endpoint:http://192.168.5.123:54448',
      ' SRV-KITCHEN ',
    ),
    { status: 'ok', value: 'srv-kitchen', changed: true },
  )
})

test('different durable identities are a conflict', () => {
  assert.deepEqual(resolveServerIdentity('srv-kitchen', 'srv-garage'), {
    status: 'conflict',
    existing: 'srv-kitchen',
    candidate: 'srv-garage',
  })
})

test('identity classification distinguishes endpoint lookup keys', () => {
  assert.equal(serverIdentityKind('endpoint:http://box:54448'), 'provisional')
  assert.equal(serverIdentityKind('srv-kitchen'), 'durable')
  assert.equal(isDurableRhythmServerIdentity('srv-kitchen'), true)
  assert.equal(isDurableRhythmServerIdentity('endpoint:http://box:54448'), false)
})
