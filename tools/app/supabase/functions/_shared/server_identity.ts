export type ServerIdentityKind = 'provisional' | 'durable'

export type ServerIdentityResolution =
  | {
    status: 'ok'
    value: string | null
    changed: boolean
  }
  | {
    status: 'conflict'
    existing: string
    candidate: string
  }

export function normalizeServerIdentity(value: unknown): string | null {
  if (typeof value !== 'string') return null
  const clean = value.trim().toLowerCase()
  return clean ? clean.slice(0, 256) : null
}

export function serverIdentityKind(
  value: unknown,
): ServerIdentityKind | null {
  const clean = normalizeServerIdentity(value)
  if (!clean) return null
  return clean.startsWith('endpoint:') ? 'provisional' : 'durable'
}

export function isDurableRhythmServerIdentity(value: unknown): boolean {
  return normalizeServerIdentity(value)?.startsWith('srv-') === true
}

export function resolveServerIdentity(
  existingValue: unknown,
  candidateValue: unknown,
): ServerIdentityResolution {
  const existing = normalizeServerIdentity(existingValue)
  const candidate = normalizeServerIdentity(candidateValue)
  if (!candidate) return { status: 'ok', value: existing, changed: false }
  if (!existing) return { status: 'ok', value: candidate, changed: true }
  if (existing === candidate) {
    return { status: 'ok', value: existing, changed: false }
  }

  const existingKind = serverIdentityKind(existing)
  const candidateKind = serverIdentityKind(candidate)
  if (existingKind === 'durable' && candidateKind === 'provisional') {
    return { status: 'ok', value: existing, changed: false }
  }
  if (existingKind === 'durable' && candidateKind === 'durable') {
    return { status: 'conflict', existing, candidate }
  }
  return { status: 'ok', value: candidate, changed: true }
}
