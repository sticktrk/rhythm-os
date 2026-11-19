import { useState } from 'react';
import { SectionCard } from '../../components/ui/SectionCard';
import { asArray, asRecord, asString } from '../../device/values';
import { errorMessage } from '../../lib/format';

type Obj = Record<string, unknown>;

export function AssistantTools({operations, identity, canWrite, call}: {
  operations: Obj[];
  identity: string;
  canWrite: boolean;
  call: (id: string, args: Obj) => Promise<Obj>;
}) {
  const [id, setId] = useState('');
  const [fields, setFields] = useState<Record<string, string>>({});
  const [receipt, setReceipt] = useState<Obj | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const op = operations.find(o => o.id === id);
  const props = asRecord(asRecord(op?.input_schema).properties);
  const apply = operations.find(o => receipt?.operation === o.id && o.tier === 'persistent_change');
  const mayConfirm = canWrite && op?.tier === 'read' && apply &&
    receipt?.server_instance_id === identity && receipt.apply_arguments &&
    typeof receipt.apply_arguments === 'object';

  async function run(confirm: boolean) {
    if (busy || !op || !identity || (op.tier !== 'read' && !canWrite)) return;
    let tool = id;
    const args: Obj = {};
    setError('');
    try {
      if (confirm) {
        if (!mayConfirm || !apply) return;
        tool = String(apply.id);
        Object.assign(args, receipt?.apply_arguments, {confirmed: true});
      } else {
        const required = asArray(asRecord(op.input_schema).required);
        for (const [key, definition] of Object.entries(props)) {
          const schema = asRecord(definition);
          const input = fields[key]?.trim() ?? '';
          if (!input && required.includes(key)) throw new Error(`Enter ${key.replaceAll('_',' ')}.`);
          if (!input) continue;
          args[key] = ['integer','number','array','object','boolean'].includes(String(schema.type))
            ? JSON.parse(input) : input;
        }
      }
      setBusy(true); setReceipt(null);
      setReceipt(await call(tool, args));
    } catch (err) {
      setError(errorMessage(err) + (confirm ? ' Read the current state and preview again before retrying.' : ''));
    } finally { setBusy(false); }
  }

  return <SectionCard title="Hub tools" error={error} busy={busy}>
    <div className="deviceAdminGrid">
      <label>Tool<select value={id} disabled={busy || !identity} onChange={e => {
        setId(e.target.value); setFields({}); setReceipt(null); setError('');
      }}>
        <option value="">Choose a tool</option>
        {operations.filter(o => o.tier === 'read' || o.tier === 'temporary_light_output').map(o =>
          <option key={String(o.id)} value={String(o.id)}>{String(o.id).replaceAll('_',' ')}</option>)}
      </select></label>
      {op && <p>{String(op.description)}</p>}
      {Object.entries(props).map(([key, def]) => <label key={key}>{key.replaceAll('_',' ')}
        <input value={fields[key] ?? ''} disabled={busy}
          placeholder={asArray(asRecord(def).enum).join(' / ') || String(asRecord(def).type ?? '')}
          onChange={e => { setFields({...fields,[key]: e.target.value}); setReceipt(null); }}/>
        {asString(asRecord(def).description)}
      </label>)}
      {op && <button className="consoleButton" disabled={busy || !identity || (op.tier !== 'read' && !canWrite)}
        onClick={() => void run(false)}>{op.tier === 'read' ? 'Inspect or preview' : 'Run on this hub'}</button>}
    </div>
    {receipt && <div className="deviceAdminResult"><pre>{JSON.stringify(receipt, null, 2)}</pre></div>}
    {Boolean(mayConfirm) && <div><button className="consoleButton" disabled={busy}
      onClick={() => void run(true)}>Confirm reviewed change</button>
      <button className="consoleButton" disabled={busy} onClick={() => setReceipt(null)}>Cancel</button></div>}
  </SectionCard>;
}
