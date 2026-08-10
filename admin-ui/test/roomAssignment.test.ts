import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildRoomMoveProposal,
  createRoomMoveRequestId,
  moveDeviceBetweenRoomsVerified,
  type RoomMoveClient,
  type RoomMoveReceipt
} from '../src/pages/hub/roomAssignment.ts';
import { buildTopologyMembership } from '../src/pages/hub/topologyMembership.ts';
import type { DeviceAdminProxyResponse } from '../src/types.ts';

const beforeTopology = [
  { id: 'room-kitchen', name: 'Kitchen', kind: 'room' },
  { id: 'room-office', name: 'Office', kind: 'room' },
  {
    id: 'bulb-desk',
    name: 'Desk Bulb',
    kind: 'light_device',
    parent_id: 'room-kitchen'
  }
];

const afterTopology = beforeTopology.map((node) =>
  node.id === 'bulb-desk' ? { ...node, parent_id: 'room-office' } : node
);

function receipt(
  body: unknown,
  overrides: Partial<DeviceAdminProxyResponse> = {}
): DeviceAdminProxyResponse {
  return {
    hubId: 'hub-1',
    route: 'remote',
    baseUrl: 'https://device.invalid',
    method: 'GET',
    path: 'api/topology/nodes',
    statusCode: 200,
    completedAt: '2026-08-10T12:00:00Z',
    tokenAvailable: true,
    hasEncryptedToken: true,
    bodySha256: 'a'.repeat(64),
    body,
    ...overrides
  };
}

function proposal() {
  const result = buildRoomMoveProposal(
    buildTopologyMembership(beforeTopology),
    'bulb-desk',
    'room-office'
  );
  assert.ok(result);
  return result;
}

test('builds proposals only for a different known room', () => {
  const membership = buildTopologyMembership(beforeTopology);
  assert.equal(
    buildRoomMoveProposal(membership, 'bulb-desk', 'room-kitchen'),
    null
  );
  assert.equal(
    buildRoomMoveProposal(membership, 'missing-device', 'room-office'),
    null
  );
  assert.equal(
    buildRoomMoveProposal(membership, 'bulb-desk', 'missing-room'),
    null
  );
  assert.equal(proposal().fromRoomName, 'Kitchen');
});

test('uses a safe correlation id for the complete verified move', () => {
  assert.match(
    createRoomMoveRequestId(),
    /^admin-room-move:[0-9a-f-]{36}$/
  );
});

test('rejects a same-room move before any server call', async () => {
  let calls = 0;
  const sameRoom = {
    ...proposal(),
    toRoomId: 'room-kitchen',
    toRoomName: 'Kitchen'
  };
  const client: RoomMoveClient = {
    async getReceipt() {
      calls += 1;
      return receipt(beforeTopology);
    },
    async putReceipt() {
      calls += 1;
      return receipt(null);
    }
  };

  await assert.rejects(
    moveDeviceBetweenRoomsVerified(client, sameRoom, {
      requestId: 'admin-room-move:same-room'
    }),
    /already assigned to that room/
  );
  assert.equal(calls, 0);
});

test('rejects a topology snapshot without a freshness hash', async () => {
  let writes = 0;
  const client: RoomMoveClient = {
    async getReceipt() {
      return receipt(beforeTopology, { bodySha256: undefined });
    },
    async putReceipt() {
      writes += 1;
      return receipt(null);
    }
  };

  await assert.rejects(
    moveDeviceBetweenRoomsVerified(client, proposal(), {
      requestId: 'admin-room-move:no-hash'
    }),
    /did not provide a topology freshness hash/
  );
  assert.equal(writes, 0);
});

