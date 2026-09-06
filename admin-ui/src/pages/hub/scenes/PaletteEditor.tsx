import { useState } from 'react';
import {
  ChevronLeft,
  ChevronRight,
  Plus,
  Sparkles,
  Trash2,
  Wand2
} from 'lucide-react';

import {
  hsvToRgb,
  rgbToCss,
  rgbToHsv
} from '../../../components/controls/colorMath';
import { NumberField } from '../../../components/controls/fields';
import { SegmentedControl } from '../../../components/controls/SegmentedControl';
import { asRecordArray, asString } from '../../../device/values';
import { OutputEditor } from './OutputEditor';
import {
  analogous,
  complementary,
  hueSweep,
  outputRgb,
  outputToRecord,
  palettePathSamples,
  parseOutput,
  warmToCool,
  type PaletteMode,
  type SceneOutput
} from './scenePalette';

/** The palette section of the scene editor: the anchors, how they are dealt
    across lights, and a few generators to get a themed palette started. */
export function PaletteEditor({
  layer,
  disabled,
  onChange
}: {
  layer: Record<string, unknown>;
  disabled?: boolean;
  onChange: (next: Record<string, unknown>) => void;
}) {
  const anchors = asRecordArray(layer.palette);
  const mode: PaletteMode =
    asString(layer.palette_mode) === 'cycle' ? 'cycle' : 'spread';
  const [selected, setSelected] = useState<number | null>(
    anchors.length > 0 ? 0 : null
  );
  const [sweepCount, setSweepCount] = useState<number | undefined>(6);

  const parsed = anchors.map(parseOutput);
  const pathSamples = palettePathSamples(parsed, mode, 48);

  function writeAnchors(next: Record<string, unknown>[]) {
    onChange({ ...layer, palette: next });
  }

  function replaceWith(outputs: SceneOutput[]) {
    writeAnchors(outputs.map(outputToRecord));
    setSelected(outputs.length > 0 ? 0 : null);
  }

  function move(index: number, delta: number) {
    const target = index + delta;
    if (target < 0 || target >= anchors.length) return;
    const next = [...anchors];
    [next[index], next[target]] = [next[target], next[index]];
    writeAnchors(next);
    setSelected(target);
  }

  function remove(index: number) {
    const next = anchors.filter((_, i) => i !== index);
    writeAnchors(next);
    setSelected(next.length === 0 ? null : Math.min(index, next.length - 1));
  }

  function add() {
    // Continue the hue walk so a new anchor never lands on an existing one.
    const last = parsed[parsed.length - 1];
    const seed = last
      ? hsvToRgb({
          h: (hueOf(outputRgb(last)) + 137.5) % 360,
          s: 1,
          v: 1
        })
      : { r: 255, g: 120, b: 20 };
    const next = [
      ...anchors,
      outputToRecord({ on: true, brightness: last?.brightness ?? 75, rgb: seed })
    ];
    writeAnchors(next);
    setSelected(next.length - 1);
  }

  const base = parsed[0] ? outputRgb(parsed[0]) : { r: 255, g: 120, b: 20 };
  const baseBrightness = parsed[0]?.brightness ?? 75;

  return (
    <div className="ssPalette">
      <div className="ssPaletteHeader">
        <SegmentedControl
          value={mode}
          disabled={disabled}
          onChange={(value) =>
            onChange({ ...layer, palette_mode: value as PaletteMode })
          }
          options={[
            { value: 'spread', label: 'Spread', icon: <Sparkles size={14} /> },
            { value: 'cycle', label: 'Cycle' }
          ]}
        />
        <p className="cardNote ssPaletteNote">
          {mode === 'spread'
            ? 'Anchors mark a colour path. Every light gets its own point along it, first anchor to last, so no two bulbs match even with a short palette.'
            : 'Anchors are dealt out in order and repeat, so colours recur every few lights.'}
        </p>
      </div>

      {parsed.length > 0 ? (
        <div
          className="ssPathBar"
          style={{
            background: `linear-gradient(90deg, ${pathSamples
              .map(
                (rgb, index) =>
                  `${rgbToCss(rgb)} ${(index / (pathSamples.length - 1)) * 100}%`
              )
              .join(', ')})`
          }}
          title="The colour path lights are placed along"
        >
          {parsed.map((anchor, index) => (
            <span
              key={index}
              className="ssPathAnchor"
              style={{
                left: `${parsed.length === 1 ? 0 : (index / (parsed.length - 1)) * 100}%`,
                background: rgbToCss(outputRgb(anchor))
              }}
            />
          ))}
        </div>
      ) : null}

      <div className="ssAnchorStrip">
        {anchors.map((anchor, index) => {
          const output = parsed[index];
          return (
            <button
              key={index}
              type="button"
              className={`ssAnchor${selected === index ? ' selected' : ''}${
                output.on ? '' : ' off'
              }`}
              disabled={disabled}
              onClick={() => setSelected(index)}
              title={`Anchor ${index + 1}`}
            >
              <span
                className="ssAnchorSwatch"
                style={{ background: rgbToCss(outputRgb(output)) }}
              />
              <span className="ssAnchorMeta">
                <strong>{index + 1}</strong>
                <span>{output.on ? `${output.brightness}%` : 'off'}</span>
              </span>
            </button>
          );
        })}
        <button
          type="button"
          className="ssAnchor add"
          disabled={disabled}
          onClick={add}
          title="Add anchor"
        >
          <Plus size={16} />
        </button>
      </div>

      {selected !== null && anchors[selected] ? (
        <div className="ssAnchorEditor">
          <div className="ssAnchorEditorHeader">
            <span>Anchor {selected + 1}</span>
            <div className="ssAnchorEditorActions">
              <button
                className="iconOnlyButton"
                type="button"
                aria-label="Move earlier"
                disabled={disabled || selected === 0}
                onClick={() => move(selected, -1)}
              >
                <ChevronLeft size={14} />
              </button>
              <button
                className="iconOnlyButton"
                type="button"
                aria-label="Move later"
                disabled={disabled || selected === anchors.length - 1}
                onClick={() => move(selected, 1)}
              >
                <ChevronRight size={14} />
              </button>
              <button
                className="iconOnlyButton"
                type="button"
                aria-label="Remove anchor"
                disabled={disabled}
                onClick={() => remove(selected)}
              >
                <Trash2 size={14} />
              </button>
            </div>
          </div>
          <OutputEditor
            output={anchors[selected]}
            disabled={disabled}
            compact
            onChange={(next) =>
              writeAnchors(anchors.map((item, i) => (i === selected ? next : item)))
            }
          />
        </div>
      ) : null}

      <div className="ssGenerators">
        <span className="ssGeneratorsLabel">
          <Wand2 size={14} />
          Start from
        </span>
        <div className="ssGeneratorRow">
          <NumberField
            value={sweepCount}
            min={2}
            max={24}
            onChange={setSweepCount}
          />
          <button
            className="consoleButton small"
            type="button"
            disabled={disabled}
            onClick={() =>
              replaceWith(
                hueSweep(sweepCount ?? 6, hueOf(base), baseBrightness)
              )
            }
          >
            Hue sweep
          </button>
        </div>
        <button
          className="consoleButton small"
          type="button"
          disabled={disabled}
          onClick={() => replaceWith(complementary(base, baseBrightness))}
        >
          Complement
        </button>
        <button
          className="consoleButton small"
          type="button"
          disabled={disabled}
          onClick={() => replaceWith(analogous(base, 30, baseBrightness))}
        >
          Analogous
        </button>
        <button
          className="consoleButton small"
          type="button"
          disabled={disabled}
          onClick={() => replaceWith(warmToCool(2200, 6500, baseBrightness))}
        >
          Warm → cool
        </button>
      </div>
    </div>
  );
}

function hueOf(rgb: { r: number; g: number; b: number }): number {
  return rgbToHsv(rgb).h;
}
