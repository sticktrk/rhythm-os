import { ColorWheel } from '../../../components/controls/ColorWheel';
import { kelvinToRgb, rgbToCss } from '../../../components/controls/colorMath';
import { SegmentedControl } from '../../../components/controls/SegmentedControl';
import { KelvinSlider, Slider } from '../../../components/controls/Slider';
import { ToggleSwitch } from '../../../components/controls/ToggleSwitch';
import {
  asBoolean,
  asNumber,
  asRecord
} from '../../../device/values';

/** A scene "output" is a loose record with power/brightness/color keys.
    Color is kelvin | rgb | xy; xy stays JSON-only (no wheel mapping). */
export function OutputEditor({
  output,
  disabled,
  onChange
}: {
  output: Record<string, unknown>;
  disabled?: boolean;
  onChange: (next: Record<string, unknown>) => void;
}) {
  const power = asBoolean(output.power) ?? asBoolean(output.on) ?? true;
  const brightness = asNumber(output.brightness) ?? 80;
  const color = asRecord(output.color);
  const kelvin = asNumber(color.kelvin);
  const rgbRecord = asRecord(color.rgb);
  const rgb = {
    r: asNumber(rgbRecord.r) ?? 255,
    g: asNumber(rgbRecord.g) ?? 200,
    b: asNumber(rgbRecord.b) ?? 140
  };
  const colorKind =
    kelvin !== undefined
      ? 'kelvin'
      : asNumber(rgbRecord.r) !== undefined
        ? 'rgb'
        : 'kelvin';

  function patch(next: Record<string, unknown>) {
    onChange({ ...output, ...next });
  }

  return (
    <div className="p5Output">
      <div className="p5OutputRow">
        <ToggleSwitch
          checked={power}
          disabled={disabled}
          onChange={(value) => patch({ power: value })}
          label={power ? 'On' : 'Off'}
        />
        <SegmentedControl
          value={colorKind}
          disabled={disabled}
          onChange={(kind) => {
            if (kind === colorKind) return;
            patch({
              color:
                kind === 'kelvin'
                  ? { kelvin: kelvin ?? 3000 }
                  : { rgb }
            });
          }}
          options={[
            { value: 'kelvin', label: 'White' },
            { value: 'rgb', label: 'Color' }
          ]}
        />
        <span
          className="p5Swatch"
          style={{
            background: rgbToCss(
              colorKind === 'kelvin' ? kelvinToRgb(kelvin ?? 3000) : rgb
            )
          }}
        />
      </div>

      <Slider
        value={brightness}
        min={1}
        max={100}
        label="Brightness"
        format={(value) => `${Math.round(value)}%`}
        disabled={disabled}
        onCommit={(value) => patch({ brightness: Math.round(value) })}
      />

      {colorKind === 'kelvin' ? (
        <KelvinSlider
          value={kelvin ?? 3000}
          label="Color temp"
          disabled={disabled}
          onCommit={(value) => patch({ color: { kelvin: Math.round(value) } })}
        />
      ) : (
        <div className="p5WheelRow">
          <ColorWheel
            rgb={rgb}
            size={150}
            disabled={disabled}
            onCommit={(next) => patch({ color: { rgb: next } })}
          />
        </div>
      )}
    </div>
  );
}
