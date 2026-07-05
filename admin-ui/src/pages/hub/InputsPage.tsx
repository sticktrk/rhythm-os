import { useCallback, useState } from 'react';
import { Pencil, Plus, RefreshCw, Save, Trash2 } from 'lucide-react';

import '../../styles/pages-phase5.css';

import { JsonEditor } from '../../components/controls/JsonEditor';
import { SelectField, TextField } from '../../components/controls/fields';
import { EmptyState, ErrorNotice } from '../../components/ui/bits';
import { useConfirm } from '../../components/ui/ConfirmDialog';
import { Modal } from '../../components/ui/Modal';
import { SectionCard } from '../../components/ui/SectionCard';
import {
  createInputBinding,
  deleteInputBinding,
  listInputBindings,
  updateInputBinding
} from '../../device/inputs';
import { getNodesState } from '../../device/state';
import { asRecord, asRecordArray, asString } from '../../device/values';
import { useDeviceCall } from '../../hooks/useDeviceCall';
import { useDeviceClient } from '../../hooks/useDeviceClient';
import { usePolling } from '../../hooks/usePolling';
import { prettyJson } from '../../lib/json';
import { nodeOptionsFromState } from './scenes/sceneShape';

const NEW_BINDING: Record<string, unknown> = {
  action: 'toggle',
  target_id: ''
};

