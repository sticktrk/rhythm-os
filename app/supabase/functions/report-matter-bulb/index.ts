import { createClient } from 'https://esm.sh/@supabase/supabase-js@2'
import {
  errorMessage,
  jsonResponse,
  readJson,
  withAuthenticatedRequest,
} from '../_shared/auth.ts'

type JsonObject = Record<string, unknown>
type SupabaseClient = ReturnType<typeof createClient>

Deno.serve(async (req) => {
  return withAuthenticatedRequest(req, async ({ userId, claims, adminClient }) => {
    if (req.method !== 'POST') {
      return jsonResponse({ error: 'Method not allowed' }, 405)
    }

    let body: JsonObject
    try {
      body = await readJson(req)
    } catch (error) {
      return jsonResponse({ error: errorMessage(error) }, 400)
    }

    const report = asObject(body.report)
    if (!report) {
      return jsonResponse({ error: 'Missing report' }, 400)
    }

    const device = asObject(report.device) ?? {}
    const observations = Array.isArray(report.observations)
      ? report.observations
      : []
    const inferredQuirks = Array.isArray(report.inferred_quirks)
      ? report.inferred_quirks
      : []
    const capabilityHints = asObject(report.capability_hints) ?? {}

    const { data, error } = await adminClient
      .from('matter_bulb_test_reports')
      .insert({
        user_id: userId,
        user_email: readString(claims, 'email'),
        is_anonymous: readBoolean(claims, 'is_anonymous') ?? true,
        app_version: readString(body, 'app_version'),
        app_build: readString(body, 'app_build'),
        app_platform: readString(body, 'app_platform'),
        server_version: readString(body, 'server_version'),
        server_platform_context: readString(body, 'server_platform_context'),
        client_report_id: readString(report, 'report_id'),
        canonical_device_id: readString(report, 'canonical_device_id'),
        native_device_id:
          readString(device, 'native_device_id') ?? readString(report, 'device_id'),
        device_name: readString(device, 'name'),
        manufacturer: readString(device, 'manufacturer'),
        model: readString(device, 'model'),
        inferred_quirks: inferredQuirks,
        capability_hints: capabilityHints,
        observations,
        report_payload: report,
      })
      .select('id')
      .single()

    if (error) {
      console.error('Matter bulb report insert failed:', error.message)
      return jsonResponse({ error: error.message }, 500)
    }

    const profileResult = await upsertPublishedMatterDeviceProfile(
      adminClient,
      data?.id ?? null,
      report,
    )

    return jsonResponse({
      success: true,
      id: data?.id,
      profile: profileResult,
    })
  })
})

async function upsertPublishedMatterDeviceProfile(
  adminClient: SupabaseClient,
  sourceReportId: string | null,
  report: JsonObject,
): Promise<JsonObject> {
  const profile = buildMatterDeviceProfile(sourceReportId, report)
  if (!profile) {
    return {
      upserted: false,
      reason: 'insufficient_device_identity',
    }
  }
  const profileKey = readString(profile, 'profile_key')
  if (!profileKey) {
    return {
      upserted: false,
      reason: 'invalid_profile_key',
    }
  }

  const { data: existing, error: existingError } = await adminClient
    .from('published_matter_device_profiles')
    .select('profile_version,report_count')
    .eq('profile_key', profileKey)
    .maybeSingle()

  if (existingError) {
    console.error('Matter profile lookup failed:', existingError.message)
    return {
      upserted: false,
      profile_key: profileKey,
      error: existingError.message,
    }
  }

  const profileVersion = readNumber(existing ?? {}, 'profile_version') ?? 0
  const reportCount = readNumber(existing ?? {}, 'report_count') ?? 0

  const { error } = await adminClient
    .from('published_matter_device_profiles')
    .upsert(
      {
        ...profile,
        profile_version: profileVersion + 1,
        report_count: reportCount + 1,
        approved: false,
        approved_at: null,
        approved_by: null,
      },
      { onConflict: 'profile_key' },
    )

  if (error) {
    console.error('Matter profile upsert failed:', error.message)
    return {
      upserted: false,
      profile_key: profileKey,
      error: error.message,
    }
  }

  return {
    upserted: true,
    approved: false,
    profile_key: profileKey,
    profile_version: profileVersion + 1,
  }
}

