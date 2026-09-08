import { retiredSubscriptionResponse } from './response.ts'

// Retain the route for released clients without changing subscription rows.
Deno.serve(retiredSubscriptionResponse)
