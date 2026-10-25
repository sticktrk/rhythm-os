// This adapter intentionally targets one exact OHF storage implementation.
// Never load matter-server's entrypoint: importing it would start a live server.
import './bootstrap.mjs';
import '@matter-server/custom-clusters';
import { NodeJsFilesystem, DirectoryBlobStorageDriver } from '@matter/nodejs';
import { Bytes, Environment, FabricId, FabricIndex, NodeId, StandardCrypto, StorageService, Logger,
  Filesystem, WalStorageDriver, Crypto, Entropy } from '@matter/main';
import { VendorId } from '@matter/main/types';
import { CertificateAuthority, FabricBuilder, Noc, Rcac, Icac } from '@matter/main/protocol';
import { LegacyDataInjector } from '@matter-server/ws-controller';
import { createECDH, createHash } from 'node:crypto';
import { chmod, lstat, mkdir, mkdtemp, readFile, readdir, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { exactId, parseJson, readSource, requireThat } from './source.mjs';

Logger.destinations = {}; // Raw upstream diagnostics can contain credential material.
export const TARGET_VERSION = '1.5.5';
export const SERVER_ID = 'server-2-134b'; // HA's launcher selects fabric=2/vendor=4939 storage namespace.
const serverEntry = fileURLToPath(import.meta.resolve('matter-server'));
const { ChipConfigData } = await import(pathToFileURL(join(dirname(serverEntry), 'converter/ChipConfigData.js')));
const crypto = new StandardCrypto();
const hexId = id => id.toString(16).padStart(16, '0');
const json = value => JSON.stringify(value, (_key, item) => typeof item === 'bigint' ? item.toString() : item, 2) + '\n';

export async function assertVersions(targetVersion) {
  requireThat(targetVersion === TARGET_VERSION, 'Unsupported target: require OHF matter-server 1.5.5 (matter.js 0.18.0).');
  for (const [name, version] of [['matter-server', TARGET_VERSION], ['@matter-server/ws-controller', TARGET_VERSION],
    ['@matter/main', '0.18.0'], ['@matter/nodejs', '0.18.0']]) {
    const entry = fileURLToPath(import.meta.resolve(name));
    const pkg = JSON.parse(await readFile(resolve(dirname(entry), '../../package.json'), 'utf8'));
    requireThat(pkg.version === version, 'Dependency version mismatch; reinstall with npm ci.');
  }
}

function checkSigningKey(pair, certificate) {
  requireThat(pair?.privateKey?.length === 32 && pair?.publicKey?.length === 65, 'Incomplete CA key material.');
  const key = createECDH('prime256v1');
  key.setPrivateKey(pair.privateKey);
  requireThat(Bytes.areEqual(key.getPublicKey(), pair.publicKey) &&
    Bytes.areEqual(pair.publicKey, certificate.cert.ellipticCurvePublicKey), 'CA private key does not match its certificate.');
}

export function storageEnvironment(path) {
  const env = new Environment('rhythm-fabric-migration');
  env.vars.set('storage.path', path);
  env.vars.set('storage.driver', 'wal');
  env.vars.set('storage.blobDriver', 'dir');
  env.vars.set('storage.clear', false);
  env.vars.set('runtime.signals', false);
  env.set(Filesystem, new NodeJsFilesystem(path));
  env.set(Crypto, crypto);
  env.set(Entropy, crypto);
  const service = env.get(StorageService);
  service.registerDriver(WalStorageDriver);
  service.registerBlobDriver(DirectoryBlobStorageDriver);
  service.configuredDriver = 'wal';
  service.configuredBlobDriver = 'dir';
  return env;
}

export async function validateSource(path, targetVersion) {
  await assertVersions(targetVersion);
  const source = await readSource(path);
  const { identity, inventory } = source;
  requireThat((identity.schema_version ?? 1) === 1 && typeof identity.label === 'string' && identity.label.trim(),
    'Unsupported or incomplete fabric identity.');
  requireThat(typeof identity.ipk_hex === 'string' && /^[a-fA-F0-9]{32}$/.test(identity.ipk_hex), 'Missing or invalid original IPK.');
  const fabricId = exactId(identity.operational_fabric_id);
  const temp = await mkdtemp(join(tmpdir(), 'rhythm-matter-'));
  const chip = new ChipConfigData();
  try {
    const input = join(temp, 'chip.json');
    await writeFile(input, JSON.stringify({ 'sdk-config': source.sdk }), { mode: 0o600 });
    await chip.load(input);
  } finally { await rm(temp, { recursive: true, force: true }); }
  const indices = chip.getFabricIndices();
  requireThat(indices.length === 1, 'Source must contain exactly one fabric.');
  const index = indices[0];
  requireThat(index >= 1 && index <= 254, 'Invalid source fabric index.');
  requireThat((await chip.verifyCertificateChain(index)).valid, 'Source certificate chain is invalid.');
  const imported = chip.getFabricConfig(index);
  requireThat(imported && BigInt(imported.fabricId) === fabricId, 'Stored fabric identity does not match the signed fabric.');
  if (imported.intermediateCACert?.length === 0) imported.intermediateCACert = undefined;
  requireThat(NodeId.isOperationalNodeId(imported.nodeId), 'Invalid controller node ID.');
  if (source.sdk.LocalNodeId !== undefined) {
    const localNode = Buffer.from(source.sdk.LocalNodeId, 'base64');
    requireThat(localNode.length === 8 && localNode.readBigUInt64LE() === BigInt(imported.nodeId),
      'Configured controller node ID differs from its certificate.');
  }
  const credentials = [];
  for (const issuer of chip.getOperationalCredentialsIndices()) {
    const config = await chip.getCertificateAuthorityConfig(issuer);
    if (config && Bytes.areEqual(config.rootCertBytes, imported.rootCert)) credentials.push(config);
  }
  requireThat(credentials.length === 1, 'Missing or ambiguous CA signing credentials.');
  const ca = credentials[0];
  const root = Rcac.fromTlv(imported.rootCert);
  checkSigningKey(ca.rootKeyPair, root);
  if (imported.intermediateCACert) {
    requireThat(ca.icacCertBytes && Bytes.areEqual(ca.icacCertBytes, imported.intermediateCACert), 'Missing or mismatched intermediate CA.');
    checkSigningKey(ca.icacKeyPair, Icac.fromTlv(imported.intermediateCACert));
  } else {
    requireThat(!ca.icacCertBytes, 'Unexpected intermediate CA credentials.');
  }
  // The upstream Python-data reader substitutes a fixed IPK. Always use Rhythm's original.
  const ipk = Bytes.fromHex(identity.ipk_hex);
  const builder = await FabricBuilder.create(crypto);
  await builder.setRootCert(imported.rootCert);
  builder.setRootNodeId(NodeId(imported.nodeId)).setIdentityProtectionKey(ipk)
    .setRootVendorId(VendorId(imported.rootVendorId)).setLabel(imported.label);
  const authority = new CertificateAuthority(crypto, ca);
  const cats = Noc.fromTlv(imported.operationalCert).cert.subject.caseAuthenticatedTags;
  await builder.setOperationalCert(await authority.generateNoc(builder.publicKey, FabricId(fabricId), NodeId(imported.nodeId), cats),
    imported.intermediateCACert);
  const fabric = await builder.build(FabricIndex(index));
  const derivedKeys = chip.getFabric(index).keysDecoded.get(0)?.keys ?? [];
  requireThat(derivedKeys.some(entry => Bytes.areEqual(entry.key, fabric.operationalIdentityProtectionKey)),
    'Original IPK does not match the controller group-key store.');
  await Noc.fromTlv(fabric.operationalCert).verify(crypto, root,
    imported.intermediateCACert ? Icac.fromTlv(imported.intermediateCACert) : undefined);

  const legacyInventory = Array.isArray(inventory);
  requireThat(legacyInventory || inventory.schema_version === 1, 'Unsupported commissioned-device schema.');
  const devices = legacyInventory ? inventory : inventory.devices;
  requireThat(Array.isArray(devices) && devices.length > 0 && devices.length <= 4096, 'Expected 1–4096 commissioned devices.');
  const warnings = [];
  if (!legacyInventory && inventory.compressed_fabric_id != null) {
    const compressed = inventory.compressed_fabric_id;
    requireThat(typeof compressed === 'string' && /^(?:0x)?[a-fA-F0-9]{1,16}$/.test(compressed) &&
      BigInt(compressed.startsWith('0x') ? compressed : `0x${compressed}`) === fabric.globalId,
    'Device inventory belongs to a different fabric.');
  } else warnings.push('inventory_has_no_independent_fabric_fingerprint');
  const seen = new Set();
  const nodes = devices.map(device => {
    const nodeId = exactId(device.node_id, 0xffffffefffffffffn);
    requireThat(nodeId !== fabric.nodeId && !seen.has(nodeId.toString()), 'Duplicate node or controller/node ID collision.');
    seen.add(nodeId.toString());
    requireThat(Number.isInteger(device.light_endpoint) && device.light_endpoint > 0 && device.light_endpoint <= 0xfffe,
      'Invalid light endpoint in inventory.');
    requireThat(typeof device.vendor_name === 'string' && Buffer.byteLength(device.vendor_name) <= 128,
      'Invalid device vendor metadata.');
    return { nodeId, endpoint: device.light_endpoint, vendorName: device.vendor_name };
  }).sort((a, b) => a.nodeId < b.nodeId ? -1 : 1);
  const nextNodeId = [fabric.nodeId, ...nodes.map(node => node.nodeId)].reduce((a, b) => a > b ? a : b) + 1n;
  requireThat(NodeId.isOperationalNodeId(nextNodeId), 'No node ID available above the imported inventory.');
  return { source, fabric, credentials: authority.config, nodes, nextNodeId, warnings };
}

export function summary(validated) {
  return { target: `OHF matter-server ${TARGET_VERSION}`, node_count: validated.nodes.length,
    warnings: validated.warnings, status: 'validated_offline_not_connected' };
}

async function storageFiles(directory) {
  const files = [];
  async function walk(path, prefix) {
    for (const entry of await readdir(path, { withFileTypes: true })) {
      requireThat(!entry.isSymbolicLink(), 'Unexpected symlink in prepared storage.');
      const name = prefix ? `${prefix}/${entry.name}` : entry.name;
      if (entry.isDirectory()) await walk(join(path, entry.name), name);
      else {
        requireThat(entry.isFile(), 'Unexpected non-file in prepared storage.');
        files.push([name, createHash('sha256').update(await readFile(join(path, entry.name))).digest('hex')]);
      }
    }
  }
  await walk(directory, '');
  return Object.fromEntries(files.sort(([a], [b]) => a.localeCompare(b)));
}

export async function prepare(validated, output) {
  const destination = resolve(output);
  const sourcePath = await realpath(validated.source.sourcePath);
  const resolvedDestination = join(await realpath(dirname(destination)), basename(destination));
  const relation = relative(sourcePath, resolvedDestination);
  requireThat(relation === '..' || relation.startsWith(`..${sep}`) || isAbsolute(relation), 'Output must be outside the source snapshot.');
  await mkdir(destination, { mode: 0o700 }); // Exclusive; never reuse or remove a partial attempt.
  await writeFile(join(destination, 'INCOMPLETE'), 'Do not start a Matter server with this directory.\n', { mode: 0o600, flag: 'wx' });
  const storagePath = join(destination, 'matter-server');
  await mkdir(storagePath, { mode: 0o700 });
  const { fabric, credentials, nodes, nextNodeId } = validated;
  const generatedAt = new Date().toISOString();
  const env = storageEnvironment(storagePath);
  const service = env.get(StorageService);
  const storage = await service.open(SERVER_ID);
  try {
    await storage.createContext('certificates').set(credentials);
    await storage.createContext('fabrics').set('fabrics', [fabric.config]);
    const nodeData = { vendor_info: {}, last_node_id: nextNodeId - 1n, nodes: Object.fromEntries(nodes.map(node => [node.nodeId.toString(), {
      node_id: node.nodeId, date_commissioned: generatedAt, last_interview: generatedAt,
      interview_version: 0, available: false, is_bridge: false,
      // Real cached source metadata ensures the injector records each peer. HA must re-interview it.
      attributes: { '0/40/1': node.vendorName }, attribute_subscriptions: [],
    }])) };
    requireThat(await LegacyDataInjector.injectNodeData(storage, nodeData, fabric.fabricIndex), 'Matter.js rejected the node inventory.');
    const peers = await storage.createContext('nodes').get('commissionedNodes', []);
    requireThat(peers.length === nodes.length, 'Incomplete node import.');
  } finally { await storage.close(); }
  const config = await service.open('config');
  try { await config.createContext('values').set({ nextNodeId, fabricLabel: 'HomeAssistant' }); }
  finally { await config.close(); }
  // Close/reopen through the native driver, not just an in-memory assertion.
  const reopened = await storageEnvironment(storagePath).get(StorageService).open(SERVER_ID);
  try {
    const persisted = await reopened.createContext('fabrics').get('fabrics');
    requireThat(persisted.length === 1 && persisted[0].fabricId === fabric.fabricId &&
      persisted[0].nodeId === fabric.nodeId && Bytes.areEqual(persisted[0].identityProtectionKey, fabric.identityProtectionKey),
    'Native storage round-trip failed.');
  } finally { await reopened.close(); }
  const inventory = nodes.map(node => ({ node_id: node.nodeId.toString(), endpoint: node.endpoint,
    compressed_fabric_id: hexId(fabric.globalId), ha_entity_id: null }));
  await writeFile(join(destination, 'device-map.json'), json(inventory), { mode: 0o600, flag: 'wx' });
  const receipt = { schema_version: 1, status: 'prepared_offline', generated_at: generatedAt,
    target_server_version: TARGET_VERSION, matter_js_version: '0.18.0', server_id: SERVER_ID,
    source_digest: validated.source.sourceDigest, fabric_id: hexId(fabric.fabricId),
    controller_node_id: fabric.nodeId.toString(), compressed_fabric_id: hexId(fabric.globalId),
    root_certificate_sha256: createHash('sha256').update(fabric.rootCert).digest('hex'),
    node_count: nodes.length, next_node_id: nextNodeId.toString(), warnings: validated.warnings,
    device_map_sha256: createHash('sha256').update(await readFile(join(destination, 'device-map.json'))).digest('hex'),
    storage_sha256: await storageFiles(storagePath) };
  await writeFile(join(destination, 'receipt.json'), json(receipt), { mode: 0o600, flag: 'wx' });
  // Native drivers inherit the CLI's umask; enforce private modes for library callers too.
  async function restrict(path) {
    const stat = await lstat(path);
    await chmod(path, stat.isDirectory() ? 0o700 : 0o600);
    if (stat.isDirectory()) for (const name of await readdir(path)) await restrict(join(path, name));
  }
  await restrict(destination);
  await writeFile(join(destination, 'PREPARED'), 'Offline preparation complete. Stop the source controller before cutover.\n', { mode: 0o600, flag: 'wx' });
  await rm(join(destination, 'INCOMPLETE'));
  return receipt;
}

export async function verify(output) {
  const receipt = parseJson(await readFile(join(output, 'receipt.json'), 'utf8'));
  requireThat(receipt.schema_version === 1 && receipt.status === 'prepared_offline', 'Invalid migration receipt.');
  await assertVersions(receipt.target_server_version);
  const names = await readdir(output);
  requireThat(names.includes('PREPARED') && !names.includes('INCOMPLETE'), 'Conversion did not complete.');
  requireThat(createHash('sha256').update(await readFile(join(output, 'device-map.json'))).digest('hex') === receipt.device_map_sha256,
    'Prepared device map changed or is incomplete.');
  requireThat(JSON.stringify(await storageFiles(join(output, 'matter-server'))) === JSON.stringify(receipt.storage_sha256),
    'Prepared storage changed or is incomplete. Verify only before starting the target.');
  return { status: 'prepared_storage_verified', node_count: receipt.node_count, target: `OHF matter-server ${TARGET_VERSION}` };
}
