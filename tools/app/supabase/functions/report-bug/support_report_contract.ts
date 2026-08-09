export type SupportReportKind = 'bug' | 'feature'

const classificationLabels = new Set([
  'app bug report',
  'bug',
  'enhancement',
  'feature',
])

export function normalizeSupportReportKind(
  value: unknown,
): SupportReportKind {
  return value === 'feature' ? 'feature' : 'bug'
}

export function githubLabelsForSupportReport({
  kind,
  fleet,
  configuredLabels,
}: {
  kind: unknown
  fleet: boolean
  configuredLabels: string[]
}): string[] {
  const effectiveKind = fleet ? 'bug' : normalizeSupportReportKind(kind)
  const requestedLabels = effectiveKind === 'feature'
    ? ['enhancement', 'feature']
    : ['App Bug Report', 'bug']
  const safeConfiguredLabels = configuredLabels.filter(
    (label) => !classificationLabels.has(label.trim().toLowerCase()),
  )

  return dedupe([
    ...requestedLabels,
    'light-hub',
    ...(fleet ? ['fleet-detected'] : []),
    ...safeConfiguredLabels,
  ])
}

export function supportReportIssueCopy(
  kind: unknown,
  fleet: boolean,
): {
  kind: SupportReportKind
  titlePrefix: string
  intro: string
  summaryHeading: string
} {
  const effectiveKind = fleet ? 'bug' : normalizeSupportReportKind(kind)
  if (fleet) {
    return {
      kind: effectiveKind,
      titlePrefix: 'fleet-report: ',
      intro: 'Created automatically from the Rhythm fleet log triage flow.',
      summaryHeading: 'Detected Finding',
    }
  }
  if (effectiveKind === 'feature') {
    return {
      kind: effectiveKind,
      titlePrefix: 'app-feature: ',
      intro:
        'Created automatically from the Rhythm app Report / Request flow.',
      summaryHeading: 'Requested Feature',
    }
  }
  return {
    kind: effectiveKind,
    titlePrefix: 'app-report: ',
    intro: 'Created automatically from the Rhythm app Report / Request flow.',
    summaryHeading: 'User Summary',
  }
}

function dedupe(values: string[]): string[] {
  return Array.from(new Set(values))
}
