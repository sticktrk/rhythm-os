import { createClient } from 'https://esm.sh/@supabase/supabase-js@2'

const corsHeaders = {
  'Access-Control-Allow-Origin': '*',
  'Access-Control-Allow-Methods': 'GET, OPTIONS',
  'Access-Control-Allow-Headers': 'authorization, x-client-info, apikey, content-type',
}

type PublishedMatterDeviceProfileRow = {
  id: string
  profile_key: string
  schema_version: number
  profile_version: number
  manufacturer: string | null
  model: string | null
  device_name: string | null
  matter_vendor_id: number | null
  matter_product_id: number | null
  firmware_version: string | null
  cluster_fingerprint: Record<string, unknown>
  capabilities: Record<string, unknown>
  quirks: Record<string, unknown>
  recommended_control_strategy: Record<string, unknown>
  evidence: Record<string, unknown>
  report_count: number
  updated_at: string
}

Deno.serve(async (req) => {
  if (req.method === 'OPTIONS') {
    return new Response('ok', { headers: corsHeaders })
  }
  if (req.method !== 'GET') {
    return jsonResponse({ error: 'Method not allowed' }, 405)
  }

  try {
    const supabaseUrl = requireEnv('SUPABASE_URL')
    const supabaseSecretKey = readSupabaseSecretKey()
    const adminClient = createClient(supabaseUrl, supabaseSecretKey)

    const columns = [
      'id',
      'profile_key',
      'schema_version',
      'profile_version',
      'manufacturer',
      'model',
      'device_name',
      'matter_vendor_id',
      'matter_product_id',
      'firmware_version',
      'cluster_fingerprint',
      'capabilities',
      'quirks',
      'recommended_control_strategy',
      'evidence',
      'report_count',
      'updated_at',
    ].join(',')
    const rows: PublishedMatterDeviceProfileRow[] = []
    let afterId: string | undefined
    for (;;) {
      let query = adminClient.from('published_matter_device_profiles')
        .select(columns).eq('approved', true).order('id').limit(500)
      if (afterId) query = query.gt('id', afterId)
      const { data, error } = await query
      if (error) throw new Error(error.message)
      const page = (data ?? []) as unknown as PublishedMatterDeviceProfileRow[]
      if (!page.length) break
      rows.push(...page)
      afterId = page[page.length - 1].id
    }
    // Hubs select the first matching profile. Preserve newest-first precedence.
    rows.sort((a, b) => b.profile_version - a.profile_version)
    const maxVersion = rows.reduce(
      (max, row) => Math.max(max, Number(row.profile_version) || 0),
      0,
    )

    return jsonResponse({
      schema_version: 1,
      profile_version: maxVersion,
      generated_at: new Date().toISOString(),
      profiles: rows.map((row) => ({
        profile_key: row.profile_key,
        schema_version: row.schema_version,
        profile_version: row.profile_version,
        match: {
          manufacturer: row.manufacturer,
          model: row.model,
          device_name: row.device_name,
          matter_vendor_id: row.matter_vendor_id,
          matter_product_id: row.matter_product_id,
          firmware_version: row.firmware_version,
          cluster_fingerprint: row.cluster_fingerprint ?? {},
        },
        capabilities: row.capabilities ?? {},
        quirks: row.quirks ?? {},
        recommended_control_strategy: row.recommended_control_strategy ?? {},
        evidence: {
          ...(row.evidence ?? {}),
          report_count: row.report_count,
          updated_at: row.updated_at,
        },
      })),
    })
  } catch (error) {
    console.error('Matter profile feed error:', error)
    return jsonResponse({ error: errorMessage(error) }, 500)
  }
})

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      ...corsHeaders,
      'Content-Type': 'application/json',
      'Cache-Control': 'public, max-age=3600',
    },
  })
}

function requireEnv(name: string): string {
  const value = Deno.env.get(name)?.trim()
  if (!value) throw new Error(`Missing ${name}`)
  return value
}

function readSupabaseSecretKey(): string {
  return Deno.env.get('SB_SECRET_KEY')?.trim() ??
    requireEnv('SUPABASE_SERVICE_ROLE_KEY')
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
