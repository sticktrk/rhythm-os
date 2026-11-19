import { useState } from 'react';
import { SectionCard } from '../../components/ui/SectionCard';
import { asRecord, asString } from '../../device/values';
import { argumentTypes, assistantArguments } from '../../device/assistantInputs';
import { errorMessage } from '../../lib/format';

type Obj = Record<string, unknown>;

export function AssistantTools({operations, nodes, identity, canWrite, call}: {
  operations: Obj[];
  nodes: Obj[];
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
        Object.assign(args, assistantArguments(asRecord(op.input_schema), fields, () => crypto.randomUUID()));
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
        {operations.filter(o => o.tier === 'read' || (canWrite && o.tier === 'temporary_light_output')).map(o =>
          <option key={String(o.id)} value={String(o.id)}>{String(o.id).replaceAll('_',' ')}</option>)}
      </select></label>
      {op && <p>{String(op.description)}</p>}
      {Object.entries(props).map(([key, def]) => {
        if (key === 'correlation_id') return null;
        const schema = asRecord(def);
        const types = argumentTypes(schema);
        const choices = Array.isArray(schema.enum) ? schema.enum : types.length === 1 && types[0] === 'boolean' ? [true, false] : [];
        const options = ['room_id', 'from_room_id', 'to_room_id'].includes(key) ? nodes.filter(n => n.kind === 'room') :
          key === 'sensor_id' ? nodes.filter(n => n.kind === 'motion_sensor') :
          ['device_id','light_id','node_id','source_node_id','target_id'].includes(key) ? nodes : [];
        const set = (value: string) => { setFields({...fields,[key]:value}); setReceipt(null); };
        return <label key={key}>{key.replaceAll('_',' ')}
          {choices.length ? <select value={fields[key] ?? ''} disabled={busy} onChange={e=>set(e.target.value)}>
            <option value="">Choose a value</option>
            {choices.map(value=><option key={String(value)} value={String(value)}>{String(value)}</option>)}
          </select> : types.includes('object') || types.includes('array') ?
            <textarea value={fields[key] ?? ''} disabled={busy} onChange={e=>set(e.target.value)} placeholder={types.join(' or ')}/>
            : <input value={fields[key] ?? ''} disabled={busy} list={options.length ? `assistant-${key}` : undefined}
              onChange={e=>set(e.target.value)} placeholder={types.join(' or ')}/>}
          {options.length > 0 && <datalist id={`assistant-${key}`}>{options.map(n=><option key={String(n.id)} value={String(n.id)}>{String(n.name ?? n.id)}</option>)}</datalist>}
          {asString(schema.description)}
        </label>;
      })}
      {op && <button className="consoleButton" disabled={busy || !identity || (op.tier !== 'read' && !canWrite)}
        onClick={() => void run(false)}>{op.tier === 'read' ? 'Inspect or preview' : 'Run on this hub'}</button>}
    </div>
    {receipt && <div className="deviceAdminResult"><pre>{JSON.stringify(receipt, null, 2)}</pre></div>}
    {Boolean(mayConfirm) && <div><button className="consoleButton" disabled={busy}
      onClick={() => void run(true)}>Confirm reviewed change</button>
      <button className="consoleButton" disabled={busy} onClick={() => setReceipt(null)}>Cancel</button></div>}
  </SectionCard>;
}
