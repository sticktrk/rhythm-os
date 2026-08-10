import assert from 'node:assert/strict';
import test from 'node:test';

import type { DeviceClient } from '../src/device/client.ts';
import { renameRoomGuarded } from '../src/device/topology.ts';
import {
  buildRoomRenameProposal,
  roomNameFromTopology,
  roomRenameConfirmationStatus
} from '../src/pages/hub/roomRename.ts';

test('builds a trimmed rename proposal and rejects empty or unchanged names', () => {
  assert.deepEqual(
    buildRoomRenameProposal('room-1', 'Kitchen', '  Galley  '),
    {
      roomId: 'room-1',
      before: 'Kitchen',
      candidate: 'Galley'
    }
  );
  assert.equal(buildRoomRenameProposal('room-1', 'Kitchen', ' Kitchen '), null);
  assert.equal(buildRoomRenameProposal('room-1', 'Kitchen', '   '), null);
  assert.equal(buildRoomRenameProposal('', 'Kitchen', 'Galley'), null);
});

test('finds authoritative room names across topology payload wrappers', () => {
  const nodes = [
    { id: 'room-1', kind: 'room', name: 'Kitchen' },
    { id: 'light-1', kind: 'light_device', name: 'Pendant' }
  ];

  assert.equal(roomNameFromTopology(nodes, 'room-1'), 'Kitchen');
  assert.equal(roomNameFromTopology({ nodes }, 'room-1'), 'Kitchen');
  assert.equal(
    roomNameFromTopology({ rooms: [{ id: 'room-1', name: 'Kitchen' }] }, 'room-1'),
    'Kitchen'
  );
  assert.equal(roomNameFromTopology({ nodes }, 'light-1'), null);
  assert.equal(roomNameFromTopology({ nodes }, 'missing'), null);
});

test('distinguishes confirmed and conflicting topology outcomes', () => {
  assert.equal(roomRenameConfirmationStatus('Galley', 'Galley'), 'confirmed');
  assert.equal(roomRenameConfirmationStatus('Kitchen', 'Galley'), 'conflict');
  assert.equal(roomRenameConfirmationStatus(null, 'Galley'), 'conflict');
});

test('guarded rename forwards route, correlation, and freshness precondition', async () => {
  const calls: Array<{ path: string; options: unknown }> = [];
  const client = {
    putReceipt(path: string, options: unknown) {
      calls.push({ path, options });
      return Promise.resolve({ statusCode: 204 });
    }
  } as unknown as DeviceClient;

  await renameRoomGuarded(client, 'room/a', 'Galley', {
    requestId: 'admin-room-rename:test',
    resourcePrecondition: {
      path: 'api/topology/nodes',
      bodySha256: 'a'.repeat(64)
    }
  });

  assert.deepEqual(calls, [
    {
      path: 'api/topology/rooms/room%2Fa',
      options: {
        body: {
          name: 'Galley',
          correlation_id: 'admin-room-rename:test'
        },
        requestId: 'admin-room-rename:test',
        resourcePrecondition: {
          path: 'api/topology/nodes',
          bodySha256: 'a'.repeat(64)
        }
      }
    }
  ]);
});
