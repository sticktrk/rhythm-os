import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Loader2, RefreshCw, RotateCcw, Save, Undo2 } from 'lucide-react';

import { CurveChart, type CurveSeries } from '../../components/controls/CurveChart';
import { SegmentedControl } from '../../components/controls/SegmentedControl';
import { RangeSlider, Slider } from '../../components/controls/Slider';
import {
  TimerSettingRow,
  parseTimerSetting,
  timerSettingToJson
} from '../../components/controls/TimerSettingRow';
import { NumberField } from '../../components/controls/fields';
import { hexToRgb, rgbToHex } from '../../components/controls/colorMath';
import { EmptyState, ErrorNotice } from '../../components/ui/bits';
import { SectionCard } from '../../components/ui/SectionCard';
import { useConfirm } from '../../components/ui/ConfirmDialog';
import {
  absorbOffset,
  getConfig,
  getProfiles,
  getSolar,
  putConfig,
  resetConfig,
  sampleCurve
} from '../../device/curves';
import { setNodesOffset } from '../../device/nodes';
import {
  asNumber,
  asRecord,
  asRecordArray,
  asString
} from '../../device/values';
import { useDeviceCall } from '../../hooks/useDeviceCall';
import { useDeviceClient } from '../../hooks/useDeviceClient';
import { usePolling } from '../../hooks/usePolling';
import { errorMessage } from '../../lib/format';
import { parseCurveSamples, type CurvePoints } from './profileCurveData';
import '../../styles/pages-phase4.css';

const CURVE_TYPES = [
  { value: 'super-gaussian', label: 'Super-Gaussian' },
  { value: 'sigmoid', label: 'Sigmoid' },
  { value: 'palette', label: 'Palette' },
  { value: 'constant', label: 'Constant' },
  { value: 'inherit-active', label: 'Inherit' }
];

type ProfileRef = { id: string; name: string };

function parseProfiles(payload: unknown): ProfileRef[] {
  const record = asRecord(payload);
  const raw = Array.isArray(payload)
    ? asRecordArray(payload)
    : asRecordArray(record.profiles ?? record.configs ?? record.items);
  return raw
    .map((entry) => {
      const id = asString(entry.id) ?? asString(entry.profile_id);
      if (!id) return null;
      return { id, name: asString(entry.name) ?? asString(entry.label) ?? id };
    })
    .filter((entry): entry is ProfileRef => entry !== null);
}

/** Locate the tagged curve-shape record within a config, tolerating both
    nested (`config.curve = {type,...}`) and flat (`config.type = ...`) forms. */
function curveShapeOf(config: Record<string, unknown>): {
  shape: Record<string, unknown>;
  nested: boolean;
} {
  const nestedCurve = asRecord(config.curve);
  if (asString(nestedCurve.type)) return { shape: nestedCurve, nested: true };
  return { shape: config, nested: false };
}

