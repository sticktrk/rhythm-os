import { StaffHubDiagnostics } from './StaffHubDiagnostics';
import { AssistantTools } from './AssistantTools';
import { useEffect, useState } from 'react';
import { SectionCard } from '../../components/ui/SectionCard';
import { asArray, asRecord, asRecordArray, asString } from '../../device/values';
import { useDeviceClient } from '../../hooks/useDeviceClient';
import { walkReports } from '../../device/assistantInputs';
import { isNotImplemented } from '../../device/client';
import { errorMessage } from '../../lib/format';
import { useHub } from '../../state/HubContext';
import { useSnapshot } from '../../state/SnapshotContext';

type Obj = Record<string, unknown>;
const prefix = 'api/assistant/operations/';
const walk = 'start_motion_walk', inspect = 'inspect_motion_walk';
const preview = 'plan_motion_target', apply = 'apply_motion_target';

export default function AssistantPage() {
  const client = useDeviceClient();
  const { hub } = useHub();
  const canWrite = useSnapshot().me?.staffStatus.isAdmin === true;
  const [operations, setOperations] = useState<Obj[]>([]);
  const [nodes, setNodes] = useState<Obj[]>([]);
  const [identity, setIdentity] = useState('');
  const [room, setRoom] = useState('');
  const [sensor, setSensor] = useState('');
  const [walkId, setWalkId] = useState('');
  const [reports, setReports] = useState<Record<string, boolean>>({});
  const [review, setReview] = useState<Obj | null>(null);
  const [observations, setObservations] = useState<Obj | null>(null);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(true);
  const [refresh, setRefresh] = useState(0);
  const [needsRefresh, setNeedsRefresh] = useState(false);

  useEffect(() => {
    let active = true;
    setBusy(true);
    setReview(null);
    setWalkId('');
    setReports({});
    setObservations(null);
    setIdentity('');
    setError('');
    setMessage('');
    setOperations([]);
    setNodes([]);
    (async () => {
      const contract = await client.get<Obj>('api/assistant/contract');
      if (!active) return;
      const ops = asRecordArray(contract.operations);
      setOperations(ops);
      if (!ops.some(op => op.id === walk && op.tier === 'read')) {
        setMessage('This hub does not advertise motion walk diagnostics.');
        if (!ops.some(op => op.tier === 'read')) return;
      }
      const snapshot = await client.get<Obj>('api/assistant/topology');
      if (!active) return;
      const instance = asString(snapshot.server_instance_id);
      if (!instance || (hub.serverInstanceId && hub.serverInstanceId !== instance)) {
        throw new Error('The connected server does not match the selected hub.');
      }
      setNodes(asRecordArray(snapshot.nodes));
      setIdentity(instance);
      setNeedsRefresh(false);
      if (ops.some(op => op.id === walk)) setMessage('Select the motion sensor and the room it should control.');
    })().catch(err => {
      if (!active) return;
      if (isNotImplemented(err) || /returned HTTP 404/.test(errorMessage(err))) {
        setMessage('This firmware cannot run the assistant diagnostics. Other hub controls are available in the console.');
      } else setError(errorMessage(err));
    })
      .finally(() => { if (active) setBusy(false); });
    return () => { active = false; };
  }, [client, hub.serverInstanceId, refresh]);

  const has = (id: string) => operations.some(op => op.id === id);
  const name = (id: unknown) => asString(nodes.find(n => n.id === id)?.name) ?? String(id ?? 'Unassigned');
  const describeTargets = (ids: unknown) => asArray(ids).map(name).join(', ') || 'None';
  const disabled = busy || !identity || !room || needsRefresh;
  function change(callback: () => void) {
    setReview(null);
    setWalkId('');
    setReports({});
    setObservations(null);
    setMessage('');
    callback();
  }
  async function invoke(id: string, args: Obj) {
    const op = operations.find(o => o.id === id);
    if (!op || op.location !== 'hub' || op.invocation_path !== '/' + prefix + id ||
        !['read','temporary_light_output','persistent_change'].includes(String(op.tier))) throw new Error('Operation unavailable on this hub.');
    const result = await client.post<Obj>(prefix + id, {
      body: args,
      expectedServerInstanceId: identity,
    });
    if (result.server_instance_id && result.server_instance_id !== identity) {
      throw new Error('The result belongs to a different hub. Refresh the connection.');
    }
    return result;
  }

  async function run(id: string, observation?: { device_id: string; power_on: boolean }) {
    if (disabled) return;
    setBusy(true); setError('');
    try {
      const target = { sensor_id: sensor, room_id: room };
      if (id === walk) {
        const result = await invoke(walk, target);
        setReports({});
        setWalkId(asString(result.walk_id) ?? '');
        setReview(null); setObservations(null);
        if (result.status === 'handoff') {
          const handoff = asRecord(result.handoff);
          setMessage(`${handoff.actor} · ${handoff.surface} — ${handoff.instruction}`);
        } else {
          setMessage(`Ask the resident to walk past ${name(sensor)} within ${result.window_seconds} seconds, then check the walk.`);
        }
      } else if (id === inspect) {
        const next = walkReports(reports, observation);
        const result = await invoke(id, {walk_id: walkId, resident_observations: next.observations});
        setReports(next.reports);
        setObservations(result);
      } else if (id === preview) {
        const result = await invoke(id, {...target, correlation_id: crypto.randomUUID()});
        if (result.status === 'handoff') {
          const handoff = asRecord(result.handoff);
          setReview(null); setMessage(`${handoff.actor} · ${handoff.surface} — ${handoff.instruction}`);
        } else if (result.status === 'already_linked') {
          setMessage('The hub already routes this sensor to the selected room. Start a fresh walk.');
          setReview(null);
        } else if (result.status === 'ready' && result.server_instance_id === identity &&
                   result.sensor_id === sensor && result.room_id === room && asString(result.plan_id)) {
          setReview(result); setMessage('');
        } else {
          throw new Error('The hub did not return a review for the selected sensor and room.');
        }
      } else if (id === apply && canWrite && review) {
        setReview(null);
        const result = await invoke(id, {
          plan_id: review.plan_id, server_instance_id: identity, confirmed: true
        });
        if (result.canonical_readback_verified !== true) throw new Error('The hub could not verify the link.');
        setMessage('The hub verified the link. Start a new walk and check the readback for each light.');
        setWalkId(''); setObservations(null);
      }
    } catch (err) {
      setReview(null);
      setError(errorMessage(err));
      if (id === apply) {
        setNeedsRefresh(true);
        setMessage('Refresh the hub and review its current targets before another repair.');
      }
    } finally { setBusy(false); }
  }

  return <div className="consolePage">
    <header className="pageHeader"><h2>Lighting assistant</h2></header>
    <StaffHubDiagnostics key={hub.id} hubId={hub.id}/>
    <SectionCard title="Motion walk test" subtitle={hub.name} busy={busy} error={error}
      actions={<button className="consoleButton" disabled={busy} onClick={() => setRefresh(v => v + 1)}>Refresh hub</button>}>
      {identity && has(walk) && <div className="deviceAdminGrid">
        <label>Sensor<select value={sensor} disabled={busy} onChange={e => change(() => setSensor(e.target.value))}>
          <option value="">Sensor missing from discovery</option>
          {nodes.filter(n => n.kind === 'motion_sensor').map(n => <option key={String(n.id)} value={String(n.id)}>{name(n.id)}</option>)}
        </select></label>
        <label>Intended room<select value={room} disabled={busy} onChange={e => change(() => setRoom(e.target.value))}>
          <option value="">Choose a room</option>
          {nodes.filter(n => n.kind === 'room').map(n => <option key={String(n.id)} value={String(n.id)}>{name(n.id)}</option>)}
        </select></label>
        <div>
          <button className="consoleButton" disabled={disabled || !has(walk)} onClick={() => void run(walk)}>Start walk</button>
          <button className="consoleButton" disabled={disabled || !walkId || !has(inspect)} onClick={() => void run(inspect)}>Check walk</button>
          <button className="consoleButton" disabled={disabled || !sensor || !has(preview)} onClick={() => void run(preview)}>Review room link</button>
        </div>
      </div>}
      <p role="status">{message}</p>
      {observations && <div>
        <p>{String(observations.stage ?? '').replaceAll('_', ' ')} · {String(observations.reason ?? '').replaceAll('_', ' ')}</p>
        <p>Sensor · {name(observations.sensor_id)} · {asArray(observations.integrations).join(', ')}<br/>
        Physical room · {name(observations.physical_room_id)}<br/>
        Control targets · {describeTargets(observations.control_targets)}<br/>
        {typeof observations.detected_at_epoch_ms === 'number' && <>Detected · {new Date(observations.detected_at_epoch_ms).toLocaleTimeString()}<br/></>}
        Routed to · {describeTargets(observations.routed_targets)}</p>
        <p>{String(observations.next_check ?? '')}</p>
        <ul>{asRecordArray(observations.lights).map(light =>
          <li key={String(light.device_id)}>{name(light.device_id)} · {String(light.availability)} · {
            light.readback === 'fresh_device_readback' && light.availability === 'available' && typeof light.power_on === 'boolean'
            ? `reported ${light.power_on ? 'on' : 'off'}` : 'no fresh device readback'}
            {typeof light.resident_power_on === 'boolean' && ` · resident sees ${light.resident_power_on ? 'on' : 'off'}`}
            {['command_attempted','command_acknowledged','command_not_needed','command_uncertain'].includes(String(observations.stage)) &&
              [true,false].map(power => <button key={String(power)} className="consoleButton"
                disabled={disabled || light.resident_power_on === power}
                onClick={() => void run(inspect,{device_id:String(light.device_id),power_on:power})}>
                Resident sees {power ? 'on' : 'off'}
              </button>)}</li>)}</ul>
      </div>}
      {review && <div>
        <h3>Review room link</h3>
        <p>Sensor · {name(review.sensor_id)}<br/>Physical room · {name(review.physical_room_id)}<br/>
        Current targets · {describeTargets(review.before_targets)}<br/>Proposed targets · {describeTargets(review.after_targets)}</p>
        {canWrite ? <button disabled={disabled || !has(apply)} className="consoleButton" onClick={() => void run(apply)}>Confirm room link</button>
            : <p>An administrator can apply this repair.</p>}
        <button className="consoleButton" disabled={busy} onClick={() => setReview(null)}>Cancel</button>
      </div>}
    </SectionCard>
    {identity && <AssistantTools key={identity + refresh} operations={operations} nodes={nodes}
      identity={identity} canWrite={canWrite} call={invoke}/>}
  </div>;
}
