import assert from 'node:assert/strict';
import test from 'node:test';

import {
  NODE_ACTIONS,
  sendNodeAction,
  type NodeAction
} from '../src/device/nodes.ts';
import { flashCanonicalDevice } from '../src/device/topology.ts';

test('On and Off controls send canonical node action service names', async () => {
  const requests: Array<{ path: string; body: unknown }> = [];
  const client = {
    put(path: string, options: { body: unknown }) {
      requests.push({ path, body: options.body });
      return Promise.resolve();
    }
  };
  const actionByLabel = new Map(
    NODE_ACTIONS.map((action) => [action.label, action.id])
  );

  await sendNodeAction(
    client as never,
    'room-1',
    actionByLabel.get('On') as NodeAction
  );
  await sendNodeAction(
    client as never,
    'room-1',
    actionByLabel.get('Off') as NodeAction
  );

  assert.deepEqual(requests, [
    {
      path: 'api/nodes/action',
      body: { node_id: 'room-1', action: 'on' }
    },
    {
      path: 'api/nodes/action',
      body: { node_id: 'room-1', action: 'off' }
    }
  ]);
});

test('button-event action names are not exposed as node controls', () => {
  const actionIds = new Set<string>(NODE_ACTIONS.map((action) => action.id));

  assert.equal(actionIds.has('on_press'), false);
  assert.equal(actionIds.has('off_press'), false);
});

test('bulb Identify uses the canonical flash endpoint', async () => {
  const requests: Array<{
    path: string;
    options: { timeoutSeconds?: number } | undefined;
  }> = [];
  const client = {
    post(path: string, options?: { timeoutSeconds?: number }) {
      requests.push({ path, options });
      return Promise.resolve();
    }
  };

  await flashCanonicalDevice(client as never, 'bulb/desk');

  assert.deepEqual(requests, [
    {
      path: 'api/devices/canonical/bulb%2Fdesk/flash',
      options: { timeoutSeconds: 30 }
    }
  ]);
});
