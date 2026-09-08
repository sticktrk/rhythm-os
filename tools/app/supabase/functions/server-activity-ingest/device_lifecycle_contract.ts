export type JsonObject = Record<string, unknown>

type DeviceLifecycleRowInput = {
  userId: string
  homeId: string
  hubId: string
  serverInstanceId: string | null
  event: JsonObject
}

function boundedToken(
  value: unknown,
  maxLength: number,
): string | null {
  if (typeof value !== 'string') return null
  const trimmed = value.trim()
  if (
    trimmed.length === 0 || trimmed.length > maxLength ||
    !/^[A-Za-z0-9_.:-]+$/.test(trimmed)
  ) return null
  return trimmed
}

function epochMillis(value: unknown): number | null {
  if (typeof value !== 'number' || !Number.isSafeInteger(value)) return null
  if (value < 0 || value > 8_640_000_000_000_000) return null
  return value
}

/**
 * Build the allow-listed Supabase row for one appliance lifecycle outcome.
 * Names, device IDs, bridge addresses, serials, raw errors, warnings, and the
 * full local pairing-history entry are intentionally impossible to forward.
 */
export function rowForDeviceLifecycle({
  userId,
  homeId,
  hubId,
  serverInstanceId,
  event,
}: DeviceLifecycleRowInput): Record<string, unknown> | null {
  const eventId = boundedToken(event.id, 96)
  const action = boundedToken(event.action, 16)
  const hubType = boundedToken(event.hub_type, 64)
  const outcome = boundedToken(event.outcome, 32)
  const epochMs = epochMillis(event.epoch_ms)
  if (
    !eventId || !hubType || !outcome || epochMs == null ||
    (action !== 'pair' && action !== 'unpair')
  ) return null

  const rawDeviceType = boundedToken(event.device_type, 16)
  const deviceType = rawDeviceType &&
      ['light', 'button', 'motion', 'contact'].includes(rawDeviceType)
    ? rawDeviceType
    : null
  const failureStage = boundedToken(event.failure_stage, 64)
  const correlationId = boundedToken(event.correlation_id, 96)
  const rawCommissioner = boundedToken(event.commissioner, 16)
  const commissioner = rawCommissioner &&
      ['phone', 'server'].includes(rawCommissioner)
    ? rawCommissioner
    : null

  return {
    user_id: userId,
    home_id: homeId,
    hub_id: hubId,
    ...(serverInstanceId ? { server_instance_id: serverInstanceId } : {}),
    event_id: eventId,
    occurred_at: new Date(epochMs).toISOString(),
    epoch_ms: epochMs,
    action,
    hub_type: hubType,
    ...(deviceType ? { device_type: deviceType } : {}),
    outcome,
    ...(failureStage ? { failure_stage: failureStage } : {}),
    ...(commissioner ? { commissioner } : {}),
    force: event.force === true,
    ...(correlationId ? { correlation_id: correlationId } : {}),
    sync_source: 'device_server_direct',
    last_seen_at: new Date().toISOString(),
  }
}
