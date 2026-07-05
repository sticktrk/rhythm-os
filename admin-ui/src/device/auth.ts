import type { DeviceClient } from './client';

export function getAuthStatus(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/auth/status');
}

export function claimOwner(client: DeviceClient, label: string) {
  return client.post('api/auth/claim', { body: { label } });
}

export function createSupportToken(client: DeviceClient, label: string) {
  return client.post('api/auth/support-token', { body: { label } });
}

export function setAuthSettings(
  client: DeviceClient,
  options: { requireApiAuth: boolean; label?: string }
) {
  return client.put('api/auth/settings', {
    body: {
      require_api_auth: options.requireApiAuth,
      ...(options.label ? { label: options.label } : {})
    }
  });
}

export function getRemoteAccessStatus(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/remote-access/status');
}

export function setRemoteAccessConfig(
  client: DeviceClient,
  config: Record<string, unknown>
) {
  return client.put('api/remote-access/config', { body: config });
}

export function deleteRemoteAccessConfig(client: DeviceClient) {
  return client.delete('api/remote-access/config');
}
