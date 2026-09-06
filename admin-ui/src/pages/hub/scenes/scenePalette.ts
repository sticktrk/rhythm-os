import {
  hsvToRgb,
  kelvinToRgb,
  rgbToHsv,
  type Rgb
} from '../../../components/controls/colorMath.ts';
import {
  asBoolean,
  asNumber,
  asRecord,
  asRecordArray,
  asString
} from '../../../device/values.ts';
import { isBulbKind, isRoomKind } from '../topologyMembership.ts';

/** A scene output as the server renders it: power, brightness and a colour
    that is either a colour temperature or an RGB triple. Mirrors
    `LightSceneOutput` in rhythm-os. */
export type SceneOutput = {
  on: boolean;
  brightness: number;
  kelvin?: number;
  rgb?: Rgb;
  transitionMs?: number;
};

/** `spread` walks the anchor path so every light differs; `cycle` deals the
    anchors out and repeats; `shuffle` deals the spread colours in a random
    order fixed by the layer's `palette_seed`. */
export type PaletteMode = 'spread' | 'cycle' | 'shuffle';

export function parsePaletteMode(value: unknown): PaletteMode {
  return value === 'cycle' || value === 'shuffle' ? value : 'spread';
}

/** The seed a shuffled layer deals with; zero (the wire default) when unset. */
export function parsePaletteSeed(value: unknown): number {
  const seed = asNumber(value);
  return seed === undefined || !Number.isFinite(seed) ? 0 : Math.floor(seed) >>> 0;
}

/** A fresh non-zero 32-bit seed for a re-roll. */
export function newPaletteSeed(): number {
  const bytes = new Uint32Array(1);
  if (typeof crypto !== 'undefined' && typeof crypto.getRandomValues === 'function') {
    crypto.getRandomValues(bytes);
  } else {
    bytes[0] = Math.floor(Math.random() * 0xffffffff);
  }
  return bytes[0] === 0 ? 1 : bytes[0];
}

/** mulberry32: the same generator `rhythm-os` runs, so a shuffled preview
    matches what the house receives slot for slot. Keep the two in step. */
export function mulberry32(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return (t ^ (t >>> 14)) >>> 0;
  };
}

/** The Fisher-Yates order of `0..span` under `seed`: entry `i` is the spread
    slot that shuffled slot `i` takes. Mirrors `shuffled_slot` in rhythm-os. */
export function slotPermutation(seed: number, span: number): number[] {
  const order = Array.from({ length: Math.max(0, span) }, (_, index) => index);
  const next = mulberry32(seed);
  for (let index = order.length - 1; index > 0; index -= 1) {
    const swapWith = next() % (index + 1);
    [order[index], order[swapWith]] = [order[swapWith], order[index]];
  }
  return order;
}

export function parseOutput(record: Record<string, unknown>): SceneOutput {
  const color = asRecord(record.color);
  const rgbRecord = asRecord(color.rgb);
  const rgb =
    asNumber(rgbRecord.r) !== undefined
      ? {
          r: asNumber(rgbRecord.r) ?? 0,
          g: asNumber(rgbRecord.g) ?? 0,
          b: asNumber(rgbRecord.b) ?? 0
        }
      : undefined;
  const powerValue = record.power;
  const on =
    typeof powerValue === 'string'
      ? powerValue !== 'off'
      : (asBoolean(powerValue) ?? asBoolean(record.on) ?? true);
  return {
    on,
    brightness: Math.min(100, Math.max(1, asNumber(record.brightness) ?? 80)),
    kelvin: asNumber(color.kelvin),
    rgb,
    transitionMs: asNumber(record.transition_ms)
  };
}

export function outputToRecord(output: SceneOutput): Record<string, unknown> {
  return {
    power: output.on ? 'on' : 'off',
    brightness: Math.round(output.brightness),
    ...(output.rgb
      ? { color: { rgb: output.rgb } }
      : output.kelvin !== undefined
        ? { color: { kelvin: Math.round(output.kelvin) } }
        : {}),
    ...(output.transitionMs !== undefined
      ? { transition_ms: output.transitionMs }
      : {})
  };
}

/** The colour a swatch should show for an output. */
export function outputRgb(output: SceneOutput): Rgb {
  if (output.rgb) return output.rgb;
  if (output.kelvin !== undefined) return kelvinToRgb(output.kelvin);
  return { r: 255, g: 214, b: 170 };
}

