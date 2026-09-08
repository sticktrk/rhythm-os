import { deepStrictEqual } from 'node:assert/strict'
import { retiredSubscriptionResponse } from './response.ts'

Deno.test('legacy subscription requests are retired without credentials or database access', async () => {
  for (const tier of ['basic', 'pro']) {
    const response = retiredSubscriptionResponse(new Request('https://example.test', {
      method: 'POST', body: JSON.stringify({ tier }),
    }))
    deepStrictEqual(response.status, 410)
    deepStrictEqual(await response.json(), { error: 'Subscription plans are unavailable.' })
  }
})

Deno.test('retired endpoint still supports browser preflight', () => {
  const response = retiredSubscriptionResponse(new Request('https://example.test', { method: 'OPTIONS' }))
  deepStrictEqual(response.status, 200)
  deepStrictEqual(response.headers.get('Access-Control-Allow-Origin'), '*')
})
