import { useCallback, useMemo, useState } from 'react';
import { Lightbulb, RefreshCw, RotateCcw } from 'lucide-react';

import { ColorWheel } from '../../components/controls/ColorWheel';
import { SegmentedControl } from '../../components/controls/SegmentedControl';
import { KelvinSlider, Slider } from '../../components/controls/Slider';
import { ToggleSwitch } from '../../components/controls/ToggleSwitch';
import { EmptyState, ErrorNotice } from '../../components/ui/bits';
import { SectionCard } from '../../components/ui/SectionCard';
import {
  sendNodeAction,
  setNodeBrightness,
  setNodeColor,
  setNodeCurveColorTemperature,
  setNodePreferences,
  setNodesOffset,
  type RgbColor
} from '../../device/nodes';
import { getNodesState } from '../../device/state';
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
import '../../styles/pages-phase4.css';

type NodeSummary = {
  id: string;
  name: string;
  kind?: string;
  room?: string;
  powerOn?: boolean;
  rhythmEnabled?: boolean;
  disabled?: boolean;
  brightness?: number;
  kelvin?: number;
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

  const nodes = useMemo(() => parseNodes(nodesQuery.data), [nodesQuery.data]);
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
            controls.
          </p>
        </div>
        <div className="pageHeaderActions">
          <button
            className="consoleButton"
            type="button"
            onClick={() => void nodesQuery.refresh()}
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
              onWrite={nodesQuery.refresh}
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
  onWrite
}: {
  node: NodeSummary;
  onWrite: () => Promise<void>;
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