export default function ProfilesPage() {
  const client = useDeviceClient();
  const confirm = useConfirm();

  const profilesQuery = usePolling(
    useCallback(() => getProfiles(client), [client])
  );
  const solarQuery = usePolling(useCallback(() => getSolar(client), [client]));

  const profiles = useMemo(
    () => parseProfiles(profilesQuery.data),
    [profilesQuery.data]
  );
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const profileId = selectedId ?? profiles[0]?.id ?? null;

  const [config, setConfig] = useState<Record<string, unknown> | null>(null);
  const [savedConfig, setSavedConfig] = useState<string | null>(null);
  const [configError, setConfigError] = useState<string | null>(null);
  const [loadingConfig, setLoadingConfig] = useState(false);
  const configLoadSeq = useRef(0);

  const [samples, setSamples] = useState<CurvePoints | null>(null);
  const [sampling, setSampling] = useState(false);
  const sampleSeq = useRef(0);
  const lastSampleStartedAt = useRef(0);

  const [offsetHours, setOffsetHours] = useState(0);

  const loadConfig = useCallback(
    async (id: string) => {
      const seq = (configLoadSeq.current += 1);
      setLoadingConfig(true);
      setConfigError(null);
      setConfig(null);
      setSavedConfig(null);
      setSamples(null);
      sampleSeq.current += 1;
      setSampling(false);
      try {
        const result = await getConfig(client, id);
        if (configLoadSeq.current !== seq) return;
        const record = asRecord(result);
        setConfig(record);
        setSavedConfig(JSON.stringify(record));
      } catch (error) {
        if (configLoadSeq.current !== seq) return;
        setConfig(null);
        setSavedConfig(null);
        setConfigError(errorMessage(error));
      } finally {
        if (configLoadSeq.current === seq) setLoadingConfig(false);
      }
    },
    [client]
  );

  useEffect(() => {
    if (profileId) void loadConfig(profileId);
  }, [profileId, loadConfig]);

  // Device-side re-sampling: curve math lives in Rust, so the chart renders
  // the appliance's exact output. Cap live drag previews at 10 requests/sec.
  useEffect(() => {
    if (!config || !profileId) return;
    const seq = (sampleSeq.current += 1);
    setSampling(true);
    const elapsed = window.performance.now() - lastSampleStartedAt.current;
    const delay = Math.max(0, 100 - elapsed);
    const timer = window.setTimeout(() => {
      lastSampleStartedAt.current = window.performance.now();
      sampleCurve(client, config, { id: profileId, samplesPerHour: 6 })
        .then((result) => {
          if (sampleSeq.current !== seq) return;
          setSamples(parseCurveSamples(result));
        })
        .catch(() => {
          /* keep previous samples; errors surface via save/refresh */
        })
        .finally(() => {
          if (sampleSeq.current === seq) setSampling(false);
        });
    }, delay);
    return () => window.clearTimeout(timer);
  }, [client, config, profileId]);

  const dirty =
    config !== null &&
    savedConfig !== null &&
    JSON.stringify(config) !== savedConfig;

  const saveCall = useDeviceCall(
    useCallback(async () => {
      if (!config || !profileId) return;
      await putConfig(client, config, { id: profileId, apply: true });
      setSavedConfig(JSON.stringify(config));
    }, [client, config, profileId])
  );

  const patchConfig = useCallback(
    (patch: Record<string, unknown>) => {
      setConfig((current) => (current ? { ...current, ...patch } : current));
    },
    []
  );

  const patchShape = useCallback(
    (patch: Record<string, unknown>) => {
      setConfig((current) => {
        if (!current) return current;
        const { nested } = curveShapeOf(current);
        if (nested) {
          return {
            ...current,
            curve: { ...asRecord(current.curve), ...patch }
          };
        }
        return { ...current, ...patch };
      });
    },
    []
  );

  const solar = asRecord(solarQuery.data);
  const sunriseHour =
    asNumber(solar.sunrise_hour) ??
    asNumber(solar.sunrise) ??
    asNumber(asRecord(solar.sunrise).hour);
  const sunsetHour =
    asNumber(solar.sunset_hour) ??
    asNumber(solar.sunset) ??
    asNumber(asRecord(solar.sunset).hour);

  const shapeInfo = config ? curveShapeOf(config) : null;
  const shapeType = shapeInfo ? asString(shapeInfo.shape.type) ?? 'super-gaussian' : null;

  const minCct = config ? asNumber(config.min_color_temp) ?? 2200 : 2200;
  const maxCct = config ? asNumber(config.max_color_temp) ?? 6500 : 6500;
  const minBri = config ? asNumber(config.min_brightness) ?? 1 : 1;
  const maxBri = config ? asNumber(config.max_brightness) ?? 100 : 100;

  const series: CurveSeries[] = samples
    ? [
        {
          id: 'bri',
          label: 'Brightness %',
          axis: 'brightness',
          color: 'var(--amber)',
          points: samples.brightness
        },
        {
          id: 'cct',
          label: 'Color temp K',
          axis: 'kelvin',
          color: 'var(--blue)',
          points: samples.kelvin
        }
      ]
    : [];

  const nowHour = new Date().getHours() + new Date().getMinutes() / 60;

  return (
    <div className="consolePage wide">
      <header className="pageHeader">
        <div>
          <div className="eyebrow">Device console</div>
          <h2>Profiles &amp; Curves</h2>
          <p className="pageIntro">
            Full rhythm profile editor. The chart is sampled by the device
            itself, so what you see is exactly what will drive the lights.
          </p>
        </div>
        <div className="pageHeaderActions">
          <button
            className="consoleButton"
            type="button"
            onClick={() => {
              void profilesQuery.refresh();
              if (profileId) void loadConfig(profileId);
            }}
          >
            <RefreshCw size={15} />
            <span>Refresh</span>
          </button>
        </div>
      </header>

      {profilesQuery.error ? <ErrorNotice message={profilesQuery.error} /> : null}
      {configError ? <ErrorNotice message={configError} /> : null}

      <div className="profileTabs">
        {profiles.map((profile) => (
          <button
            key={profile.id}
            type="button"
            className={`profileTab${profile.id === profileId ? ' active' : ''}`}
            onClick={() => setSelectedId(profile.id)}
          >
            {profile.name}
          </button>
        ))}
        {profiles.length === 0 && !profilesQuery.loading ? (
          <span className="cardNote">No profiles reported by the device.</span>
        ) : null}
      </div>

      <SectionCard
        title="Curve"
        subtitle={
          profileId
            ? `Profile ${profileId} — 24h brightness and color temperature`
            : 'Select a profile'
        }
        busy={sampling || loadingConfig}
        rawPayload={config ?? undefined}
      >
        {samples ? (
          <div className={sampling ? 'curveSamplingWrap' : undefined}>
            <CurveChart
              series={series}
              height={280}
              nowHour={nowHour}
              solar={{ sunriseHour, sunsetHour }}
              yLeft={{ min: 0, max: 100 }}
              yRight={{ min: Math.min(minCct, 500), max: Math.max(maxCct, 6500) }}
              xAxisLabel="Time (24-hour)"
              yLeftAxisLabel="Brightness (%)"
              yRightAxisLabel="Color temperature (K)"
            />
          </div>
        ) : loadingConfig || sampling ? (
          <div className="consolePageLoading">
            <Loader2 className="spin" size={18} />
          </div>
        ) : (
          <EmptyState message="No curve samples available." />
        )}
      </SectionCard>

      {config && shapeInfo && shapeType ? (
        <>
          <div className="cardGrid two">
            <SectionCard title="Envelope" subtitle="Output limits for the whole curve">
              <RangeSlider
                label="Brightness"
                low={minBri}
                high={maxBri}
                min={1}
                max={100}
                minGap={1}
                format={(value) => `${Math.round(value)}%`}
                onChange={(low, high) =>
                  patchConfig({ min_brightness: low, max_brightness: high })
                }
                onCommit={(low, high) =>
                  patchConfig({ min_brightness: low, max_brightness: high })
                }
              />
              <RangeSlider
                label="Color temp"
                low={minCct}
                high={maxCct}
                min={500}
                max={6500}
                step={50}
                minGap={100}
                trackStyle="kelvin"
                format={(value) => `${Math.round(value)}K`}
                onCommit={(low, high) =>
                  patchConfig({ min_color_temp: low, max_color_temp: high })
                }
              />
              <div className="inlineFields">
                <span className="timerUnit">Max dim steps</span>
                <NumberField
                  value={asNumber(config.max_dim_steps)}
                  min={1}
                  max={64}
                  onChange={(value) =>
                    value !== undefined && patchConfig({ max_dim_steps: value })
                  }
                />
              </div>
            </SectionCard>

            <SectionCard title="Shape" subtitle="Curve algorithm and its parameters">
              <SegmentedControl
                value={shapeType}
                onChange={(nextType) => {
                  if (nextType === shapeType) return;
                  void (async () => {
                    const confirmed = await confirm({
                      title: 'Change curve shape',
                      message: `Switch this profile to the ${nextType} shape? Parameters of the previous shape are kept in the config but ignored.`,
                      confirmLabel: 'Switch'
                    });
                    if (confirmed) patchShape({ type: nextType });
                  })();
                }}
                options={CURVE_TYPES}
              />
              <ShapeParams
                type={shapeType}
                shape={shapeInfo.shape}
                onPatch={patchShape}
              />
            </SectionCard>
          </div>

          <SectionCard title="Timers" subtitle="Fade, motion timeout and rhythm interval">
            <div className="timerGrid">
              <TimerSettingRow
                label="Fade"
                unit="ms"
                defaultFixed={500}
                value={parseTimerSetting(config.fade_ms)}
                onCommit={(value) =>
                  patchConfig({ fade_ms: timerSettingToJson(value) })
                }
              />
              <TimerSettingRow
                label="Motion timeout"
                unit="secs"
                defaultFixed={600}
                value={parseTimerSetting(config.motion_timeout_secs)}
                onCommit={(value) =>
                  patchConfig({ motion_timeout_secs: timerSettingToJson(value) })
                }
              />
              <TimerSettingRow
                label="Rhythm interval"
                unit="secs"
                defaultFixed={60}
                value={parseTimerSetting(config.rhythm_interval_secs)}
                onCommit={(value) =>
                  patchConfig({ rhythm_interval_secs: timerSettingToJson(value) })
                }
              />
            </div>
          </SectionCard>

          <SectionCard
            title="Time simulator"
            subtitle="Preview the home at another time of day, then reset or absorb"
          >
            <div className="offsetRow">
              <Slider
                label="Offset"
                value={offsetHours}
                min={-12}
                max={12}
                step={0.25}
                format={(value) => `${value > 0 ? '+' : ''}${value}h`}
                onChange={setOffsetHours}
                onCommit={(value) => {
                  setOffsetHours(value);
                  void setNodesOffset(client, value).catch(() => undefined);
                }}
              />
              <button
                className="consoleButton small"
                type="button"
                onClick={() => {
                  setOffsetHours(0);
                  void setNodesOffset(client, 0).catch(() => undefined);
                }}
              >
                <RotateCcw size={13} />
                <span>Reset</span>
              </button>
              <button
                className="consoleButton small"
                type="button"
                disabled={offsetHours === 0 || !profileId}
                onClick={() => {
                  void (async () => {
                    const confirmed = await confirm({
                      title: 'Absorb time offset',
                      message: `Permanently bake a ${offsetHours > 0 ? '+' : ''}${offsetHours}h offset into profile ${profileId}? This reshapes the stored curve.`,
                      confirmLabel: 'Absorb',
                      danger: true
                    });
                    if (!confirmed || !profileId) return;
                    await absorbOffset(client, offsetHours * 60, profileId);
                    setOffsetHours(0);
                    await setNodesOffset(client, 0).catch(() => undefined);
                    await loadConfig(profileId);
                  })();
                }}
              >
                Absorb into profile
              </button>
            </div>
          </SectionCard>
        </>
      ) : null}

      {config ? (
        <div className={`dirtyBar${dirty ? ' visible' : ''}`}>
          <span>
            {dirty ? 'Unsaved profile changes' : 'Profile saved'}
            {saveCall.error ? ` — ${saveCall.error}` : ''}
          </span>
          <div className="actionRow">
            <button
              className="consoleButton small danger"
              type="button"
              onClick={() => {
                void (async () => {
                  const confirmed = await confirm({
                    title: 'Reset profile to defaults',
                    message: `Reset profile ${profileId} to its factory curve? Customer tuning is lost.`,
                    confirmLabel: 'Reset',
                    danger: true,
                    requireTypedText: 'reset-profile'
                  });
                  if (!confirmed || !profileId) return;
                  await resetConfig(client, profileId);
                  await loadConfig(profileId);
                })();
              }}
            >
              <RotateCcw size={13} />
              <span>Factory reset profile</span>
            </button>
            <button
              className="consoleButton small"
              type="button"
              disabled={!dirty || saveCall.busy}
              onClick={() => {
                if (savedConfig) setConfig(JSON.parse(savedConfig));
              }}
            >
              <Undo2 size={13} />
              <span>Discard</span>
            </button>
            <button
              className="consoleButton small primary"
              type="button"
              disabled={!dirty || saveCall.busy}
              onClick={() => void saveCall.run()}
            >
              {saveCall.busy ? (
                <Loader2 className="spin" size={13} />
              ) : (
                <Save size={13} />
              )}
              <span>Save &amp; apply</span>
            </button>
          </div>
        </div>
      ) : null}
    </div>
  );
}

