import { Plus, Trash2 } from 'lucide-react';

import { asNumber, asRecord, asRecordArray, asString } from '../../device/values';
import { SegmentedControl } from './SegmentedControl';
import { NumberField } from './fields';

/** Device timer settings are a tagged union:
    "auto" | {type:"fixed", value} | {type:"scheduled", breakpoints:[{hour,value}]}
    (string forms and object forms both occur in the wild — parse tolerantly). */
export type TimerSetting =
  | { type: 'auto' }
  | { type: 'fixed'; value: number }
  | { type: 'scheduled'; breakpoints: Array<{ hour: number; value: number }> };

export function parseTimerSetting(raw: unknown): TimerSetting | undefined {
  if (raw === undefined || raw === null) return undefined;
  if (typeof raw === 'string') {
    return raw === 'auto' ? { type: 'auto' } : undefined;
  }
  if (typeof raw === 'number') {
    return { type: 'fixed', value: raw };
  }
  const record = asRecord(raw);
  const type = asString(record.type);
  if (type === 'auto') return { type: 'auto' };
  if (type === 'fixed') {
    const value = asNumber(record.value);
    return value === undefined ? undefined : { type: 'fixed', value };
  }
  if (type === 'scheduled') {
    return {
      type: 'scheduled',
      breakpoints: asRecordArray(record.breakpoints).map((entry) => ({
        hour: asNumber(entry.hour) ?? 0,
        value: asNumber(entry.value) ?? 0
      }))
    };
  }
  // Bare {value} objects mean fixed.
  const bareValue = asNumber(record.value);
  if (bareValue !== undefined) return { type: 'fixed', value: bareValue };
  return undefined;
}

export function timerSettingToJson(setting: TimerSetting): unknown {
  if (setting.type === 'auto') return { type: 'auto' };
  if (setting.type === 'fixed') {
    return { type: 'fixed', value: setting.value };
  }
  return {
    type: 'scheduled',
    breakpoints: setting.breakpoints.map((breakpoint) => ({
      hour: breakpoint.hour,
      value: breakpoint.value
    }))
  };
}

export function TimerSettingRow({
  label,
  unit,
  value,
  defaultFixed,
  disabled,
  onCommit
}: {
  label: string;
  unit: 'ms' | 'secs';
  value: TimerSetting | undefined;
  /** Seed value when switching to fixed mode. */
  defaultFixed: number;
  disabled?: boolean;
  onCommit: (value: TimerSetting) => void;
}) {
  const current: TimerSetting = value ?? { type: 'auto' };

  function switchType(type: string) {
    if (type === current.type) return;
    if (type === 'auto') onCommit({ type: 'auto' });
    else if (type === 'fixed') onCommit({ type: 'fixed', value: defaultFixed });
    else {
      onCommit({
        type: 'scheduled',
        breakpoints: [
          { hour: 7, value: defaultFixed },
          { hour: 22, value: defaultFixed }
        ]
      });
    }
  }

  function patchBreakpoint(
    index: number,
    patch: Partial<{ hour: number; value: number }>
  ) {
    if (current.type !== 'scheduled') return;
    const breakpoints = current.breakpoints.map((entry, i) =>
      i === index ? { ...entry, ...patch } : entry
    );
    onCommit({ type: 'scheduled', breakpoints });
  }

  return (
    <div className="timerSetting">
      <div className="timerSettingHeader">
        <span className="timerSettingLabel">{label}</span>
        <SegmentedControl
          value={current.type}
          disabled={disabled}
          onChange={switchType}
          options={[
            { value: 'auto', label: 'Auto' },
            { value: 'fixed', label: 'Fixed' },
            { value: 'scheduled', label: 'Scheduled' }
          ]}
        />
      </div>

      {current.type === 'fixed' ? (
        <div className="timerSettingFixed">
          <NumberField
            value={current.value}
            min={0}
            disabled={disabled}
            onChange={(next) => {
              if (next !== undefined) onCommit({ type: 'fixed', value: next });
            }}
          />
          <span className="timerUnit">{unit}</span>
        </div>
      ) : null}

      {current.type === 'scheduled' ? (
        <div className="timerBreakpoints">
          {current.breakpoints.map((breakpoint, index) => (
            <div className="timerBreakpoint" key={index}>
              <NumberField
                value={breakpoint.hour}
                min={0}
                max={24}
                step={0.25}
                disabled={disabled}
                onChange={(next) => {
                  if (next !== undefined) patchBreakpoint(index, { hour: next });
                }}
              />
              <span className="timerUnit">h →</span>
              <NumberField
                value={breakpoint.value}
                min={0}
                disabled={disabled}
                onChange={(next) => {
                  if (next !== undefined) patchBreakpoint(index, { value: next });
                }}
              />
              <span className="timerUnit">{unit}</span>
              <button
                className="iconOnlyButton"
                type="button"
                aria-label="Remove breakpoint"
                disabled={disabled || current.breakpoints.length <= 1}
                onClick={() =>
                  onCommit({
                    type: 'scheduled',
                    breakpoints: current.breakpoints.filter((_, i) => i !== index)
                  })
                }
              >
                <Trash2 size={14} />
              </button>
            </div>
          ))}
          <button
            className="consoleButton small"
            type="button"
            disabled={disabled}
            onClick={() =>
              onCommit({
                type: 'scheduled',
                breakpoints: [
                  ...current.breakpoints,
                  { hour: 12, value: defaultFixed }
                ]
              })
            }
          >
            <Plus size={14} />
            <span>Add breakpoint</span>
          </button>
        </div>
      ) : null}
    </div>
  );
}
