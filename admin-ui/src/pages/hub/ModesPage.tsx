import { useCallback, useState } from 'react';
import { Moon, Play, RefreshCw, Save, Sun } from 'lucide-react';

import '../../styles/pages-phase5.css';

import { ClockDial } from '../../components/controls/ClockDial';
import { JsonEditor } from '../../components/controls/JsonEditor';
import { NumberField, SelectField, TextField } from '../../components/controls/fields';
import { SegmentedControl } from '../../components/controls/SegmentedControl';
import { ToggleSwitch } from '../../components/controls/ToggleSwitch';
import {
  parseTimerSetting,
  timerSettingToJson
} from '../../components/controls/TimerSettingRow';
import { EmptyState, ErrorNotice, KeyValueGrid } from '../../components/ui/bits';
import { useConfirm } from '../../components/ui/ConfirmDialog';
import { SectionCard } from '../../components/ui/SectionCard';
import { getProfiles } from '../../device/curves';
import {
  getLightRuntime,
  getMode,
  getTransitions,
  setActiveMode,
  setLightRuntime,
  setMode,
  setTransitions,
  triggerTransition
} from '../../device/modes';
import {
  asBoolean,
  asNumber,
  asRecord,
  asRecordArray,
  asString
} from '../../device/values';
import { useDeviceCall } from '../../hooks/useDeviceCall';
import { useDeviceClient } from '../../hooks/useDeviceClient';
import { usePolling } from '../../hooks/usePolling';
import { formatHour } from '../../lib/format';
import { prettyJson } from '../../lib/json';

const PROFILE_KEYS = [
  ['active_profile_id', 'Active'],
  ['idle_profile_id', 'Idle'],
  ['wake_profile_id', 'Wake'],
  ['warning_profile_id', 'Warning']
] as const;