function ShapeParams({
  type,
  shape,
  onPatch
}: {
  type: string;
  shape: Record<string, unknown>;
  onPatch: (patch: Record<string, unknown>) => void;
}) {
  if (type === 'super-gaussian') {
    const widthSlider = (key: string, label: string, fallback: number) => (
      <Slider
        label={label}
        value={asNumber(shape[key]) ?? fallback}
        min={0.2}
        max={2}
        step={0.05}
        format={(value) => value.toFixed(2)}
        onChange={(value) => onPatch({ [key]: value })}
        onCommit={(value) => onPatch({ [key]: value })}
      />
    );
    return (
      <div className="formStack">
        {widthSlider('width_left_bri', 'Morning ramp (bri)', 0.95)}
        {widthSlider('width_right_bri', 'Evening ramp (bri)', 0.85)}
        {widthSlider('width_left_cct', 'Morning ramp (cct)', 0.95)}
        {widthSlider('width_right_cct', 'Evening ramp (cct)', 1.15)}
        <Slider
          label="Shape (peak↔plateau)"
          value={asNumber(shape.shape_p) ?? 6}
          min={2}
          max={10}
          step={0.1}
          format={(value) => value.toFixed(1)}
          onChange={(value) => onPatch({ shape_p: value })}
          onCommit={(value) => onPatch({ shape_p: value })}
        />
      </div>
    );
  }

  if (type === 'sigmoid') {
    const schedule = asRecord(shape.schedule);
    const wake = asRecord(schedule.wake);
    const bed = asRecord(schedule.bed);
    const patchSchedule = (which: 'wake' | 'bed', hour: number) =>
      onPatch({
        schedule: {
          ...schedule,
          [which]: { ...asRecord(schedule[which]), hour }
        }
      });
    const numberRow = (
      label: string,
      key: string,
      fallback: number,
      min: number,
      max: number,
      step = 1
    ) => (
      <div className="inlineFields" key={key}>
        <span className="timerUnit">{label}</span>
        <NumberField
          value={asNumber(shape[key]) ?? fallback}
          min={min}
          max={max}
          step={step}
          onChange={(value) => value !== undefined && onPatch({ [key]: value })}
        />
      </div>
    );
    return (
      <div className="formStack">
        <Slider
          label="Wake hour"
          value={asNumber(wake.hour) ?? 6}
          min={0}
          max={12}
          step={0.25}
          format={(value) => `${value}h`}
          onCommit={(value) => patchSchedule('wake', value)}
        />
        <Slider
          label="Bed hour"
          value={asNumber(bed.hour) ?? 22}
          min={12}
          max={24}
          step={0.25}
          format={(value) => `${value}h`}
          onCommit={(value) => patchSchedule('bed', value)}
        />
        <div className="inlineFieldsWrap">
          {numberRow('Ascend start', 'ascend_start', 3, 0, 12, 0.25)}
          {numberRow('Descend start', 'descend_start', 12, 6, 24, 0.25)}
          {numberRow('Wake speed', 'wake_speed', 8, 1, 24)}
          {numberRow('Bed speed', 'bed_speed', 6, 1, 24)}
          {numberRow('Wake brightness', 'wake_brightness', 50, 1, 100)}
          {numberRow('Bed brightness', 'bed_brightness', 50, 1, 100)}
        </div>
      </div>
    );
  }

  if (type === 'palette') {
    const keyframes = asRecordArray(shape.keyframes);
    const patchKeyframe = (
      index: number,
      patch: Record<string, unknown> | null
    ) => {
      const next = keyframes
        .map((frame, i) => (i === index ? (patch ? { ...frame, ...patch } : null) : frame))
        .filter((frame): frame is Record<string, unknown> => frame !== null);
      onPatch({ keyframes: next });
    };
    return (
      <div className="formStack">
        {keyframes.map((frame, index) => {
          const rgb = {
            r: asNumber(frame.r) ?? 255,
            g: asNumber(frame.g) ?? 255,
            b: asNumber(frame.b) ?? 255
          };
          return (
            <div className="inlineFields" key={index}>
              <NumberField
                value={asNumber(frame.hour) ?? 0}
                min={0}
                max={24}
                step={0.25}
                onChange={(value) =>
                  value !== undefined && patchKeyframe(index, { hour: value })
                }
              />
              <span className="timerUnit">h</span>
              <input
                type="color"
                className="colorSwatchInput"
                value={rgbToHex(rgb)}
                onChange={(event) => {
                  const next = hexToRgb(event.target.value);
                  if (next) patchKeyframe(index, next);
                }}
              />
              <button
                className="iconOnlyButton"
                type="button"
                aria-label="Remove keyframe"
                disabled={keyframes.length <= 2}
                onClick={() => patchKeyframe(index, null)}
              >
                ×
              </button>
            </div>
          );
        })}
        <button
          className="consoleButton small"
          type="button"
          onClick={() =>
            onPatch({
              keyframes: [...keyframes, { hour: 12, r: 255, g: 200, b: 150 }]
            })
          }
        >
          Add keyframe
        </button>
      </div>
    );
  }

  if (type === 'constant') {
    return (
      <div className="formStack">
        <Slider
          label="Brightness"
          value={asNumber(shape.brightness) ?? 80}
          min={1}
          max={100}
          format={(value) => `${Math.round(value)}%`}
          onCommit={(value) => onPatch({ brightness: value })}
        />
        <Slider
          label="Color temp"
          value={asNumber(shape.color_temp) ?? 3000}
          min={500}
          max={6500}
          step={50}
          trackStyle="kelvin"
          format={(value) => `${Math.round(value)}K`}
          onCommit={(value) => onPatch({ color_temp: value })}
        />
      </div>
    );
  }

  return (
    <p className="cardNote">
      This profile inherits the currently active profile's curve. There are no
      shape parameters to edit.
    </p>
  );
}
