import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  CheckCircle2,
  Clock3,
  Lightbulb,
  RefreshCw,
  RotateCcw,
  Save,
  Undo2
} from 'lucide-react';

import { ColorWheel } from '../../components/controls/ColorWheel';
import { SegmentedControl } from '../../components/controls/SegmentedControl';
import {
  KelvinSlider,
  RangeSlider,
  Slider
} from '../../components/controls/Slider';
import { kelvinToRgb } from '../../components/controls/colorMath';
import { ToggleSwitch } from '../../components/controls/ToggleSwitch';
import { EmptyState, ErrorNotice } from '../../components/ui/bits';
import { useConfirm } from '../../components/ui/ConfirmDialog';
import {
  RawPayloadToggle,
  SectionCard
} from '../../components/ui/SectionCard';
import {
  sendNodeAction,
  setNodeBrightness,
  setNodeColor,
  setNodeCurveColorTemperature,
  setNodePreferences,
  setNodeProfileOverrides,
  setNodesOffset,
  type RgbColor
} from '../../device/nodes';
import { getNodesState, getState } from '../../device/state';
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
import { errorMessage } from '../../lib/format';
import type { DeviceAdminProxyResponse } from '../../types';
import {
  buildInheritedProfileOverride,
  changedOverrideFields,
  createLightSettingsJourneyId,
  directColorRgb,
  effectiveProfileConfig,
  hasLightProfileOverrideCapability,
  inferredLocalProfileOverrides,
  isLightAddressableKind,
  isSleepProfile,
  lightSettingsProfilesFromState,
  profileConfigsEqual,
  profileOverrideMapsEqual,
  profileOverridesForNode,
  profileOverridesFromNode,
  recordOf,
  withDirectColor,
  withSelectedProfileOverride,
  type JsonRecord,
  type LightSettingsProfile
} from './nodeLightSettings';
import '../../styles/pages-phase4.css';

type NodeSummary = {
  id: string;
  name: string;
  kind?: string;
  parentId?: string;
  room?: string;
  powerOn?: boolean;
  rhythmEnabled?: boolean;
  disabled?: boolean;
  brightness?: number;
  kelvin?: number;
  profileOverrides: JsonRecord;
  raw: Record<string, unknown>;
};

const NODE_ACTIONS: Array<{ id: string; label: string }> = [
  { id: 'on_press', label: 'On' },
  { id: 'off_press', label: 'Off' },
  { id: 'toggle', label: 'Toggle' },
  { id: 'reset', label: 'Reset' },
  { id: 'step_up', label: 'Step up' },
  { id: 'step_down', label: 'Step down' },
  { id: 'rhythm_on', label: 'Rhythm on' },
  { id: 'rhythm_off', label: 'Rhythm off' }
];

function parseNodes(payload: unknown): NodeSummary[] {
  const record = asRecord(payload);
  const rawNodes = Array.isArray(payload)
    ? asRecordArray(payload)
    : asRecordArray(record.nodes ?? record.states ?? record.rooms);
  return rawNodes
    .map((raw): NodeSummary | null => {
      const state = asRecord(raw.state);
      const id = asString(raw.node_id) ?? asString(raw.id);
      if (!id) return null;
      return {
        id,
        name: asString(raw.name) ?? asString(raw.label) ?? id,
        kind: asString(raw.kind) ?? asString(raw.node_kind),
        parentId: asString(raw.parent_id),
        room:
          asString(raw.room_name) ??
          asString(raw.room) ??
          asString(raw.room_id) ??
          asString(raw.parent_name),
        powerOn:
          asBoolean(raw.power_on) ??
          asBoolean(state.power_on) ??
          asBoolean(raw.lights_on),
        rhythmEnabled:
          asBoolean(raw.rhythm_enabled) ?? asBoolean(state.rhythm_enabled),
        disabled: asBoolean(raw.disabled),
        brightness: asNumber(raw.brightness) ?? asNumber(state.brightness),
        kelvin:
          asNumber(raw.kelvin) ??
          asNumber(state.kelvin) ??
          asNumber(raw.color_temperature),
        profileOverrides: profileOverridesFromNode(raw),
        raw
      };
    })
    .filter((node): node is NodeSummary => node !== null);
}

