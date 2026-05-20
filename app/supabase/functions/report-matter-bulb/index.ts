import {
  errorMessage,
  jsonResponse,
  readJson,
  withAuthenticatedRequest,
} from '../_shared/auth.ts'

type JsonObject = Record<string, unknown>

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

    return jsonResponse({ success: true, id: data?.id })
  })
})

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

function readBoolean(data: JsonObject, key: string): boolean | null {
  const value = data[key]
  return typeof value === 'boolean' ? value : null
}
