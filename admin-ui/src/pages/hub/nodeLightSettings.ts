export type JsonRecord = Record<string, unknown>;

export type LightSettingsProfile = {
  id: string;
  name: string;
  config: JsonRecord;
};

export type RgbColor = { r: number; g: number; b: number };

const PROFILE_OVERRIDE_FIELDS = [
  'curve',
  'min_color_temp',
  'max_color_temp',
  'min_brightness',
  'max_brightness',
  'max_dim_steps',
  'fade_ms',
  'motion_timeout_secs',
  'rhythm_interval_secs'
] as const;

const PROFILE_OVERRIDE_FIELD_SET = new Set<string>(PROFILE_OVERRIDE_FIELDS);

export function recordOf(value: unknown): JsonRecord {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as JsonRecord)
    : {};
}

export function profileOverridesFromNode(node: unknown): JsonRecord {
  const raw = recordOf(node);
  const settings = recordOf(raw.profile_settings ?? raw.room_profile);
  return cloneRecord(recordOf(settings.profile_overrides));
}

export function profileOverridesForNode(
  payload: unknown,
  nodeId: string
): JsonRecord | null {
  const root = recordOf(payload);
  const rawNodes = Array.isArray(payload)
    ? payload
    : Array.isArray(root.nodes)
      ? root.nodes
      : [];
  for (const candidate of rawNodes) {
    const node = recordOf(candidate);
    if (stringOf(node.id ?? node.node_id) === nodeId) {
      return profileOverridesFromNode(node);
    }
  }
  return null;
}

export function hasLightProfileOverrideCapability(payload: unknown): boolean {
  const root = recordOf(payload);
  const capabilities = recordOf(root.capabilities);
  const features = new Set(
    arrayOf(capabilities.features).map((feature) => stringOf(feature))
  );
  return (
    features.has('room_light_profile_overrides') &&
    features.has('guarded_room_light_profile_overrides')
  );
}

export function isLightAddressableKind(kind: string | undefined): boolean {
  return kind === 'room' || kind === 'light_device';
}

export function lightSettingsProfilesFromState(
  payload: unknown
): LightSettingsProfile[] {
  const root = recordOf(payload);
  const profiles = arrayOf(root.profiles)
    .map((value): LightSettingsProfile | null => {
      const config = recordOf(value);
      const id = stringOf(config.id);
      if (!id) return null;
      return {
        id,
        name: profileDisplayName(id, stringOf(config.name)),
        config: cloneRecord(config)
      };
    })
    .filter(
      (profile): profile is LightSettingsProfile => profile !== null
    );

  const appProfiles = profiles.filter((profile) =>
    ['rhythm', 'day', 'sleep'].includes(profile.id.toLowerCase())
  );
  const visible = appProfiles.length > 0 ? appProfiles : profiles;
  return visible
    .filter((profile) => !/(?:^|[_-])idle$/i.test(profile.id))
    .sort((left, right) => profilePriority(left.id) - profilePriority(right.id));
}

export function effectiveProfileConfig(
  base: JsonRecord,
  override: unknown
): JsonRecord {
  return {
    ...cloneRecord(base),
    ...cloneRecord(recordOf(override))
  };
}

/**
 * Build the exact selected-profile replacement used by the app contract.
 *
 * Known fields are stored only when they differ from the global profile.
 * Additive fields from a newer appliance are retained so an older admin UI
 * does not erase settings it cannot render.
 */
export function buildProfileOverride(
  base: JsonRecord,
  effective: JsonRecord,
  currentOverride: unknown
): JsonRecord | null {
  const next: JsonRecord = {};
  for (const [key, value] of Object.entries(recordOf(currentOverride))) {
    if (!PROFILE_OVERRIDE_FIELD_SET.has(key)) next[key] = cloneJson(value);
  }
  for (const field of PROFILE_OVERRIDE_FIELDS) {
    if (!jsonEqual(effective[field], base[field])) {
      next[field] = cloneJson(effective[field]);
    }
  }
  return Object.keys(next).length > 0 ? next : null;
}

/**
 * Build a node-local profile replacement in the presence of parent-room
 * inheritance. Most values remain sparse relative to the home profile. When a
 * child deliberately returns a parent-customized field to the home value, keep
 * that one explicit home value so the non-empty child profile replacement
 * masks the parent's whole-profile entry.
 */
export function buildInheritedProfileOverride(
  base: JsonRecord,
  effective: JsonRecord,
  currentLocalOverride: unknown,
  parentOverride: unknown
): JsonRecord | null {
  const parentEffective = effectiveProfileConfig(base, parentOverride);
  if (profileConfigsEqual(effective, parentEffective)) return null;

  const inheritedAndLocalOverride = {
    ...recordOf(parentOverride),
    ...recordOf(currentLocalOverride)
  };
  const sparse = buildProfileOverride(
    base,
    effective,
    inheritedAndLocalOverride
  );
  if (sparse !== null) return sparse;

  for (const field of PROFILE_OVERRIDE_FIELDS) {
    if (
      effective[field] !== undefined &&
      !jsonEqual(effective[field], parentEffective[field])
    ) {
      return { [field]: cloneJson(effective[field]) };
    }
  }
  return null;
}

