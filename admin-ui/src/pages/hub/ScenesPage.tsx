import { useCallback, useState } from 'react';
import {
  Check,
  Eye,
  Play,
  Plus,
  RefreshCw,
  Save,
  Trash2,
  X
} from 'lucide-react';

import '../../styles/pages-phase5.css';

import { JsonEditor } from '../../components/controls/JsonEditor';
import { NumberField, SelectField, TextField } from '../../components/controls/fields';
import { EmptyState, ErrorNotice } from '../../components/ui/bits';
import { useConfirm } from '../../components/ui/ConfirmDialog';
import { RawPayloadToggle, SectionCard } from '../../components/ui/SectionCard';
import {
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
  asArray,
  asRecord,
  asRecordArray,
  asString,
  pick
} from '../../device/values';
import { useDeviceCall } from '../../hooks/useDeviceCall';
import { useDeviceClient } from '../../hooks/useDeviceClient';
import { usePolling } from '../../hooks/usePolling';
import { prettyJson } from '../../lib/json';
import { OutputEditor } from './scenes/OutputEditor';
import {
  colorSummaryCss,
  nodeOptionsFromState,
  outputContainer
} from './scenes/sceneShape';

const NEW_SCENE: Record<string, unknown> = {
  name: 'New scene',
  default_output: { power: true, brightness: 80, color: { kelvin: 3000 } },
  entries: []
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
  const [targetId, setTargetId] = useState('');
  const [transitionMs, setTransitionMs] = useState<number | undefined>(undefined);
  const [previewId, setPreviewId] = useState<string | null>(null);

  const mutate = useDeviceCall(
    useCallback(async (action: () => Promise<unknown>) => action(), [])
  );

  const scenes = scenesFromPayload(scenesQuery.data);
  const nodeOptions = nodeOptionsFromState(nodesQuery.data);

  function openScene(scene: Record<string, unknown>, id: string | null) {
    const clone = JSON.parse(JSON.stringify(scene)) as Record<string, unknown>;
    setSelectedId(id);
    setIsNew(id === null);
    setDraft(clone);
    setDraftText(prettyJson(clone));
    mutate.reset();
  }

  function updateDraft(next: Record<string, unknown>) {
    setDraft(next);
    setDraftText(prettyJson(next));
  }

  async function handleSave() {
    if (!draft) return;
    await mutate.run(async () => {
      if (isNew || !selectedId) {
        await createScene(client, draft);
        setIsNew(false);
        setDraft(null);
      } else {
        await updateScene(client, selectedId, draft);
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

  async function handleApply() {
    if (!selectedId || !targetId) return;
    await mutate.run(() =>
      applyScene(client, selectedId, { targetId, transitionMs })
    );
  }

  async function handlePreviewDraft() {
    if (!draft || !targetId) return;
    await mutate.run(async () => {
      const result = await previewDraftScene(client, {
        scene: draft,
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

  const entries = draft ? asRecordArray(draft.entries) : [];
  const defaultOutput = draft ? asRecord(draft.default_output) : {};
  const palette = draft ? asArray(draft.palette) : [];

  return (
    <div className="consolePage">
      <header className="pageHeader">
        <div>
          <div className="eyebrow">Device console</div>
          <h2>Scenes</h2>
          <p className="pageIntro">
            Saved lighting presets. Preview drafts live on a target before
            committing.
          </p>
        </div>
        <div className="pageHeaderActions">
          <button
            className="consoleButton"
            type="button"
            onClick={() => void scenesQuery.refresh()}
          >
            <RefreshCw size={15} />
            <span>Refresh</span>
          </button>
          <button
            className="consoleButton primary"
            type="button"
            onClick={() => openScene(NEW_SCENE, null)}
          >
            <Plus size={15} />
            <span>New scene</span>
          </button>
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
                return (
                  <button
                    key={id ?? sceneName(scene)}
                    type="button"
                    className={`p5SceneItem${id !== null && id === selectedId ? ' selected' : ''}`}
                    onClick={() => id !== null && openScene(scene, id)}
                  >
                    <span
                      className="p5Swatch"
                      style={{ background: colorSummaryCss(scene) }}
                    />
                    <span className="p5SceneName">{sceneName(scene) || id}</span>
                    <span className="p5SceneMeta">
                      {asRecordArray(scene.entries).length} entries
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
                  disabled={mutate.busy}
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

            <h4 className="p5SubHeading">Default output</h4>
            <OutputEditor
              output={defaultOutput}
              onChange={(next) => updateDraft({ ...draft, default_output: next })}
            />

            {palette.length > 0 ? (
              <>
                <h4 className="p5SubHeading">Palette</h4>
                <div className="p5Palette">
                  {palette.map((item, index) => (
                    <span
                      key={index}
                      className="p5Swatch large"
                      style={{
                        background: colorSummaryCss({
                          default_output: { color: item }
                        })
                      }}
                      title={prettyJson(item)}
                    />
                  ))}
                </div>
              </>
            ) : null}

            <h4 className="p5SubHeading">Per-target entries</h4>
            {entries.length === 0 ? (
              <p className="cardNote">
                No per-target entries — the default output applies everywhere.
              </p>
            ) : null}
            {entries.map((entry, index) => {
              const container = outputContainer(entry);
              return (
                <div className="p5Entry" key={index}>
                  <div className="p5EntryHeader">
                    <TextField
                      value={
                        asString(entry.target_id) ??
                        asString(entry.node_id) ??
                        asString(entry.id) ??
                        ''
                      }
                      placeholder="target id"
                      mono
                      onChange={(value) => {
                        const next = entries.map((item, i) =>
                          i === index ? { ...item, target_id: value } : item
                        );
                        updateDraft({ ...draft, entries: next });
                      }}
                    />
                    <button
                      className="iconOnlyButton"
                      type="button"
                      aria-label="Remove entry"
                      onClick={() =>
                        updateDraft({
                          ...draft,
                          entries: entries.filter((_, i) => i !== index)
                        })
                      }
                    >
                      <Trash2 size={14} />
                    </button>
                  </div>
                  <OutputEditor
                    output={container.output}
                    onChange={(next) => {
                      const nextEntries = entries.map((item, i) =>
                        i === index ? container.write(item, next) : item
                      );
                      updateDraft({ ...draft, entries: nextEntries });
                    }}
                  />
                </div>
              );
            })}
            <button
              className="consoleButton small"
              type="button"
              onClick={() =>
                updateDraft({
                  ...draft,
                  entries: [
                    ...entries,
                    {
                      target_id: nodeOptions[0]?.value ?? '',
                      power: true,
                      brightness: 80,
                      color: { kelvin: 3000 }
                    }
                  ]
                })
              }
            >
              <Plus size={14} />
              <span>Add entry</span>
            </button>

            <h4 className="p5SubHeading">Apply / preview target</h4>
            <div className="p5ApplyRow">
              <SelectField
                value={targetId}
                placeholder="Pick a node…"
                options={nodeOptions}
                onChange={setTargetId}
              />
              <NumberField
                value={transitionMs}
                min={0}
                placeholder="transition ms"
                onChange={setTransitionMs}
              />
              <button
                className="consoleButton"
                type="button"
                disabled={mutate.busy || !targetId || isNew || !selectedId}
                onClick={() => void handleApply()}
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
                <span>Preview draft</span>
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
            <EmptyState message="Select a scene or create a new one." />
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
