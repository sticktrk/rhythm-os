import {
  assertEquals,
  assertObjectMatch,
} from 'https://deno.land/std@0.224.0/assert/mod.ts'
import {
  LIGHT_USAGE_SCHEMA_VERSION,
  readLightUsageBatch,
} from './light_usage_contract.ts'

const segment = {
  segment_id: '00112233445566778899aabbccddeeff',
  subject_id: 'canonical-light-1',
  subject_kind: 'bulb',
  usage_date: '2026-08-16',
  revision: 3,
  on_ms: 4000,
  covered_ms: 5000,
  transition_uncertainty_ms: 1000,
  observation_count: 4,
  transition_count: 1,
  first_observed_at_epoch_ms: 1_755_300_000_000,
  last_observed_at_epoch_ms: 1_755_300_005_000,
  source_summary: {
    periodic: 2,
    sync_poll: 0,
    live_subscription: 2,
    authoritative_refresh: 0,
  },
}

Deno.test('usage batch is optional for legacy event-only requests', () => {
  assertEquals(readLightUsageBatch({ events: [] }), null)
})

Deno.test('usage batch normalizes the bounded aggregate contract', () => {
  const result = readLightUsageBatch({
    usage_schema_version: LIGHT_USAGE_SCHEMA_VERSION,
    usage_batch_id: 'ffeeddccbbaa99887766554433221100',
    usage_segments: [segment],
  })
  assertObjectMatch(result as Record<string, unknown>, {
    schemaVersion: 1,
    batchId: 'ffeeddccbbaa99887766554433221100',
  })
  assertEquals((result as { segments: unknown[] }).segments, [segment])
})

Deno.test('usage rejects invented duration and unsafe identity', () => {
  const invalidDuration = readLightUsageBatch({
    usage_schema_version: 1,
    usage_batch_id: 'ffeeddccbbaa99887766554433221100',
    usage_segments: [{ ...segment, on_ms: 5001 }],
  })
  assertEquals(invalidDuration, { error: 'usage segment duration invariant failed' })

  const unsafeIdentity = readLightUsageBatch({
    usage_schema_version: 1,
    usage_batch_id: 'ffeeddccbbaa99887766554433221100',
    usage_segments: [{ ...segment, subject_id: 'Kitchen / token=secret' }],
  })
  assertEquals(unsafeIdentity, { error: 'usage segment identity is invalid' })
})

Deno.test('usage requires exact schema and stable batch identity', () => {
  assertEquals(
    readLightUsageBatch({
      usage_schema_version: 2,
      usage_batch_id: 'ffeeddccbbaa99887766554433221100',
      usage_segments: [segment],
    }),
    { error: 'Unsupported usage_schema_version' },
  )
  assertEquals(
    readLightUsageBatch({
      usage_schema_version: 1,
      usage_batch_id: 'home-123',
      usage_segments: [segment],
    }),
    { error: 'Invalid usage_batch_id' },
  )
})
