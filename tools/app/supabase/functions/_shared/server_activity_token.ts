export const SERVER_ACTIVITY_TOKEN_ACTIVATION_TTL_MS = 15 * 60 * 1000

export type ServerActivityTokenState = {
  revoked_at?: string | null
  activated_at?: string | null
  activation_expires_at?: string | null
}

export function pendingServerActivityTokenExpiryIso(
  nowEpochMs = Date.now(),
): string {
  return new Date(
    nowEpochMs + SERVER_ACTIVITY_TOKEN_ACTIVATION_TTL_MS,
  ).toISOString()
}

export function serverActivityTokenNeedsActivation(
  token: ServerActivityTokenState,
): boolean {
  return !token.activated_at
}

export function serverActivityTokenCanAuthenticate(
  token: ServerActivityTokenState,
  nowEpochMs = Date.now(),
): boolean {
  if (token.revoked_at) return false
  if (!serverActivityTokenNeedsActivation(token)) return true

  const expiresAt = Date.parse(token.activation_expires_at ?? '')
  return Number.isFinite(expiresAt) && expiresAt > nowEpochMs
}
