import type { DeviceClient } from './client';

export function getOtaStatus(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/ota/status');
}

export function factoryReset(client: DeviceClient) {
  return client.post('api/factory-reset', { timeoutSeconds: 60 });
}

export function restartDevice(client: DeviceClient) {
  return client.post('api/restart', { timeoutSeconds: 30 });
}
