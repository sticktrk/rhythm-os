// Deployment-specific destinations must never default to the upstream operator.
export function remoteAccessDomain(value: string | undefined): string {
  const domain = value?.trim().replace(/^\.+|\.+$/g, '').toLowerCase()
  if (!domain) throw new Error('Missing RHYTHM_REMOTE_ACCESS_DOMAIN')
  if (domain.length > 253 || !domain.includes('.') ||
    !domain.split('.').every((label) =>
      /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(label))) {
    throw new Error('RHYTHM_REMOTE_ACCESS_DOMAIN must be a DNS domain without a scheme or path')
  }
  return domain
}

export function githubIssuesRepo(value: string | undefined): string {
  return value?.trim() ?? ''
}
