import assert from 'node:assert/strict';
import test from 'node:test';

import { modeConfigWriteBody } from '../src/device/modes.ts';

test('mode config saves exclude read-only mode response fields', () => {
  const receivedModeResponse = {
    active: 'sleep',
    configs: [{ mode: 'sleep', room_defaults: [] }],
    last_change: { cause: 'schedule', epoch_ms: 1_700_000_000_000 }
  };

  const body = modeConfigWriteBody(receivedModeResponse.configs);

  assert.deepEqual(body, { configs: receivedModeResponse.configs });
  assert.equal('last_change' in body, false);
  assert.equal('active' in body, false);
});