export default function NodesPage() {
  const client = useDeviceClient();
  const [selectedId, setSelectedId] = useState<string | null>(null);

  const nodesQuery = usePolling(
    useCallback(() => getNodesState(client), [client]),
    { intervalMs: 5_000 }
  );
  const stateQuery = usePolling(
    useCallback(() => getState(client), [client])
  );

  const nodes = useMemo(() => parseNodes(nodesQuery.data), [nodesQuery.data]);
  const lightSettingsProfiles = useMemo(
    () => lightSettingsProfilesFromState(stateQuery.data),
    [stateQuery.data]
  );
  const lightSettingsSupported = useMemo(
    () => hasLightProfileOverrideCapability(stateQuery.data),
    [stateQuery.data]
  );
  const selected =
    nodes.find((node) => node.id === selectedId) ?? nodes[0] ?? null;

  const grouped = useMemo(() => {
    const groups = new Map<string, NodeSummary[]>();
    for (const node of nodes) {
      const key = node.room ?? node.kind ?? 'Ungrouped';
      const list = groups.get(key) ?? [];
      list.push(node);
      groups.set(key, list);
    }
    return [...groups.entries()].sort(([a], [b]) => a.localeCompare(b));
  }, [nodes]);

  return (
    <div className="consolePage wide">
      <header className="pageHeader">
        <div>
          <div className="eyebrow">Device console</div>
          <h2>Rooms &amp; Nodes</h2>
          <p className="pageIntro">
            Live node tree (refreshes every 5 seconds) with per-node lighting
            controls and persistent Day/Sleep settings.
          </p>
        </div>
        <div className="pageHeaderActions">
          <button
            className="consoleButton"
            type="button"
            onClick={() =>
              void Promise.all([nodesQuery.refresh(), stateQuery.refresh()])
            }
          >
            <RefreshCw size={15} />
            <span>Refresh</span>
          </button>
        </div>
      </header>

      {nodesQuery.error && !nodesQuery.data ? (
        <ErrorNotice message={nodesQuery.error} />
      ) : null}

      <div className="nodesLayout">
        <aside className="nodeTree">
          {nodes.length === 0 ? (
            <EmptyState
              message={nodesQuery.loading ? 'Loading nodes…' : 'No nodes reported.'}
              icon={<Lightbulb size={20} />}
            />
          ) : (
            grouped.map(([group, members]) => (
              <div className="nodeGroup" key={group}>
                <div className="nodeGroupTitle">{group}</div>
                {members.map((node) => (
                  <button
                    key={node.id}
                    type="button"
                    className={`nodeItem${selected?.id === node.id ? ' selected' : ''}`}
                    onClick={() => setSelectedId(node.id)}
                  >
                    <span
                      className={`nodeDot${node.powerOn ? ' on' : ''}${node.disabled ? ' disabled' : ''}`}
                    />
                    <span className="nodeItemName">{node.name}</span>
                    <span className="nodeItemMeta readout">
                      {node.brightness !== undefined ? `${Math.round(node.brightness)}%` : ''}
                      {node.kelvin !== undefined ? ` ${Math.round(node.kelvin)}K` : ''}
                    </span>
                  </button>
                ))}
              </div>
            ))
          )}
        </aside>

        <div className="nodeDetail">
          {selected ? (
            <NodeDetail
              key={selected.id}
              node={selected}
              parentProfileOverrides={
                nodes.find((node) => node.id === selected.parentId)
                  ?.profileOverrides ?? {}
              }
              onWrite={nodesQuery.refresh}
              lightSettingsProfiles={lightSettingsProfiles}
              lightSettingsSupported={lightSettingsSupported}
              lightSettingsLoading={stateQuery.loading}
              lightSettingsError={stateQuery.error}
            />
          ) : (
            <EmptyState message="Select a node." icon={<Lightbulb size={20} />} />
          )}
        </div>
      </div>
    </div>
  );
}

