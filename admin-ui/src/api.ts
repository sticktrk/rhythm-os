import type {
  AdminApiHealth,
  AdminApiReadiness,
  DebugBundleDownload,
  DeviceLogSources,
  DeviceLogTail,
  DeviceOtaAction,
  DeviceAdminProxyRequest,
  DeviceAdminProxyResponse,
  DeviceStatus,
  MeResponse,
  ProbeResult,
  SupportSnapshot
} from './types';

const configuredBaseUrl = import.meta.env.VITE_ADMIN_API_URL?.trim();
const apiBaseUrl =
  configuredBaseUrl && configuredBaseUrl.length > 0
    ? configuredBaseUrl.replace(/\/+$/, '')
    : 'http://127.0.0.1:8787';

export async function fetchMe(accessToken: string): Promise<MeResponse> {
  return apiFetch<MeResponse>('/api/me', accessToken);
}

export async function fetchHealth(): Promise<AdminApiHealth> {
  const response = await fetch(`${apiBaseUrl}/health`, {
    headers: { Accept: 'application/json' }
  });
  if (!response.ok) {
    throw new Error(await responseErrorMessage(response));
  }
  return (await response.json()) as AdminApiHealth;
}

export async function fetchReadiness(): Promise<AdminApiReadiness> {
  const response = await fetch(`${apiBaseUrl}/ready`, {
    headers: { Accept: 'application/json' }
  });
  const contentType = response.headers.get('content-type') ?? '';
  if (contentType.includes('application/json')) {
    return (await response.json()) as AdminApiReadiness;
  }
  if (!response.ok) {
    throw new Error(await responseErrorMessage(response));
  }
  throw new Error('admin-api returned an invalid readiness response.');
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

export async function fetchHubStatus(
  accessToken: string,
  hubId: string
): Promise<DeviceStatus> {
  return apiFetch<DeviceStatus>(
    `/api/hubs/${encodeURIComponent(hubId)}/status`,
    accessToken
  );
}

export async function checkHubUpdate(
  accessToken: string,
  hubId: string
): Promise<DeviceOtaAction> {
  return apiFetch<DeviceOtaAction>(
    `/api/hubs/${encodeURIComponent(hubId)}/ota/check`,
    accessToken,
    { method: 'POST' }
  );
}

export async function applyHubUpdate(
  accessToken: string,
  hubId: string
): Promise<DeviceOtaAction> {
  return apiFetch<DeviceOtaAction>(
    `/api/hubs/${encodeURIComponent(hubId)}/ota/update`,
    accessToken,
    { method: 'POST' }
  );
}

export async function downloadDebugBundle(
  accessToken: string,
  hubId: string
): Promise<DebugBundleDownload> {
  const response = await fetch(
    `${apiBaseUrl}/api/hubs/${encodeURIComponent(hubId)}/debug-bundle`,
    {
      method: 'POST',
      headers: {
        Accept: 'application/gzip,application/octet-stream,*/*',
        Authorization: `Bearer ${accessToken}`
      }
    }
  );

  if (!response.ok) {
    throw new Error(await responseErrorMessage(response));
  }

  return {
    blob: await response.blob(),
    fileName:
      fileNameFromContentDisposition(response.headers.get('content-disposition')) ??
      fallbackDebugBundleFileName(hubId),
    route: routeFromHeader(response.headers.get('x-rhythm-route')),
    baseUrl: response.headers.get('x-rhythm-base-url') ?? undefined
  };
}

export async function fetchHubLogSources(
  accessToken: string,
  hubId: string
): Promise<DeviceLogSources> {
  return apiFetch<DeviceLogSources>(
    `/api/hubs/${encodeURIComponent(hubId)}/logs`,
    accessToken
  );
}

export async function fetchHubLogTail(
  accessToken: string,
  hubId: string,
  sourceId: string,
  lines = 120
): Promise<DeviceLogTail> {
  const params = new URLSearchParams({ lines: String(lines) });
  return apiFetch<DeviceLogTail>(
    `/api/hubs/${encodeURIComponent(hubId)}/logs/${encodeURIComponent(
      sourceId
    )}/tail?${params}`,
    accessToken
  );
}

export async function runDeviceAdminProxy(
  accessToken: string,
  hubId: string,
  request: DeviceAdminProxyRequest
): Promise<DeviceAdminProxyResponse> {
  return apiFetch<DeviceAdminProxyResponse>(
    `/api/hubs/${encodeURIComponent(hubId)}/device-admin/proxy`,
    accessToken,
    {
      method: 'POST',
      body: JSON.stringify(request)
    }
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

async function responseErrorMessage(response: Response): Promise<string> {
  const contentType = response.headers.get('content-type') ?? '';
  if (contentType.includes('application/json')) {
    const body = await response.json();
    if (typeof body === 'object' && body !== null && 'error' in body) {
      return String(body.error);
    }
  } else {
    const body = await response.text();
    if (body.trim().length > 0) return body.trim();
  }
  return `Request failed with HTTP ${response.status}.`;
}

function fileNameFromContentDisposition(value: string | null): string | null {
  if (!value) return null;
  const starMatch = /filename\*=UTF-8''([^;]+)/i.exec(value);
  if (starMatch?.[1]) {
    return safeFileName(decodeURIComponent(starMatch[1]));
  }
  const quotedMatch = /filename="([^"]+)"/i.exec(value);
  if (quotedMatch?.[1]) return safeFileName(quotedMatch[1]);
  const bareMatch = /filename=([^;]+)/i.exec(value);
  return bareMatch?.[1] ? safeFileName(bareMatch[1]) : null;
}

function safeFileName(value: string): string | null {
  const safe = value
    .trim()
    .split(/[\\/]/)
    .pop()
    ?.replace(/[\u0000-\u001f\u007f"]/g, '')
    .trim();
  return safe && safe.length > 0 ? safe : null;
}

function fallbackDebugBundleFileName(hubId: string): string {
  const safeHubId = hubId.replace(/[^A-Za-z0-9_.-]+/g, '-');
  return `rhythm-debug-bundle-${safeHubId || 'hub'}.tar.gz`;
}

function routeFromHeader(value: string | null): 'remote' | 'local' | undefined {
  return value === 'remote' || value === 'local' ? value : undefined;
}
