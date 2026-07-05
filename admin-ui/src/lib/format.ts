export function nonEmptyString(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim().length > 0
    ? value.trim()
    : undefined;
}

export function compactJoin(
  values: Array<string | undefined>,
  separator: string
): string | undefined {
  const present = values.filter((value): value is string => Boolean(value));
  return present.length === 0 ? undefined : present.join(separator);
}

export function yesNo(value: unknown): string | undefined {
  if (typeof value !== 'boolean') return undefined;
  return value ? 'yes' : 'no';
}

export function statusBool(value: unknown): string {
  if (typeof value !== 'boolean') return 'unknown';
  return value ? 'healthy' : 'unhealthy';
}

export function stringValue(value: unknown): string {
  if (value === undefined || value === null || value === '') return 'unknown';
  if (typeof value === 'number') return new Intl.NumberFormat().format(value);
  if (typeof value === 'boolean') return value ? 'yes' : 'no';
  return String(value);
}

export function formatEpochMs(value: number | undefined): string | undefined {
  if (typeof value !== 'number' || value <= 0) return undefined;
  return formatDateTime(new Date(value).toISOString());
}

export function shortId(value: string): string {
  return value.length <= 10 ? value : `${value.slice(0, 10)}...`;
}

export function formatDateTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit'
  }).format(date);
}

export function formatHour(hour: number): string {
  const normalized = ((hour % 24) + 24) % 24;
  const h = Math.floor(normalized);
  const m = Math.round((normalized - h) * 60);
  const clampedM = m >= 60 ? 0 : m;
  const displayH = m >= 60 ? (h + 1) % 24 : h;
  return `${String(displayH).padStart(2, '0')}:${String(clampedM).padStart(2, '0')}`;
}

export function formatDurationSecs(totalSecs: number): string {
  if (!Number.isFinite(totalSecs) || totalSecs < 0) return '—';
  if (totalSecs < 60) return `${Math.round(totalSecs)}s`;
  const mins = Math.floor(totalSecs / 60);
  if (mins < 60) {
    const secs = Math.round(totalSecs % 60);
    return secs > 0 ? `${mins}m ${secs}s` : `${mins}m`;
  }
  const hours = Math.floor(mins / 60);
  const remMins = mins % 60;
  return remMins > 0 ? `${hours}h ${remMins}m` : `${hours}h`;
}

export function triggerBrowserDownload(blob: Blob, fileName: string) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = fileName;
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(url);
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