function NodeDetail({
  node,
  parentProfileOverrides,
  onWrite,
  lightSettingsProfiles,
  lightSettingsSupported,
  lightSettingsLoading,
  lightSettingsError
}: {
  node: NodeSummary;
  parentProfileOverrides: JsonRecord;
  onWrite: () => Promise<void>;
  lightSettingsProfiles: LightSettingsProfile[];
  lightSettingsSupported: boolean;
  lightSettingsLoading: boolean;
  lightSettingsError: string | null;
}) {
  const client = useDeviceClient();

  const [colorScope, setColorScope] = useState<'preview' | 'mood' | 'auto'>('auto');
  const [wheelRgb, setWheelRgb] = useState<RgbColor>({ r: 255, g: 180, b: 120 });
  const [offsetHours, setOffsetHours] = useState(0);

  const write = useDeviceCall(
    useCallback(
      async (action: () => Promise<unknown>) => {
        await action();
        await onWrite();
      },
      [onWrite]
    )
  );

  return (
    <div className="nodeDetailStack">
      <SectionCard
        title={node.name}
        subtitle={`${node.kind ?? 'node'} · ${node.id}`}
        busy={write.busy}
        error={write.error}
        rawPayload={node.raw}
        actions={
          <span className={`consoleStatus${node.powerOn ? ' online' : ' idle'}`}>
            {node.powerOn ? 'On' : 'Off'}
          </span>
        }
      >
        <div className="actionRow">
          {NODE_ACTIONS.map((action) => (
            <button
              key={action.id}
              className="consoleButton small"
              type="button"
              disabled={write.busy}
              onClick={() =>
                void write.run(() => sendNodeAction(client, node.id, action.id))
              }
            >
              {action.label}
            </button>
          ))}
        </div>

        <Slider
          label="Brightness"
          value={node.brightness ?? 50}
          min={1}
          max={100}
          format={(value) => `${Math.round(value)}%`}
          disabled={write.busy}
          onCommit={(value) =>
            void write.run(() => setNodeBrightness(client, node.id, value))
          }
        />
        <KelvinSlider
          label="Color temp"
          value={node.kelvin ?? 3000}
          min={500}
          max={6500}
          disabled={write.busy}
          onCommit={(value) =>
            void write.run(() =>
              setNodeCurveColorTemperature(client, node.id, value, true)
            )
          }
        />
      </SectionCard>

      {isLightAddressableKind(node.kind) ? (
        <NodeLightSettingsCard
          node={node}
          parentProfileOverrides={parentProfileOverrides}
          profiles={lightSettingsProfiles}
          supported={lightSettingsSupported}
          loading={lightSettingsLoading}
          loadError={lightSettingsError}
          onWrite={onWrite}
        />
      ) : null}

      <div className="cardGrid two">
        <SectionCard title="Color" subtitle="Direct RGB with scope">
          <div className="colorRow">
            <ColorWheel
              rgb={wheelRgb}
              size={180}
              onChange={setWheelRgb}
              onCommit={(rgb) => {
                setWheelRgb(rgb);
                void write.run(() =>
                  setNodeColor(client, node.id, { rgb, scope: colorScope })
                );
              }}
            />
            <div className="colorControls">
              <SegmentedControl
                value={colorScope}
                onChange={(value) =>
                  setColorScope(value as 'preview' | 'mood' | 'auto')
                }
                options={[
                  { value: 'auto', label: 'Auto' },
                  { value: 'mood', label: 'Mood' },
                  { value: 'preview', label: 'Preview' }
                ]}
              />
              <p className="cardNote">
                Preview is temporary, mood persists as an override, auto lets
                the device decide.
              </p>
            </div>
          </div>
        </SectionCard>

        <SectionCard title="Preferences" subtitle="Per-node runtime flags">
          <ToggleSwitch
            checked={node.rhythmEnabled ?? false}
            label="Rhythm enabled"
            busy={write.busy}
            onChange={(value) =>
              void write.run(() =>
                setNodePreferences(client, {
                  node_id: node.id,
                  rhythm_enabled: value
                })
              )
            }
          />
          <ToggleSwitch
            checked={node.disabled ?? false}
            label="Disabled (hidden from runtime)"
            busy={write.busy}
            onChange={(value) =>
              void write.run(() =>
                setNodePreferences(client, {
                  node_id: node.id,
                  disabled: value
                })
              )
            }
          />
          <div className="offsetRow">
            <Slider
              label="Time offset"
              value={offsetHours}
              min={-12}
              max={12}
              step={0.25}
              format={(value) => `${value > 0 ? '+' : ''}${value}h`}
              onChange={setOffsetHours}
              onCommit={(value) => {
                setOffsetHours(value);
                void write.run(() => setNodesOffset(client, value, [node.id]));
              }}
            />
            <button
              className="consoleButton small"
              type="button"
              onClick={() => {
                setOffsetHours(0);
                void write.run(() => setNodesOffset(client, 0, [node.id]));
              }}
            >
              <RotateCcw size={13} />
              <span>Reset</span>
            </button>
          </div>
        </SectionCard>
      </div>
    </div>
  );
}

