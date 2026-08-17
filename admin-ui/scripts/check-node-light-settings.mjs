import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

import ts from 'typescript';

const sourceUrl = new URL(
  '../src/pages/hub/nodeLightSettings.ts',
  import.meta.url,
);
const source = await readFile(sourceUrl, 'utf8');
const pageSource = await readFile(
  new URL('../src/pages/hub/NodesPage.tsx', import.meta.url),
  'utf8',
);
const compiled = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
  },
  fileName: sourceUrl.pathname,
}).outputText;
const moduleUrl = `data:text/javascript;base64,${Buffer.from(compiled).toString('base64')}`;
const lightSettings = await import(moduleUrl);

const {
  buildProfileOverride,
  buildInheritedProfileOverride,
  changedOverrideFields,
  directColorRgb,
  effectiveProfileConfig,
  inferredLocalProfileOverrides,
  isLightAddressableKind,
  lightProfileOverrideSupport,
  lightSettingsProfilesFromState,
  profileOverrideMapsEqual,
  profileOverridesForNode,
  profileOverridesFromNode,
  serverInstanceIdFromState,
  withDirectColor,
  withSelectedProfileOverride,
} = lightSettings;

const state = {
  capabilities: {
    api_schema_version: 2,
    features: [
      'motion_activation_toggle',
      'room_light_profile_overrides',
      'guarded_room_light_profile_overrides',
      'target_guarded_room_light_profile_overrides',
    ],
  },
  profiles: [
    {
      id: 'sleep_idle',
      name: 'Sleep Idle',
      min_brightness: 1,
      max_brightness: 1,
    },
    {
      id: 'sleep',
      name: 'Night',
      min_brightness: 20,
      max_brightness: 20,
      min_color_temp: 2200,
      max_color_temp: 2200,
      curve: {
        type: 'constant',
        brightness: 0,
        color_temp: 0,
      },
    },
    {
      id: 'rhythm',
      name: 'Rhythm',
      min_brightness: 10,
      max_brightness: 90,
      min_color_temp: 2200,
      max_color_temp: 6000,
      curve: { type: 'super-gaussian', shape_p: 6 },
    },
  ],
};

assert.equal(
  lightProfileOverrideSupport(state),
  'target_guarded',
  'prefers the target-specific compare-and-set capability',
);
assert.equal(
  lightProfileOverrideSupport({
    ...state,
    capabilities: {
      ...state.capabilities,
      features: state.capabilities.features.filter(
        (feature) => feature !== 'target_guarded_room_light_profile_overrides',
      ),
    },
  }),
  'guarded',
  'keeps the legacy whole-resource guard for previous appliances',
);
assert.equal(
  lightProfileOverrideSupport({
    ...state,
    capabilities: {
      ...state.capabilities,
      features: ['room_light_profile_overrides'],
    },
  }),
  'unguarded',
  'distinguishes base profile support from queued compare-and-set support',
);
assert.equal(
  lightProfileOverrideSupport({
    ...state,
    capabilities: {
      ...state.capabilities,
      features: [],
    },
  }),
  'unsupported',
  'distinguishes appliances without profile-override support',
);
assert.deepEqual(
  lightSettingsProfilesFromState(state).map(({ id, name }) => ({ id, name })),
  [
    { id: 'rhythm', name: 'Day' },
    { id: 'sleep', name: 'Sleep' },
  ],
  'matches the app Day/Sleep profile order and omits idle profiles',
);
assert.equal(isLightAddressableKind('room'), true);
assert.equal(isLightAddressableKind('light_device'), true);
assert.equal(isLightAddressableKind('motion_sensor'), false);
assert.equal(
  serverInstanceIdFromState({ server_instance_id: 'server-1' }),
  'server-1',
);
assert.equal(
  serverInstanceIdFromState({ serverInstanceId: 'server-2' }),
  'server-2',
);

const day = state.profiles[2];
const existingOverride = {
  min_brightness: 15,
  max_brightness: 75,
  future_appliance_field: { enabled: true },
};
const effective = effectiveProfileConfig(day, existingOverride);
assert.equal(effective.min_brightness, 15);
assert.equal(effective.max_brightness, 75);
assert.equal(effective.max_color_temp, 6000);

const edited = {
  ...effective,
  min_brightness: 20,
  max_brightness: 80,
  max_color_temp: 5500,
};
assert.deepEqual(
  buildProfileOverride(day, edited, existingOverride),
  {
    future_appliance_field: { enabled: true },
    max_color_temp: 5500,
    min_brightness: 20,
    max_brightness: 80,
  },
  'stores a sparse replacement and preserves fields from a newer appliance',
);
assert.equal(
  buildProfileOverride(day, effectiveProfileConfig(day, null), null),
  null,
  'returning known values to the global profile removes an empty override',
);
assert.deepEqual(
  changedOverrideFields(
    { min_brightness: 15, max_brightness: 75 },
    { min_brightness: 20, max_brightness: 80 },
  ),
  ['max_brightness', 'min_brightness'],
);

