import { readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, '..', '..');
const catalogPath = path.join(repoRoot, 'admin-ui/src/deviceAdminOperations.ts');
const sdkApiDir = path.join(repoRoot, 'sdk/lib/src/api');

const explicitExemptions = new Set([
  // Binary/download endpoints are handled by existing bespoke admin UI actions,
  // or intentionally excluded from the JSON proxy.
  'POST api/diag/debug-bundle',
  'GET api/diag/logs',
  'POST api/ota/upload',
  // Same-origin app discovery is not a remote-device setting surface.
  'GET api/discover',
  // Legacy ESP32-only endpoint; rhythm-server 404s it.
  'GET api/ota/version',
  // Owner-only Matter setup secrets must never become staff/admin operations.
  'GET api/matter/setup-code/{}',
]);

const catalog = readFileSync(catalogPath, 'utf8');
const catalogOperations = new Set(
  [...catalog.matchAll(/method:\s*'([A-Z]+)'[\s\S]*?path:\s*'([^']+)'/g)].map(
    (match) => operationKey(match[1], match[2]),
  ),
);

const sdkOperations = new Set();
for (const file of listFiles(sdkApiDir).filter((entry) =>
  entry.endsWith('.dart'),
)) {
  const text = readFileSync(file, 'utf8');
  collectSdkOperations(text, sdkOperations);
}

const missing = [...sdkOperations]
  .filter((sdkOperation) => !explicitExemptions.has(sdkOperation))
  .filter((sdkOperation) => !catalogOperations.has(sdkOperation))
  .sort();

if (missing.length > 0) {
  console.error('Device admin operation catalog is missing SDK operations:');
  for (const entry of missing) console.error(`  - ${entry}`);
  process.exit(1);
}

console.log(
  `Device admin catalog covers ${sdkOperations.size - explicitExemptions.size} JSON-capable SDK operation patterns.`,
);

function listFiles(dir) {
  const entries = [];
  for (const name of readdirSync(dir)) {
    const fullPath = path.join(dir, name);
    if (statSync(fullPath).isDirectory()) {
      entries.push(...listFiles(fullPath));
    } else {
      entries.push(fullPath);
    }
  }
  return entries;
}

function collectSdkOperations(text, operations) {
  const directCallPattern =
    /\b(?:_dio|dio|uploadDio|versionDio)\.(get|put|post|patch|delete)(?:<[^)]*>)?\(\s*'([^']+)'/gi;
  for (const match of text.matchAll(directCallPattern)) {
    addOperation(operations, match[1], match[2]);
  }

  const helperMethods = [
    ['PUT', /_safePut\(\s*'([^']+)'/g],
    ['PUT', /_putScopeLayerItems\(\s*'([^']+)'/g],
    ['POST', /_postMap\(\s*'([^']+)'/g],
    ['POST', /_postSceneAction\(\s*'([^']+)'/g],
    ['GET', /_getBundleJson\(\s*'([^']+)'/g],
    ['PUT', /_putBundleJson\(\s*'([^']+)'/g],
    ['POST', /_postBundleJson\(\s*'([^']+)'/g],
  ];
  for (const [method, pattern] of helperMethods) {
    for (const match of text.matchAll(pattern)) addOperation(operations, method, match[1]);
  }

  if (text.includes("_baseUri.resolve('api/backup')")) {
    operations.add(operationKey('GET', 'api/backup'));
    operations.add(operationKey('PUT', 'api/backup'));
  }
}

function addOperation(operations, method, value) {
  const normalized = normalizeSdkLiteral(value);
  if (normalized) operations.add(`${method.toUpperCase()} ${normalized}`);
}

function normalizeSdkLiteral(value) {
  if (value === 'health') return normalizePath(value);
  const apiIndex = value.indexOf('api/');
  if (apiIndex < 0) return null;
  return normalizePath(value.slice(apiIndex));
}

function operationKey(method, value) {
  return `${method.toUpperCase()} ${normalizePath(value)}`;
}

function normalizePath(value) {
  return value
    .trim()
    .replace(/^\/+/, '')
    .replace(/\$\{[^}]+\}/g, '{}')
    .replace(/\$[A-Za-z_][A-Za-z0-9_]*/g, '{}')
    .replace(/\{[^}]+\}/g, '{}');
}
