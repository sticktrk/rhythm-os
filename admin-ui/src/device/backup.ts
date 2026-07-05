import type { DeviceClient } from './client';

export function getProfileBundle(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/profile-bundle');
}

export function getProfileBundleFactoryDefault(client: DeviceClient) {
  return client.get<Record<string, unknown>>(
    'api/profile-bundle/factory-default'
  );
}

export function putProfileBundle(
  client: DeviceClient,
  bundle: Record<string, unknown>
) {
  return client.put('api/profile-bundle', { body: bundle });
}

export function resetProfileBundle(client: DeviceClient) {
  return client.post('api/profile-bundle/reset');
}

export function getShareBundle(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/share-bundle');
}

export function getShareBundleFactoryDefault(client: DeviceClient) {
  return client.get<Record<string, unknown>>(
    'api/share-bundle/factory-default'
  );
}

export function putShareBundle(
  client: DeviceClient,
  bundle: Record<string, unknown>
) {
  return client.put('api/share-bundle', { body: bundle });
}

export function resetShareBundle(client: DeviceClient) {
  return client.post('api/share-bundle/reset');
}

export function getBackup(client: DeviceClient, includeSecrets = false) {
  return client.get<Record<string, unknown>>('api/backup', {
    query: includeSecrets ? { include_secrets: 'true' } : undefined,
    timeoutSeconds: 60
  });
}

export function restoreBackup(
  client: DeviceClient,
  backup: Record<string, unknown>
) {
  return client.put('api/backup', { body: backup, timeoutSeconds: 120 });
}
