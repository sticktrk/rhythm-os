#!/usr/bin/env node
import { parseArgs } from 'node:util';
import { MigrationError } from './source.mjs';

const HELP = `Prepare native Home Assistant matter.js storage from a Rhythm cold backup.

node migrate.mjs --source BACKUP_OR_DATA_DIR --target-version 1.5.5 --dry-run
node migrate.mjs --source BACKUP_OR_DATA_DIR --target-version 1.5.5 --output NEW_DIRECTORY
node migrate.mjs --verify PREPARED_DIRECTORY

The source is read-only. Output must not exist. No device or network connections
are made. Target version is the OHF matter-server version, not the HA Core or app
version. Read README.md before cutover; both controllers must not run together.
`;
try {
  const { values: args } = parseArgs({ options: {
    source: { type: 'string' }, output: { type: 'string' }, 'target-version': { type: 'string' },
    'dry-run': { type: 'boolean' }, verify: { type: 'string' }, help: { type: 'boolean' },
  } });
  if (args.help) { console.log(HELP); }
  else {
    if (args.verify ? (args.source || args.output || args['target-version'] || args['dry-run']) :
      (!args.source || !args['target-version'] || Boolean(args.output) === Boolean(args['dry-run']))) {
      throw new MigrationError('Choose dry-run, conversion to a new output, or verification; see --help.');
    }
    process.umask(0o077);
    // matter.js has its own CLI/environment parser; never pass migration flags to it.
    process.argv = process.argv.slice(0, 2);
    const { validateSource, prepare, summary, verify } = await import('./migration.mjs');
    if (args.verify) console.log(JSON.stringify(await verify(args.verify)));
    else {
      const validated = await validateSource(args.source, args['target-version']);
      if (args['dry-run']) console.log(JSON.stringify(summary(validated)));
      else {
        await prepare(validated, args.output);
        console.log(JSON.stringify({ ...summary(validated), status: 'prepared_offline_not_connected' }));
      }
    }
  }
} catch (error) {
  // Never echo raw dependency/JSON/crypto exceptions, paths or source contents.
  const message = error instanceof MigrationError ? error.message :
    error.code === 'EEXIST' ? 'Output already exists. Use a new directory; existing data was not overwritten.' :
    error.code === 'ENOENT' ? 'A required file or parent directory is missing. See README.md.' :
    'Migration failed. Check source completeness, file permissions and pinned dependencies; no live installation was changed.';
  console.error(message);
  process.exitCode = 1;
}
