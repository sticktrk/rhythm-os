import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { runGuardedDeviceOperation } from '../src/api.ts';
import { DEVICE_ADMIN_OPERATIONS } from '../src/deviceAdminOperations.ts';

test('the staff console addresses each live operation and applies the advertised freshness guard', async () => {
  const contract = JSON.parse(readFileSync(process.env.RHYTHM_ASSISTANT_CONTRACT, 'utf8'));
  const call = DEVICE_ADMIN_OPERATIONS.find(item => item.id === 'assistant-operation');
  const discovery = DEVICE_ADMIN_OPERATIONS.find(item => item.id === 'assistant-contract-get');
  assert.ok(call && discovery);
  for (const op of contract.operations) {
    const path = call.path.replace('{operation_id}',op.id);
    assert.equal('/'+path,op.invocation_path);
    const sent = [];
    await runGuardedDeviceOperation({method:call.method,path,body:{}}, async request => {
      sent.push(request);
      if (request.path === discovery.path && request.method === discovery.method) return {body:contract};
      if (request.path === 'api/state') return {body:{server_instance_id:'selected-hub'}};
      return {body:{status:'ok'}};
    });
    const invocation=sent.at(-1);
    assert.equal(invocation.path,path);
    assert.equal(invocation.method,'POST');
    assert.equal(Boolean(invocation.requestId),op.tier!=='read',op.id);
    assert.equal(invocation.expectedServerInstanceId,op.tier==='read'?undefined:'selected-hub',op.id);
  }
});