assert.deepEqual(
  withSelectedProfileOverride(
    { rhythm: existingOverride, sleep: { max_brightness: 8 } },
    'rhythm',
    null,
  ),
  { sleep: { max_brightness: 8 } },
  'profile reset removes only the selected profile',
);
assert.equal(
  profileOverrideMapsEqual(
    { rhythm: { min_brightness: 8, max_brightness: 70 } },
    { rhythm: { max_brightness: 70, min_brightness: 8 } },
  ),
  true,
  'freshness comparison is stable across object key order',
);
assert.deepEqual(
  inferredLocalProfileOverrides(
    {
      rhythm: { min_brightness: 8 },
      sleep: { max_brightness: 10 },
    },
    { rhythm: { min_brightness: 8 } },
  ),
  { sleep: { max_brightness: 10 } },
  'distinguishes a bulb-local profile from an inherited room profile',
);
assert.equal(
  buildInheritedProfileOverride(
    day,
    effectiveProfileConfig(day, { min_brightness: 8 }),
    {},
    { min_brightness: 8 },
  ),
  null,
  'keeps an unchanged bulb inherited from its room',
);
assert.deepEqual(
  buildInheritedProfileOverride(
    day,
    day,
    {},
    { min_brightness: 8 },
  ),
  { min_brightness: 10 },
  'can deliberately return a parent-customized field to the home value',
);
assert.deepEqual(
  buildInheritedProfileOverride(
    day,
    {
      ...day,
      min_brightness: 8,
      max_brightness: 80,
    },
    {},
    {
      min_brightness: 8,
      future_transition_shape: { kind: 'gentle' },
    },
  ),
  {
    min_brightness: 8,
    max_brightness: 80,
    future_transition_shape: { kind: 'gentle' },
  },
  'preserves additive parent fields when a bulb creates its own profile',
);
assert.deepEqual(
  withSelectedProfileOverride(
    {
      rhythm: { min_brightness: 5 },
      sleep: { max_brightness: 10 },
    },
    'sleep',
    { max_brightness: 20 },
  ),
  {
    rhythm: { min_brightness: 5 },
    sleep: { max_brightness: 20 },
  },
  'models a bulb reset as falling back to the parent effective profile',
);

const coloredSleep = withDirectColor(state.profiles[1], {
  r: 255,
  g: 107,
  b: 74,
});
assert.deepEqual(
  directColorRgb(coloredSleep),
  { r: 255, g: 107, b: 74 },
);
assert.equal(coloredSleep.curve.type, 'constant');
assert.deepEqual(coloredSleep.curve.direct_color.xy, {
  x: 0.5292,
  y: 0.3578,
});

const nodesPayload = {
  nodes: [
    {
      id: 'kitchen',
      kind: 'room',
      profile_settings: {
        profile_overrides: {
          rhythm: { min_brightness: 8 },
        },
      },
    },
  ],
};
assert.deepEqual(profileOverridesForNode(nodesPayload, 'kitchen'), {
  rhythm: { min_brightness: 8 },
});
assert.deepEqual(profileOverridesFromNode(nodesPayload.nodes[0]), {
  rhythm: { min_brightness: 8 },
});
assert.equal(profileOverridesForNode(nodesPayload, 'missing'), null);

assert.match(
  pageSource,
  /lightProfileOverrideSupport\(latestState\)/,
  'rechecks the live capability immediately before a guarded write',
);
assert.match(
  pageSource,
  /supports per-room light settings, but its current build cannot safely guard admin edits/,
  'explains unguarded appliances without falsely claiming profile settings are unsupported',
);
assert.match(
  pageSource,
  /expectedProfileOverrides: baselineOverrides/,
  'sends the reviewed effective override map for appliance-side queued compare-and-set',
);
assert.match(
  pageSource,
  /latestSupport === 'guarded'[\s\S]*resourcePrecondition/,
  'uses the volatile whole-resource hash only for legacy guarded appliances',
);
assert.match(
  pageSource,
  /expectedServerInstanceId/,
  'pins the reviewed server identity without another client-side state fetch',
);
assert.match(
  pageSource,
  /function adoptLatestCanonicalState\(\)[\s\S]*setBaselineBase\(nextBase\)[\s\S]*setBaselineOverrides\(nextOverrides\)[\s\S]*setBaselineParentOverrides\(nextParentOverrides\)/,
  'lets a stale draft adopt the latest canonical base, target, and parent state',
);

console.log('Admin room/node light-settings contract checks passed.');
