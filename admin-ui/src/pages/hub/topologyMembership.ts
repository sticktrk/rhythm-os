export type TopologyItem = {
  id: string;
  name: string;
  kind?: string;
  parentId?: string;
  raw: Record<string, unknown>;
};

export type RoomMembership = {
  room: TopologyItem;
  children: TopologyItem[];
  bulbs: TopologyItem[];
};

export type TopologyMembership = {
  rooms: RoomMembership[];
  unassigned: TopologyItem[];
  unassignedBulbs: TopologyItem[];
};

export type TopologyGroup<T> = {
  id: string;
  label: string;
  items: T[];
};

type TopologyItemLike = {
  id: string;
  name: string;
  kind?: string;
  parentId?: string;
};

function recordOf(value: unknown): Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

function stringOf(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() !== ''
    ? value.trim()
    : undefined;
}

function normalizedKind(kind: string | undefined): string | undefined {
  return kind?.trim().toLowerCase().replace(/[\s-]+/g, '_');
}

function compareItems(left: TopologyItemLike, right: TopologyItemLike): number {
  return left.name.localeCompare(right.name) || left.id.localeCompare(right.id);
}

export function isRoomKind(kind: string | undefined): boolean {
  return ['room', 'area', 'zone'].includes(normalizedKind(kind) ?? '');
}

export function isBulbKind(kind: string | undefined): boolean {
  return ['light_device', 'light', 'bulb'].includes(normalizedKind(kind) ?? '');
}

function rawNodesFromPayload(payload: unknown): Array<{
  raw: Record<string, unknown>;
  fallbackKind?: string;
}> {
  if (Array.isArray(payload)) {
    return payload.map((value) => ({ raw: recordOf(value) }));
  }

  const record = recordOf(payload);
  if (Array.isArray(record.nodes)) {
    return record.nodes.map((value) => ({ raw: recordOf(value) }));
  }
  if (Array.isArray(record.states)) {
    return record.states.map((value) => ({ raw: recordOf(value) }));
  }
  if (Array.isArray(record.rooms)) {
    return record.rooms.map((value) => ({
      raw: recordOf(value),
      fallbackKind: 'room'
    }));
  }
  return [];
}

export function topologyItemsFromPayload(payload: unknown): TopologyItem[] {
  return rawNodesFromPayload(payload)
    .map(({ raw, fallbackKind }): TopologyItem | null => {
      const id =
        stringOf(raw.id) ?? stringOf(raw.node_id) ?? stringOf(raw.room_id);
      if (!id) return null;
      return {
        id,
        name: stringOf(raw.name) ?? stringOf(raw.label) ?? id,
        kind:
          stringOf(raw.kind) ?? stringOf(raw.node_kind) ?? stringOf(raw.type) ?? fallbackKind,
        parentId:
          stringOf(raw.parent_id) ?? stringOf(raw.room_id) ?? undefined,
        raw
      };
    })
    .filter((item): item is TopologyItem => item !== null);
}

export function buildTopologyMembership(payload: unknown): TopologyMembership {
  const items = topologyItemsFromPayload(payload);
  const rooms = items
    .filter((item) => isRoomKind(item.kind))
    .sort(compareItems);
  const roomIds = new Set(rooms.map((room) => room.id));
  const childrenByRoom = new Map<string, TopologyItem[]>();
  const unassigned: TopologyItem[] = [];

  for (const item of items) {
    if (isRoomKind(item.kind)) continue;
    if (item.parentId && roomIds.has(item.parentId)) {
      const children = childrenByRoom.get(item.parentId) ?? [];
      children.push(item);
      childrenByRoom.set(item.parentId, children);
    } else {
      unassigned.push(item);
    }
  }

  const memberships = rooms.map((room): RoomMembership => {
    const children = (childrenByRoom.get(room.id) ?? []).sort(compareItems);
    return {
      room,
      children,
      bulbs: children.filter((child) => isBulbKind(child.kind))
    };
  });

  return {
    rooms: memberships,
    unassigned,
    unassignedBulbs: unassigned.filter((item) => isBulbKind(item.kind))
  };
}

export function groupTopologyItems<T extends TopologyItemLike>(
  items: T[]
): TopologyGroup<T>[] {
  const rooms = items
    .filter((item) => isRoomKind(item.kind))
    .sort(compareItems);
  const roomIds = new Set(rooms.map((room) => room.id));
  const assignedIds = new Set<string>();
  const groups: TopologyGroup<T>[] = rooms.map((room) => {
    const children = items
      .filter((item) => !isRoomKind(item.kind) && item.parentId === room.id)
      .sort(compareItems);
    for (const child of children) assignedIds.add(child.id);
    return {
      id: room.id,
      label: room.name,
      items: [room, ...children]
    };
  });

  const standalone = items
    .filter(
      (item) => !roomIds.has(item.id) && !assignedIds.has(item.id)
    )
    .sort(compareItems);
  if (standalone.length > 0) {
    groups.push({
      id: 'unassigned',
      label: 'Unassigned / standalone',
      items: standalone
    });
  }
  return groups;
}