export default function ModesPage() {
  const client = useDeviceClient();
  const confirm = useConfirm();

  const modeQuery = usePolling(useCallback(() => getMode(client), [client]));
  const transitionsQuery = usePolling(
    useCallback(() => getTransitions(client), [client])
  );
  const runtimeQuery = usePolling(
    useCallback(() => getLightRuntime(client), [client])
  );
  const profilesQuery = usePolling(
    useCallback(() => getProfiles(client), [client])
  );

  const mutate = useDeviceCall(
    useCallback(async (action: () => Promise<unknown>) => action(), [])
  );

  const mode = asRecord(modeQuery.data);
  const activeMode = asString(mode.active) ?? '';
  const serverConfigs = asRecordArray(mode.configs);
  const [configsDraft, setConfigsDraft] = useState<
    Record<string, unknown>[] | null
  >(null);
  const configs = configsDraft ?? serverConfigs;

  const transitions = transitionsFromPayload(transitionsQuery.data);
  const [selectedTransitionId, setSelectedTransitionId] = useState<string | null>(
    null
  );
  const [transitionDraft, setTransitionDraft] = useState<Record<
    string,
    unknown
  > | null>(null);
  const [transitionText, setTransitionText] = useState('');

  const runtime = asRecord(runtimeQuery.data);
  const [runtimeId, setRuntimeId] = useState<string | null>(null);
  const [runtimeTransitionMs, setRuntimeTransitionMs] = useState<
    number | undefined
  >(undefined);

  const profileOptions = profileOptionsFromPayload(profilesQuery.data);

  function patchConfig(index: number, key: string, value: string) {
    const base = configsDraft ?? serverConfigs;
    setConfigsDraft(
      base.map((config, i) =>
        i === index ? { ...config, [key]: value || null } : config
      )
    );
  }

  async function saveConfigs() {
    if (!configsDraft) return;
    await mutate.run(async () => {
      await setMode(client, { ...mode, configs: configsDraft });
      setConfigsDraft(null);
      await modeQuery.refresh();
    });
  }

  function openTransition(transition: Record<string, unknown>) {
    const clone = JSON.parse(
      JSON.stringify(transition)
    ) as Record<string, unknown>;
    setSelectedTransitionId(transitionId(transition));
    setTransitionDraft(clone);
    setTransitionText(prettyJson(clone));
  }

  function updateTransitionDraft(next: Record<string, unknown>) {
    setTransitionDraft(next);
    setTransitionText(prettyJson(next));
  }

  async function saveTransition() {
    if (!transitionDraft || selectedTransitionId === null) return;
    const nextList = transitions.map((transition) =>
      transitionId(transition) === selectedTransitionId
        ? transitionDraft
        : transition
    );
    await mutate.run(async () => {
      await setTransitions(client, nextList);
      setTransitionDraft(null);
      setSelectedTransitionId(null);
      await transitionsQuery.refresh();
    });
  }

  async function handleTrigger(transition: Record<string, unknown>) {
    const id = transitionId(transition);
    if (!id) return;
    const ok = await confirm({
      title: 'Trigger transition',
      message: `Run "${asString(transition.label) ?? id}" now? Lights will change for the customer immediately.`,
      confirmLabel: 'Trigger'
    });
    if (!ok) return;
    await mutate.run(async () => {
      await triggerTransition(client, id);
      await modeQuery.refresh();
    });
  }

  async function saveRuntime() {
    const nextId = runtimeId ?? asString(runtime.runtime_id);
    if (!nextId) return;
    await mutate.run(async () => {
      await setLightRuntime(client, nextId, runtimeTransitionMs);
      setRuntimeId(null);
      setRuntimeTransitionMs(undefined);
      await runtimeQuery.refresh();
    });
  }

  return (
    <div className="consolePage">
      <header className="pageHeader">
        <div>
          <div className="eyebrow">Device console</div>
          <h2>Modes &amp; Transitions</h2>
          <p className="pageIntro">
            Day/sleep mode, per-mode profile assignments, and the transitions
            that move between them.
          </p>
        </div>
        <div className="pageHeaderActions">
          <button
            className="consoleButton"
            type="button"
            onClick={() => {
              void modeQuery.refresh();
              void transitionsQuery.refresh();
              void runtimeQuery.refresh();
            }}
          >
            <RefreshCw size={15} />
            <span>Refresh</span>
          </button>
        </div>
      </header>

      {mutate.error ? <ErrorNotice message={mutate.error} /> : null}

      <div className="cardGrid two">
        <SectionCard
          title="Active mode"
          busy={modeQuery.refreshing || mutate.busy}
          error={modeQuery.error}
          rawPayload={modeQuery.data ?? undefined}
        >
          <SegmentedControl
            value={activeMode}
            disabled={mutate.busy || !modeQuery.data}
            onChange={(value) =>
              void mutate.run(async () => {
                await setActiveMode(client, value);
                await modeQuery.refresh();
              })
            }
            options={[
              { value: 'day', label: 'Day', icon: <Sun size={15} /> },
              { value: 'sleep', label: 'Sleep', icon: <Moon size={15} /> }
            ]}
          />
        </SectionCard>

        <SectionCard
          title="Light runtime"
          busy={runtimeQuery.refreshing || mutate.busy}
          error={runtimeQuery.error}
          rawPayload={runtimeQuery.data ?? undefined}
        >
          <KeyValueGrid
            rows={[
              ['Runtime', asString(runtime.runtime_id)],
              ['Transition ms', asNumber(runtime.transition_ms)]
            ]}
          />
          <div className="p5ApplyRow">
            <TextField
              value={runtimeId ?? asString(runtime.runtime_id) ?? ''}
              mono
              onChange={setRuntimeId}
            />
            <NumberField
              value={runtimeTransitionMs ?? asNumber(runtime.transition_ms)}
              min={0}
              placeholder="transition ms"
              onChange={setRuntimeTransitionMs}
            />
            <button
              className="consoleButton"
              type="button"
              disabled={mutate.busy}
              onClick={() => void saveRuntime()}
            >
              <Save size={15} />
              <span>Save</span>
            </button>
          </div>
        </SectionCard>
      </div>

      <SectionCard
        title="Mode configs"
        subtitle="Profiles used by each mode"
        busy={modeQuery.refreshing || mutate.busy}
        error={profilesQuery.error}
        rawPayload={modeQuery.data ?? undefined}
        actions={
          configsDraft ? (
            <button
              className="consoleButton primary"
              type="button"
              disabled={mutate.busy}
              onClick={() => void saveConfigs()}
            >
              <Save size={15} />
              <span>Save configs</span>
            </button>
          ) : null
        }
      >
        {configs.length === 0 && !modeQuery.loading ? (
          <EmptyState message="No mode configs returned." />
        ) : (
          <div className="p5ConfigGrid">
            {configs.map((config, index) => (
              <div className="p5Config" key={asString(config.mode) ?? index}>
                <h4 className="p5SubHeading">
                  {asString(config.mode) ?? `Config ${index + 1}`}
                </h4>
                {PROFILE_KEYS.map(([key, label]) => (
                  <div className="formRow" key={key}>
                    <div className="formRowLabel">
                      <span>{label} profile</span>
                    </div>
                    <div className="formRowControl">
                      <SelectField
                        value={asString(config[key]) ?? ''}
                        placeholder="(none)"
                        options={profileOptions}
                        onChange={(value) => patchConfig(index, key, value)}
                      />
                    </div>
                  </div>
                ))}
              </div>
            ))}
          </div>
        )}
        {configsDraft ? (
          <p className="cardNote">Unsaved changes — press Save configs.</p>
        ) : null}
      </SectionCard>

      <SectionCard
        title="Transitions"
        subtitle="Automated or manual mode changes"
        busy={transitionsQuery.refreshing || mutate.busy}
        error={transitionsQuery.error}
        rawPayload={transitionsQuery.data ?? undefined}
      >
        {transitions.length === 0 && !transitionsQuery.loading ? (
          <EmptyState message="No transitions configured." />
        ) : (
          <div className="p5TransitionList">
            {transitions.map((transition, index) => {
              const id = transitionId(transition);
              const enabled = asBoolean(transition.trigger_enabled);
              return (
                <div className="p5TransitionRow" key={id ?? index}>
                  <span
                    className={`p5Dot${enabled === false ? ' off' : ''}`}
                    title={enabled === false ? 'Trigger disabled' : 'Trigger enabled'}
                  />
                  <span className="p5SceneName">
                    {asString(transition.label) ?? id ?? `Transition ${index + 1}`}
                  </span>
                  <span className="p5SceneMeta">
                    {asString(transition.from_mode) ?? '?'} →{' '}
                    {asString(transition.to_mode) ?? '?'} ·{' '}
                    {triggerSummary(transition)}
                  </span>
                  <div className="p5RowActions">
                    <button
                      className="consoleButton small"
                      type="button"
                      onClick={() => openTransition(transition)}
                    >
                      Edit
                    </button>
                    <button
                      className="consoleButton small"
                      type="button"
                      disabled={mutate.busy || !id}
                      onClick={() => void handleTrigger(transition)}
                    >
                      <Play size={13} />
                      <span>Trigger now</span>
                    </button>
                  </div>
                </div>
              );
            })}
          </div>
        )}

        {transitionDraft ? (
          <TransitionEditor
            draft={transitionDraft}
            text={transitionText}
            busy={mutate.busy}
            onChange={updateTransitionDraft}
            onTextChange={(text, parsed) => {
              setTransitionText(text);
              if (
                parsed !== undefined &&
                typeof parsed === 'object' &&
                parsed !== null &&
                !Array.isArray(parsed)
              ) {
                setTransitionDraft(parsed as Record<string, unknown>);
              }
            }}
            onSave={() => void saveTransition()}
            onClose={() => {
              setTransitionDraft(null);
              setSelectedTransitionId(null);
            }}
          />
        ) : null}
      </SectionCard>
    </div>
  );
}