function lerp(from: number, to: number, fraction: number): number {
  return from + (to - from) * fraction;
}

/** Blend two colours along the shortest arc of the hue wheel, as the server
    does: orange to purple passes through red rather than fading through
    grey. A near-grey side borrows the other side's hue. */
export function blendRgbOnHueWheel(from: Rgb, to: Rgb, fraction: number): Rgb {
  const a = rgbToHsv(from);
  const b = rgbToHsv(to);
  const fromHue = a.s < 0.05 ? b.h : a.h;
  const toHue = b.s < 0.05 ? fromHue : b.h;
  const delta = ((((toHue - fromHue + 540) % 360) + 360) % 360) - 180;
  return hsvToRgb({
    h: fromHue + delta * fraction,
    s: lerp(a.s, b.s, fraction),
    v: lerp(a.v, b.v, fraction)
  });
}

/** The output `fraction` of the way from `from` to `to`. Brightness blends
    linearly, RGB colours blend around the hue wheel, colour temperatures
    blend linearly; anything else (an off anchor, mismatched kinds) falls
    back to `from`. Mirrors `blend_outputs` in rhythm-os. */
export function blendOutputs(
  from: SceneOutput,
  to: SceneOutput,
  fraction: number
): SceneOutput {
  if (fraction <= 1e-6) return from;
  if (fraction >= 1 - 1e-6) return to;
  if (!from.on || !to.on) return from;
  const brightness = Math.min(
    100,
    Math.max(1, Math.round(lerp(from.brightness, to.brightness, fraction)))
  );
  if (from.rgb && to.rgb) {
    return {
      on: true,
      brightness,
      rgb: blendRgbOnHueWheel(from.rgb, to.rgb, fraction),
      transitionMs: from.transitionMs
    };
  }
  if (
    from.kelvin !== undefined &&
    to.kelvin !== undefined &&
    !from.rgb &&
    !to.rgb
  ) {
    return {
      on: true,
      brightness,
      kelvin: Math.round(lerp(from.kelvin, to.kelvin, fraction)),
      transitionMs: from.transitionMs
    };
  }
  return { ...from, brightness };
}

/** The palette output for slot `slot` of an apply covering `span` slots.
    Mirrors `LightSceneLayer::palette_output` in rhythm-os: cycle deals the
    anchors out and wraps; spread uses the anchors as they are while the
    span fits, and otherwise walks the open path from the first anchor to
    the last so every slot is distinct; shuffle deals the spread colours to
    the slots in the order `seed` fixes. Pass `permutation` when dealing a
    whole span so the shuffle order is built once. */
export function paletteOutputForSlot(
  anchors: SceneOutput[],
  mode: PaletteMode,
  slot: number,
  span: number,
  seed = 0,
  permutation?: number[]
): SceneOutput | null {
  const count = anchors.length;
  if (count === 0) return null;
  span = Math.max(1, span);
  if (mode === 'shuffle' && slot < span) {
    slot = (permutation ?? slotPermutation(seed, span))[slot] ?? slot;
  }
  if (mode === 'cycle' || span <= count) return anchors[slot % count];
  const lastLeg = count - 1;
  const position = (Math.min(slot, span - 1) / (span - 1)) * lastLeg;
  const lower = Math.min(Math.floor(position), Math.max(0, lastLeg - 1));
  const fraction = position - lower;
  return blendOutputs(
    anchors[lower],
    anchors[Math.min(lower + 1, lastLeg)],
    fraction
  );
}

/** The whole colour path sampled for a gradient bar. A shuffle deals the
    spread path, so the bar shows the path rather than the deal. */
export function palettePathSamples(
  anchors: SceneOutput[],
  mode: PaletteMode,
  samples: number
): Rgb[] {
  const result: Rgb[] = [];
  const pathMode: PaletteMode = mode === 'cycle' ? 'cycle' : 'spread';
  for (let index = 0; index < samples; index += 1) {
    const output = paletteOutputForSlot(anchors, pathMode, index, samples);
    if (output) result.push(outputRgb(output));
  }
  return result;
}

// ---------------------------------------------------------------------------
// House order
// ---------------------------------------------------------------------------

export type HouseLight = {
  id: string;
  name: string;
  roomId: string | null;
  roomName: string | null;
  disabled: boolean;
};

