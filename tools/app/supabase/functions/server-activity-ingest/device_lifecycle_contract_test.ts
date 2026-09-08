import { rowForDeviceLifecycle } from './device_lifecycle_contract.ts'

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message)
}

Deno.test('device lifecycle row forwards only the privacy-safe contract', () => {
  const row = rowForDeviceLifecycle({
    userId: 'user-1',
    homeId: 'home-1',
    hubId: 'hub-1',
    serverInstanceId: 'server-1',
    event: {
      id: 'device-lifecycle-aabbcc',
      epoch_ms: 1_786_277_600_000,
      action: 'unpair',
      hub_type: 'hue',
      device_type: 'button',
      outcome: 'complete',
      force: true,
      correlation_id: 'hue-remove-journey',
      device_id: 'private-device-id',
      name: 'Private Switch Name',
      error: 'private raw bridge error',
      warnings: ['private warning'],
    },
  })

  assert(row != null, 'expected a valid row')
  assert(row.action === 'unpair', 'action should be retained')
  assert(row.device_type === 'button', 'device type should be retained')
  assert(row.force === true, 'force outcome should be retained')
  const serialized = JSON.stringify(row)
  for (
    const privateValue of [
      'private-device-id',
      'Private Switch Name',
      'private raw bridge error',
      'private warning',
    ]
  ) {
    assert(!serialized.includes(privateValue), `leaked ${privateValue}`)
  }
})

Deno.test('device lifecycle row rejects malformed identity and action fields', () => {
  const common = {
    userId: 'user-1',
    homeId: 'home-1',
    hubId: 'hub-1',
    serverInstanceId: null,
  }
  assert(
    rowForDeviceLifecycle({
      ...common,
      event: {
        id: 'contains a secret-like space',
        epoch_ms: 1,
        action: 'pair',
        hub_type: 'hue',
        outcome: 'complete',
      },
    }) == null,
    'unsafe event id should be rejected',
  )
  assert(
    rowForDeviceLifecycle({
      ...common,
      event: {
        id: 'device-lifecycle-aabbcc',
        epoch_ms: 1,
        action: 'rename',
        hub_type: 'hue',
        outcome: 'complete',
      },
    }) == null,
    'unknown action should be rejected',
  )
})

Deno.test('device lifecycle row accepts only bounded commissioner values', () => {
  const common = {
    userId: 'user-1',
    homeId: 'home-1',
    hubId: 'hub-1',
    serverInstanceId: null,
  }
  const row = rowForDeviceLifecycle({
    ...common,
    event: {
      id: 'device-lifecycle-phone',
      epoch_ms: 1,
      action: 'pair',
      hub_type: 'matter',
      outcome: 'complete',
      commissioner: 'phone',
      handoff_passcode: 20202021,
      handoff_address: '192.0.2.42',
    },
  })
  assert(row?.commissioner === 'phone', 'phone commissioner should be retained')
  const serialized = JSON.stringify(row)
  assert(!serialized.includes('20202021'), 'passcode must not be forwarded')
  assert(!serialized.includes('192.0.2.42'), 'address must not be forwarded')

  const invalid = rowForDeviceLifecycle({
    ...common,
    event: {
      id: 'device-lifecycle-invalid-commissioner',
      epoch_ms: 1,
      action: 'pair',
      hub_type: 'matter',
      outcome: 'complete',
      commissioner: 'phone-with-address-192.0.2.42',
    },
  })
  assert(invalid != null, 'invalid optional commissioner should not reject event')
  assert(invalid.commissioner == null, 'invalid commissioner must be omitted')
})
