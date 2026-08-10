import type { DeviceRequestOptions } from '../../device/client';
import type { DeviceAdminProxyResponse } from '../../types';
import {
  buildTopologyMembership,
  topologyItemsFromPayload,
  type TopologyItem,
  type TopologyMembership
} from './topologyMembership.ts';

export type RoomMoveProposal = {
  deviceId: string;
  deviceName: string;
  deviceKind?: string;
  fromRoomId: string;
  fromRoomName: string;
  toRoomId: string;
  toRoomName: string;
};

export type RoomMoveState = {
  deviceId: string;
  deviceName: string;
  roomId: string | null;
  roomName: string;
};

export type RoomMoveReceipt = {
  status: 'pending' | 'confirmed' | 'conflict';
  requestId: string;
  before: RoomMoveState;
  candidate: RoomMoveState;
  after?: RoomMoveState;
  proxy: {
    route?: 'remote' | 'local';
    completedAt: string;
    statusCode: number;
    verifiedServerInstanceId?: string;
    preconditionBodySha256?: string;
    responseBodySha256?: string;
  };
};

export type RoomMoveClient = {
  getReceipt(
    path: string,
    options?: DeviceRequestOptions
  ): Promise<DeviceAdminProxyResponse>;
  putReceipt(
    path: string,
    options?: DeviceRequestOptions
  ): Promise<DeviceAdminProxyResponse>;
};

export function createRoomMoveRequestId(): string {
  return `admin-room-move:${crypto.randomUUID()}`;
}

export function buildRoomMoveProposal(
  membership: TopologyMembership,
  deviceId: string,
  toRoomId: string
): RoomMoveProposal | null {
  const target = membership.rooms.find(({ room }) => room.id === toRoomId)?.room;
  const source = membership.rooms.find(({ children }) =>
    children.some((child) => child.id === deviceId)
  );
  const device = source?.children.find((child) => child.id === deviceId);
  if (!target || !source || !device || source.room.id === target.id) return null;
  return {
    deviceId: device.id,
    deviceName: device.name,
    deviceKind: device.kind,
    fromRoomId: source.room.id,
    fromRoomName: source.room.name,
    toRoomId: target.id,
    toRoomName: target.name
  };
}

function roomNameFor(
  membership: TopologyMembership,
  roomId: string | undefined
): string {
  if (!roomId) return 'Unassigned';
  return (
    membership.rooms.find(({ room }) => room.id === roomId)?.room.name ?? roomId
  );
}

function stateFor(
  item: TopologyItem | undefined,
  membership: TopologyMembership,
  fallback: Pick<RoomMoveProposal, 'deviceId' | 'deviceName'>
): RoomMoveState {
  return {
    deviceId: item?.id ?? fallback.deviceId,
    deviceName: item?.name ?? fallback.deviceName,
    roomId: item?.parentId ?? null,
    roomName: item ? roomNameFor(membership, item.parentId) : 'Missing from topology'
  };
}

function proxySummary(proxy: DeviceAdminProxyResponse): RoomMoveReceipt['proxy'] {
  return {
    route: proxy.route,
    completedAt: proxy.completedAt,
    statusCode: proxy.statusCode,
    verifiedServerInstanceId: proxy.verifiedServerInstanceId,
    preconditionBodySha256: proxy.preconditionBodySha256,
    responseBodySha256: proxy.bodySha256
  };
}

export async function moveDeviceBetweenRoomsVerified(
  client: RoomMoveClient,
  proposal: RoomMoveProposal,
  options: {
    requestId: string;
    onAccepted?: (receipt: RoomMoveReceipt) => void;
  }
): Promise<RoomMoveReceipt> {
  if (proposal.fromRoomId === proposal.toRoomId) {
    throw new Error('The device is already assigned to that room; no write was sent.');
  }

  const snapshot = await client.getReceipt('api/topology/nodes', {
    requestId: options.requestId
  });
  if (!snapshot.bodySha256) {
    throw new Error(
      'The admin API did not provide a topology freshness hash; no write was sent.'
    );
  }

  const latestMembership = buildTopologyMembership(snapshot.body);
  const latestItems = topologyItemsFromPayload(snapshot.body);
  const latestDevice = latestItems.find((item) => item.id === proposal.deviceId);
  const latestSource = latestMembership.rooms.find(
    ({ room }) => room.id === proposal.fromRoomId
  )?.room;
  const latestTarget = latestMembership.rooms.find(
    ({ room }) => room.id === proposal.toRoomId
  )?.room;

  if (!latestDevice) {
    throw new Error(`${proposal.deviceName} is no longer present; no write was sent.`);
  }
  if (!latestSource || !latestTarget) {
    throw new Error('The source or destination room no longer exists; no write was sent.');
  }
  if (latestDevice.parentId !== proposal.fromRoomId) {
    throw new Error(
      `${proposal.deviceName} moved after this proposal was opened; no write was sent.`
    );
  }

  const before: RoomMoveState = {
    deviceId: latestDevice.id,
    deviceName: latestDevice.name,
    roomId: latestSource.id,
    roomName: latestSource.name
  };
  const candidate: RoomMoveState = {
    deviceId: latestDevice.id,
    deviceName: latestDevice.name,
    roomId: latestTarget.id,
    roomName: latestTarget.name
  };
  const correlation_id = options.requestId;
  const resourcePrecondition = {
    path: 'api/topology/nodes',
    bodySha256: snapshot.bodySha256
  };
  const proxy = await client.putReceipt(
    `api/topology/rooms/${encodeURIComponent(latestTarget.id)}/devices/move`,
    {
      body: {
        device_id: latestDevice.id,
        from_room: latestSource.id,
        correlation_id
      },
      requestId: options.requestId,
      resourcePrecondition
    }
  );

  const pending: RoomMoveReceipt = {
    status: 'pending',
    requestId: options.requestId,
    before,
    candidate,
    proxy: proxySummary(proxy)
  };
  options.onAccepted?.(pending);

  const readBack = await client.getReceipt('api/topology/nodes', {
    requestId: options.requestId
  });
  const afterMembership = buildTopologyMembership(readBack.body);
  const afterItem = topologyItemsFromPayload(readBack.body).find(
    (item) => item.id === proposal.deviceId
  );
  const after = stateFor(afterItem, afterMembership, proposal);
  return {
    ...pending,
    status: after.roomId === proposal.toRoomId ? 'confirmed' : 'conflict',
    after
  };
}
