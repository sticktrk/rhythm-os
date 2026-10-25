import { createHash } from 'node:crypto';
import { constants } from 'node:fs';
import { lstat, open, readdir } from 'node:fs/promises';
import { join, resolve } from 'node:path';

export class MigrationError extends Error {}
export function requireThat(condition, message) {
  if (!condition) throw new MigrationError(message);
}
const LIMIT = 64 * 1024 * 1024;
const IDENTITY = 'matter/fabric-identity.json';
const DEVICES = 'matter/chip/devices.json';
const CONTROLLERS = [
  'matter/chip/chip_tool_config.controller-storage.ini',
  'matter/chip/controller-storage.json',
  'matter/chip/chip_tool_config.ini',
  'matter/chip/chip_tool_config.chip.ini',
  'matter/chip/chip.json',
];

// Node 22's reviver source preserves u64 tokens before floating-point rounding.
export function parseJson(text) {
  try {
    return JSON.parse(text, (_key, value, context) => {
      if (typeof value !== 'number') return value;
      requireThat(Number.isFinite(value), 'Non-finite JSON number.');
      if (Number.isInteger(value) && !Number.isSafeInteger(value)) {
        requireThat(/^-?\d+$/.test(context.source), 'Unsafe JSON number encoding.');
        return BigInt(context.source);
      }
      return value;
    });
  } catch {
    throw new MigrationError('Invalid JSON or unsupported number encoding.');
  }
}

export function exactId(value, maximum = 0xffffffffffffffffn) {
  requireThat(typeof value === 'bigint' || (typeof value === 'number' && Number.isSafeInteger(value)) ||
    (typeof value === 'string' && /^(0x[0-9a-fA-F]+|[0-9]+)$/.test(value)), 'Invalid integer identity.');
  const id = BigInt(value);
  requireThat(id > 0n && id <= maximum, 'Identity outside the supported range.');
  return id;
}

async function readRegular(path) {
  const file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const stat = await file.stat();
    requireThat(stat.isFile() && stat.size <= LIMIT, 'Expected a regular file of at most 64 MiB.');
    const data = await file.readFile();
    requireThat(data.length <= LIMIT, 'Source file exceeds 64 MiB.');
    return new TextDecoder('utf-8', { fatal: true }).decode(data);
  } finally { await file.close(); }
}

function validPath(path) {
  return typeof path === 'string' && path.length < 1024 &&
    !path.includes('\\') && !path.includes('\0') &&
    path.split('/').every(part => part && part !== '.' && part !== '..') && !path.startsWith('/');
}

export async function readSource(source) {
  const absolute = resolve(source);
  const stat = await lstat(absolute);
  requireThat(!stat.isSymbolicLink(), 'Source must not be a symlink.');
  const files = new Map();
  if (stat.isDirectory()) {
    // Do not recursively traverse unrelated installation data or follow symlinks.
    for (const path of ['matter', 'matter/chip']) {
      const info = await lstat(join(absolute, path));
      requireThat(info.isDirectory() && !info.isSymbolicLink(), 'Expected a real Matter data directory.');
    }
    const names = await readdir(join(absolute, 'matter/chip'));
    requireThat(!names.some(name => /^(chip_tool_config.*\.ini|.*storage\.json|chip\.json)$/.test(name) &&
      !CONTROLLERS.includes(`matter/chip/${name}`)), 'Unrecognized controller store; review the source version.');
    files.set(IDENTITY, await readRegular(join(absolute, IDENTITY)));
    files.set(DEVICES, await readRegular(join(absolute, DEVICES)));
    for (const path of CONTROLLERS) {
      try { files.set(path, await readRegular(join(absolute, path))); }
      catch (error) { if (error.code !== 'ENOENT') throw error; }
    }
  } else {
    requireThat(stat.isFile(), 'Expected a backup JSON file or cold data directory.');
    const backup = parseJson(await readRegular(absolute));
    requireThat([1, 2, 3].includes(backup.schema_version ?? 1) &&
      (backup.kind === undefined || backup.kind === 'backup_bundle'), 'Unsupported backup schema or kind.');
    requireThat(backup.secrets_included === true, 'A full backup with secrets is required.');
    const entries = backup.installation?.integration_files;
    requireThat(Array.isArray(entries) && entries.length <= 10000, 'Backup has no supported integration files.');
    const seen = new Set();
    for (const entry of entries) {
      requireThat(validPath(entry.path) && !seen.has(entry.path), 'Unsafe or duplicate integration-file path.');
      seen.add(entry.path);
      if (entry.path === IDENTITY || entry.path === DEVICES || CONTROLLERS.includes(entry.path)) {
        requireThat(typeof entry.content === 'string', 'Integration file content must be text.');
        files.set(entry.path, entry.content);
      } else if (/^matter\/chip\/.*(\.ini|storage\.json|chip\.json)$/.test(entry.path)) {
        throw new MigrationError('Unrecognized controller store; review the source version.');
      }
    }
  }
  requireThat(files.has(IDENTITY) && files.has(DEVICES), 'Missing Matter identity or commissioned-device inventory.');
  const controllers = CONTROLLERS.filter(path => files.has(path));
  requireThat(controllers.length === 1, 'Expected exactly one controller store; missing or ambiguous source.');
  const total = [...files.values()].reduce((sum, text) => sum + Buffer.byteLength(text), 0);
  requireThat(total <= LIMIT, 'Matter source exceeds 64 MiB.');
  const hash = createHash('sha256');
  for (const [path, text] of [...files].sort(([a], [b]) => a.localeCompare(b))) {
    hash.update(path).update('\0').update(text).update('\0');
  }
  return {
    sourcePath: absolute,
    sourceDigest: hash.digest('hex'),
    identity: parseJson(files.get(IDENTITY)),
    inventory: parseJson(files.get(DEVICES)),
    sdk: parseController(files.get(controllers[0]), controllers[0].endsWith('.ini')),
  };
}

export function parseController(text, ini) {
  let entries;
  if (ini) {
    entries = [];
    let section;
    for (const raw of text.split(/\r?\n/)) {
      const line = raw.trim();
      if (!line || line.startsWith(';') || line.startsWith('#')) continue;
      if (line.startsWith('[')) {
        requireThat(line === '[Default]' && section === undefined, 'Unsupported CHIP INI section.');
        section = 'Default';
        continue;
      }
      const separator = line.indexOf('=');
      requireThat(section === 'Default' && separator > 0, 'Malformed CHIP INI record.');
      const encoded = line.slice(0, separator).trim();
      requireThat(!/\\(?!x[0-9a-f]{2})/.test(encoded), 'Invalid CHIP key escape.');
      const key = encoded.replace(/\\x([0-9a-f]{2})/g, (_match, hex) => String.fromCharCode(parseInt(hex, 16)));
      entries.push([key, line.slice(separator + 1).trim()]);
    }
  } else {
    const config = parseJson(text);
    requireThat(config['sdk-config'] && typeof config['sdk-config'] === 'object' &&
      !Array.isArray(config['sdk-config']), 'Unsupported CHIP JSON store.');
    entries = Object.entries(config['sdk-config']);
  }
  const result = Object.create(null);
  requireThat(entries.length > 0 && entries.length <= 100000, 'Empty or oversized controller store.');
  for (const [key, value] of entries) {
    requireThat(key.length > 0 && !Object.hasOwn(result, key), 'Duplicate or empty CHIP key.');
    requireThat(typeof value === 'string' && /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value) &&
      Buffer.from(value, 'base64').toString('base64') === value, 'Invalid base64 in controller store.');
    result[key] = value;
  }
  return result;
}
