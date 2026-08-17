import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildTopologyMembership,
  groupTopologyItems,
  topologyKindLabel,
  topologyItemsFromPayload
} from '../src/pages/hub/topologyMembership.ts';

const flatTopology = [
  { id: 'room-kitchen', name: 'Kitchen', kind: 'room' },
  { id: 'bulb-pendant', name: 'Pendant', kind: 'light_device', parent_id: 'room-kitchen' },
  { id: 'button-wall', name: 'Wall Button', kind: 'button', parent_id: 'room-kitchen' },
  { id: 'future-node', name: 'Future Node', kind: 'new_additive_kind', parent_id: 'room-kitchen' },
  { id: 'room-bedroom', name: 'Bedroom', kind: 'room' },
  { id: 'bulb-orphan', name: 'Orphan Bulb', kind: 'light_device', parent_id: 'missing-room' },
  { id: 'bulb-loose', name: 'Loose Bulb', kind: 'light_device' }
];

test('joins bulbs to rooms by parent_id without counting other child nodes', () => {
  const membership = buildTopologyMembership(flatTopology);

  assert.deepEqual(
    membership.rooms.map(({ room }) => room.name),
    ['Bedroom', 'Kitchen']
  );
  assert.deepEqual(membership.rooms[0].bulbs, []);
  assert.deepEqual(
    membership.rooms[1].children.map((child) => child.name),
    ['Future Node', 'Pendant', 'Wall Button']
  );
  assert.deepEqual(
    membership.rooms[1].bulbs.map((bulb) => bulb.name),
    ['Pendant']
  );
});

test('surfaces roomless bulbs and bulbs with unknown parents as unassigned', () => {
  const membership = buildTopologyMembership(flatTopology);

  assert.deepEqual(
    membership.unassignedBulbs.map((bulb) => bulb.name).sort(),
    ['Loose Bulb', 'Orphan Bulb']
  );
});

test('uses explicit node kinds for readable device labels', () => {
  assert.equal(topologyKindLabel('light_device'), 'Bulb');
  assert.equal(topologyKindLabel('motion_sensor'), 'Motion sensor');
  assert.equal(topologyKindLabel('new_additive_kind'), 'New additive kind');
  assert.equal(topologyKindLabel(undefined), 'Device');
});

test('accepts both direct arrays and nodes wrappers', () => {
  assert.deepEqual(
    buildTopologyMembership({ nodes: flatTopology }),
    buildTopologyMembership(flatTopology)
  );
});

test('retains compatibility with a rooms-only wrapper', () => {
  const items = topologyItemsFromPayload({
    rooms: [{ id: 'legacy-room', name: 'Legacy Room' }]
  });

  assert.equal(items.length, 1);
  assert.equal(items[0].kind, 'room');
  assert.equal(buildTopologyMembership({ rooms: [items[0].raw] }).rooms.length, 1);
});

test('groups live nodes under room names and keeps standalone nodes explicit', () => {
  const items = topologyItemsFromPayload({ nodes: flatTopology });
  const groups = groupTopologyItems(items);

  assert.deepEqual(
    groups.map((group) => group.label),
    ['Bedroom', 'Kitchen', 'Unassigned / standalone']
  );
  assert.deepEqual(
    groups[1].items.map((item) => item.name),
    ['Kitchen', 'Future Node', 'Pendant', 'Wall Button']
  );
  assert.deepEqual(
    groups[2].items.map((item) => item.name),
    ['Loose Bulb', 'Orphan Bulb']
  );
  assert.equal(
    groups.flatMap((group) => group.items).filter((item) => item.id === 'room-kitchen').length,
    1
  );
});
