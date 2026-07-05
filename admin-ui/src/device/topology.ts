import type { DeviceClient } from './client';

export function listCanonicalDevices(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/devices/canonical');
}

export function getCanonicalDevice(client: DeviceClient, deviceId: string) {
  return client.get<Record<string, unknown>>(
    `api/devices/canonical/${encodeURIComponent(deviceId)}`
  );
}

export function renameCanonicalDevice(
  client: DeviceClient,
  deviceId: string,
  name: string
) {
  return client.put(`api/devices/canonical/${encodeURIComponent(deviceId)}`, {
    body: { name }
  });
}

export function flashCanonicalDevice(client: DeviceClient, deviceId: string) {
  return client.post(
    `api/devices/canonical/${encodeURIComponent(deviceId)}/flash`,
    { timeoutSeconds: 30 }
  );
}

export function setCanonicalDeviceParent(
  client: DeviceClient,
  deviceId: string,
  parentId: string | null
) {
  return client.put(
    `api/devices/canonical/${encodeURIComponent(deviceId)}/parent`,
    { body: { parent_id: parentId } }
  );
}

export function pairDevice(
  client: DeviceClient,
  options: {
    hubType: string;
    sessionId?: string;
    params: Record<string, unknown>;
  }
) {
  return client.post('api/devices/pair', {
    body: {
      hub_type: options.hubType,
      ...(options.sessionId ? { session_id: options.sessionId } : {}),
      params: options.params
    },
    timeoutSeconds: 120
  });
}

export function unpairDevice(
  client: DeviceClient,
  options: { hubType: string; deviceId: string; force?: boolean }
) {
  return client.post('api/devices/unpair', {
    body: {
      hub_type: options.hubType,
      params: { device_id: options.deviceId, force: options.force ?? false }
    },
    timeoutSeconds: 60
  });
}

export function getTopologyNodes(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/topology/nodes');
}

export function createRoom(client: DeviceClient, name: string) {
  return client.post('api/topology/rooms', { body: { name } });
}

export function renameRoom(
  client: DeviceClient,
  roomId: string,
  name: string
) {
  return client.put(`api/topology/rooms/${encodeURIComponent(roomId)}`, {
    body: { name }
  });
}

export function deleteRoom(client: DeviceClient, roomId: string) {
  return client.delete(`api/topology/rooms/${encodeURIComponent(roomId)}`);
}

export function mergeRooms(
  client: DeviceClient,
  targetRoomId: string,
  sourceRoomId: string
) {
  return client.put(
    `api/topology/rooms/${encodeURIComponent(targetRoomId)}/merge`,
    { body: { source_id: sourceRoomId } }
  );
}

export function moveDeviceToRoom(
  client: DeviceClient,
  toRoomId: string,
  deviceId: string,
  fromRoom?: string
) {
  return client.put(
    `api/topology/rooms/${encodeURIComponent(toRoomId)}/devices/move`,
    {
      body: {
        device_id: deviceId,
        ...(fromRoom ? { from_room: fromRoom } : {})
      }
    }
  );
}

export function setNodeControlTarget(
  client: DeviceClient,
  nodeId: string,
  controlKind: string,
  targetId: string
) {
  return client.put(
    `api/topology/nodes/${encodeURIComponent(nodeId)}/controls/${encodeURIComponent(controlKind)}`,
    { body: { target_id: targetId } }
  );
}

export function listTriage(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/triage');
}

export function getTriageCount(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/triage/count');
}

export function resolveTriage(
  client: DeviceClient,
  entryId: string,
  verb: 'merge' | 'new' | 'dismiss' | 'bind' | 'room',
  body?: Record<string, unknown>
) {
  return client.put(
    `api/triage/${encodeURIComponent(entryId)}/${verb}`,
    body === undefined ? undefined : { body }
  );
}

export function syncAll(client: DeviceClient) {
  return client.post('api/sync', { timeoutSeconds: 60 });
}

export function listMatterCaptures(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/matter/captures');
}

export function getMatterCapture(client: DeviceClient, captureId: string) {
  return client.get<Record<string, unknown>>(
    `api/matter/captures/${encodeURIComponent(captureId)}`
  );
}

export function runBulbTest(
  client: DeviceClient,
  deviceId: string,
  test: string
) {
  return client.post('api/matter/bulb-test/run', {
    body: { device_id: deviceId, test },
    timeoutSeconds: 60
  });
}

export function reportBulbTest(
  client: DeviceClient,
  body: Record<string, unknown>
) {
  return client.post('api/matter/bulb-test/report', { body });
}

export function putHubCredentials(
  client: DeviceClient,
  options: {
    hubType: string;
    address: string;
    credentials: Record<string, unknown>;
  }
) {
  return client.put('api/hub/credentials', {
    body: {
      hub_type: options.hubType,
      address: options.address,
      credentials: options.credentials
    }
  });
}

export function deleteHubCredentials(
  client: DeviceClient,
  options: { hubType?: string; address?: string } = {}
) {
  const query: Record<string, string> = {};
  if (options.hubType) query.hub_type = options.hubType;
  if (options.address) query.address = options.address;
  return client.delete('api/hub/credentials', { query });
}

export function retryHub(
  client: DeviceClient,
  hubType: string,
  address: string
) {
  return client.post('api/hub/retry', {
    body: { hub_type: hubType, address }
  });
}

export function getWifi(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/wifi');
}

export function setWifi(client: DeviceClient, ssid: string, password: string) {
  return client.put('api/wifi', { body: { ssid, password } });
}

export function resetWifi(client: DeviceClient) {
  return client.delete('api/wifi');
}
