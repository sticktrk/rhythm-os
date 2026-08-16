export type ConfiguredIntegrationHub = {
  type: string;
  address: string;
  connected: boolean;
  raw: Record<string, unknown>;
};

// Hub details live at integrations/:type/:address, so returning to the
// configured-hub list must remove both dynamic path segments.
export const HUB_LIST_BACK_TARGET = '../..';

function record(value: unknown): Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

function records(value: unknown): Record<string, unknown>[] {
  return Array.isArray(value) ? value.map(record) : [];
}

function stringValue(value: unknown): string {
  return typeof value === 'string' ? value.trim() : '';
}

export function configuredHubsFromState(
  payload: unknown
): ConfiguredIntegrationHub[] {
  return records(record(payload).hubs)
    .map((raw) => ({
      type: stringValue(raw.type),
      address: stringValue(raw.address),
      connected: raw.connected === true,
      raw
    }))
    .filter((hub) => hub.type !== '' && hub.type !== 'none')
    .sort((left, right) =>
      `${left.type}\u0000${left.address}`.localeCompare(
        `${right.type}\u0000${right.address}`
      )
    );
}

export function canonicalDevicesFromPayload(
  payload: unknown
): Record<string, unknown>[] {
  if (Array.isArray(payload)) return records(payload);
  const root = record(payload);
  const devices = records(root.devices);
  return devices.length > 0 ? devices : records(root.items);
}

export function canonicalDevicesForHub(
  payload: unknown,
  hub: Pick<ConfiguredIntegrationHub, 'type' | 'address'>
): Record<string, unknown>[] {
  const expectedType = hub.type.toLowerCase();
  const expectedAddress = hub.address.toLowerCase();
  return canonicalDevicesFromPayload(payload).filter((device) =>
    records(device.endpoints).some((endpoint) => {
      const hubKey = record(endpoint.hub_key);
      const type = stringValue(hubKey.hub_type).toLowerCase();
      const address = stringValue(hubKey.address).toLowerCase();
      return (
        type === expectedType &&
        (expectedAddress === '' || address === expectedAddress)
      );
    })
  );
}

export function hueAuthorityBridgeForHub(
  payload: unknown,
  address: string
): Record<string, unknown> | null {
  const expected = address.trim().toLowerCase();
  const bridges = records(record(payload).bridges);
  if (expected === '') return bridges.length === 1 ? bridges[0] : null;
  return (
    bridges.find((bridge) => {
      const candidate = stringValue(bridge.address).toLowerCase();
      return (
        candidate === expected ||
        candidate.startsWith(`${expected}:`) ||
        expected.startsWith(`${candidate}:`)
      );
    }) ?? null
  );
}

export function hueAuthorityUpdateBody(options: {
  bridge: Record<string, unknown>;
  owners: Record<string, 'hue' | 'rhythm'>;
  correlationId: string;
  topologySyncEnabled?: boolean;
}): Record<string, unknown> {
  const rooms = records(options.bridge.rooms).flatMap((room) => {
    const roomId = stringValue(room.room_id);
    if (roomId === '') return [];
    return [
      {
        room_id: roomId,
        owner: options.owners[roomId] ?? 'hue'
      }
    ];
  });
  return {
    address: stringValue(options.bridge.address),
    revision: stringValue(options.bridge.revision),
    correlation_id: options.correlationId,
    ...(options.topologySyncEnabled === undefined
      ? {}
      : { topology_sync_enabled: options.topologySyncEnabled }),
    rooms
  };
}

export function hubDisplayName(type: string): string {
  switch (type) {
    case 'hue':
      return 'Philips Hue';
    case 'hue_ble':
      return 'Hue Bluetooth';
    case 'local_ble':
      return 'Local Bluetooth';
    case 'homeassistant':
    case 'home_assistant':
    case 'ha':
      return 'Home Assistant';
    case 'matter':
      return 'Matter';
    case 'zigbee':
      return 'Zigbee';
    default:
      return type
        .split('_')
        .filter(Boolean)
        .map((part) => `${part[0]?.toUpperCase() ?? ''}${part.slice(1)}`)
        .join(' ');
  }
}
