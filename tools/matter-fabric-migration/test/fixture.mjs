import '../bootstrap.mjs';
import '@matter-server/custom-clusters';
import '@matter/nodejs';
import { FabricId, FabricIndex, NodeId, StandardCrypto } from '@matter/main';
import { VendorId, CaseAuthenticatedTag } from '@matter/main/types';
import { CertificateAuthority, FabricBuilder, Rcac, Icac } from '@matter/main/protocol';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { mkdir, writeFile } from 'node:fs/promises';

const server = fileURLToPath(import.meta.resolve('matter-server'));
const { TlvFabricMetadata, TlvGroupKeySet } = await import(pathToFileURL(join(dirname(server), 'converter/TlvSchemas.js')));
export const stringify = data => JSON.stringify(data, (_key, value) =>
  typeof value === 'bigint' ? JSON.rawJSON(value.toString()) : value, 2);
export const LARGE_FABRIC = 0xfedcba9876543211n;
export const LARGE_NODE = 0x20000000000003n;
const b64 = value => Buffer.from(value).toString('base64');
const pairBytes = pair => Buffer.concat([pair.publicKey, pair.privateKey]);

export async function fixture(directory, { legacy = false, intermediate = true, cats = false } = {}) {
  const crypto = new StandardCrypto();
  const ca = await CertificateAuthority.create(crypto, intermediate);
  const builder = await FabricBuilder.create(crypto);
  await builder.setRootCert(ca.rootCert);
  builder.setRootNodeId(NodeId(112233)).setIdentityProtectionKey(crypto.randomBytes(16))
    .setRootVendorId(VendorId(0xfff1)).setLabel('Synthetic');
  await builder.setOperationalCert(await ca.generateNoc(builder.publicKey, FabricId(LARGE_FABRIC), NodeId(112233),
    cats ? [CaseAuthenticatedTag(0x00010001)] : undefined), ca.icacCert);
  const fabric = await builder.build(FabricIndex(1));
  const config = ca.config;
  const sdk = {
    ExampleOpCredsCAKey0: b64(pairBytes(config.rootKeyPair)), ExampleCARootCert0: b64(Rcac.fromTlv(config.rootCertBytes).asSignedDer()),
    'f/1/r': b64(fabric.rootCert), 'f/1/n': b64(fabric.operationalCert),
    'f/1/m': b64(TlvFabricMetadata.encode({ vendorId: 0xfff1, label: 'Synthetic' })),
    'f/1/k/0': b64(TlvGroupKeySet.encode({ policy: 0, keyCount: 1, groupKeySetId: 0,
      keys: [{ startTime: 0n, keyHash: 0, key: fabric.operationalIdentityProtectionKey }] })),
  };
  if (intermediate) Object.assign(sdk, {
    ExampleOpCredsICAKey0: b64(pairBytes(config.icacKeyPair)), ExampleCAIntermediateCert0: b64(Icac.fromTlv(config.icacCertBytes).asSignedDer()),
    'f/1/i': b64(fabric.intermediateCACert),
  });
  const devices = [7n, LARGE_NODE].map(node_id => ({ node_id, light_endpoint: 1, vendor_name: 'Synthetic vendor',
    product_name: 'Test light', vendor_id: 0xfff1, product_id: 1, color_modes: ['color_temperature'] }));
  const identity = { schema_version: 1, label: 'test-fabric', operational_fabric_id: LARGE_FABRIC,
    ipk_hex: Buffer.from(fabric.identityProtectionKey).toString('hex') };
  const inventory = legacy ? devices : { schema_version: 1, compressed_fabric_id: fabric.globalId.toString(16), devices };
  const ini = '[Default]\n' + Object.entries(sdk).map(([key, value]) => `${key}=${value}`).join('\n') + '\n';
  const files = [
    { path: 'matter/fabric-identity.json', content: stringify(identity), secret: true },
    { path: 'matter/chip/devices.json', content: stringify(inventory), secret: true },
    { path: 'matter/chip/chip_tool_config.controller-storage.ini', content: ini, secret: true },
  ];
  const backup = { schema_version: 3, kind: 'backup_bundle', secrets_included: true,
    created_at: '2026-01-01T00:00:00Z', installation: { integration_files: files } };
  const backupPath = join(directory, 'backup.json');
  await writeFile(backupPath, stringify(backup), { mode: 0o600 });
  const cold = join(directory, 'cold');
  await mkdir(join(cold, 'matter/chip'), { recursive: true, mode: 0o700 });
  for (const file of files) await writeFile(join(cold, file.path), file.content, { mode: 0o600 });
  return { backupPath, backup, cold, fabric, sdk, identity, inventory };
}