export default function InputsPage() {
  const client = useDeviceClient();
  const confirm = useConfirm();

  const bindingsQuery = usePolling(
    useCallback(() => listInputBindings(client), [client])
  );
  const nodesQuery = usePolling(
    useCallback(() => getNodesState(client), [client])
  );

  const mutate = useDeviceCall(
    useCallback(async (action: () => Promise<unknown>) => action(), [])
  );

  const bindings = bindingsFromPayload(bindingsQuery.data);
  const nodeOptions = nodeOptionsFromState(nodesQuery.data);

  const [editor, setEditor] = useState<{
    id: string | null;
    record: Record<string, unknown>;
    text: string;
  } | null>(null);

  function openEditor(record: Record<string, unknown>, id: string | null) {
    const clone = JSON.parse(JSON.stringify(record)) as Record<string, unknown>;
    setEditor({ id, record: clone, text: prettyJson(clone) });
  }

  function patchEditor(patch: Record<string, unknown>) {
    setEditor((current) => {
      if (!current) return current;
      const record = { ...current.record, ...patch };
      return { ...current, record, text: prettyJson(record) };
    });
  }

  async function saveBinding() {
    if (!editor) return;
    // The JSON text is authoritative — reject saves while it is invalid.
    let payload: Record<string, unknown>;
    try {
      const parsed = JSON.parse(editor.text) as unknown;
      if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
        throw new Error('Binding JSON must be an object.');
      }
      payload = parsed as Record<string, unknown>;
    } catch {
      return;
    }
    await mutate.run(async () => {
      if (editor.id) await updateInputBinding(client, editor.id, payload);
      else await createInputBinding(client, payload);
      setEditor(null);
      await bindingsQuery.refresh();
    });
  }

  async function removeBinding(binding: Record<string, unknown>) {
    const id = bindingId(binding);
    if (!id) return;
    const ok = await confirm({
      title: 'Delete binding',
      message: `Delete binding ${id}? The physical input will stop doing anything until rebound.`,
      confirmLabel: 'Delete',
      danger: true
    });
    if (!ok) return;
    await mutate.run(async () => {
      await deleteInputBinding(client, id);
      await bindingsQuery.refresh();
    });
  }

  return (
    <div className="consolePage">
      <header className="pageHeader">
        <div>
          <div className="eyebrow">Device console</div>
          <h2>Inputs &amp; Controls</h2>
          <p className="pageIntro">
            Bindings from buttons and switches to lighting actions.
          </p>
        </div>
        <div className="pageHeaderActions">
          <button
            className="consoleButton"
            type="button"
            onClick={() => void bindingsQuery.refresh()}
          >
            <RefreshCw size={15} />
            <span>Refresh</span>
          </button>
          <button
            className="consoleButton primary"
            type="button"
            onClick={() => openEditor(NEW_BINDING, null)}
          >
            <Plus size={15} />
            <span>New binding</span>
          </button>
        </div>
      </header>

      {mutate.error ? <ErrorNotice message={mutate.error} /> : null}

      <SectionCard
        title="Bindings"
        subtitle={`${bindings.length} input binding${bindings.length === 1 ? '' : 's'}`}
        busy={bindingsQuery.refreshing || mutate.busy}
        error={bindingsQuery.error}
        rawPayload={bindingsQuery.data ?? undefined}
      >
        {bindings.length === 0 && !bindingsQuery.loading ? (
          <EmptyState message="No input bindings." />
        ) : (
          <div className="p5TableWrap">
            <table className="p5Table">
              <thead>
                <tr>
                  <th>Id</th>
                  <th>Action</th>
                  <th>Input</th>
                  <th>Target</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {bindings.map((binding, index) => (
                  <tr key={bindingId(binding) ?? index}>
                    <td className="mono">{bindingId(binding) ?? '—'}</td>
                    <td>{asString(binding.action) ?? '—'}</td>
                    <td className="mono">
                      {asString(binding.input_id) ??
                        asString(binding.source) ??
                        asString(binding.device_id) ??
                        '—'}
                    </td>
                    <td className="mono">
                      {asString(binding.target_id) ??
                        asString(binding.target) ??
                        '—'}
                    </td>
                    <td>
                      <div className="p5RowActions">
                        <button
                          className="iconOnlyButton"
                          type="button"
                          aria-label="Edit binding"
                          onClick={() => openEditor(binding, bindingId(binding))}
                        >
                          <Pencil size={14} />
                        </button>
                        <button
                          className="iconOnlyButton"
                          type="button"
                          aria-label="Delete binding"
                          disabled={!bindingId(binding)}
                          onClick={() => void removeBinding(binding)}
                        >
                          <Trash2 size={14} />
                        </button>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </SectionCard>

      {editor ? (
        <Modal
          title={editor.id ? `Edit binding ${editor.id}` : 'New binding'}
          onClose={() => setEditor(null)}
          wide
        >
          <div className="formRow">
            <div className="formRowLabel">
              <span>Action</span>
              <small>e.g. toggle, on_press, off_press, reset</small>
            </div>
            <div className="formRowControl">
              <TextField
                value={asString(editor.record.action) ?? ''}
                mono
                onChange={(value) => patchEditor({ action: value })}
              />
            </div>
          </div>

          <div className="formRow">
            <div className="formRowLabel">
              <span>Target</span>
            </div>
            <div className="formRowControl">
              <SelectField
                value={asString(editor.record.target_id) ?? ''}
                placeholder="Pick a node…"
                options={nodeOptions}
                onChange={(value) => patchEditor({ target_id: value })}
              />
            </div>
          </div>

          <p className="cardNote">
            The JSON below is authoritative on save — structured fields above
            just patch it. Add input/source identifiers and action parameters
            here.
          </p>
          <JsonEditor
            value={editor.text}
            rows={12}
            onChange={(text, parsed) =>
              setEditor((current) => {
                if (!current) return current;
                if (
                  parsed !== undefined &&
                  typeof parsed === 'object' &&
                  parsed !== null &&
                  !Array.isArray(parsed)
                ) {
                  return {
                    ...current,
                    text,
                    record: parsed as Record<string, unknown>
                  };
                }
                return { ...current, text };
              })
            }
          />

          <div className="confirmActions">
            <button
              className="consoleButton"
              type="button"
              onClick={() => setEditor(null)}
            >
              Cancel
            </button>
            <button
              className="consoleButton primary"
              type="button"
              disabled={mutate.busy}
              onClick={() => void saveBinding()}
            >
              <Save size={15} />
              <span>{editor.id ? 'Save binding' : 'Create binding'}</span>
            </button>
          </div>
        </Modal>
      ) : null}
    </div>
  );
}

function bindingsFromPayload(payload: unknown): Record<string, unknown>[] {
  if (Array.isArray(payload)) return asRecordArray(payload);
  const record = asRecord(payload);
  const fromBindings = asRecordArray(record.bindings);
  if (fromBindings.length > 0) return fromBindings;
  return asRecordArray(record.input_bindings);
}

function bindingId(binding: Record<string, unknown>): string | null {
  return asString(binding.id) ?? asString(binding.binding_id) ?? null;
}
