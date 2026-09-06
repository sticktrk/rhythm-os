import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Check,
  ChevronDown,
  Eye,
  Home,
  Play,
  Plus,
  RefreshCw,
  Save,
  Trash2,
  X
} from 'lucide-react';

import '../../styles/pages-phase5.css';
import '../../styles/scene-studio.css';

import { JsonEditor } from '../../components/controls/JsonEditor';
import {
  NumberField,
  SelectField,
  TextField
} from '../../components/controls/fields';
import { SegmentedControl } from '../../components/controls/SegmentedControl';
import { EmptyState, ErrorNotice } from '../../components/ui/bits';
import { useConfirm } from '../../components/ui/ConfirmDialog';
import { RawPayloadToggle, SectionCard } from '../../components/ui/SectionCard';
import {
  applyHomeScene,
  applyScene,
  cancelScenePreview,
  commitScenePreview,
  createScene,
  deleteScene,
  listScenes,
  previewDraftScene,
  updateScene
} from '../../device/scenes';
import { getNodesState } from '../../device/state';
import {
  asNumber,
  asRecord,
  asRecordArray,
  asString,
  pick
} from '../../device/values';
import { useDeviceCall } from '../../hooks/useDeviceCall';
import { useDeviceClient } from '../../hooks/useDeviceClient';
import { usePolling } from '../../hooks/usePolling';
import { prettyJson } from '../../lib/json';
import { HousePreview } from './scenes/HousePreview';
import { OutputEditor } from './scenes/OutputEditor';
import { PaletteEditor } from './scenes/PaletteEditor';
import { houseLightOrder, newPaletteSeed } from './scenes/scenePalette';
import {
  SCENE_TEMPLATES,
  entryNodeId,
  entryWithNodeId,
  lightLayer,
  lightNodeOptionsFromState,
  nodeOptionsFromState,
  outputContainer,
  swatchStripCss,
  withLightLayer
} from './scenes/sceneShape';

type HomeApplyOutcome = {
  sceneId: string;
  applied: number;
  attempted: number;
  skipped: number;
  lanes: string[];
  mode: string;
  queued: boolean;
};

