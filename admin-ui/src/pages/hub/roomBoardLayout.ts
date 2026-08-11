import {
  isBulbKind,
  type TopologyItem
} from './topologyMembership.ts';

export type RoomDeviceSectionId =
  | 'lights'
  | 'motion-sensors'
  | 'controls'
  | 'other';

export type RoomDeviceSection = {
  id: RoomDeviceSectionId;
  label: string;
  items: TopologyItem[];
};

const sectionDefinitions: Array<{
  id: RoomDeviceSectionId;
  label: string;
}> = [
  { id: 'lights', label: 'Lights' },
  { id: 'motion-sensors', label: 'Motion & sensors' },
  { id: 'controls', label: 'Controls' },
  { id: 'other', label: 'Other devices' }
];

function normalizedKind(kind: string | undefined): string {
  return kind?.trim().toLowerCase().replace(/[\s-]+/g, '_') ?? '';
}

export function roomDeviceSectionId(
  kind: string | undefined
): RoomDeviceSectionId {
  if (isBulbKind(kind)) return 'lights';

  switch (normalizedKind(kind)) {
    case 'motion':
    case 'motion_sensor':
    case 'occupancy_sensor':
    case 'sensor':
      return 'motion-sensors';
    case 'button':
    case 'controller':
    case 'remote':
    case 'switch':
    case 'switch_device':
      return 'controls';
    default:
      return 'other';
  }
}

export function segmentRoomDevices(
  children: TopologyItem[]
): RoomDeviceSection[] {
  return sectionDefinitions
    .map((section) => ({
      ...section,
      items: children.filter(
        (child) => roomDeviceSectionId(child.kind) === section.id
      )
    }))
    .filter((section) => section.items.length > 0);
}

export function roomOrderStorageKey(hubId: string): string {
  return `rhythm-admin:room-board-order:v1:${hubId}`;
}

export function parseStoredRoomOrder(raw: string | null): string[] {
  if (!raw) return [];
  try {
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed)
      ? parsed.filter(
          (value, index): value is string =>
            typeof value === 'string' &&
            value.trim() !== '' &&
            parsed.indexOf(value) === index
        )
      : [];
  } catch {
    return [];
  }
}

export function reconcileRoomOrder(
  roomIds: string[],
  preferredOrder: string[]
): string[] {
  const available = new Set(roomIds);
  const reconciled = preferredOrder.filter(
    (id, index) => available.has(id) && preferredOrder.indexOf(id) === index
  );
  const included = new Set(reconciled);
  for (const roomId of roomIds) {
    if (!included.has(roomId)) {
      reconciled.push(roomId);
      included.add(roomId);
    }
  }
  return reconciled;
}

export function moveRoomBefore(
  order: string[],
  movedRoomId: string,
  targetRoomId: string
): string[] {
  if (movedRoomId === targetRoomId) return order;
  if (!order.includes(movedRoomId) || !order.includes(targetRoomId)) return order;

  const next = order.filter((roomId) => roomId !== movedRoomId);
  const targetIndex = next.indexOf(targetRoomId);
  next.splice(targetIndex, 0, movedRoomId);
  return next;
}

export function moveRoomByOffset(
  order: string[],
  roomId: string,
  offset: -1 | 1
): string[] {
  const currentIndex = order.indexOf(roomId);
  const targetIndex = currentIndex + offset;
  if (
    currentIndex < 0 ||
    targetIndex < 0 ||
    targetIndex >= order.length
  ) {
    return order;
  }
  const next = [...order];
  [next[currentIndex], next[targetIndex]] = [next[targetIndex], next[currentIndex]];
  return next;
}
