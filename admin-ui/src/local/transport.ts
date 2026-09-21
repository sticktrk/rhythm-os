import { runGuardedDeviceOperation } from '../api';
import { DeviceClient } from '../device/client';
import type { DeviceAdminProxyResponse } from '../types';

export function ingressBasePath(value: string | null): string {
  if (!value || value === '__RHYTHM_BASE__') return '/';
  if (!/^\/(?:[A-Za-z0-9_-]+\/)*$/.test(value)) throw new Error('Invalid Home Assistant Ingress path.');
  return value;
}

export function localApiUrl(path: string): string {
  const base = ingressBasePath(document.querySelector('base')?.getAttribute('href') ?? null);
  return `${base}${path}`;
}

export async function localFetch<T>(path: string, body?: unknown): Promise<T> {
  const response = await fetch(localApiUrl(path), {
    method: body === undefined ? 'GET' : 'POST',
    credentials: 'same-origin',
    headers: {'Accept': 'application/json', 'X-Rhythm-Local-Request': '1',
      ...(body === undefined ? {} : {'Content-Type': 'application/json'})},
    ...(body === undefined ? {} : {body: JSON.stringify(body)})
  });
  const result = await response.json();
  if (!response.ok) throw new Error(result.error ?? `Rhythm returned HTTP ${response.status}.`);
  return result as T;
}

export function createLocalDeviceClient() {
  return new DeviceClient((request) => runGuardedDeviceOperation(request,
    (operation) => localFetch<DeviceAdminProxyResponse>('api/local/device-admin/proxy', operation)));
}