export default function ScenesPage() {
  const client = useDeviceClient();
  const confirm = useConfirm();

  const scenesQuery = usePolling(
    useCallback(() => listScenes(client), [client])
  );
  const nodesQuery = usePolling(
    useCallback(() => getNodesState(client), [client])
  );

  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [isNew, setIsNew] = useState(false);
  const [draft, setDraft] = useState<Record<string, unknown> | null>(null);
  const [draftText, setDraftText] = useState('');
  const [templateMenuOpen, setTemplateMenuOpen] = useState(false);
  const templateMenuRef = useRef<HTMLDivElement | null>(null);

  const [targetId, setTargetId] = useState('');
  const [transitionMs, setTransitionMs] = useState<number | undefined>(undefined);
  const [previewId, setPreviewId] = useState<string | null>(null);

  const [homeMode, setHomeMode] = useState<'devices' | 'rooms'>('devices');
  const [dispatchSpacingMs, setDispatchSpacingMs] = useState<number | undefined>(120);
  const [homeOutcome, setHomeOutcome] = useState<HomeApplyOutcome | null>(null);

  const mutate = useDeviceCall(
    useCallback(async (action: () => Promise<unknown>) => action(), [])
  );

  const scenes = scenesFromPayload(scenesQuery.data);
  const nodeOptions = nodeOptionsFromState(nodesQuery.data);
  const lightOptions = lightNodeOptionsFromState(nodesQuery.data);
  const house = useMemo(
    () => (nodesQuery.data ? houseLightOrder(nodesQuery.data) : null),
    [nodesQuery.data]
  );

  useEffect(() => {
    if (!templateMenuOpen) return;
    function close(event: MouseEvent) {
      if (!templateMenuRef.current?.contains(event.target as Node)) {
        setTemplateMenuOpen(false);
      }
    }
    document.addEventListener('mousedown', close);
    return () => document.removeEventListener('mousedown', close);
  }, [templateMenuOpen]);

  function openScene(scene: Record<string, unknown>, id: string | null) {
    const clone = JSON.parse(JSON.stringify(scene)) as Record<string, unknown>;
    // Always edit the canonical shape; a legacy top-level layer is folded
    // into `light` so the server sees one definition on save.
    const canonical = withLightLayer(clone, lightLayer(clone));
    setSelectedId(id);
    setIsNew(id === null);
    setDraft(canonical);
    setDraftText(prettyJson(canonical));
    setHomeOutcome(null);
    setTemplateMenuOpen(false);
    mutate.reset();
  }

  function updateDraft(next: Record<string, unknown>) {
    setDraft(next);
    setDraftText(prettyJson(next));
  }

  function updateLayer(next: Record<string, unknown>) {
    if (!draft) return;
    updateDraft(withLightLayer(draft, next));
  }

  async function handleSave() {
    if (!draft) return;
    const name = sceneName(draft);
    const id = asString(draft.id)?.trim() || slugify(name);
    if (!id) return;
    const body = { ...draft, id, name };
    await mutate.run(async () => {
      if (isNew || !selectedId) {
        await createScene(client, body);
        setIsNew(false);
        setSelectedId(id);
        updateDraft(body);
      } else {
        await updateScene(client, selectedId, body);
      }
      await scenesQuery.refresh();
    });
  }

  async function handleDelete() {
    if (!selectedId) return;
    const ok = await confirm({
      title: 'Delete scene',
      message: `Delete scene ${sceneName(draft ?? {}) || selectedId}? Bindings or moods referencing it will stop working.`,
      confirmLabel: 'Delete',
      danger: true
    });
    if (!ok) return;
    await mutate.run(async () => {
      await deleteScene(client, selectedId);
      setSelectedId(null);
      setDraft(null);
      await scenesQuery.refresh();
    });
  }

  async function handleApplyRoom() {
    if (!selectedId || !targetId) return;
    await mutate.run(() =>
      applyScene(client, selectedId, { targetId, transitionMs })
    );
  }

  async function handleApplyHome() {
    if (!selectedId) return;
    setHomeOutcome(null);
    await mutate.run(async () => {
      const result = await applyHomeScene(client, selectedId, {
        targetMode: homeMode,
        dispatchSpacingMs,
        transitionMs
      });
      const targets = asRecordArray(pick(result, 'targets'));
      setHomeOutcome({
        sceneId: selectedId,
        applied:
          asNumber(pick(result, 'applied_target_count')) ??
          targets.filter((target) => !('error' in target)).length,
        attempted: targets.length,
        skipped: asNumber(pick(result, 'skipped_target_count')) ?? 0,
        lanes: asRecordArray(pick(result, 'dispatch_lanes')).map(
          (lane) =>
            `${asString(lane.hub) ?? 'hub'} ×${asNumber(lane.dispatch_count) ?? 0}`
        ),
        mode: asString(pick(result, 'target_mode')) ?? homeMode,
        queued: pick(result, 'queued') === true
      });
    });
  }

  async function handlePreviewDraft() {
    if (!draft || !targetId) return;
    await mutate.run(async () => {
      const result = await previewDraftScene(client, {
        scene: { ...draft, id: asString(draft.id) || slugify(sceneName(draft)) },
        targetId,
        transitionMs
      });
      const id =
        asString(pick(result, 'preview_id')) ??
        asString(pick(result, 'id')) ??
        asString(pick(result, 'preview', 'id'));
      setPreviewId(id ?? null);
    });
  }

  async function settlePreview(action: 'commit' | 'cancel') {
    if (!previewId) return;
    await mutate.run(async () => {
      if (action === 'commit') await commitScenePreview(client, previewId);
      else await cancelScenePreview(client, previewId);
      setPreviewId(null);
      await scenesQuery.refresh();
    });
  }

  const layer = draft ? lightLayer(draft) : {};
  const entries = asRecordArray(layer.entries);
  const defaultOutput = asRecord(layer.default_output);
  const hasDefaultOutput = Object.keys(defaultOutput).length > 0;
  const roomOptions = useMemo(
    () =>
      house
        ? house.rooms.map((room) => ({
            value: room.id,
            label: `${room.name} (${room.lights.length} lights)`
          }))
        : [],
    [house]
  );
  const targetOptions = roomOptions.length > 0 ? [...roomOptions, ...nodeOptions.filter((option) => !roomOptions.some((room) => room.value === option.value))] : nodeOptions;

  return (
    <div className="consolePage">
      <header className="pageHeader">
        <div>
          <div className="eyebrow">Device console</div>
          <h2>Scenes</h2>
          <p className="pageIntro">
            Compose palettes and per-light looks, see how they land on every
            bulb in the house, then apply to one room or the whole home.
          </p>
        </div>
        <div className="pageHeaderActions">
          <button
            className="consoleButton"
            type="button"
            onClick={() => {
              void scenesQuery.refresh();
              void nodesQuery.refresh();
            }}
          >
            <RefreshCw size={15} />
            <span>Refresh</span>
          </button>
          <div className="ssTemplateAnchor" ref={templateMenuRef}>
            <button
              className="consoleButton primary"
              type="button"
              onClick={() => setTemplateMenuOpen((open) => !open)}
            >
              <Plus size={15} />
              <span>New scene</span>
              <ChevronDown size={14} />
            </button>
            {templateMenuOpen ? (
              <div className="ssTemplateMenu" role="menu">
                {SCENE_TEMPLATES.map((template) => (
                  <button
                    key={template.id}
                    type="button"
                    role="menuitem"
                    className="ssTemplateItem"
                    onClick={() => openScene(template.scene, null)}
                  >
                    <strong>{template.label}</strong>
                    <small>{template.description}</small>
                  </button>
                ))}
              </div>
            ) : null}
          </div>
        </div>
      </header>

      {previewId ? (
        <div className="p5StickyBar">
          <span>
            Draft preview running on <code>{targetId}</code>
          </span>
          <div className="p5StickyActions">
            <button
              className="consoleButton primary"
              type="button"
              disabled={mutate.busy}
              onClick={() => void settlePreview('commit')}
            >
              <Check size={15} />
              <span>Commit</span>
            </button>
            <button
              className="consoleButton"
              type="button"
              disabled={mutate.busy}
              onClick={() => void settlePreview('cancel')}
            >
              <X size={15} />
              <span>Cancel preview</span>
            </button>
          </div>
        </div>
      ) : null}

      {mutate.error ? <ErrorNotice message={mutate.error} /> : null}

      <div className="p5Split">
        <SectionCard
          title="Scene list"
          subtitle={`${scenes.length} scene${scenes.length === 1 ? '' : 's'}`}
          busy={scenesQuery.refreshing}
          error={scenesQuery.error}
          rawPayload={scenesQuery.data ?? undefined}
        >
          {scenes.length === 0 && !scenesQuery.loading ? (
            <EmptyState message="No scenes on this device." />
          ) : (
            <div className="p5SceneList">
              {scenes.map((scene) => {
                const id = sceneId(scene);
                const swatches = swatchStripCss(scene);
                const anchorCount = asRecordArray(lightLayer(scene).palette).length;
                return (
                  <button
                    key={id ?? sceneName(scene)}
                    type="button"
                    className={`p5SceneItem${id !== null && id === selectedId ? ' selected' : ''}`}
                    onClick={() => id !== null && openScene(scene, id)}
                  >
                    <span className="ssSwatchStrip">
                      {swatches.map((css, index) => (
                        <span
                          key={index}
                          className="p5Swatch"
                          style={{ background: css }}
                        />
                      ))}
                    </span>
                    <span className="p5SceneName">{sceneName(scene) || id}</span>
                    <span className="p5SceneMeta">
                      {anchorCount > 0
                        ? `${anchorCount} anchor${anchorCount === 1 ? '' : 's'}`
                        : `${asRecordArray(lightLayer(scene).entries).length} entries`}
                    </span>
                  </button>
                );
              })}
            </div>
          )}
        </SectionCard>

        {draft ? (
          <SectionCard
            title={isNew ? 'New scene' : `Edit: ${sceneName(draft) || selectedId}`}
            busy={mutate.busy}
            actions={
              <>
                <button
                  className="consoleButton primary"
                  type="button"
                  disabled={mutate.busy || !sceneName(draft).trim()}
                  onClick={() => void handleSave()}
                >
                  <Save size={15} />
                  <span>Save</span>
                </button>
                {!isNew && selectedId ? (
                  <button
                    className="consoleButton danger"
                    type="button"
                    disabled={mutate.busy}
                    onClick={() => void handleDelete()}
                  >
                    <Trash2 size={15} />
                    <span>Delete</span>
                  </button>
                ) : null}
              </>
            }
          >
            <div className="formRow">
              <div className="formRowLabel">
                <span>Name</span>
              </div>
              <div className="formRowControl">
                <TextField
                  value={sceneName(draft)}
                  onChange={(value) => updateDraft({ ...draft, name: value })}
                />
              </div>
            </div>
            <div className="formRow">
              <div className="formRowLabel">
                <span>Id</span>
                <small>{isNew ? 'Derived from the name unless set' : 'Fixed once saved'}</small>
              </div>
              <div className="formRowControl">
                <TextField
                  value={asString(draft.id) ?? ''}
                  placeholder={slugify(sceneName(draft))}
                  mono
                  disabled={!isNew}
                  onChange={(value) => updateDraft({ ...draft, id: value })}
                />
              </div>
            </div>
            <div className="formRow">
              <div className="formRowLabel">
                <span>Description</span>
              </div>
              <div className="formRowControl">
                <TextField
                  value={asString(draft.description) ?? ''}
                  placeholder="What this scene feels like"
                  onChange={(value) =>
                    updateDraft({ ...draft, description: value || undefined })
                  }
                />
              </div>
            </div>
            <div className="formRow">
              <div className="formRowLabel">
                <span>Default transition</span>
                <small>ms, unless an output sets its own</small>
              </div>
              <div className="formRowControl">
                <NumberField
                  value={asNumber(layer.default_transition_ms)}
                  min={0}
                  placeholder="e.g. 1200"
                  onChange={(value) =>
                    updateLayer({ ...layer, default_transition_ms: value })
                  }
                />
              </div>
            </div>

            <h4 className="p5SubHeading">Palette</h4>
            <PaletteEditor
              key={selectedId ?? 'new-scene'}
              layer={layer}
              disabled={mutate.busy}
              onChange={updateLayer}
            />

            <h4 className="p5SubHeading">House preview</h4>
            <HousePreview
              scene={draft}
              house={house}
              loading={nodesQuery.loading}
              onRandomize={
                mutate.busy
                  ? undefined
                  : () =>
                      updateLayer({
                        ...layer,
                        palette_mode: 'shuffle',
                        palette_seed: newPaletteSeed()
                      })
              }
            />

            <h4 className="p5SubHeading">Default output</h4>
            {hasDefaultOutput ? (
              <>
                <p className="cardNote">
                  Used by lights the palette does not reach (an empty palette) and
                  by grouped rooms that take one command.
                </p>
                <OutputEditor
                  output={defaultOutput}
                  onChange={(next) => updateLayer({ ...layer, default_output: next })}
                />
                <button
                  className="consoleButton small"
                  type="button"
                  onClick={() => {
                    const next = { ...layer };
                    delete next.default_output;
                    updateLayer(next);
                  }}
                >
                  <Trash2 size={14} />
                  <span>Remove default output</span>
                </button>
              </>
            ) : (
              <button
                className="consoleButton small"
                type="button"
                onClick={() =>
                  updateLayer({
                    ...layer,
                    default_output: {
                      power: 'on',
                      brightness: 80,
                      color: { kind: 'kelvin', kelvin: 3000 }
                    }
                  })
                }
              >
                <Plus size={14} />
                <span>Add default output</span>
              </button>
            )}

            <h4 className="p5SubHeading">Pinned lights</h4>
            {entries.length === 0 ? (
              <p className="cardNote">
                No pinned lights. Pin one to give it its own look; it steps out
                of the palette rotation.
              </p>
            ) : null}
            {entries.map((entry, index) => {
              const container = outputContainer(entry);
              return (
                <div className="p5Entry" key={index}>
                  <div className="p5EntryHeader">
                    <SelectField
                      value={entryNodeId(entry)}
                      placeholder="Pick a light…"
                      options={lightOptions}
                      onChange={(value) => {
                        const next = entries.map((item, i) =>
                          i === index ? entryWithNodeId(item, value) : item
                        );
                        updateLayer({ ...layer, entries: next });
                      }}
                    />
                    <button
                      className="iconOnlyButton"
                      type="button"
                      aria-label="Remove entry"
                      onClick={() =>
                        updateLayer({
                          ...layer,
                          entries: entries.filter((_, i) => i !== index)
                        })
                      }
                    >
                      <Trash2 size={14} />
                    </button>
                  </div>
                  <OutputEditor
                    output={container.output}
                    compact
                    onChange={(next) => {
                      const nextEntries = entries.map((item, i) =>
                        i === index ? container.write(item, next) : item
                      );
                      updateLayer({ ...layer, entries: nextEntries });
                    }}
                  />
                </div>
              );
            })}
            <button
              className="consoleButton small"
              type="button"
              onClick={() =>
                updateLayer({
                  ...layer,
                  entries: [
                    ...entries,
                    {
                      target: { kind: 'node', node_id: lightOptions[0]?.value ?? '' },
                      output: {
                        power: 'on',
                        brightness: 80,
                        color: { kind: 'kelvin', kelvin: 3000 }
                      }
                    }
                  ]
                })
              }
            >
              <Plus size={14} />
              <span>Pin a light</span>
            </button>

            <h4 className="p5SubHeading">Apply to the whole home</h4>
            <div className="ssApply">
              <div className="ssApplyRow">
                <SegmentedControl
                  value={homeMode}
                  onChange={(value) => setHomeMode(value as 'devices' | 'rooms')}
                  options={[
                    { value: 'devices', label: 'Every light' },
                    { value: 'rooms', label: 'Room by room' }
                  ]}
                />
                <NumberField
                  value={dispatchSpacingMs}
                  min={0}
                  max={60000}
                  placeholder="spacing ms"
                  onChange={setDispatchSpacingMs}
                />
                <NumberField
                  value={transitionMs}
                  min={0}
                  placeholder="transition ms"
                  onChange={setTransitionMs}
                />
                <button
                  className="consoleButton primary"
                  type="button"
                  disabled={mutate.busy || isNew || !selectedId}
                  onClick={() => void handleApplyHome()}
                >
                  <Home size={15} />
                  <span>Apply to whole home</span>
                </button>
              </div>
              <p className="cardNote">
                {homeMode === 'devices'
                  ? 'Every light gets its own command and the palette runs through the house in the order shown above, one paced lane per hub.'
                  : 'Each room is planned as a unit: grouped rooms recall one projection and the palette rotates room by room.'}
                {isNew ? ' Save the scene first.' : ''}
              </p>
              {homeOutcome ? (
                <div className={`ssResult${homeOutcome.applied < homeOutcome.attempted ? ' warn' : ''}`}>
                  <Check size={15} />
                  <span>
                    Set {sceneName(draft)} on {homeOutcome.applied} of{' '}
                    {homeOutcome.attempted}{' '}
                    {homeOutcome.mode === 'devices' ? 'lights' : 'rooms'}
                    {homeOutcome.skipped > 0 ? `, ${homeOutcome.skipped} skipped` : ''}
                    {homeOutcome.queued ? ', pacing in the background' : ''}
                  </span>
                  {homeOutcome.lanes.length > 0 ? (
                    <span className="ssTag">{homeOutcome.lanes.join(' · ')}</span>
                  ) : null}
                </div>
              ) : null}
            </div>

            <h4 className="p5SubHeading">Apply or preview on one target</h4>
            <div className="p5ApplyRow">
              <SelectField
                value={targetId}
                placeholder="Pick a room or light…"
                options={targetOptions}
                onChange={setTargetId}
              />
              <button
                className="consoleButton"
                type="button"
                disabled={mutate.busy || !targetId || isNew || !selectedId}
                onClick={() => void handleApplyRoom()}
              >
                <Play size={15} />
                <span>Apply saved</span>
              </button>
              <button
                className="consoleButton"
                type="button"
                disabled={mutate.busy || !targetId || previewId !== null}
                onClick={() => void handlePreviewDraft()}
              >
                <Eye size={15} />
                <span>Preview draft live</span>
              </button>
            </div>

            <h4 className="p5SubHeading">Full scene JSON</h4>
            <JsonEditor
              value={draftText}
              rows={12}
              onChange={(text, parsed) => {
                setDraftText(text);
                if (
                  parsed !== undefined &&
                  typeof parsed === 'object' &&
                  parsed !== null &&
                  !Array.isArray(parsed)
                ) {
                  setDraft(parsed as Record<string, unknown>);
                }
              }}
            />
            <RawPayloadToggle payload={scenesQuery.data} />
          </SectionCard>
        ) : (
          <SectionCard title="Editor">
            <EmptyState message="Select a scene or start one from a template." />
          </SectionCard>
        )}
      </div>
    </div>
  );
}

function scenesFromPayload(payload: unknown): Record<string, unknown>[] {
  if (Array.isArray(payload)) return asRecordArray(payload);
  const record = asRecord(payload);
  return asRecordArray(record.scenes);
}

function sceneId(scene: Record<string, unknown>): string | null {
  return asString(scene.id) ?? asString(scene.scene_id) ?? null;
}

function sceneName(scene: Record<string, unknown>): string {
  return asString(scene.name) ?? asString(scene.label) ?? '';
}

/** The server derives an id from the name the same way; keep the two in step
    so the list and the editor agree before the first save. */
function slugify(name: string): string {
  return name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
}
