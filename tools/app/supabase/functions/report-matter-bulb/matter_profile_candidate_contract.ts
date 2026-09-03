export type PublishedMatterProfileReceipt = {
  profile_version?: number | null
  report_count?: number | null
  approved?: boolean | null
}

export function matterProfileCandidateIngestionPlan(
  profileKey: string,
  profilePayload: Record<string, unknown>,
  sourceReportId: string | null,
  existing: PublishedMatterProfileReceipt | null,
  latestCandidateVersion: number | null = null,
) {
  const profileVersion = finiteNumber(existing?.profile_version) ?? 0
  const reportCount = finiteNumber(existing?.report_count) ?? 0
  return {
    candidateInsert: {
      profile_key: profileKey,
      candidate_version:
        Math.max(profileVersion, finiteNumber(latestCandidateVersion) ?? 0) + 1,
      status: 'pending' as const,
      profile_payload: profilePayload,
      source_report_id: sourceReportId,
    },
    // Deliberately the only write to the curated row. In particular, report
    // ingestion never writes `approved` or replaces profile content.
    publishedCountUpdate: existing ? { report_count: reportCount + 1 } : null,
    approvedProfilePreserved: existing?.approved === true,
  }
}

function finiteNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}
