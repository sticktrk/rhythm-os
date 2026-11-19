import { useState } from 'react';
import { runStaffAssistantTool } from '../../api';
import { SectionCard } from '../../components/ui/SectionCard';
import { asRecordArray, asString } from '../../device/values';
import { errorMessage } from '../../lib/format';
import { useOptionalSession } from '../../state/SessionContext';

type Source = { ref: string; bytes: number | null };

export function StaffHubDiagnostics({ hubId }: { hubId: string }) {
  const session = useOptionalSession();
  const [hubRef, setHubRef] = useState('');
  const [sources, setSources] = useState<Source[]>([]);
  const [source, setSource] = useState('');
  const [diagnostics, setDiagnostics] = useState<string[]>([]);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  if (!session) return null;
  const { accessToken } = session;
  const call = (name: string, args: Record<string, unknown>) =>
    runStaffAssistantTool(accessToken, name, args);

  async function run(action: 'select' | 'status' | 'list' | 'read') {
    setBusy(true);
    setError('');
    setDiagnostics([]);
    setMessage('');
    try {
      if (action === 'select') {
        setHubRef('');
        setSources([]);
        setSource('');
        const matches = await call('staff_list_fleet', { hub_id: hubId });
        const ref = asString(asRecordArray(matches.hubs)[0]?.hub_ref);
        if (!ref) throw new Error('This hub is unavailable in the staff directory.');
        const probe = await call('staff_probe_hub', { hub_ref: ref });
        if (probe.status !== 'online') {
          setMessage('The hub is currently unreachable.');
          return;
        }
        const selected = await call('staff_select_hub', { hub_ref: ref });
        if (selected.selected !== true) throw new Error('The selected hub could not be verified.');
        setHubRef(ref);
        setMessage(`Online. The support grant allows ${asRecordArray(selected.tools).length} hub operations.`);
      } else if (action === 'status') {
        const status = await call('staff_read_hub_status', { hub_ref: hubRef });
        const version = asString(status.server_version);
        setMessage(version ? `Online. Server version ${version}.` : 'The hub is online.');
      } else if (action === 'list') {
        const response = await call('staff_list_logs', { hub_ref: hubRef });
        const available = asRecordArray(response.sources).flatMap(item => {
          const ref = asString(item.source_ref);
          return ref ? [{ ref, bytes: typeof item.bytes === 'number' ? item.bytes : null }] : [];
        });
        setSources(available);
        setSource(available[0]?.ref ?? '');
        setMessage(available.length ? 'Choose a log source to check recent diagnostics.' : 'The hub reported no log sources.');
      } else {
        const response = await call('staff_read_log', { hub_ref: hubRef, source_ref: source });
        setDiagnostics(asRecordArray(response.diagnostics).map(item =>
          `Line ${item.line ?? 'unknown'} · ${String(item.category).replaceAll(/_/g, ' ')}`));
        setMessage(`Checked ${response.lines_reviewed} recent log lines.`);
      }
    } catch (cause) {
      setHubRef('');
      setSources([]);
      setSource('');
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  return <SectionCard title="Support connection" busy={busy} error={error}>
    <button className="consoleButton" disabled={busy} onClick={() => void run('select')}>Check support access</button>
    {hubRef && <>
      <button className="consoleButton" disabled={busy} onClick={() => void run('status')}>Read hub status</button>
      <button className="consoleButton" disabled={busy} onClick={() => void run('list')}>List diagnostic logs</button>
    </>}
    {sources.length > 0 && <div>
      <label>Log source <select value={source} disabled={busy} onChange={event => {
        setSource(event.target.value);
        setDiagnostics([]);
        setMessage('');
      }}>
        {sources.map((item, index) => <option key={item.ref} value={item.ref}>
          Source {index + 1}{item.bytes !== null ? ` · ${item.bytes} bytes` : ''}
        </option>)}
      </select></label>
      <button className="consoleButton" disabled={busy || !source} onClick={() => void run('read')}>Read recent diagnostics</button>
    </div>}
    {message && <p role="status">{message}</p>}
    {diagnostics.length > 0 && <ul>{diagnostics.map((line, index) => <li key={index}>{line}</li>)}</ul>}
  </SectionCard>;
}
