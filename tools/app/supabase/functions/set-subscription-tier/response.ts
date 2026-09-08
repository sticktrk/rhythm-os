const headers = {
  'Access-Control-Allow-Origin': '*',
  'Access-Control-Allow-Methods': 'POST, OPTIONS',
  'Access-Control-Allow-Headers': 'authorization, x-client-info, apikey, content-type',
  'Content-Type': 'application/json',
}

export function retiredSubscriptionResponse(req: Request): Response {
  if (req.method === 'OPTIONS') return new Response(null, { headers })
  return new Response(JSON.stringify({ error: 'Subscription plans are unavailable.' }), {
    status: 410,
    headers,
  })
}
