import { createClient } from 'https://esm.sh/@supabase/supabase-js@2'

export const corsHeaders = {
  'Access-Control-Allow-Origin': '*',
  'Access-Control-Allow-Methods': 'POST, OPTIONS',
  'Access-Control-Allow-Headers':
    'authorization, x-client-info, apikey, content-type',
}

export type JsonObject = Record<string, unknown>

export type AuthenticatedRequestContext = {
  userId: string
  claims: JsonObject
  // Supabase's generic factory currently infers `never` for an untyped schema
  // when captured through ReturnType, even though the runtime client is valid.
  adminClient: any
}

export async function withAuthenticatedRequest(
  req: Request,
  handler: (context: AuthenticatedRequestContext) => Promise<Response>,
): Promise<Response> {
  if (req.method === 'OPTIONS') {
    return new Response('ok', { headers: corsHeaders })
  }

  try {
    const authHeader = req.headers.get('Authorization')
    const token = readBearerToken(authHeader)
    if (!token) {
      return jsonResponse({ error: 'Missing bearer token' }, 401)
    }

    const supabaseUrl = requireEnv('SUPABASE_URL')
    const supabasePublishableKey = readSupabasePublishableKey()
    const supabaseSecretKey = readSupabaseSecretKey()

    const authClient = createClient(supabaseUrl, supabasePublishableKey)
    const { data, error } = await authClient.auth.getClaims(token)
    const claims = asJsonObject(data?.claims)
    const userId = readClaimString(claims, 'sub')
    if (error || !claims || !userId) {
      return jsonResponse({ error: 'Invalid token' }, 401)
    }

    const adminClient = createClient(supabaseUrl, supabaseSecretKey)
    return await handler({
      userId,
      claims,
      adminClient,
    })
  } catch (error) {
    console.error('Authenticated request error:', error)
    return jsonResponse({ error: errorMessage(error) }, 500)
  }
}

export async function readJson(req: Request): Promise<JsonObject> {
  try {
    const value = await req.json()
    const json = asJsonObject(value)
    if (json) return json
  } catch (_) {
    // Fall through to the validation error below.
  }
  throw new Error('Invalid JSON body')
}

export function readString(data: JsonObject, key: string): string | null {
  const value = data[key]
  if (typeof value !== 'string') return null
  const trimmed = value.trim()
  return trimmed.length > 0 ? trimmed : null
}

export function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      ...corsHeaders,
      'Content-Type': 'application/json',
    },
  })
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function readBearerToken(authHeader: string | null): string | null {
  if (!authHeader) return null
  const [scheme, token] = authHeader.split(' ')
  if (scheme !== 'Bearer' || !token) return null
  const trimmed = token.trim()
  return trimmed.length > 0 ? trimmed : null
}

function readClaimString(claims: JsonObject | null, key: string): string | null {
  const value = claims?.[key]
  return typeof value === 'string' && value.length > 0 ? value : null
}

function asJsonObject(value: unknown): JsonObject | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    return null
  }
  return value as JsonObject
}

function requireEnv(name: string): string {
  const value = Deno.env.get(name)?.trim()
  if (!value) throw new Error(`Missing ${name}`)
  return value
}

function readSupabasePublishableKey(): string {
  return Deno.env.get('SB_PUBLISHABLE_KEY')?.trim() ??
    requireEnv('SUPABASE_ANON_KEY')
}

function readSupabaseSecretKey(): string {
  return Deno.env.get('SB_SECRET_KEY')?.trim() ??
    requireEnv('SUPABASE_SERVICE_ROLE_KEY')
}
