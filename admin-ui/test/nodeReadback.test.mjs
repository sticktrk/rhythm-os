import assert from 'node:assert/strict';
import { after, test } from 'node:test';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';
import react from '@vitejs/plugin-react';

// Load the actual page through the project's TSX toolchain. The local client
// replaces external I/O only; parsing, control values and rendering stay real.
const server = await createServer({
  configFile: false,
  plugins: [react()],
  server: { middlewareMode: true, watch: null },
  appType: 'custom',
});
after(() => server.close());
const page = await server.ssrLoadModule('/src/pages/hub/NodesPage.tsx');
const { LocalDeviceContext } = await server.ssrLoadModule('/src/state/LocalDeviceContext.tsx');
const fixture = (observed = {}) => ({
  id: 'light-fixture', name: 'Test light', kind: 'light_device',
  power_on: false, brightness: 100, kelvin: 5500,
  observed_light: {
    availability: 'available', lights_on: true, brightness: 90, kelvin: 5524,
    received_at_epoch_ms: 1_700_000_000_000, ...observed,
  },
});
function render(raw, snapshotStale = false) {
  const node = page.parseNodes([raw])[0];
  return renderToStaticMarkup(React.createElement(LocalDeviceContext.Provider, {
    value: { put() { assert.fail('Rendering must not issue a control command'); } },
  }, React.createElement(page.NodeDetail, {
    node, parentProfileOverrides: {}, onWrite: async () => {}, lightSettingsProfiles: [],
    lightSettingsSupport: 'unsupported', lightSettingsLoading: false,
    lightSettingsError: null, snapshotStale,
  })));
}

test('actual node card separates observed readback from requested control values', () => {
  const markup = render(fixture());
  assert.match(markup, /Observed/);
  assert.match(markup, /90%/);
  assert.match(markup, /5524K/);
  assert.match(markup, /aria-label="Requested brightness"[^>]*aria-valuenow="100"/);
  assert.match(markup, /aria-label="Requested color temp"[^>]*aria-valuenow="5500"/);
  assert.match(markup, /consoleStatus online[^>]*>On</);
});

test('node list uses observed values and never replaces requested values in the parsed node', () => {
  const node = page.parseNodes([fixture()])[0];
  const markup = renderToStaticMarkup(React.createElement(page.NodeReadout, { node, compact: true }));
  assert.match(markup, /Observed: On · 90% 5524K/);
  assert.doesNotMatch(markup, /100%|5500K/);
  assert.equal(node.brightness, 100);
  assert.equal(node.kelvin, 5500);
  assert.equal(node.powerOn, false);
});

test('disconnected, unavailable, unknown and failed-refresh readback is clearly last reported', () => {
  for (const [availability, stale, status] of [
    ['disconnected', false, 'Disconnected'], ['unavailable', false, 'Unavailable'],
    ['future-state', false, 'Unknown'], ['available', true, 'Stale'],
  ]) {
    const raw = fixture({ availability });
    const markup = render(raw, stale);
    assert.match(markup, new RegExp(`Last reported · ${status}`));
    assert.match(markup, /Current light state is unknown/);
    assert.match(markup, /90%/);
    assert.doesNotMatch(markup, /consoleStatus online/);
    const node = page.parseNodes([raw])[0];
    const list = renderToStaticMarkup(React.createElement(page.NodeReadout, { node, compact: true, stale }));
    assert.match(list, new RegExp(`Last reported: ${status}`));
  }
});

test('missing and partial observations never borrow desired values or imply off', () => {
  const partial = render(fixture({ lights_on: null, brightness: null, kelvin: null }));
  assert.match(partial, /Power unknown · Brightness unknown · Color temp unknown/);
  assert.match(partial, /consoleStatus idle[^>]*>Unknown</);
  assert.doesNotMatch(partial, /consoleStatus idle[^>]*>Off</);
  const missing = fixture(); delete missing.observed_light;
  const markup = render(missing);
  assert.match(markup, /Current light state is not reported/);
  assert.match(markup, /Requested off/);
  assert.match(markup, /aria-label="Requested brightness"[^>]*aria-valuenow="100"/);
  const list = renderToStaticMarkup(React.createElement(page.NodeReadout, { node: page.parseNodes([missing])[0], compact: true }));
  assert.match(list, /Requested: 100% 5500K/);
});

test('zero brightness and explicit off remain valid observations; invalid numbers remain unknown', () => {
  const off = render(fixture({ lights_on: false, brightness: 0 }));
  assert.match(off, /Off · 0% · 5524K/);
  assert.match(off, /consoleStatus idle[^>]*>Off</);
  const malformed = render(fixture({ brightness: -1, kelvin: 'invalid', received_at_epoch_ms: -1 }));
  assert.match(malformed, /Brightness unknown · Color temp unknown/);
  assert.doesNotMatch(malformed, /<time/);
});
