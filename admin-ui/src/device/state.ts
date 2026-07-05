import type { DeviceClient } from './client';

export function getHealth(client: DeviceClient) {
  return client.get<Record<string, unknown>>('health');
}

export function getState(client: DeviceClient, authoritative = false) {
  return client.get<Record<string, unknown>>('api/state', {
    query: authoritative ? { authoritative: 'true' } : undefined
  });
}

export function getNodesState(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/nodes/state');
}