export type HouseRoom = {
  id: string;
  name: string;
  disabled: boolean;
  lights: HouseLight[];
};

export type HouseOrder = {
  rooms: HouseRoom[];
  /** Lights with no room, listed after every room. */
  roomless: HouseLight[];
  /** Every enabled light in the order the server deals palette slots. */
  ordered: HouseLight[];
};

/** Build the light order the server uses for a device-mode whole-home apply:
    rooms by name then id, lights by id within a room, roomless lights last.
    Disabled lights, and lights in disabled rooms, are listed but skipped. */
export function houseLightOrder(nodesPayload: unknown): HouseOrder {
  const record = asRecord(nodesPayload);
  const nodes = Array.isArray(nodesPayload)
    ? asRecordArray(nodesPayload)
    : asRecordArray(record.nodes);
  const rooms = new Map<string, HouseRoom>();
  const lights: Array<{
    id: string;
    name: string;
    parentId: string | null;
    disabled: boolean;
  }> = [];
  for (const node of nodes) {
    const id = asString(node.node_id) ?? asString(node.id);
    if (!id) continue;
    if (id.startsWith('__rhythm_light_node__')) continue;
    const name = asString(node.name) ?? asString(node.label) ?? id;
    const kind = asString(node.kind) ?? asString(node.node_kind);
    const disabled = asBoolean(node.disabled) ?? false;
    if (isRoomKind(kind)) {
      rooms.set(id, { id, name, disabled, lights: [] });
    } else if (isBulbKind(kind)) {
      lights.push({
        id,
        name,
        parentId: asString(node.parent_id) ?? null,
        disabled
      });
    }
  }
  const roomless: HouseLight[] = [];
  for (const light of lights) {
    const room = light.parentId ? rooms.get(light.parentId) : undefined;
    const entry: HouseLight = {
      id: light.id,
      name: light.name,
      roomId: room?.id ?? null,
      roomName: room?.name ?? null,
      disabled: light.disabled || (room?.disabled ?? false)
    };
    if (room) room.lights.push(entry);
    else roomless.push(entry);
  }
  const orderedRooms = [...rooms.values()]
    .filter((room) => room.lights.length > 0)
    .sort(
      (left, right) =>
        left.name.localeCompare(right.name) || left.id.localeCompare(right.id)
    );
  for (const room of orderedRooms) {
    room.lights.sort((left, right) => left.id.localeCompare(right.id));
  }
  roomless.sort((left, right) => left.id.localeCompare(right.id));
  const ordered = [
    ...orderedRooms.flatMap((room) => room.lights),
    ...roomless
  ].filter((light) => !light.disabled);
  return { rooms: orderedRooms, roomless, ordered };
}

// ---------------------------------------------------------------------------
// Preview rendering
// ---------------------------------------------------------------------------

export type PreviewScope =
  | { kind: 'home' }
  | { kind: 'room'; roomId: string }
  | { kind: 'strip'; count: number };

export type PreviewLight = {
  id: string;
  name: string;
  roomId: string | null;
  roomName: string | null;
  /** Slot on the palette path, or null for a pinned entry / default. */
  slot: number | null;
  output: SceneOutput | null;
  pinned: boolean;
  skipped: boolean;
};

export type ScenePreview = {
  lights: PreviewLight[];
  span: number;
  anchorCount: number;
  mode: PaletteMode;
  seed: number;
};

export function sceneLayer(scene: Record<string, unknown>): {
  anchors: SceneOutput[];
  mode: PaletteMode;
  seed: number;
  defaultOutput: SceneOutput | null;
  entries: Map<string, SceneOutput>;
} {
  const light = asRecord(scene.light);
  const layer = Object.keys(light).length > 0 ? light : scene;
  const anchors = asRecordArray(layer.palette).map(parseOutput);
  const mode = parsePaletteMode(asString(layer.palette_mode));
  const seed = parsePaletteSeed(layer.palette_seed);
  const defaultRecord = asRecord(layer.default_output);
  const defaultOutput =
    Object.keys(defaultRecord).length > 0 ? parseOutput(defaultRecord) : null;
  const entries = new Map<string, SceneOutput>();
  for (const entry of asRecordArray(layer.entries)) {
    const target = asRecord(entry.target);
    const nodeId =
      asString(target.node_id) ??
      asString(entry.target_id) ??
      asString(entry.node_id);
    if (!nodeId) continue;
    const nested = asRecord(entry.output);
    entries.set(
      nodeId,
      parseOutput(Object.keys(nested).length > 0 ? nested : entry)
    );
  }
  return { anchors, mode, seed, defaultOutput, entries };
}

