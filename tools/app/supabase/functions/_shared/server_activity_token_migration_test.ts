import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'

const migration = readFileSync(
  new URL(
    '../../migrations/20260831050000_stage_server_activity_token_rotation.sql',
    import.meta.url,
  ),
  'utf8',
)
const joinFunction = readFileSync(
  new URL('../join-home-by-local-device-proof/index.ts', import.meta.url),
  'utf8',
)

test('Home join permits overlapping credentials owned by the same hub', () => {
  assert.match(
    migration,
    /FROM public\.server_light_activity_device_tokens other\s+WHERE other\.hub_id <> hub_uuid/,
  )
})

test('Home join rejects an expired staged credential at both auth boundaries', () => {
  assert.match(
    migration,
    /AND \(\s*activated_at IS NOT NULL\s*OR activation_expires_at > NOW\(\)\s*\)\s*FOR UPDATE/,
  )
  assert.match(
    joinFunction,
    /serverActivityTokenCanAuthenticate\(row\)/,
  )
})