export function withSelectedProfileOverride(
  current: JsonRecord,
  profileId: string,
  override: JsonRecord | null
): JsonRecord {
  const next = cloneRecord(current);
  if (override === null) {
    delete next[profileId];
  } else {
    next[profileId] = cloneRecord(override);
  }
  return next;
}

/**
 * Node state exposes effective settings after parent-room inheritance. A
 * profile whose effective value is byte-for-byte equal to its parent's value
 * is therefore treated as inherited; a different whole-profile value is the
 * node-local replacement used by RoomProfileSettings::merged_with_parent.
 */
export function inferredLocalProfileOverrides(
  effective: JsonRecord,
  parent: JsonRecord
): JsonRecord {
  return Object.fromEntries(
    Object.entries(effective)
      .filter(
        ([profileId, value]) =>
          !profileOverrideMapsEqual(
            recordOf(value),
            recordOf(parent[profileId])
          )
      )
      .map(([profileId, value]) => [profileId, cloneJson(value)])
  );
}

export function changedOverrideFields(
  before: unknown,
  after: unknown
): string[] {
  const beforeRecord = recordOf(before);
  const afterRecord = recordOf(after);
  return [...new Set([...Object.keys(beforeRecord), ...Object.keys(afterRecord)])]
    .filter((key) => !jsonEqual(beforeRecord[key], afterRecord[key]))
    .sort();
}

export function profileConfigsEqual(
  left: JsonRecord,
  right: JsonRecord
): boolean {
  return jsonEqual(left, right);
}

export function profileOverrideMapsEqual(
  left: JsonRecord,
  right: JsonRecord
): boolean {
  return jsonEqual(left, right);
}

export function isSleepProfile(profileId: string): boolean {
  return profileId.toLowerCase() === 'sleep';
}

export function directColorRgb(config: JsonRecord): RgbColor | null {
  const curve = recordOf(config.curve);
  const directColor = recordOf(curve.direct_color);
  const rgb = recordOf(directColor.rgb);
  const r = numberOf(rgb.r);
  const g = numberOf(rgb.g);
  const b = numberOf(rgb.b);
  if (r === undefined || g === undefined || b === undefined) return null;
  return {
    r: clampByte(r),
    g: clampByte(g),
    b: clampByte(b)
  };
}

export function withDirectColor(
  config: JsonRecord,
  color: RgbColor
): JsonRecord {
  const rgb = {
    r: clampByte(color.r),
    g: clampByte(color.g),
    b: clampByte(color.b)
  };
  const curve = recordOf(config.curve);
  return {
    ...config,
    curve: {
      ...curve,
      direct_color: {
        ...recordOf(curve.direct_color),
        rgb,
        xy: rgbToXy(rgb)
      }
    }
  };
}

export function createLightSettingsJourneyId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return `admin-light-settings:${crypto.randomUUID()}`;
  }
  return `admin-light-settings:${Date.now()}:${Math.random()
    .toString(16)
    .slice(2)}`;
}

function profileDisplayName(id: string, reported: string | undefined): string {
  const normalized = id.toLowerCase();
  if (normalized === 'rhythm' || normalized === 'day') return 'Day';
  if (normalized === 'sleep') return 'Sleep';
  return reported ?? id;
}

function profilePriority(id: string): number {
  const normalized = id.toLowerCase();
  if (normalized === 'rhythm' || normalized === 'day') return 0;
  if (normalized === 'sleep') return 1;
  return 2;
}

function rgbToXy({ r, g, b }: RgbColor): { x: number; y: number } {
  const linearize = (value: number) => {
    const scaled = clampByte(value) / 255;
    return scaled <= 0.04045
      ? scaled / 12.92
      : Math.pow((scaled + 0.055) / 1.055, 2.4);
  };
  const red = linearize(r);
  const green = linearize(g);
  const blue = linearize(b);
  const x = red * 0.4124564 + green * 0.3575761 + blue * 0.1804375;
  const y = red * 0.2126729 + green * 0.7151522 + blue * 0.072175;
  const z = red * 0.0193339 + green * 0.119192 + blue * 0.9503041;
  const sum = x + y + z;
  return {
    x: round4(sum > 0 ? x / sum : 0.3127),
    y: round4(sum > 0 ? y / sum : 0.329)
  };
}

function round4(value: number): number {
  return Number(value.toFixed(4));
}

function cloneRecord(value: JsonRecord): JsonRecord {
  return cloneJson(value) as JsonRecord;
}

function cloneJson(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(cloneJson);
  if (value !== null && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value as JsonRecord).map(([key, entry]) => [
        key,
        cloneJson(entry)
      ])
    );
  }
  return value;
}

function jsonEqual(left: unknown, right: unknown): boolean {
  return canonicalJson(left) === canonicalJson(right);
}

function canonicalJson(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.map(canonicalJson).join(',')}]`;
  }
  if (value !== null && typeof value === 'object') {
    return `{${Object.entries(value as JsonRecord)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, entry]) => `${JSON.stringify(key)}:${canonicalJson(entry)}`)
      .join(',')}}`;
  }
  const serialized = JSON.stringify(value);
  return serialized === undefined ? 'undefined' : serialized;
}

function arrayOf(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

function stringOf(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim().length > 0
    ? value.trim()
    : undefined;
}

function numberOf(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value)
    ? value
    : undefined;
}

function clampByte(value: number): number {
  return Math.max(0, Math.min(255, Math.round(value)));
}
