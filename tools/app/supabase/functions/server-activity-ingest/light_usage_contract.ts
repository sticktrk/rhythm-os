export type JsonObject = Record<string, unknown>

export const LIGHT_USAGE_SCHEMA_VERSION = 1
export const LIGHT_USAGE_BATCH_LIMIT = 512

export type LightUsageBatch = {
  schemaVersion: number
  batchId: string
  segments: JsonObject[]
}

export type LightUsageContractError = { error: string }

export function readLightUsageBatch(
  body: JsonObject,
): LightUsageBatch | LightUsageContractError | null {
  const schemaValue = body.usage_schema_version
  const batchValue = body.usage_batch_id
  const segmentsValue = body.usage_segments
  if (schemaValue == null && batchValue == null && segmentsValue == null) {
    return null
  }
  if (schemaValue !== LIGHT_USAGE_SCHEMA_VERSION) {
    return { error: 'Unsupported usage_schema_version' }
  }
  const batchId = readOpaqueId(batchValue)
  if (!batchId) return { error: 'Invalid usage_batch_id' }
  if (!Array.isArray(segmentsValue) || segmentsValue.length === 0) {
    return { error: 'usage_segments must be a non-empty array' }
  }
  if (segmentsValue.length > LIGHT_USAGE_BATCH_LIMIT) {
    return { error: `usage_segments exceeds ${LIGHT_USAGE_BATCH_LIMIT}` }
  }

  const segments: JsonObject[] = []
  const segmentIds = new Set<string>()
  for (const value of segmentsValue) {
    if (!isObject(value)) return { error: 'usage_segments must contain objects' }
    const normalized = normalizeSegment(value)
    if ('error' in normalized) return normalized
    const segmentId = String(normalized.segment_id)
    if (segmentIds.has(segmentId)) {
      return { error: 'usage_segments contains duplicate segment_id' }
    }
    segmentIds.add(segmentId)
    segments.push(normalized)
  }
  return { schemaVersion: schemaValue, batchId, segments }
}

function normalizeSegment(value: JsonObject): JsonObject | LightUsageContractError {
  const segmentId = readOpaqueId(value.segment_id)
  const subjectId = readBoundedToken(value.subject_id, 160)
  const subjectKind = value.subject_kind
  const usageDate = readUsageDate(value.usage_date)
  const revision = readNonNegativeSafeInt(value.revision, 1)
  const onMs = readNonNegativeSafeInt(value.on_ms)
  const coveredMs = readNonNegativeSafeInt(value.covered_ms)
  const uncertaintyMs = readNonNegativeSafeInt(value.transition_uncertainty_ms)
  const observationCount = readNonNegativeSafeInt(value.observation_count)
  const transitionCount = readNonNegativeSafeInt(value.transition_count)
  const firstObservedAt = readNonNegativeSafeInt(value.first_observed_at_epoch_ms)
  const lastObservedAt = readNonNegativeSafeInt(value.last_observed_at_epoch_ms)
  const sourceSummary = normalizeSourceSummary(value.source_summary)

  if (!segmentId || !subjectId || !usageDate || revision == null) {
    return { error: 'usage segment identity is invalid' }
  }
  if (subjectKind !== 'bulb' && subjectKind !== 'room_aggregate') {
    return { error: 'usage segment subject_kind is invalid' }
  }
  if (
    onMs == null || coveredMs == null || uncertaintyMs == null ||
    observationCount == null || transitionCount == null ||
    firstObservedAt == null || lastObservedAt == null || !sourceSummary
  ) {
    return { error: 'usage segment counters are invalid' }
  }
  if (onMs > coveredMs || uncertaintyMs > coveredMs) {
    return { error: 'usage segment duration invariant failed' }
  }
  if (transitionCount > observationCount || firstObservedAt > lastObservedAt) {
    return { error: 'usage segment observation invariant failed' }
  }

  return {
    segment_id: segmentId,
    subject_id: subjectId,
    subject_kind: subjectKind,
    usage_date: usageDate,
    revision,
    on_ms: onMs,
    covered_ms: coveredMs,
    transition_uncertainty_ms: uncertaintyMs,
    observation_count: observationCount,
    transition_count: transitionCount,
    first_observed_at_epoch_ms: firstObservedAt,
    last_observed_at_epoch_ms: lastObservedAt,
    source_summary: sourceSummary,
  }
}

function normalizeSourceSummary(value: unknown): JsonObject | null {
  if (!isObject(value)) return null
  const keys = [
    'periodic',
    'sync_poll',
    'live_subscription',
    'authoritative_refresh',
  ]
  const result: JsonObject = {}
  for (const key of keys) {
    const count = readNonNegativeSafeInt(value[key])
    if (count == null) return null
    result[key] = count
  }
  return result
}

function readOpaqueId(value: unknown): string | null {
  return typeof value === 'string' && /^[a-f0-9]{32}$/.test(value)
    ? value
    : null
}

function readBoundedToken(value: unknown, maxLength: number): string | null {
  if (typeof value !== 'string') return null
  const trimmed = value.trim()
  return trimmed.length > 0 && trimmed.length <= maxLength &&
      /^[A-Za-z0-9_.:-]+$/.test(trimmed)
    ? trimmed
    : null
}

function readUsageDate(value: unknown): string | null {
  if (typeof value !== 'string' || !/^\d{4}-\d{2}-\d{2}$/.test(value)) {
    return null
  }
  const date = new Date(`${value}T00:00:00.000Z`)
  return Number.isFinite(date.getTime()) && date.toISOString().slice(0, 10) === value
    ? value
    : null
}

function readNonNegativeSafeInt(value: unknown, min = 0): number | null {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= min
    ? value
    : null
}

function isObject(value: unknown): value is JsonObject {
  return value != null && typeof value === 'object' && !Array.isArray(value)
}
