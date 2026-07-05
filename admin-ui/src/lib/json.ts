export function parseQueryText(text: string): Record<string, string> {
  const trimmed = text.trim();
  if (!trimmed) return {};
  const parsed = JSON.parse(trimmed) as unknown;
  if (!isPlainObject(parsed)) {
    throw new Error('Query JSON must be an object.');
  }
  return Object.fromEntries(
    Object.entries(parsed)
      .filter(([, value]) => value !== undefined && value !== null)
      .map(([key, value]) => [key, String(value)])
  );
}

export function parseOptionalJson(text: string, label: string): unknown {
  const trimmed = text.trim();
  if (!trimmed) return undefined;
  try {
    return JSON.parse(trimmed) as unknown;
  } catch (error) {
    throw new Error(
      `${label} JSON is invalid: ${
        error instanceof Error ? error.message : String(error)
      }`
    );
  }
}

export function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function prettyJson(value: unknown): string {
  const text = JSON.stringify(value, null, 2);
  return text === undefined ? 'null' : text;
}
