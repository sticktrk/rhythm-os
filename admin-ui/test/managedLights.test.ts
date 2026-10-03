import assert from 'node:assert/strict';
import test from 'node:test';
import { canSaveLightSelection, canToggleManagedLight } from '../src/local/managedLights.ts';

test('selected lights can be removed after their identity disappears or HA disconnects', () => {
  assert.equal(canToggleManagedLight(true, true, false), true);
  assert.equal(canToggleManagedLight(true, false, false), true);
  assert.equal(canSaveLightSelection([], ['light.old'], 'snapshot:2', false), true);
  assert.equal(canSaveLightSelection(['light.kept'], ['light.old', 'light.kept'], 'snapshot:2', false), true);
});

test('unready or unreviewable identities cannot be added', () => {
  assert.equal(canToggleManagedLight(false, false, true), false);
  assert.equal(canToggleManagedLight(false, true, false), false);
  assert.equal(canToggleManagedLight(false, true, true), true);
  assert.equal(canSaveLightSelection(['light.new'], ['light.old'], 'snapshot:2', false), false);
  assert.equal(canSaveLightSelection(['light.new'], ['light.old'], 'snapshot:2', true), true);
});

test('removing lights still requires a captured selection and catalog revision', () => {
  assert.equal(canSaveLightSelection([], ['light.old'], null, false), false);
  assert.equal(canSaveLightSelection([], null, 'snapshot:2', false), false);
  assert.equal(canSaveLightSelection(null, ['light.old'], 'snapshot:2', true), false);
});