test('sends reviewed source and hash, then confirms from fresh topology', async () => {
  const requests: Array<{ method: string; path: string; options: unknown }> = [];
  const reads = [
    receipt(beforeTopology),
    receipt(afterTopology, { bodySha256: 'b'.repeat(64) })
  ];
  const client: RoomMoveClient = {
    async getReceipt(path, options) {
      requests.push({ method: 'GET', path, options });
      return reads.shift()!;
    },
    async putReceipt(path, options) {
      requests.push({ method: 'PUT', path, options });
      return receipt(null, {
        method: 'PUT',
        path,
        statusCode: 204,
        requestId: 'admin-room-move:test-1234',
        verifiedServerInstanceId: 'server-1',
        preconditionBodySha256: 'a'.repeat(64),
        bodySha256: undefined
      });
    }
  };
  let pending: RoomMoveReceipt | undefined;

  const result = await moveDeviceBetweenRoomsVerified(client, proposal(), {
    requestId: 'admin-room-move:test-1234',
    onAccepted: (next) => {
      pending = next;
    }
  });

  assert.equal(pending?.status, 'pending');
  assert.equal(result.status, 'confirmed');
  assert.equal(result.before.roomName, 'Kitchen');
  assert.equal(result.candidate.roomName, 'Office');
  assert.equal(result.after?.roomId, 'room-office');
  assert.deepEqual(requests[1], {
    method: 'PUT',
    path: 'api/topology/rooms/room-office/devices/move',
    options: {
      body: {
        device_id: 'bulb-desk',
        from_room: 'room-kitchen',
        correlation_id: 'admin-room-move:test-1234'
      },
      requestId: 'admin-room-move:test-1234',
      resourcePrecondition: {
        path: 'api/topology/nodes',
        bodySha256: 'a'.repeat(64)
      }
    }
  });
});

test('rejects a stale source before sending a mutation', async () => {
  let writes = 0;
  const staleTopology = beforeTopology.map((node) =>
    node.id === 'bulb-desk' ? { ...node, parent_id: 'room-office' } : node
  );
  const client: RoomMoveClient = {
    async getReceipt() {
      return receipt(staleTopology);
    },
    async putReceipt() {
      writes += 1;
      return receipt(null);
    }
  };

  await assert.rejects(
    moveDeviceBetweenRoomsVerified(client, proposal(), {
      requestId: 'admin-room-move:stale-1234'
    }),
    /moved after this proposal was opened/
  );
  assert.equal(writes, 0);
});

test('rejects a removed device before sending a mutation', async () => {
  let writes = 0;
  const noDevice = beforeTopology.filter((node) => node.id !== 'bulb-desk');
  const client: RoomMoveClient = {
    async getReceipt() {
      return receipt(noDevice);
    },
    async putReceipt() {
      writes += 1;
      return receipt(null);
    }
  };

  await assert.rejects(
    moveDeviceBetweenRoomsVerified(client, proposal(), {
      requestId: 'admin-room-move:missing-device'
    }),
    /is no longer present/
  );
  assert.equal(writes, 0);
});

test('rejects a removed target room before sending a mutation', async () => {
  let writes = 0;
  const noOffice = beforeTopology.filter((node) => node.id !== 'room-office');
  const client: RoomMoveClient = {
    async getReceipt() {
      return receipt(noOffice);
    },
    async putReceipt() {
      writes += 1;
      return receipt(null);
    }
  };

  await assert.rejects(
    moveDeviceBetweenRoomsVerified(client, proposal(), {
      requestId: 'admin-room-move:missing-target'
    }),
    /source or destination room no longer exists/
  );
  assert.equal(writes, 0);
});

test('reports conflict when accepted state does not confirm the target', async () => {
  const reads = [receipt(beforeTopology), receipt(beforeTopology)];
  const client: RoomMoveClient = {
    async getReceipt() {
      return reads.shift()!;
    },
    async putReceipt(path) {
      return receipt(null, { method: 'PUT', path, statusCode: 204 });
    }
  };

  const result = await moveDeviceBetweenRoomsVerified(client, proposal(), {
    requestId: 'admin-room-move:conflict-1234'
  });
  assert.equal(result.status, 'conflict');
  assert.equal(result.after?.roomId, 'room-kitchen');
});