function TransitionEditor({
  draft,
  text,
  busy,
  onChange,
  onTextChange,
  onSave,
  onClose
}: {
  draft: Record<string, unknown>;
  text: string;
  busy: boolean;
  onChange: (next: Record<string, unknown>) => void;
  onTextChange: (text: string, parsed: unknown | undefined) => void;
  onSave: () => void;
  onClose: () => void;
}) {
  const trigger = asRecord(draft.trigger);
  const triggerKind =
    asString(trigger.type) ??
    (typeof draft.trigger === 'string' ? (draft.trigger as string) : 'manual');
  const scheduledHour = scheduledHourOf(trigger);
  const duration = parseTimerSetting(draft.duration_ms) ?? { type: 'auto' as const };

  function patchTrigger(nextTrigger: Record<string, unknown>) {
    onChange({ ...draft, trigger: nextTrigger });
  }

  return (
    <div className="p5Editor">
      <div className="p5EditorHeader">
        <h4 className="p5SubHeading">
          Edit: {asString(draft.label) ?? transitionId(draft) ?? 'transition'}
        </h4>
        <div className="p5RowActions">
          <button
            className="consoleButton primary small"
            type="button"
            disabled={busy}
            onClick={onSave}
          >
            <Save size={13} />
            <span>Save transition</span>
          </button>
          <button className="consoleButton small" type="button" onClick={onClose}>
            Close
          </button>
        </div>
      </div>

      <div className="formRow">
        <div className="formRowLabel">
          <span>Label</span>
        </div>
        <div className="formRowControl">
          <TextField
            value={asString(draft.label) ?? ''}
            onChange={(value) => onChange({ ...draft, label: value })}
          />
        </div>
      </div>

      <div className="formRow">
        <div className="formRowLabel">
          <span>Trigger</span>
          <small>How this transition fires</small>
        </div>
        <div className="formRowControl">
          <SegmentedControl
            value={triggerKind}
            onChange={(kind) => {
              if (kind === 'manual') patchTrigger({ type: 'manual' });
              else if (kind === 'solar')
                patchTrigger({
                  type: 'solar',
                  event: asString(trigger.event) ?? 'sunset'
                });
              else
                patchTrigger({
                  type: 'scheduled',
                  time: scheduledHour ?? 21
                });
            }}
            options={[
              { value: 'manual', label: 'Manual' },
              { value: 'solar', label: 'Solar' },
              { value: 'scheduled', label: 'Scheduled' }
            ]}
          />
        </div>
      </div>

      {triggerKind === 'solar' ? (
        <div className="formRow">
          <div className="formRowLabel">
            <span>Solar event</span>
          </div>
          <div className="formRowControl">
            <TextField
              value={asString(trigger.event) ?? ''}
              placeholder="sunrise | sunset | dusk | dawn"
              mono
              onChange={(value) => patchTrigger({ ...trigger, event: value })}
            />
          </div>
        </div>
      ) : null}

      {triggerKind === 'scheduled' ? (
        <div className="p5DialRow">
          <ClockDial
            size={190}
            markers={[
              {
                id: 'time',
                hour: scheduledHour ?? 21,
                label: 'at'
              }
            ]}
            onCommit={(_, hour) => patchTrigger({ ...trigger, time: hour })}
          />
          <span className="cardNote">
            Fires at {formatHour(scheduledHour ?? 21)} (drag the marker).
          </span>
        </div>
      ) : null}

      <div className="formRow">
        <div className="formRowLabel">
          <span>Trigger enabled</span>
        </div>
        <div className="formRowControl">
          <ToggleSwitch
            checked={asBoolean(draft.trigger_enabled) ?? true}
            onChange={(value) => onChange({ ...draft, trigger_enabled: value })}
          />
        </div>
      </div>

      <div className="formRow">
        <div className="formRowLabel">
          <span>Duration</span>
        </div>
        <div className="formRowControl">
          <SegmentedControl
            value={duration.type === 'fixed' ? 'fixed' : 'auto'}
            onChange={(kind) =>
              onChange({
                ...draft,
                duration_ms: timerSettingToJson(
                  kind === 'auto'
                    ? { type: 'auto' }
                    : {
                        type: 'fixed',
                        value: duration.type === 'fixed' ? duration.value : 3000
                      }
                )
              })
            }
            options={[
              { value: 'auto', label: 'Auto' },
              { value: 'fixed', label: 'Fixed' }
            ]}
          />
          {duration.type === 'fixed' ? (
            <>
              <NumberField
                value={duration.value}
                min={0}
                onChange={(value) => {
                  if (value !== undefined) {
                    onChange({
                      ...draft,
                      duration_ms: timerSettingToJson({ type: 'fixed', value })
                    });
                  }
                }}
              />
              <span className="timerUnit">ms</span>
            </>
          ) : null}
        </div>
      </div>

      <div className="formRow">
        <div className="formRowLabel">
          <span>Preserve hard off</span>
          <small>Skip rooms the customer switched hard-off</small>
        </div>
        <div className="formRowControl">
          <ToggleSwitch
            checked={asBoolean(draft.preserve_hard_off) ?? false}
            onChange={(value) =>
              onChange({ ...draft, preserve_hard_off: value })
            }
          />
        </div>
      </div>

      <h4 className="p5SubHeading">Full transition JSON</h4>
      <JsonEditor value={text} rows={10} onChange={onTextChange} />
    </div>
  );
}

