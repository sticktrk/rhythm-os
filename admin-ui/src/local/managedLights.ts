// Revoking ownership does not require a usable current device identity.
export function canToggleManagedLight(selected: boolean, snapshotReady: boolean, reviewable: boolean): boolean {
  return selected || (snapshotReady && reviewable);
}

export function canSaveLightSelection(
  draft: readonly string[] | null,
  expected: readonly string[] | null,
  revision: string | null,
  snapshotReady: boolean
): boolean {
  if (!draft || !expected || !revision) return false;
  return snapshotReady || draft.every(id => expected.includes(id));
}
