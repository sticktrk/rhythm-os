import { formatDurationSecs } from '../../lib/format';
import { NumberField, SelectField } from './fields';

export type ExpiryValue =
  | { kind: 'none' }
  | { kind: 'preset'; secs: number }
  | { kind: 'custom'; secs: number };

const DEFAULT_PRESETS = [900, 3600, 14400];

/** Picker for freeze/boost/pause expirations: never, preset, or custom secs. */
export function ExpiryPicker({
  value,
  presets = DEFAULT_PRESETS,
  disabled,
  onChange
}: {
  value: ExpiryValue;
  presets?: number[];
  disabled?: boolean;
  onChange: (value: ExpiryValue) => void;
}) {
  const selectValue =
    value.kind === 'none'
      ? 'none'
      : value.kind === 'custom'
        ? 'custom'
        : String(value.secs);

  return (
    <div className="expiryPicker">
      <SelectField
        value={selectValue}
        disabled={disabled}
        onChange={(next) => {
          if (next === 'none') onChange({ kind: 'none' });
          else if (next === 'custom') {
            onChange({
              kind: 'custom',
              secs: value.kind !== 'none' ? value.secs : 1800
            });
          } else onChange({ kind: 'preset', secs: Number(next) });
        }}
        options={[
          { value: 'none', label: 'No expiry' },
          ...presets.map((secs) => ({
            value: String(secs),
            label: `Expires in ${formatDurationSecs(secs)}`
          })),
          { value: 'custom', label: 'Custom…' }
        ]}
      />
      {value.kind === 'custom' ? (
        <>
          <NumberField
            value={value.secs}
            min={1}
            disabled={disabled}
            onChange={(next) => {
              if (next !== undefined) onChange({ kind: 'custom', secs: next });
            }}
          />
          <span className="timerUnit">secs</span>
        </>
      ) : null}
    </div>
  );
}

export function expiryToSecs(value: ExpiryValue): number | undefined {
  return value.kind === 'none' ? undefined : value.secs;
}