type SettingsWriteReceipt = {
  status: 'pending' | 'confirmed' | 'conflict';
  operation: 'save_profile' | 'reset_profile' | 'reset_all';
  requestId: string;
  nodeId: string;
  profileId: string;
  changedFields: string[];
  before: JsonRecord;
  candidate: JsonRecord;
  after?: JsonRecord;
  proxy: {
    route?: 'remote' | 'local';
    completedAt: string;
    statusCode: number;
    verifiedServerInstanceId?: string;
    preconditionBodySha256?: string;
    responseBodySha256?: string;
  };
};

function NodeLightSettingsCard({
  node,
  parentProfileOverrides,
  profiles,
  supported,
  loading,
  loadError,
  onWrite
}: {
  node: NodeSummary;
  parentProfileOverrides: JsonRecord;
  profiles: LightSettingsProfile[];
  supported: boolean;
  loading: boolean;
  loadError: string | null;
  onWrite: () => Promise<void>;
}) {
  const [selectedProfileId, setSelectedProfileId] = useState<string | null>(
    null
  );
  const selectedProfile =
    profiles.find((profile) => profile.id === selectedProfileId) ??
    profiles[0] ??
    null;
  const localOverrides = inferredLocalProfileOverrides(
    node.profileOverrides,
    parentProfileOverrides
  );
  const hasLocalOverrides = Object.keys(localOverrides).length > 0;
  const hasInheritedOverrides =
    !hasLocalOverrides && Object.keys(node.profileOverrides).length > 0;

  useEffect(() => {
    if (!selectedProfileId && profiles[0]) {
      setSelectedProfileId(profiles[0].id);
    } else if (
      selectedProfileId &&
      !profiles.some((profile) => profile.id === selectedProfileId)
    ) {
      setSelectedProfileId(profiles[0]?.id ?? null);
    }
  }, [profiles, selectedProfileId]);

  return (
    <SectionCard
      title="Light settings"
      subtitle="Persistent Day and Sleep overrides for this target"
      actions={
        <span
          className={`consoleStatus${hasLocalOverrides ? ' checking' : ' idle'}`}
        >
          {hasLocalOverrides
            ? 'Custom'
            : hasInheritedOverrides
              ? 'Inherited'
              : 'Automatic'}
        </span>
      }
    >
      {loadError ? <ErrorNotice message={loadError} /> : null}
      {loading ? (
        <EmptyState message="Loading profile settings…" />
      ) : loadError && profiles.length === 0 ? null : !supported ? (
        <EmptyState message="This appliance does not advertise per-room light settings. Update the appliance before editing." />
      ) : profiles.length === 0 ? (
        <EmptyState message="The appliance did not report Day or Sleep profiles." />
      ) : (
        <div className="nodeLightSettings">
          <div className="profileTabs nodeLightProfileTabs">
            <div className="profileTabGroup primary">
              {profiles.map((profile) => {
                const customized =
                  Object.keys(recordOf(localOverrides[profile.id]))
                    .length > 0;
                const inherited =
                  !customized &&
                  Object.keys(recordOf(parentProfileOverrides[profile.id]))
                    .length > 0;
                return (
                  <button
                    key={profile.id}
                    type="button"
                    className={`profileTab${profile.id === selectedProfile?.id ? ' active' : ''}`}
                    onClick={() => setSelectedProfileId(profile.id)}
                  >
                    <span>{profile.name}</span>
                    {customized || inherited ? (
                      <span
                        className={`nodeLightCustomBadge${inherited ? ' inherited' : ''}`}
                      >
                        {inherited ? 'Inherited' : 'Custom'}
                      </span>
                    ) : null}
                  </button>
                );
              })}
            </div>
          </div>

          {selectedProfile ? (
            <ProfileLightSettingsEditor
              key={`${node.id}:${selectedProfile.id}`}
              node={node}
              parentProfileOverrides={parentProfileOverrides}
              profile={selectedProfile}
              onWrite={onWrite}
            />
          ) : null}
        </div>
      )}
    </SectionCard>
  );
}

