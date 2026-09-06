import assert from 'node:assert/strict';
import test from 'node:test';

import {
  analogous,
  blendOutputs,
  complementary,
  houseLightOrder,
  hueSweep,
  paletteOutputForSlot,
  parseOutput,
  renderScenePreview,
  type SceneOutput
} from '../src/pages/hub/scenes/scenePalette.ts';

const orange: SceneOutput = { on: true, brightness: 80, rgb: { r: 255, g: 104, b: 0 } };
const purple: SceneOutput = { on: true, brightness: 40, rgb: { r: 122, g: 0, b: 214 } };

function rgbKey(output: SceneOutput | null): string {
  assert.ok(output?.rgb, 'expected an rgb output');
  return `${output.rgb.r},${output.rgb.g},${output.rgb.b}`;
}

test('spread uses the anchors as they are while the span fits', () => {
  assert.equal(rgbKey(paletteOutputForSlot([orange, purple], 'spread', 0, 2)), '255,104,0');
  assert.equal(rgbKey(paletteOutputForSlot([orange, purple], 'spread', 1, 2)), '122,0,214');
});

test('spread derives one distinct colour per slot across a larger span', () => {
  const span = 6;
  const outputs = Array.from({ length: span }, (_, slot) =>
    paletteOutputForSlot([orange, purple], 'spread', slot, span)
  );
  const keys = new Set(outputs.map(rgbKey));
  assert.equal(keys.size, span);
  assert.equal(rgbKey(outputs[0]), '255,104,0');
  assert.equal(rgbKey(outputs[5]), '122,0,214');
  // The walk passes through red rather than fading through grey.
  const second = outputs[1]?.rgb;
  assert.ok(second && second.r > 200 && second.g < 104 && second.b < 120);
  const brightness = outputs.map((output) => output?.brightness ?? 0);
  for (let index = 1; index < brightness.length; index += 1) {
    assert.ok(brightness[index] < brightness[index - 1], `${brightness}`);
  }
});

test('cycle repeats the anchors and never blends', () => {
  assert.equal(rgbKey(paletteOutputForSlot([orange, purple], 'cycle', 2, 6)), '255,104,0');
  assert.equal(rgbKey(paletteOutputForSlot([orange, purple], 'cycle', 3, 6)), '122,0,214');
});

test('colour temperatures blend linearly and off anchors are kept as-is', () => {
  const warm: SceneOutput = { on: true, brightness: 100, kelvin: 2000 };
  const cool: SceneOutput = { on: true, brightness: 100, kelvin: 6000 };
  assert.equal(paletteOutputForSlot([warm, cool], 'spread', 1, 4)?.kelvin, 3333);
  const off: SceneOutput = { on: false, brightness: 1 };
  assert.equal(blendOutputs(warm, off, 0.5).on, true);
  assert.deepEqual(paletteOutputForSlot([warm, off], 'spread', 3, 4), off);
});

test('parses server outputs in both power spellings', () => {
  assert.equal(parseOutput({ power: 'off', brightness: 5 }).on, false);
  assert.equal(parseOutput({ power: true, brightness: 500 }).brightness, 100);
  assert.deepEqual(parseOutput({ color: { rgb: { r: 1, g: 2, b: 3 } } }).rgb, {
    r: 1,
    g: 2,
    b: 3
  });
});

const nodes = {
  nodes: [
    { id: 'room-kitchen', name: 'Kitchen', kind: 'room' },
    { id: 'room-attic', name: 'Attic', kind: 'room', disabled: true },
    { id: 'room-bedroom', name: 'Bedroom', kind: 'room' },
    { id: 'bulb-k2', name: 'Kitchen 2', kind: 'light_device', parent_id: 'room-kitchen' },
    { id: 'bulb-k1', name: 'Kitchen 1', kind: 'light_device', parent_id: 'room-kitchen' },
    { id: 'bulb-b1', name: 'Bedroom 1', kind: 'light_device', parent_id: 'room-bedroom' },
    { id: 'bulb-b0', name: 'Broken', kind: 'light_device', parent_id: 'room-bedroom', disabled: true },
    { id: 'bulb-a1', name: 'Attic 1', kind: 'light_device', parent_id: 'room-attic' },
    { id: 'bulb-z9', name: 'Porch', kind: 'light_device' },
    { id: 'sensor-1', name: 'Motion', kind: 'motion_sensor', parent_id: 'room-kitchen' },
    { id: '__rhythm_light_node__|device=bulb-k1|kind=device', name: 'alias', kind: 'light_device' }
  ]
};