function transitionsFromPayload(payload: unknown): Record<string, unknown>[] {
  if (Array.isArray(payload)) return asRecordArray(payload);
  const record = asRecord(payload);
  return asRecordArray(record.transitions);
}

function transitionId(transition: Record<string, unknown>): string | null {
  return asString(transition.id) ?? asString(transition.transition_id) ?? null;
}

function triggerSummary(transition: Record<string, unknown>): string {
  const trigger = asRecord(transition.trigger);
  const kind =
    asString(trigger.type) ??
    (typeof transition.trigger === 'string'
      ? (transition.trigger as string)
      : 'manual');
  if (kind === 'solar') return `solar ${asString(trigger.event) ?? ''}`.trim();
  if (kind === 'scheduled') {
    const hour = scheduledHourOf(trigger);
    return hour !== undefined ? `at ${formatHour(hour)}` : 'scheduled';
  }
  return kind;
}

function scheduledHourOf(trigger: Record<string, unknown>): number | undefined {
  const numeric = asNumber(trigger.time) ?? asNumber(trigger.hour);
  if (numeric !== undefined) return numeric;
  const text = asString(trigger.time);
  if (text) {
    const match = /^(\d{1,2}):(\d{2})/.exec(text);
    if (match) return Number(match[1]) + Number(match[2]) / 60;
  }
  return undefined;
}

function profileOptionsFromPayload(
  payload: unknown
): Array<{ value: string; label: string }> {
  const record = asRecord(payload);
  const profiles = Array.isArray(payload)
    ? asRecordArray(payload)
    : asRecordArray(record.profiles);
  return profiles
    .map((profile) => {
      const id = asString(profile.id) ?? asString(profile.profile_id);
      if (!id) return null;
      const name = asString(profile.name) ?? asString(profile.label);
      return { value: id, label: name ? `${name} (${id})` : id };
    })
    .filter((option): option is { value: string; label: string } => option !== null);
}
