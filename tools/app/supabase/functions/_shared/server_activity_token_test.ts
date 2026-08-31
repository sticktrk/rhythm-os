import assert from 'node:assert/strict'
import test from 'node:test'

import {
  pendingServerActivityTokenExpiryIso,
  SERVER_ACTIVITY_TOKEN_ACTIVATION_TTL_MS,
  serverActivityTokenCanAuthenticate,
  serverActivityTokenNeedsActivation,
} from './server_activity_token.ts'

const now = Date.parse('2026-08-31T04:30:00.000Z')

test('active credentials remain valid while a replacement is staged', () => {
  const active = {
    activated_at: '2026-08-01T00:00:00.000Z',
    activation_expires_at: null,
    revoked_at: null,
  }

  assert.equal(serverActivityTokenNeedsActivation(active), false)
  assert.equal(serverActivityTokenCanAuthenticate(active, now), true)
})

test('a staged replacement authenticates only during its activation window', () => {
  const pending = {
    activated_at: null,
    activation_expires_at: pendingServerActivityTokenExpiryIso(now),
    revoked_at: null,
  }

  assert.equal(serverActivityTokenNeedsActivation(pending), true)
  assert.equal(serverActivityTokenCanAuthenticate(pending, now), true)
  assert.equal(
    serverActivityTokenCanAuthenticate(
      pending,
      now + SERVER_ACTIVITY_TOKEN_ACTIVATION_TTL_MS,
    ),
    false,
  )
})

test('revoked and malformed staged credentials never authenticate', () => {
  assert.equal(
    serverActivityTokenCanAuthenticate(
      {
        activated_at: '2026-08-01T00:00:00.000Z',
        revoked_at: '2026-08-31T04:00:00.000Z',
      },
      now,
    ),
    false,
  )
  assert.equal(
    serverActivityTokenCanAuthenticate(
      {
        activated_at: null,
        activation_expires_at: null,
        revoked_at: null,
      },
      now,
    ),
    false,
  )
})
