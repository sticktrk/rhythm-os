import '../bootstrap.mjs';
import '@matter-server/custom-clusters';
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, readdir, rm, writeFile, lstat, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { Bytes, FabricId, StorageService, Network, NetworkSimulator, Crypto, Entropy } from '@matter/main';
import { VendorId } from '@matter/main/types';
import { Noc } from '@matter/main/protocol';
import { createControllerNode, ConfigStorage, resolveServerId } from '@matter-server/ws-controller';
import { fixture, stringify, LARGE_FABRIC, LARGE_NODE } from './fixture.mjs';
import { validateSource, prepare, verify, TARGET_VERSION, SERVER_ID, storageEnvironment } from '../migration.mjs';
import { parseJson, parseController } from '../source.mjs';

const exec = promisify(execFile);
const cli = new URL('../migrate.mjs', import.meta.url).pathname;
async function scenario(t, options) {
  const dir = await mkdtemp(join(tmpdir(), 'rhythm-migration-test-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  return { dir, ...await fixture(dir, options) };
}
const validate = path => validateSource(path, TARGET_VERSION);

test('real matter.js controller preserves fabric, CATs and every peer over two restarts', async t => {
  const f = await scenario(t, { cats: true });
  const original = await readFile(f.backupPath);
  const validated = await validate(f.backupPath);
  assert.equal(validated.fabric.fabricId, LARGE_FABRIC);
  assert.equal(validated.nodes[1].nodeId, LARGE_NODE);
  const output = join(f.dir, 'prepared');
  const receipt = await prepare(validated, output);
  assert.equal(receipt.next_node_id, (LARGE_NODE + 1n).toString());
  assert.equal((await verify(output)).node_count, 2);
  const path = join(output, 'matter-server');
  for (let restart = 1; restart <= 2; restart++) {
    const env = storageEnvironment(path);
    const simulator = new NetworkSimulator();
    env.set(Network, simulator.addHost(restart));
    env.set(Crypto, new (await import('@matter/main')).StandardCrypto());
    env.set(Entropy, env.get(Crypto));
    const config = await ConfigStorage.create(env);
    const id = await resolveServerId(env, config, {}, 4939, 2);
    assert.equal(id, SERVER_ID, 'HA launcher defaults must find the prepared namespace');
    const controller = await createControllerNode({ environment: env, id,
      adminVendorId: VendorId(4939), adminFabricId: FabricId(2), adminFabricLabel: 'HomeAssistant',
      serverVersion: TARGET_VERSION, enableOtaProvider: false, dcl: { acceptTestCertificates: false, offline: true } });
    try {
      assert.equal(controller.fabric.fabricId, f.fabric.fabricId);
      assert.equal(controller.fabric.nodeId, f.fabric.nodeId);
      assert.equal(controller.fabric.globalId, f.fabric.globalId);
      assert.ok(Bytes.areEqual(controller.fabric.rootCert, f.fabric.rootCert));
      assert.ok(Bytes.areEqual(controller.fabric.operationalIdentityProtectionKey, f.fabric.operationalIdentityProtectionKey));
      assert.deepEqual(Noc.fromTlv(controller.fabric.operationalCert).cert.subject.caseAuthenticatedTags, [0x00010001]);
      assert.deepEqual([...controller.node.peers].map(peer => peer.peerAddress.nodeId).sort(), [7n, LARGE_NODE].sort());
      assert.equal(config.nextNodeId, LARGE_NODE + 1n);
    } finally { await controller.close(); await config.close(); }
  }
  assert.deepEqual(await readFile(f.backupPath), original);
  await assert.rejects(verify(output), /storage changed/); // Runtime key rotation correctly invalidates transport hashes.
});

test('CLI dry-run writes no output, conversion is private, existing output is preserved', async t => {
  const f = await scenario(t);
  const before = await readdir(f.dir);
  const dry = await exec(process.execPath, [cli, '--source', f.cold, '--target-version', TARGET_VERSION, '--dry-run'], {
    env: { ...process.env, MATTER_STORAGE_PATH: f.cold, MATTER_STORAGE_CLEAR: 'true' },
  });
  assert.equal(JSON.parse(dry.stdout).node_count, 2);
  assert.equal(dry.stderr, '');
  assert.deepEqual(await readdir(f.dir), before);
  const output = join(f.dir, 'out');
  await exec(process.execPath, [cli, '--source', f.backupPath, '--target-version', TARGET_VERSION, '--output', output]);
  assert.equal(JSON.parse((await exec(process.execPath, [cli, '--verify', output])).stdout).status, 'prepared_storage_verified');
  async function privateTree(path) {
    const stat = await lstat(path);
    assert.equal(stat.mode & 0o077, 0);
    if (stat.isDirectory()) for (const name of await readdir(path)) await privateTree(join(path, name));
  }
  await privateTree(output);
  const receipt = await readFile(join(output, 'receipt.json'));
  await assert.rejects(exec(process.execPath, [cli, '--source', f.backupPath, '--target-version', TARGET_VERSION, '--output', output]),
    error => /already exists/.test(error.stderr));
  assert.deepEqual(await readFile(join(output, 'receipt.json')), receipt);
  await assert.rejects(prepare(await validate(f.backupPath), f.cold), /EEXIST/);
  await assert.rejects(prepare(await validate(f.cold), join(f.cold, 'output')), /outside the source/);
  await assert.rejects(prepare(await validate(f.cold), join(f.cold, '..looks-outside')), /outside the source/);
  await writeFile(join(output, 'device-map.json'), '[]');
  await assert.rejects(verify(output), /device map changed/);
});

test('a native storage failure leaves unusable output and a preserved source', async t => {
  const f = await scenario(t);
  const original = await readFile(f.backupPath);
  const validated = await validate(f.backupPath);
  const output = join(f.dir, 'interrupted');
  t.mock.method(StorageService.prototype, 'open', async () => { throw new Error('simulated storage I/O failure'); });
  await assert.rejects(prepare(validated, output), /storage I\/O failure/);
  assert.ok((await readdir(output)).includes('INCOMPLETE'));
  assert.ok(!(await readdir(output)).includes('PREPARED'));
  assert.deepEqual(await readFile(f.backupPath), original);
  await assert.rejects(prepare(validated, output), /EEXIST/);
});

test('legacy inventories and root-only chains work; source variants yield the same digest', async t => {
  const f = await scenario(t, { legacy: true, intermediate: false });
  const backup = await validate(f.backupPath);
  const cold = await validate(f.cold);
  assert.equal(backup.source.sourceDigest, cold.source.sourceDigest);
  assert.deepEqual(backup.warnings, ['inventory_has_no_independent_fabric_fingerprint']);
  const controller = f.backup.installation.integration_files[2];
  controller.path = 'matter/chip/controller-storage.json';
  controller.content = stringify({ 'sdk-config': f.sdk });
  f.backup.schema_version = 1;
  await writeFile(f.backupPath, stringify(f.backup));
  assert.equal((await validate(f.backupPath)).fabric.fabricId, LARGE_FABRIC);
});

test('reject inconsistent IDs, IPKs, inventory, missing credentials and private key mismatch', async t => {
  const f = await scenario(t);
  const pristine = structuredClone(f.backup);
  const cases = [
    [b => { b.secrets_included = false; }, /full backup/],
    [b => { b.schema_version = 99; }, /Unsupported backup/],
    [b => { b.installation.integration_files.pop(); }, /exactly one controller/],
    [b => { b.installation.integration_files.push({ ...b.installation.integration_files[2], path: 'matter/chip/chip_tool_config.ini' }); }, /exactly one controller/],
    [b => { b.installation.integration_files.push(b.installation.integration_files[0]); }, /duplicate/],
    [b => { b.installation.integration_files[0].path = '../matter/fabric-identity.json'; }, /Unsafe/],
    [b => { b.installation.integration_files[0].content = stringify({ ...f.identity, operational_fabric_id: LARGE_FABRIC + 1n }); }, /signed fabric/],
    [b => { b.installation.integration_files[0].content = stringify({ ...f.identity, ipk_hex: '00'.repeat(16) }); }, /IPK does not match/],
    [b => { b.installation.integration_files[1].content = stringify({ ...f.inventory, compressed_fabric_id: '1234' }); }, /different fabric/],
    [b => { b.installation.integration_files[1].content = stringify({ ...f.inventory, devices: [f.inventory.devices[0], f.inventory.devices[0]] }); }, /Duplicate node/],
    [b => { b.installation.integration_files[1].content = stringify({ ...f.inventory, schema_version: 99 }); }, /device schema/],
    [b => { b.installation.integration_files[2].content += 'LocalNodeId=AgAAAAAAAAA=\n'; }, /controller node ID differs/],
    [b => { b.installation.integration_files[2].content = b.installation.integration_files[2].content.replace(/^ExampleOpCredsCAKey0=.*\n/m, ''); }, /CA signing/],
    [b => {
      const key = Buffer.from(f.sdk.ExampleOpCredsCAKey0, 'base64'); key[96] ^= 1;
      b.installation.integration_files[2].content = b.installation.integration_files[2].content.replace(f.sdk.ExampleOpCredsCAKey0, key.toString('base64'));
    }, /private key does not match/],
  ];
  for (const [change, pattern] of cases) {
    const backup = structuredClone(pristine); change(backup);
    await writeFile(f.backupPath, stringify(backup));
    await assert.rejects(validate(f.backupPath), pattern);
  }
});

test('invalid input and dependency errors never disclose source contents', async t => {
  const f = await scenario(t);
  const secret = 'SYNTHETIC_SECRET_MUST_NOT_APPEAR';
  await writeFile(f.backupPath, `{"${secret}": broken-json}`);
  await assert.rejects(exec(process.execPath, [cli, '--source', f.backupPath, '--target-version', TARGET_VERSION, '--dry-run']), error => {
    assert.ok(!error.stdout.includes(secret) && !error.stderr.includes(secret));
    return /Invalid JSON/.test(error.stderr);
  });
  await assert.rejects(validateSource(f.cold, '1.5.4'), /Unsupported target/);
  const source = join(f.cold, 'matter/fabric-identity.json');
  await rm(source);
  await symlink(f.backupPath, source);
  await assert.rejects(validate(f.cold));
});

test('exact numeric parsing and strict INI decoding', () => {
  assert.equal(parseJson('{"id":18364758544493064721}').id, LARGE_FABRIC);
  assert.throws(() => parseJson('{"id":1e99}'), /Invalid JSON/);
  assert.equal(parseController('[Default]\nf/1/n=YQ==\n', true)['f/1/n'], 'YQ==');
  assert.throws(() => parseController('[Default]\na=YQ==\na=Yg==\n', true), /Duplicate/);
  assert.throws(() => parseController('[Default]\na=garbage\n', true), /base64/);
});
