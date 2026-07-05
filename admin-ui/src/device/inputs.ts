import type { DeviceClient } from './client';

export function listInputBindings(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/input-bindings');
}

export function createInputBinding(
  client: DeviceClient,
  binding: Record<string, unknown>
) {
  return client.post('api/input-bindings', { body: binding });
}

export function updateInputBinding(
  client: DeviceClient,
  bindingId: string,
  binding: Record<string, unknown>
) {
  return client.put(`api/input-bindings/${encodeURIComponent(bindingId)}`, {
    body: binding
  });
}

export function deleteInputBinding(client: DeviceClient, bindingId: string) {
  return client.delete(
    `api/input-bindings/${encodeURIComponent(bindingId)}`
  );
}