function buildMatterDeviceProfile(
  sourceReportId: string | null,
  report: JsonObject,
): JsonObject | null {
  const device = asObject(report.device) ?? {}
  const claimed = asObject(report.claimed_capabilities) ?? {}
  const rawSnapshot = asObject(report.raw_capability_snapshot) ?? {}
  const strategy = asObject(report.recommended_control_strategy) ?? {}
  const capabilityHints = asObject(report.capability_hints) ?? {}
  const testedCapabilities = asObject(report.tested_capabilities) ?? {}
  const readbackConsistency = asObject(report.readback_consistency) ?? {}
  const visualObservations = asObject(report.visual_observations) ?? {}
  const inferredQuirks = Array.isArray(report.inferred_quirks)
    ? report.inferred_quirks
    : []
  const behavioralQuirks = Array.isArray(report.inferred_behavioral_quirks)
    ? report.inferred_behavioral_quirks
    : []

  const manufacturer = readString(device, 'manufacturer')
  const model = readString(device, 'model')
  const deviceName = readString(device, 'name')
  const matterVendorId =
    readNumber(claimed, 'matter_vendor_id') ?? readNumber(rawSnapshot, 'matter_vendor_id')
  const matterProductId =
    readNumber(claimed, 'matter_product_id') ?? readNumber(rawSnapshot, 'matter_product_id')
  const firmwareVersion =
    readString(claimed, 'software_version_string') ??
    readString(rawSnapshot, 'software_version_string')

  const profileKey = matterVendorId !== null && matterProductId !== null
    ? `matter:${matterVendorId}:${matterProductId}`
    : modelProfileKey(manufacturer, model)
  if (!profileKey) return null

  const preferredColorCommand =
    readString(strategy, 'color_command') ??
    readString(strategy, 'preferred_color_command')
  const preferredLevelCommand =
    readString(strategy, 'preferred_level_command') ??
    readString(strategy, 'brightness_command')
  const spacingMs =
    readNumber(strategy, 'recommended_command_spacing_ms') ??
    commandThrottleFromQuirks(inferredQuirks)
  const transitionBehavior = readString(strategy, 'transition_behavior')
  const supportsTransition =
    readBoolean(capabilityHints, 'supports_transition') ??
    (transitionBehavior === 'smooth'
      ? true
      : transitionBehavior && transitionBehavior !== 'unknown'
      ? false
      : null)

  const runtimeQuirks = inferredQuirks.filter((quirk) =>
    typeof quirk === 'string' || typeof quirk === 'object'
  )
  const xyAckNoVisibleChange = behavioralQuirks.includes(
    'xy_color_commands_ack_but_no_visible_change',
  )
  const needsExplicitOn =
    runtimeQuirks.includes('needs_explicit_on') ||
    readString(strategy, 'turn_on_sequence')?.startsWith('explicit_on') === true

  return {
    profile_key: profileKey,
    schema_version: 1,
    manufacturer,
    model,
    device_name: deviceName,
    matter_vendor_id: matterVendorId,
    matter_product_id: matterProductId,
    firmware_version: firmwareVersion,
    cluster_fingerprint: {
      endpoint_list: claimed.endpoint_list ?? rawSnapshot.endpoint_list ?? null,
      server_clusters: claimed.server_clusters ?? rawSnapshot.server_clusters ?? null,
      accepted_command_lists:
        claimed.accepted_command_lists ?? rawSnapshot.accepted_command_lists ?? null,
      attribute_lists: claimed.attribute_lists ?? rawSnapshot.attribute_lists ?? null,
      level_control_feature_map: claimed.level_control_feature_map ?? null,
      color_control_feature_map: claimed.color_control_feature_map ?? null,
      color_capabilities: claimed.color_capabilities ?? null,
    },
    capabilities: stripNulls({
      preferred_color_command: preferredColorCommand,
      preferred_level_command: preferredLevelCommand,
      min_brightness: readNumber(capabilityHints, 'min_brightness'),
      supports_transition: supportsTransition,
      transition_behavior: transitionBehavior,
      usable_min_kelvin: readNumber(claimed, 'usable_min_kelvin'),
      usable_max_kelvin: readNumber(claimed, 'usable_max_kelvin'),
      color_modes: claimed.color_modes ?? null,
    }),
    quirks: stripNulls({
      runtime_quirks: runtimeQuirks,
      behavioral_quirks: behavioralQuirks,
      needs_explicit_on: needsExplicitOn,
      needs_xy_not_ct: runtimeQuirks.includes('needs_xy_not_ct'),
      xy_color_commands_ack_but_no_visible_change: xyAckNoVisibleChange,
      recommended_command_spacing_ms: spacingMs,
      on_restores_previous_level: readBoolean(strategy, 'on_restores_previous_level'),
      power_on_behavior: readString(strategy, 'power_on_behavior'),
    }),
    recommended_control_strategy: strategy,
    evidence: stripNulls({
      client_report_id: readString(report, 'report_id'),
      canonical_device_id: readString(report, 'canonical_device_id'),
      native_device_id: readString(device, 'native_device_id') ??
        readString(report, 'device_id'),
      tested_capabilities: testedCapabilities,
      visual_observations: visualObservations,
      readback_consistency: readbackConsistency,
      source_report_created_at: readString(report, 'created_at'),
    }),
    source_report_id: sourceReportId,
    client_report_id: readString(report, 'report_id'),
  }
}

function asObject(value: unknown): JsonObject | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    return null
  }
  return value as JsonObject
}

function readString(data: JsonObject, key: string): string | null {
  const value = data[key]
  if (typeof value !== 'string') return null
  const trimmed = value.trim()
  return trimmed.length > 0 ? trimmed : null
}

function readNumber(data: JsonObject, key: string): number | null {
  const value = data[key]
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value === 'string') {
    const parsed = Number(value)
    return Number.isFinite(parsed) ? parsed : null
  }
  return null
}

function readBoolean(data: JsonObject, key: string): boolean | null {
  const value = data[key]
  return typeof value === 'boolean' ? value : null
}

function modelProfileKey(
  manufacturer: string | null,
  model: string | null,
): string | null {
  const normalizedManufacturer = normalizeKeyPart(manufacturer)
  const normalizedModel = normalizeKeyPart(model)
  if (!normalizedManufacturer || !normalizedModel) return null
  return `model:${normalizedManufacturer}:${normalizedModel}`
}

function normalizeKeyPart(value: string | null): string | null {
  if (!value) return null
  const normalized = value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
  return normalized.length > 0 ? normalized : null
}

function commandThrottleFromQuirks(quirks: unknown[]): number | null {
  for (const quirk of quirks) {
    const data = asObject(quirk)
    if (!data) continue
    const ms = readNumber(data, 'command_throttle_ms')
    if (ms !== null) return ms
  }
  return null
}

function stripNulls(value: JsonObject): JsonObject {
  return Object.fromEntries(
    Object.entries(value).filter(([, entry]) => entry !== null && entry !== undefined),
  )
}
