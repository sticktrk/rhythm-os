import { useCallback, useEffect, useState } from 'react';
import { useDeviceClient } from '../hooks/useDeviceClient';
import { usePolling } from '../hooks/usePolling';
import { SectionCard } from '../components/ui/SectionCard';
import { useConfirm } from '../components/ui/ConfirmDialog';
import { errorMessage } from '../lib/format';

type ProSettings = {remote_mcp: boolean; bulb_profile_updates: boolean};
type TunnelStatus = {enabled: boolean; configured: boolean; service_error?: string};
type Enrollment = {code: string; server_instance_id: string; expires_at_epoch_ms: number};
type MobileToken = {id: string; label: string; role: string};

export default function MobileAccess() {
  const client = useDeviceClient();
  const confirm = useConfirm();
  const tokens = usePolling(useCallback(() => client.get<{tokens: MobileToken[]}>('api/addon/mobile-tokens'), [client]), {intervalMs: 15_000});
  const pro = usePolling(useCallback(() => client.get<ProSettings>('api/pro/settings'), [client]), {intervalMs: 15000});
  const tunnel = usePolling(useCallback(() => client.get<TunnelStatus>('api/remote-access/status'), [client]), {intervalMs: 15000});
  const [proError, setProError] = useState<string | null>(null);
  async function toggle(key: keyof ProSettings | 'tunnel', value: boolean) {
    setBusy(true);
    setProError(null);
    try {
      if (key === 'tunnel') {
        const status = await client.put<TunnelStatus>('api/addon/tunnel', {body: {enabled: value}});
        if (status.service_error) throw new Error(status.service_error);
      } else {
        await client.put('api/pro/settings', {body: {[key]: value}});
      }
    } catch (e) {
      setProError(errorMessage(e));
    } finally {
      await Promise.all([pro.refresh(), tunnel.refresh()]);
      setBusy(false);
    }
  }
  const [enrollment, setEnrollment] = useState<Enrollment | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  useEffect(() => {
    if (!enrollment) return;
    const timer = window.setTimeout(() => setEnrollment(null), Math.max(0, enrollment.expires_at_epoch_ms - Date.now()));
    return () => window.clearTimeout(timer);
  }, [enrollment]);
  async function run(action: () => Promise<void>) {
    setBusy(true); setError(null); setNotice(null);
    try { await action(); } catch (e) { setError(errorMessage(e)); } finally { setBusy(false); }
  }
  return <div className="consolePage"><header className="pageHeader"><h1>Connect mobile app</h1></header>
    <SectionCard title="Connect a phone" error={error}>
      <p>In the Rhythm app, add your Home Assistant host's LAN address and the Rhythm port configured in the add-on Network settings (54448 by default). Paste this connection code when asked. Keep the code private: it grants control of this Rhythm installation.</p>
      <button className="consoleButton" disabled={busy} onClick={() => void run(async () => {
        setEnrollment(await client.post<Enrollment>('api/addon/enrollment', {body: {}}));
      })}>{busy ? 'Working…' : 'Generate connection code'}</button>
      {enrollment && <div>
        <p>This code works once and expires at {new Date(enrollment.expires_at_epoch_ms).toLocaleTimeString()}. Generating another replaces it.</p>
        <label>Connection code<textarea readOnly rows={6} aria-label="Mobile connection code" value={JSON.stringify({format: 'rhythm-mobile-enrollment', version: 1, ...enrollment})} onFocus={event => event.target.select()} /></label>
        <button className="consoleButton" onClick={() => setEnrollment(null)}>Hide code</button>
      </div>}
      <p>After pairing, open Settings → Rhythm Pro in the iOS or Android app to sign in and set up your tunnel.</p>
    </SectionCard>
    <SectionCard title="Rhythm Pro" error={proError ?? pro.error ?? tunnel.error}>
      {pro.data && <label className="actionRow"><span>Cloud bulb profile updates</span>
        <input type="checkbox" role="switch" checked={pro.data.bulb_profile_updates} disabled={busy || pro.refreshing} onChange={e => void toggle('bulb_profile_updates', e.target.checked)} /></label>}
      {tunnel.data && <label className="actionRow"><span>Remote access tunnel</span>
        <input type="checkbox" role="switch" checked={tunnel.data.enabled} disabled={busy || tunnel.refreshing || !tunnel.data.configured} onChange={e => void toggle('tunnel', e.target.checked)} /></label>}
      {tunnel.data && !tunnel.data.configured && <p>Set up the tunnel in the mobile app to enable this switch.</p>}
      {pro.data && <label className="actionRow"><span>Remote MCP</span>
        <input type="checkbox" role="switch" checked={pro.data.remote_mcp} disabled={busy || pro.refreshing} onChange={e => void toggle('remote_mcp', e.target.checked)} /></label>}
      <p>Remote MCP lets authorized agents use the tunnel. Agents on your local network use their existing credentials.</p>
    </SectionCard>
    <SectionCard title="Authorized credentials" error={tokens.error}>
      {tokens.data?.tokens.map(token => <div className="actionRow" key={token.id}>
        <span>{token.label || 'Rhythm client'} ({token.role})</span>
        <button className="consoleButton danger" disabled={busy} onClick={() => void run(async () => {
          if (!(await confirm({title: 'Revoke access', message: 'This credential will stop working on LAN and remotely. The phone will need a new connection code.', confirmLabel: 'Revoke', danger: true}))) return;
          await client.delete(`api/addon/mobile-tokens/${encodeURIComponent(token.id)}`);
          await tokens.refresh(); setNotice('Access revoked.');
        })}>Revoke</button>
      </div>)}
      {tokens.data?.tokens.length === 0 && <p>No mobile credentials have been issued.</p>}
      {notice && <p role="status">{notice}</p>}
    </SectionCard>
  </div>;
}
