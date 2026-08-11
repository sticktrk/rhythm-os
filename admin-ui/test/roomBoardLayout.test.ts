import assert from 'node:assert/strict';
import test from 'node:test';

import {
  moveRoomBefore,
  moveRoomByOffset,
  parseStoredRoomOrder,
  reconcileRoomOrder,
  roomDeviceSectionId,
  roomOrderStorageKey,
  segmentRoomDevices
} from '../src/pages/hub/roomBoardLayout.ts';
import { topologyItemsFromPayload } from '../src/pages/hub/topologyMembership.ts';

test('reconciles browser order against authoritative room ids', () => {
  assert.deepEqual(
    reconcileRoomOrder(
      ['room-bedroom', 'room-kitchen', 'room-office'],
      ['removed-room', 'room-kitchen', 'room-kitchen', 'room-bedroom']
    ),
    ['room-kitchen', 'room-bedroom', 'room-office']
  );
});

test('moves room cards without changing their ids', () => {
  const order = ['room-bedroom', 'room-kitchen', 'room-office'];
  assert.deepEqual(
    moveRoomBefore(order, 'room-office', 'room-bedroom'),
    ['room-office', 'room-bedroom', 'room-kitchen']
  );
  assert.deepEqual(
    moveRoomByOffset(order, 'room-kitchen', -1),
    ['room-kitchen', 'room-bedroom', 'room-office']
  );
  assert.equal(moveRoomByOffset(order, 'room-bedroom', -1), order);
  assert.equal(moveRoomBefore(order, 'missing-room', 'room-office'), order);
});

test('parses only a deduplicated string array from browser storage', () => {
  assert.deepEqual(
    parseStoredRoomOrder('["room-a", "", 3, "room-a", "room-b"]'),
    ['room-a', 'room-b']
  );
  assert.deepEqual(parseStoredRoomOrder('{"rooms":[]}'), []);
  assert.deepEqual(parseStoredRoomOrder('not-json'), []);
  assert.match(roomOrderStorageKey('hub-1'), /v1:hub-1$/);
});

test('segments exact device kinds while preserving unknown devices', () => {
  const items = topologyItemsFromPayload([
    { id: 'bulb', name: 'Bulb', kind: 'light_device' },
    { id: 'motion', name: 'Motion', kind: 'motion_sensor' },
    { id: 'sensor', name: 'Sensor', kind: 'sensor' },
    { id: 'button', name: 'Button', kind: 'button' },
    { id: 'switch', name: 'Switch', kind: 'switch_device' },
    { id: 'future', name: 'Future', kind: 'new_additive_kind' }
  ]);

  assert.deepEqual(
    segmentRoomDevices(items).map((section) => ({
      label: section.label,
      ids: section.items.map((item) => item.id)
    })),
    [
      { label: 'Lights', ids: ['bulb'] },
      { label: 'Motion & sensors', ids: ['motion', 'sensor'] },
      { label: 'Controls', ids: ['button', 'switch'] },
      { label: 'Other devices', ids: ['future'] }
    ]
  );
  assert.equal(roomDeviceSectionId(undefined), 'other');
});
