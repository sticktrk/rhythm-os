import { kelvinToRgb, rgbToCss } from '../../../components/controls/colorMath.ts';
import {
  asNumber,
  asRecord,
  asRecordArray,
  asString
} from '../../../device/values.ts';
import { isBulbKind } from '../topologyMembership.ts';

/** Some payloads nest the output under `output`, others inline the keys. */
export function outputContainer(entry: Record<string, unknown>): {
  output: Record<string, unknown>;
  write: (
    entry: Record<string, unknown>,
    next: Record<string, unknown>
  ) => Record<string, unknown>;
} {
  const nested = asRecord(entry.output);
  const nestedHasKeys = ['power', 'on', 'brightness', 'color'].some(
    (key) => key in nested
  );
  if (nestedHasKeys) {
    return {
      output: nested,
      write: (record, next) => ({ ...record, output: next })
    };
  }
  return {
    output: entry,
    write: (record, next) => ({ ...record, ...next })
  };
}

/** The scene's light layer. Stored scenes keep it under `light`; the legacy
    admin editor wrote the same keys at the top level, which the server
    ignores, so both spellings are read and the canonical one is written. */
export function lightLayer(scene: Record<string, unknown>): Record<string, unknown> {
  const nested = asRecord(scene.light);
  if (Object.keys(nested).length > 0) return nested;
  const legacy: Record<string, unknown> = {};
  for (const key of [
    'default_transition_ms',
    'default_output',
    'palette',
    'palette_mode',
    'palette_seed',
    'entries'
  ]) {
    if (key in scene) legacy[key] = scene[key];
  }
  return legacy;
}

/** Write the light layer canonically under `light`, dropping any legacy
    top-level copies so the server sees one definition. */
export function withLightLayer(
  scene: Record<string, unknown>,
  layer: Record<string, unknown>
): Record<string, unknown> {
  const next: Record<string, unknown> = { ...scene, light: layer };
  for (const key of [
    'default_transition_ms',
    'default_output',
    'palette',
    'palette_mode',
    'palette_seed',
    'entries'
  ]) {
    delete next[key];
  }
  return next;
}

/** The node id an entry targets, in the canonical or either legacy shape. */
export function entryNodeId(entry: Record<string, unknown>): string {
  const target = asRecord(entry.target);
  return (
    asString(target.node_id) ??
    asString(entry.target_id) ??
    asString(entry.node_id) ??
    asString(entry.id) ??
    ''
  );
}

export function entryWithNodeId(
  entry: Record<string, unknown>,
  nodeId: string
): Record<string, unknown> {
  const next: Record<string, unknown> = {
    ...entry,
    target: { kind: 'node', node_id: nodeId }
  };
  delete next.target_id;
  delete next.node_id;
  delete next.id;
  return next;
}

/** Best-effort swatch color for a scene or color-ish record. */
export function colorSummaryCss(scene: Record<string, unknown>): string {
  const layer = lightLayer(scene);
  const palette = asRecordArray(layer.palette);
  const output =
    palette.length > 0 ? asRecord(palette[0]) : asRecord(layer.default_output);
  const color = asRecord(output.color);
  const direct = asRecord(scene.color);
  const source = Object.keys(color).length > 0 ? color : direct;

  const kelvin = asNumber(source.kelvin);
  if (kelvin !== undefined) return rgbToCss(kelvinToRgb(kelvin));

  const rgb = asRecord(source.rgb);
  const r = asNumber(rgb.r) ?? asNumber(source.r);
  const g = asNumber(rgb.g) ?? asNumber(source.g);
  const b = asNumber(rgb.b) ?? asNumber(source.b);
  if (r !== undefined && g !== undefined && b !== undefined) {
    return rgbToCss({ r, g, b });
  }
  return 'var(--surface-muted)';
}

/** Up to `limit` swatch colours for a scene list row: palette anchors first,
    then the default output. */
export function swatchStripCss(
  scene: Record<string, unknown>,
  limit = 5
): string[] {
  const layer = lightLayer(scene);
  const outputs = asRecordArray(layer.palette);
  if (outputs.length === 0) return [colorSummaryCss(scene)];
  return outputs
    .slice(0, limit)
    .map((output) => colorSummaryCss({ light: { default_output: output } }));
}

export function nodeOptionsFromState(
  payload: unknown,
  { lightsOnly = false }: { lightsOnly?: boolean } = {}
): Array<{ value: string; label: string }> {
  const record = asRecord(payload);
  const nodes = Array.isArray(payload)
    ? asRecordArray(payload)
    : asRecordArray(record.nodes);
  const seen = new Set<string>();
  const options: Array<{ value: string; label: string }> = [];
  for (const node of nodes) {
    const id = asString(node.node_id) ?? asString(node.id);
    if (!id || seen.has(id)) continue;
    if (id.startsWith('__rhythm_light_node__')) continue;
    if (
      lightsOnly &&
      !isBulbKind(asString(node.kind) ?? asString(node.node_kind))
    ) {
      continue;
    }
    seen.add(id);
    const name = asString(node.name) ?? asString(node.label);
    options.push({ value: id, label: name ? `${name} (${id})` : id });
  }
  return options;
}

/** Only light devices can be pinned by a scene entry: the server resolves an
    entry against the target's light scope, so a room or sensor id would
    silently match nothing. */
export function lightNodeOptionsFromState(
  payload: unknown
): Array<{ value: string; label: string }> {
  return nodeOptionsFromState(payload, { lightsOnly: true });
}

const rgbOutput = (r: number, g: number, b: number, brightness: number) => ({
  power: 'on',
  brightness,
  color: { kind: 'rgb', rgb: { r, g, b } }
});

/** Starting points for the "New scene" menu. */
export const SCENE_TEMPLATES: Array<{
  id: string;
  label: string;
  description: string;
  scene: Record<string, unknown>;
}> = [
  {
    id: 'palette',
    label: 'Palette scene',
    description:
      'Anchor colours spread across every light. The house reads as one theme.',
    scene: {
      name: 'New palette scene',
      source: { kind: 'user' },
      light: {
        default_transition_ms: 1200,
        palette: [
          rgbOutput(255, 120, 20, 78),
          rgbOutput(160, 30, 220, 66),
          rgbOutput(20, 200, 160, 70)
        ],
        palette_mode: 'spread',
        entries: []
      }
    }
  },
  {
    id: 'single',
    label: 'One look everywhere',
    description: 'A single output every light shares.',
    scene: {
      name: 'New scene',
      source: { kind: 'user' },
      light: {
        default_transition_ms: 700,
        default_output: {
          power: 'on',
          brightness: 80,
          color: { kind: 'kelvin', kelvin: 3000 }
        },
        entries: []
      }
    }
  },
  {
    id: 'per-light',
    label: 'Per-light composition',
    description:
      'Pin individual lights to their own outputs; everything else takes the palette or default.',
    scene: {
      name: 'New composition',
      source: { kind: 'user' },
      light: {
        default_transition_ms: 900,
        default_output: {
          power: 'on',
          brightness: 60,
          color: { kind: 'kelvin', kelvin: 2400 }
        },
        entries: []
      }
    }
  }
];