function ProfileLightSettingsEditor({
  node,
  parentProfileOverrides,
  profile,
  onWrite
}: {
  node: NodeSummary;
  parentProfileOverrides: JsonRecord;
  profile: LightSettingsProfile;
  onWrite: () => Promise<void>;
}) {
  const client = useDeviceClient();
  const confirm = useConfirm();
  const [baselineBase, setBaselineBase] = useState<JsonRecord>(() =>
    effectiveProfileConfig(profile.config, null)
  );
  const [baselineOverrides, setBaselineOverrides] = useState<JsonRecord>(() =>
    effectiveProfileConfig(node.profileOverrides, null)
  );
  const [baselineParentOverrides, setBaselineParentOverrides] =
    useState<JsonRecord>(() =>
      effectiveProfileConfig(parentProfileOverrides, null)
    );
  const [draft, setDraft] = useState<JsonRecord>(() =>
    effectiveProfileConfig(
      profile.config,
      node.profileOverrides[profile.id]
    )
  );
  const [stale, setStale] = useState(false);
  const [busy, setBusy] = useState(false);
  const [writeError, setWriteError] = useState<string | null>(null);
  const [receipt, setReceipt] = useState<SettingsWriteReceipt | null>(null);

  const inferredLocalOverrides = inferredLocalProfileOverrides(
    baselineOverrides,
    baselineParentOverrides
  );
  const baselineParentProfileOverride = recordOf(
    baselineParentOverrides[profile.id]
  );
  const baselineLocalProfileOverride = recordOf(
    inferredLocalOverrides[profile.id]
  );
  const candidateLocalOverride = buildInheritedProfileOverride(
    baselineBase,
    draft,
    baselineLocalProfileOverride,
    baselineParentProfileOverride
  );
  const candidateEffectiveOverride =
    candidateLocalOverride ??
    (Object.keys(baselineParentProfileOverride).length > 0
      ? baselineParentProfileOverride
      : null);
  const candidateOverrides = withSelectedProfileOverride(
    baselineOverrides,
    profile.id,
    candidateEffectiveOverride
  );
  const dirty = !profileConfigsEqual(
    draft,
    effectiveProfileConfig(
      baselineBase,
      baselineOverrides[profile.id]
    )
  );
  const pending = receipt?.status === 'pending';

  useEffect(() => {
    const baseChanged = !profileConfigsEqual(profile.config, baselineBase);
    const overridesChanged = !profileOverrideMapsEqual(
      node.profileOverrides,
      baselineOverrides
    );
    const parentOverridesChanged = !profileOverrideMapsEqual(
      parentProfileOverrides,
      baselineParentOverrides
    );
    if (!baseChanged && !overridesChanged && !parentOverridesChanged) return;

    if (
      receipt?.status === 'pending' &&
      profileOverrideMapsEqual(node.profileOverrides, receipt.candidate)
    ) {
      const nextBase = effectiveProfileConfig(profile.config, null);
      const nextOverrides = effectiveProfileConfig(
        node.profileOverrides,
        null
      );
      setBaselineBase(nextBase);
      setBaselineOverrides(nextOverrides);
      setBaselineParentOverrides(
        effectiveProfileConfig(parentProfileOverrides, null)
      );
      setDraft(
        effectiveProfileConfig(
          nextBase,
          nextOverrides[profile.id]
        )
      );
      setStale(false);
      setReceipt({
        ...receipt,
        status: 'confirmed',
        after: nextOverrides
      });
      return;
    }

    if (!dirty && receipt?.status !== 'pending') {
      const nextBase = effectiveProfileConfig(profile.config, null);
      const nextOverrides = effectiveProfileConfig(
        node.profileOverrides,
        null
      );
      setBaselineBase(nextBase);
      setBaselineOverrides(nextOverrides);
      setBaselineParentOverrides(
        effectiveProfileConfig(parentProfileOverrides, null)
      );
      setDraft(
        effectiveProfileConfig(
          nextBase,
          nextOverrides[profile.id]
        )
      );
      setStale(false);
      return;
    }

    setStale(true);
    if (
      receipt?.status === 'pending' &&
      !profileOverrideMapsEqual(node.profileOverrides, baselineOverrides)
    ) {
      setReceipt({
        ...receipt,
        status: 'conflict',
        after: effectiveProfileConfig(node.profileOverrides, null)
      });
    }
  }, [
    baselineBase,
    baselineOverrides,
    baselineParentOverrides,
    dirty,
    node.profileOverrides,
    parentProfileOverrides,
    profile.config,
    profile.id,
    receipt
  ]);

  const minBrightness = asNumber(draft.min_brightness) ?? 1;
  const maxBrightness = asNumber(draft.max_brightness) ?? 100;
  const minColorTemp = asNumber(draft.min_color_temp) ?? 2200;
  const maxColorTemp = asNumber(draft.max_color_temp) ?? 6500;
  const sleep = isSleepProfile(profile.id);
  const sleepRgb =
    directColorRgb(draft) ?? kelvinToRgb(Math.max(1000, minColorTemp));

  function patchDraft(patch: JsonRecord) {
    setDraft((current) => ({ ...current, ...patch }));
    setWriteError(null);
    setReceipt(null);
  }

  function adoptLatestCanonicalState() {
    const nextBase = effectiveProfileConfig(profile.config, null);
    const nextOverrides = effectiveProfileConfig(node.profileOverrides, null);
    const nextParentOverrides = effectiveProfileConfig(
      parentProfileOverrides,
      null
    );
    setBaselineBase(nextBase);
    setBaselineOverrides(nextOverrides);
    setBaselineParentOverrides(nextParentOverrides);
    setDraft(
      effectiveProfileConfig(
        nextBase,
        nextOverrides[profile.id]
      )
    );
    setWriteError(null);
    setReceipt(null);
    setStale(false);
  }

  async function applyReviewedCandidate({
    operation,
    candidate,
    patch,
    fields
  }: {
    operation: SettingsWriteReceipt['operation'];
    candidate: JsonRecord;
    patch: JsonRecord | null;
    fields: string[];
  }) {
    setBusy(true);
    setWriteError(null);
    const requestId = createLightSettingsJourneyId();
    try {
      const latestState = await client.get<JsonRecord>('api/state', {
        requestId
      });
      if (!hasLightProfileOverrideCapability(latestState)) {
        setStale(true);
        throw new Error(
          'The appliance no longer advertises per-room light settings; no write was sent.'
        );
      }
      const latestProfile = lightSettingsProfilesFromState(latestState).find(
        (entry) => entry.id === profile.id
      );
      if (
        !latestProfile ||
        !profileConfigsEqual(latestProfile.config, baselineBase)
      ) {
        setStale(true);
        throw new Error(
          `${profile.name} changed on the appliance. Refresh and review the new automatic settings before applying.`
        );
      }

      const snapshot = await client.getReceipt('api/nodes/state', {
        requestId
      });
      if (!snapshot.bodySha256) {
        throw new Error(
          'The admin API did not provide a node-state freshness hash; no write was sent.'
        );
      }
      const latestOverrides = profileOverridesForNode(snapshot.body, node.id);
      if (latestOverrides === null) {
        setStale(true);
        throw new Error(
          `${node.name} is no longer present in appliance node state; no write was sent.`
        );
      }
      if (
        !profileOverrideMapsEqual(latestOverrides, baselineOverrides)
      ) {
        setStale(true);
        throw new Error(
          `${node.name}'s light settings changed while this proposal was open. Refresh and review before applying.`
        );
      }

      const proxy = await setNodeProfileOverrides(client, node.id, patch, {
        replace: true,
        correlationId: requestId,
        expectedProfileOverrides: baselineOverrides,
        resourcePrecondition: {
          path: 'api/nodes/state',
          bodySha256: snapshot.bodySha256
        }
      });
      setReceipt({
        status: 'pending',
        operation,
        requestId,
        nodeId: node.id,
        profileId: profile.id,
        changedFields: fields,
        before: effectiveProfileConfig(baselineOverrides, null),
        candidate: effectiveProfileConfig(candidate, null),
        proxy: proxyReceiptSummary(proxy)
      });
      await onWrite();
    } catch (error) {
      setWriteError(`${errorMessage(error)} Request ${requestId}.`);
    } finally {
      setBusy(false);
    }
  }

  async function reviewSave() {
    if (!dirty || busy || stale || pending) return;
    const fields = changedOverrideFields(
      baselineLocalProfileOverride,
      candidateLocalOverride
    );
    const ok = await confirm({
      title: `Apply ${profile.name} light settings`,
      message: `Apply the reviewed ${profile.name} override to ${node.name}? Changed fields: ${fields.join(', ') || 'automatic inheritance'}. The request will remain pending until appliance state confirms it.`,
      confirmLabel: 'Save & apply'
    });
    if (!ok) return;
    await applyReviewedCandidate({
      operation: 'save_profile',
      candidate: candidateOverrides,
      patch: { [profile.id]: candidateLocalOverride },
      fields
    });
  }

  async function reviewProfileReset() {
    if (
      Object.keys(baselineLocalProfileOverride).length === 0 ||
      busy ||
      stale ||
      pending
    ) {
      return;
    }
    const candidate = withSelectedProfileOverride(
      baselineOverrides,
      profile.id,
      Object.keys(baselineParentProfileOverride).length > 0
        ? baselineParentProfileOverride
        : null
    );
    const ok = await confirm({
      title: `Use automatic ${profile.name} settings`,
      message: `${node.name} will inherit ${node.parentId ? `its parent room's ${profile.name} settings` : `the home ${profile.name} profile`} again. Other profiles, power, motion, and runtime preferences stay unchanged.`,
      confirmLabel: 'Use automatic'
    });
    if (!ok) return;
    await applyReviewedCandidate({
      operation: 'reset_profile',
      candidate,
      patch: { [profile.id]: null },
      fields: Object.keys(baselineLocalProfileOverride).sort()
    });
  }

  async function reviewAllReset() {
    if (
      Object.keys(inferredLocalOverrides).length === 0 ||
      busy ||
      stale ||
      pending
    ) {
      return;
    }
    const ok = await confirm({
      title: 'Use automatic light settings',
      message: `${node.name} will inherit ${node.parentId ? "every light profile from its parent room" : 'every home light profile'} again. Power, motion, topology, and runtime preferences stay unchanged.`,
      confirmLabel: 'Reset all profiles',
      danger: true
    });
    if (!ok) return;
    await applyReviewedCandidate({
      operation: 'reset_all',
      candidate: baselineParentOverrides,
      patch: null,
      fields: Object.keys(inferredLocalOverrides).sort()
    });
  }

  return (
    <div className="nodeLightProfileEditor">
      <div className="nodeLightProfileSummary">
        <div>
          <strong>{profile.name}</strong>
          <span>
            {Object.keys(baselineLocalProfileOverride).length > 0
              ? 'Custom values override inherited room/home settings.'
              : Object.keys(baselineParentProfileOverride).length > 0
                ? 'Inheriting this profile from the parent room.'
                : 'Following the automatic home profile.'}
          </span>
        </div>
        <span
          className={`consoleStatus${Object.keys(baselineLocalProfileOverride).length > 0 ? ' checking' : ' idle'}`}
        >
          {Object.keys(baselineLocalProfileOverride).length > 0
            ? 'Custom'
            : Object.keys(baselineParentProfileOverride).length > 0
              ? 'Inherited'
              : 'Automatic'}
        </span>
      </div>

      {sleep ? (
        <div className="nodeLightControlGrid sleep">
          <div className="nodeLightControlPanel">
            <h4>Brightness</h4>
            <p className="cardNote">
              Fixed Sleep level, matching the app's room/bulb editor.
            </p>
            <Slider
              label="Level"
              value={maxBrightness}
              min={1}
              max={100}
              disabled={busy || pending}
              format={(value) => `${Math.round(value)}%`}
              onChange={(value) =>
                patchDraft({
                  min_brightness: Math.round(value),
                  max_brightness: Math.round(value)
                })
              }
              onCommit={(value) =>
                patchDraft({
                  min_brightness: Math.round(value),
                  max_brightness: Math.round(value)
                })
              }
            />
          </div>
          <div className="nodeLightControlPanel">
            <h4>Fixed color</h4>
            <p className="cardNote">
              Direct Sleep color. Brightness stays controlled separately.
            </p>
            <div className="nodeLightSleepColor">
              <ColorWheel
                rgb={sleepRgb}
                size={150}
                disabled={busy || pending}
                onChange={(rgb) => {
                  setDraft((current) => withDirectColor(current, rgb));
                  setWriteError(null);
                  setReceipt(null);
                }}
                onCommit={(rgb) => {
                  setDraft((current) => withDirectColor(current, rgb));
                  setWriteError(null);
                  setReceipt(null);
                }}
              />
              <span className="readout">
                rgb({sleepRgb.r}, {sleepRgb.g}, {sleepRgb.b})
              </span>
            </div>
          </div>
        </div>
      ) : (
        <div className="nodeLightControlGrid">
          <div className="nodeLightControlPanel">
            <h4>Brightness</h4>
            <p className="cardNote">
              Lowest and highest output across the Day curve.
            </p>
            <RangeSlider
              label="Range"
              low={minBrightness}
              high={maxBrightness}
              min={1}
              max={100}
              minGap={1}
              disabled={busy || pending}
              format={(value) => `${Math.round(value)}%`}
              onChange={(low, high) =>
                patchDraft({
                  min_brightness: Math.round(low),
                  max_brightness: Math.round(high)
                })
              }
              onCommit={(low, high) =>
                patchDraft({
                  min_brightness: Math.round(low),
                  max_brightness: Math.round(high)
                })
              }
            />
          </div>
          <div className="nodeLightControlPanel">
            <h4>Sun hue</h4>
            <p className="cardNote">
              Warmest and coolest color temperature across the Day curve.
            </p>
            <RangeSlider
              label="Range"
              low={minColorTemp}
              high={maxColorTemp}
              min={500}
              max={6500}
              step={50}
              minGap={100}
              trackStyle="kelvin"
              disabled={busy || pending}
              format={(value) => `${Math.round(value)}K`}
              onChange={(low, high) =>
                patchDraft({
                  min_color_temp: Math.round(low),
                  max_color_temp: Math.round(high)
                })
              }
              onCommit={(low, high) =>
                patchDraft({
                  min_color_temp: Math.round(low),
                  max_color_temp: Math.round(high)
                })
              }
            />
          </div>
        </div>
      )}

      {stale ? (
        <ErrorNotice message="The automatic profile or target overrides changed after this draft was opened. Refresh before applying." />
      ) : null}
      {writeError ? <ErrorNotice message={writeError} /> : null}

      {receipt ? (
        <div className={`nodeLightReceipt ${receipt.status}`}>
          <div>
            {receipt.status === 'confirmed' ? (
              <CheckCircle2 size={16} />
            ) : (
              <Clock3 size={16} />
            )}
            <span>
              {receipt.status === 'pending'
                ? 'Accepted; waiting for appliance state confirmation.'
                : receipt.status === 'confirmed'
                  ? 'Confirmed in canonical appliance state.'
                  : 'Canonical state diverged; review before retrying.'}
            </span>
          </div>
          <code>{receipt.requestId}</code>
          <RawPayloadToggle payload={receipt} />
        </div>
      ) : null}

      <div className="nodeLightActions">
        <button
          className="consoleButton small"
          type="button"
          disabled={!dirty || busy || pending}
          onClick={adoptLatestCanonicalState}
        >
          <Undo2 size={13} />
          <span>Discard draft</span>
        </button>
        <button
          className="consoleButton small"
          type="button"
          disabled={
            Object.keys(baselineLocalProfileOverride).length === 0 ||
            busy ||
            stale ||
            pending
          }
          onClick={() => void reviewProfileReset()}
        >
          <RotateCcw size={13} />
          <span>Use automatic for {profile.name}</span>
        </button>
        <button
          className="consoleButton small danger"
          type="button"
          disabled={
            Object.keys(inferredLocalOverrides).length === 0 ||
            busy ||
            stale ||
            pending
          }
          onClick={() => void reviewAllReset()}
        >
          <RotateCcw size={13} />
          <span>Use automatic for all</span>
        </button>
        <button
          className="consoleButton small primary"
          type="button"
          disabled={!dirty || busy || stale || pending}
          onClick={() => void reviewSave()}
        >
          <Save size={13} />
          <span>{busy ? 'Applying…' : 'Save & apply'}</span>
        </button>
      </div>
    </div>
  );
}

function proxyReceiptSummary(
  proxy: DeviceAdminProxyResponse
): SettingsWriteReceipt['proxy'] {
  return {
    route: proxy.route,
    completedAt: proxy.completedAt,
    statusCode: proxy.statusCode,
    verifiedServerInstanceId: proxy.verifiedServerInstanceId,
    preconditionBodySha256: proxy.preconditionBodySha256,
    responseBodySha256: proxy.bodySha256
  };
}
