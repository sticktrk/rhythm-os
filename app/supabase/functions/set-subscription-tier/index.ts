import {
  errorMessage,
  jsonResponse,
  readJson,
  readString,
  withAuthenticatedRequest,
} from '../_shared/auth.ts'

const ALLOWED_TIERS = new Set(['basic', 'pro'])

Deno.serve(async (req) => {
  return withAuthenticatedRequest(req, async ({ userId, adminClient }) => {
    if (req.method !== 'POST') {
      return jsonResponse({ error: 'Method not allowed' }, 405)
    }

    let tier: string | null = null
    try {
      const body = await readJson(req)
      tier = readString(body, 'tier')
    } catch (error) {
      return jsonResponse({ error: errorMessage(error) }, 400)
    }

    if (!tier || !ALLOWED_TIERS.has(tier)) {
      return jsonResponse(
        { error: `tier must be one of: ${Array.from(ALLOWED_TIERS).join(', ')}` },
        400,
      )
    }

    const nowIso = new Date().toISOString()

    // Cancel any currently-active rows so the most-recent active row is the
    // one we're about to insert. The client tier resolver picks the latest
    // active row by started_at, but pruning here keeps the table tidy and
    // matches the eventual webhook-driven shape.
    const { error: cancelError } = await adminClient
      .from('subscriptions')
      .update({ status: 'canceled', ended_at: nowIso })
      .eq('user_id', userId)
      .eq('status', 'active')
    if (cancelError) {
      console.error('Cancel prior subscriptions failed:', cancelError.message)
      return jsonResponse({ error: cancelError.message }, 500)
    }

    const { data, error: insertError } = await adminClient
      .from('subscriptions')
      .insert({
        user_id: userId,
        tier,
        status: 'active',
        source: 'manual',
      })
      .select('id, tier, status, started_at')
      .single()
    if (insertError) {
      console.error('Insert subscription failed:', insertError.message)
      return jsonResponse({ error: insertError.message }, 500)
    }

    console.log(`set-subscription-tier: user=${userId} tier=${tier}`)
    return jsonResponse({ success: true, subscription: data })
  })
})