test('house order is room-major by name, lights by id, roomless last, skipping disabled', () => {
  const house = houseLightOrder(nodes);
  assert.deepEqual(
    house.rooms.map((room) => room.id),
    ['room-attic', 'room-bedroom', 'room-kitchen']
  );
  assert.deepEqual(
    house.ordered.map((light) => light.id),
    ['bulb-b1', 'bulb-k1', 'bulb-k2', 'bulb-z9']
  );
  assert.deepEqual(house.roomless.map((light) => light.id), ['bulb-z9']);
});

test('the preview pins explicit entries, skips disabled lights and spreads the rest', () => {
  const scene = {
    light: {
      palette: [
        { power: 'on', brightness: 80, color: { rgb: { r: 255, g: 104, b: 0 } } },
        { power: 'on', brightness: 40, color: { rgb: { r: 122, g: 0, b: 214 } } }
      ],
      entries: [
        {
          target: { node_id: 'bulb-k1' },
          output: { power: 'on', brightness: 33, color: { rgb: { r: 1, g: 2, b: 3 } } }
        }
      ]
    }
  };
  const preview = renderScenePreview(scene, houseLightOrder(nodes), { kind: 'home' });
  assert.equal(preview.span, 3, 'the pinned light consumes no slot');
  const byId = new Map(preview.lights.map((light) => [light.id, light]));
  assert.equal(byId.get('bulb-k1')?.pinned, true);
  assert.equal(byId.get('bulb-k1')?.slot, null);
  assert.equal(byId.get('bulb-b0')?.skipped, true);
  assert.equal(byId.get('bulb-a1')?.skipped, true, 'a disabled room skips its lights');
  assert.deepEqual(
    preview.lights.filter((light) => light.slot !== null).map((light) => light.slot),
    [0, 1, 2]
  );
  assert.equal(rgbKey(byId.get('bulb-b1')?.output ?? null), '255,104,0');
  assert.equal(rgbKey(byId.get('bulb-z9')?.output ?? null), '122,0,214');

  const room = renderScenePreview(scene, houseLightOrder(nodes), {
    kind: 'room',
    roomId: 'room-kitchen'
  });
  assert.equal(room.span, 1);
  const strip = renderScenePreview(scene, houseLightOrder(nodes), { kind: 'strip', count: 5 });
  assert.equal(strip.span, 5);
  assert.equal(new Set(strip.lights.map((light) => rgbKey(light.output))).size, 5);
});

test('generators produce well-formed anchors', () => {
  assert.equal(hueSweep(6).length, 6);
  assert.equal(new Set(hueSweep(6).map(rgbKey)).size, 6);
  assert.equal(complementary({ r: 255, g: 0, b: 0 }).length, 2);
  assert.equal(rgbKey(complementary({ r: 255, g: 0, b: 0 })[1]), '0,255,255');
  assert.equal(analogous({ r: 255, g: 0, b: 0 }).length, 3);
});

// ---------------------------------------------------------------------------
// Scene shape helpers
// ---------------------------------------------------------------------------

import {
  entryNodeId,
  entryWithNodeId,
  lightLayer,
  lightNodeOptionsFromState,
  withLightLayer
} from '../src/pages/hub/scenes/sceneShape.ts';

test('a legacy top-level layer is read and folded into the canonical light layer', () => {
  const legacy = {
    id: 'old',
    name: 'Old',
    default_output: { power: true, brightness: 50, color: { kelvin: 3000 } },
    entries: [{ target_id: 'bulb-1', power: true, brightness: 40, color: { kelvin: 2700 } }]
  };
  const layer = lightLayer(legacy);
  assert.deepEqual(Object.keys(layer).sort(), ['default_output', 'entries']);
  const canonical = withLightLayer(legacy, layer);
  assert.equal('default_output' in canonical, false);
  assert.equal('entries' in canonical, false);
  assert.deepEqual(lightLayer(canonical), layer);

  const stored = { id: 's', name: 'S', light: { palette: [], entries: [] } };
  assert.deepEqual(lightLayer(stored), stored.light);
});

test('entries resolve their node id from every spelling and are rewritten canonically', () => {
  assert.equal(entryNodeId({ target: { kind: 'node', node_id: 'bulb-1' } }), 'bulb-1');
  assert.equal(entryNodeId({ target_id: 'bulb-2' }), 'bulb-2');
  assert.equal(entryNodeId({ node_id: 'bulb-3' }), 'bulb-3');
  assert.deepEqual(entryWithNodeId({ target_id: 'bulb-2', output: {} }, 'bulb-9'), {
    target: { kind: 'node', node_id: 'bulb-9' },
    output: {}
  });
});

test('pin options offer only light devices', () => {
  const options = lightNodeOptionsFromState(nodes);
  assert.deepEqual(
    options.map((option) => option.value),
    ['bulb-k2', 'bulb-k1', 'bulb-b1', 'bulb-b0', 'bulb-a1', 'bulb-z9']
  );
});
