import assert from 'node:assert/strict';
import { after, test } from 'node:test';
import { createServer } from 'vite';
import react from '@vitejs/plugin-react';

const server = await createServer({
  configFile: false,
  plugins: [react()],
  server: { middlewareMode: true, watch: null },
  appType: 'custom',
});
after(() => server.close());
const { restoreProfileExport } = await server.ssrLoadModule('/src/local/LocalApp.tsx');

const bundle = {
  schema_version: 1,
  kind: 'profile_bundle',
  profile: { profiles: [{ id: 'warm-evening' }], light_schedules: [] },
};
function file(value, size) {
  const text = JSON.stringify(value);
  return { size: size ?? Buffer.byteLength(text), text: async () => text };
}
function harness({ approved = true, failAt } = {}) {
  const calls = [];
  return {
    calls,
    confirm: async options => { calls.push(['confirm', options]); return approved; },
    client: {
      put: async (path, options) => {
        calls.push(['put', path, options.body]);
        if (path === failAt) throw new Error('Write failed');
      },
    },
  };
}

test('System imports raw migration output and wrapped HA exports after pausing', async () => {
  for (const value of [bundle, { format: 'rhythm-ha-profiles', version: 1, profiles: bundle }]) {
    const { calls, client, confirm } = harness();
    assert.equal(await restoreProfileExport(file(value), client, confirm), true);
    assert.equal(calls[0][0], 'confirm');
    assert.equal(calls[0][1].requireTypedText, 'RESTORE');
    assert.deepEqual(calls.slice(1), [
      ['put', 'api/light-breaker', { enabled: false }],
      ['put', 'api/profile-bundle', bundle],
    ]);
  }
});

test('schema-1 portable export aliases remain importable', async () => {
  for (const [kind, payload] of [['share_bundle', 'share'], ['configuration_bundle', 'configuration']]) {
    const value = { schema_version: 1, kind, [payload]: bundle.profile };
    const { calls, client, confirm } = harness();
    await restoreProfileExport(file(value), client, confirm);
    assert.deepEqual(calls.at(-1), ['put', 'api/profile-bundle', value]);
  }
});

test('full backups, unknown schemas and malformed portable payloads cause no confirmation or writes', async () => {
  const invalid = [
    null, [], {},
    { schema_version: 3, kind: 'backup_bundle', configuration: {} },
    { ...bundle, schema_version: 2 },
    { ...bundle, profile: [] },
    { ...bundle, profile: null },
    { format: 'rhythm-ha-profiles', version: 2, profiles: bundle },
    { format: 'rhythm-ha-profiles', version: 1, profiles: { ...bundle, schema_version: 2 } },
    { format: 'other', version: 1, profiles: bundle },
  ];
  for (const value of invalid) {
    const { calls, client, confirm } = harness();
    await assert.rejects(restoreProfileExport(file(value), client, confirm), /Choose a Rhythm profile export/);
    assert.deepEqual(calls, []);
  }
  const { calls, client, confirm } = harness();
  await assert.rejects(restoreProfileExport(file(bundle, 1024 * 1024 + 1), client, confirm), /smaller than 1 MB/);
  assert.deepEqual(calls, []);
});

test('declined restore changes nothing and a failed pause prevents import', async () => {
  const declined = harness({ approved: false });
  assert.equal(await restoreProfileExport(file(bundle), declined.client, declined.confirm), false);
  assert.equal(declined.calls.length, 1);

  const failed = harness({ failAt: 'api/light-breaker' });
  await assert.rejects(restoreProfileExport(file(bundle), failed.client, failed.confirm), /Write failed/);
  assert.equal(failed.calls.length, 2);
});

test('failed import leaves adaptation paused and never reports success', async () => {
  const { calls, client, confirm } = harness({ failAt: 'api/profile-bundle' });
  await assert.rejects(restoreProfileExport(file(bundle), client, confirm), /Write failed/);
  assert.deepEqual(calls.slice(1), [
    ['put', 'api/light-breaker', { enabled: false }],
    ['put', 'api/profile-bundle', bundle],
  ]);
});
