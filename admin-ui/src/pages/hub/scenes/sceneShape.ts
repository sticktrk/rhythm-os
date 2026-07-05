import { kelvinToRgb, rgbToCss } from '../../../components/controls/colorMath';
import {
  asNumber,
  asRecord,
  asRecordArray,
  asString
} from '../../../device/values';

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

/** Best-effort swatch color for a scene or color-ish record. */
export function colorSummaryCss(scene: Record<string, unknown>): string {
  const output = asRecord(scene.default_output);
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

export function nodeOptionsFromState(
  payload: unknown
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
    seen.add(id);
    const name = asString(node.name) ?? asString(node.label);
    options.push({ value: id, label: name ? `${name} (${id})` : id });
  }
  return options;
}
