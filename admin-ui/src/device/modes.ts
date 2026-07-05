import type { DeviceClient } from './client';

export function getMode(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/mode');
}

export function setMode(client: DeviceClient, body: Record<string, unknown>) {
  return client.put('api/mode', { body });
}

export function setActiveMode(client: DeviceClient, active: string) {
  return client.put('api/mode', { body: { active } });
}

export function getTransitions(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/transitions');
}

export function setTransitions(
  client: DeviceClient,
  transitions: unknown[]
) {
  return client.put('api/transitions', { body: { transitions } });
}

export function triggerTransition(client: DeviceClient, transitionId: string) {
  return client.post(
    `api/transitions/${encodeURIComponent(transitionId)}/trigger`
  );
}

export function getLightRuntime(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/light-runtime');
}

export function setLightRuntime(
  client: DeviceClient,
  runtimeId: string,
  transitionMs?: number
) {
  return client.put('api/light-runtime', {
    body: {
      runtime_id: runtimeId,
      ...(transitionMs !== undefined ? { transition_ms: transitionMs } : {})
    }
  });
}
