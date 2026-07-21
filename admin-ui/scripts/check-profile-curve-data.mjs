import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';

import ts from 'typescript';

const sourceUrl = new URL('../src/pages/hub/profileCurveData.ts', import.meta.url);
const source = await readFile(sourceUrl, 'utf8');
const compiled = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022
  },
  fileName: pathToFileURL(sourceUrl.pathname).href
}).outputText;
const moduleUrl = `data:text/javascript;base64,${Buffer.from(compiled).toString('base64')}`;
const { groupProfileTabs, parseCurveSamples } = await import(moduleUrl);

assert.deepEqual(
  groupProfileTabs([
    { id: 'day_idle', name: 'Day Idle' },
    { id: 'rhythm', name: 'Day' },
    { id: 'sleep', name: 'Sleep' },
    { id: 'sleep_idle', name: 'Sleep Idle' }
  ]),
  {
    primary: [
      { id: 'rhythm', name: 'Day' },
      { id: 'sleep', name: 'Sleep' }
    ],
    secondary: [
      { id: 'day_idle', name: 'Day Idle' },
      { id: 'sleep_idle', name: 'Sleep Idle' }
    ]
  },
  'keeps Day and Sleep ahead of the secondary idle profiles'
);

assert.deepEqual(
  parseCurveSamples({
    config: { id: 'rhythm' },
    curve: {
      hours: [0, 6, 12],
      brightness: [1, 25, 90],
      kelvin: [2200, 3500, 6000]
    }
  }),
  {
    brightness: [
      { hour: 0, value: 1 },
      { hour: 6, value: 25 },
      { hour: 12, value: 90 }
    ],
    kelvin: [
      { hour: 0, value: 2200 },
      { hour: 6, value: 3500 },
      { hour: 12, value: 6000 }
    ]
  },
  'parses the appliance CurveResponse.curve payload'
);

assert.deepEqual(
  parseCurveSamples({ hours: [0, 12], brightness: [5, 80], kelvin: [2200, 5000] }),
  {
    brightness: [{ hour: 0, value: 5 }, { hour: 12, value: 80 }],
    kelvin: [{ hour: 0, value: 2200 }, { hour: 12, value: 5000 }]
  },
  'retains support for flat sample payloads'
);

console.log('Profile curve payload parser checks passed.');