/** Render what every light would receive, the way the server deals the
    palette: pinned entries take their own output and no slot, every other
    light takes the next slot of the house-wide (or room-wide) span. */
export function renderScenePreview(
  scene: Record<string, unknown>,
  house: HouseOrder,
  scope: PreviewScope
): ScenePreview {
  const { anchors, mode, seed, defaultOutput, entries } = sceneLayer(scene);
  let candidates: HouseLight[];
  switch (scope.kind) {
    case 'home':
      candidates = [
        ...house.rooms.flatMap((room) => room.lights),
        ...house.roomless
      ];
      break;
    case 'room': {
      const room = house.rooms.find((item) => item.id === scope.roomId);
      candidates = room ? room.lights : [];
      break;
    }
    case 'strip':
      candidates = Array.from({ length: scope.count }, (_, index) => ({
        id: `light-${index + 1}`,
        name: `Light ${index + 1}`,
        roomId: null,
        roomName: null,
        disabled: false
      }));
      break;
  }
  const active = candidates.filter((light) => !light.disabled);
  const span = active.filter((light) => !entries.has(light.id)).length;
  const permutation =
    mode === 'shuffle' ? slotPermutation(seed, span) : undefined;
  let slot = 0;
  const lights: PreviewLight[] = candidates.map((light) => {
    const base = {
      id: light.id,
      name: light.name,
      roomId: light.roomId,
      roomName: light.roomName
    };
    if (light.disabled) {
      return { ...base, slot: null, output: null, pinned: false, skipped: true };
    }
    const pinned = entries.get(light.id);
    if (pinned) {
      return { ...base, slot: null, output: pinned, pinned: true, skipped: false };
    }
    if (anchors.length > 0) {
      const own = slot;
      slot += 1;
      return {
        ...base,
        slot: own,
        output: paletteOutputForSlot(anchors, mode, own, span, seed, permutation),
        pinned: false,
        skipped: false
      };
    }
    return {
      ...base,
      slot: null,
      output: defaultOutput,
      pinned: false,
      skipped: false
    };
  });
  return { lights, span, anchorCount: anchors.length, mode, seed };
}

// ---------------------------------------------------------------------------
// Palette generators
// ---------------------------------------------------------------------------

function anchorFrom(rgb: Rgb, brightness: number): SceneOutput {
  return { on: true, brightness, rgb };
}

/** `count` evenly spaced hues starting at `startHue`, full saturation. */
export function hueSweep(
  count: number,
  startHue = 0,
  brightness = 75
): SceneOutput[] {
  const total = Math.max(1, Math.round(count));
  return Array.from({ length: total }, (_, index) =>
    anchorFrom(
      hsvToRgb({ h: startHue + (360 * index) / total, s: 1, v: 1 }),
      brightness
    )
  );
}

/** The base colour and its opposite on the hue wheel. */
export function complementary(base: Rgb, brightness = 75): SceneOutput[] {
  const hsv = rgbToHsv(base);
  return [
    anchorFrom(base, brightness),
    anchorFrom(
      hsvToRgb({ h: hsv.h + 180, s: Math.max(0.6, hsv.s), v: 1 }),
      brightness
    )
  ];
}

/** The base colour flanked by neighbours `spread` degrees either side. */
export function analogous(
  base: Rgb,
  spread = 30,
  brightness = 75
): SceneOutput[] {
  const hsv = rgbToHsv(base);
  const saturation = Math.max(0.6, hsv.s);
  return [
    anchorFrom(hsvToRgb({ h: hsv.h - spread, s: saturation, v: 1 }), brightness),
    anchorFrom(base, brightness),
    anchorFrom(hsvToRgb({ h: hsv.h + spread, s: saturation, v: 1 }), brightness)
  ];
}

/** Candle-warm to daylight-cool as colour temperatures. */
export function warmToCool(
  from = 2200,
  to = 6500,
  brightness = 80
): SceneOutput[] {
  return [
    { on: true, brightness, kelvin: from },
    { on: true, brightness, kelvin: to }
  ];
}
