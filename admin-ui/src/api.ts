import type { MeResponse, ProbeResult, SupportSnapshot } from './types';

const configuredBaseUrl = import.meta.env.VITE_ADMIN_API_URL?.trim();
const apiBaseUrl =
  configuredBaseUrl && configuredBaseUrl.length > 0
    ? configuredBaseUrl.replace(/\/+$/, '')
    : 'http://127.0.0.1:8787';

export async function fetchMe(accessToken: string): Promise<MeResponse> {
  return apiFetch<MeResponse>('/api/me', accessToken);
}

export async function fetchSupportSnapshot(
  accessToken: string
): Promise<SupportSnapshot> {
  return apiFetch<SupportSnapshot>('/api/support/snapshot', accessToken);
}

export async function probeHub(
  accessToken: string,
  hubId: string
): Promise<ProbeResult> {
  return apiFetch<ProbeResult>(
    `/api/hubs/${encodeURIComponent(hubId)}/probe`,
    accessToken,
    { method: 'POST' }
  );
}

async function apiFetch<T>(
  path: string,
  accessToken: string,
  init: RequestInit = {}
): Promise<T> {
  const response = await fetch(`${apiBaseUrl}${path}`, {
    ...init,
    headers: {
      Accept: 'application/json',
      Authorization: `Bearer ${accessToken}`,
      ...(init.body ? { 'Content-Type': 'application/json' } : {}),
      ...init.headers
    }
  });
  const contentType = response.headers.get('content-type') ?? '';
  const body = contentType.includes('application/json')
    ? await response.json()
    : await response.text();

  if (!response.ok) {
    const message =
      typeof body === 'object' && body !== null && 'error' in body
        ? String(body.error)
        : `Request failed with HTTP ${response.status}.`;
    throw new Error(message);
  }
  return body as T;
}
