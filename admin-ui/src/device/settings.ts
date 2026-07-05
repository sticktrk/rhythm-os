import type { DeviceClient } from './client';

export function getSettings(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/settings');
}

export function setSettings(
  client: DeviceClient,
  settings: Record<string, unknown>
) {
  return client.put('api/settings', { body: settings });
}

export function getLightBreaker(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/light-breaker');
}

export function setLightBreaker(client: DeviceClient, enabled: boolean) {
  return client.put('api/light-breaker', { body: { enabled } });
}

export function getHistory(
  client: DeviceClient,
  filters: {
    limit?: number;
    area?: string;
    source?: string;
    action?: string;
  } = {}
) {
  const query: Record<string, string> = {};
  if (filters.limit !== undefined) query.limit = String(filters.limit);
  if (filters.area) query.area = filters.area;
  if (filters.source) query.source = filters.source;
  if (filters.action) query.action = filters.action;
  return client.get<Record<string, unknown>>('api/history', { query });
}

export function getActivityCloudConfig(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/activity-cloud/config');
}

export function setActivityCloudConfig(
  client: DeviceClient,
  config: Record<string, unknown>
) {
  return client.put('api/activity-cloud/config', { body: config });
}

export function deleteActivityCloudConfig(client: DeviceClient) {
  return client.delete('api/activity-cloud/config');
}
