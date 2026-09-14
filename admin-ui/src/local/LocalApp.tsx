import { useCallback, useMemo, useState } from 'react';
import { BrowserRouter, Link, NavLink, Navigate, Route, Routes } from 'react-router-dom';
import { Lightbulb, RefreshCw } from 'lucide-react';
import { ConfirmProvider, useConfirm } from '../components/ui/ConfirmDialog';
import { SectionCard } from '../components/ui/SectionCard';
import { ErrorNotice, KeyValueGrid } from '../components/ui/bits';
import { usePolling } from '../hooks/usePolling';
import { useDeviceClient } from '../hooks/useDeviceClient';
import { LocalDeviceContext } from '../state/LocalDeviceContext';
import { getState } from '../device/state';
import { getLightBreaker, setLightBreaker } from '../device/settings';
import { getProfileBundle, putProfileBundle } from '../device/backup';
import { asRecord, asString } from '../device/values';
import { errorMessage } from '../lib/format';
import NodesPage from '../pages/hub/NodesPage';
import ProfilesPage from '../pages/hub/ProfilesPage';
import ScenesPage from '../pages/hub/ScenesPage';
import ModesPage from '../pages/hub/ModesPage';
import InputsPage from '../pages/hub/InputsPage';
import HistoryPage from '../pages/hub/HistoryPage';
import { createLocalDeviceClient, ingressBasePath, localFetch } from './transport';

type LocalSession = {
  deployment: 'home_assistant_addon';
  schemaVersion: number;
  user: {id: string; name: string};
  status: Record<string, unknown> | null;
};
const pages = [
  ['overview', 'Overview'], ['managed-lights', 'Managed lights'], ['nodes', 'Rooms & lights'], ['profiles', 'Profiles'],
  ['scenes', 'Scenes'], ['modes', 'Modes'], ['inputs', 'Inputs'],
  ['environment', 'Home Assistant'], ['history', 'Activity'], ['system', 'System']
];

export default function LocalApp() {
  const client = useMemo(createLocalDeviceClient, []);
  const session = usePolling(useCallback(() => localFetch<LocalSession>('api/session'), []),
    {intervalMs: 15_000});
  const base = ingressBasePath(document.querySelector('base')?.getAttribute('href') ?? null);
  if (!session.data || session.error) {
    return <main className="centeredScreen"><section className="authPanel">
      <Lightbulb size={32} /><h1>Rhythm</h1>
      <p>{session.error ?? 'Connecting to your Home Assistant home…'}</p>
      {session.error && <button className="primaryButton" onClick={() => void session.refresh()}>Retry</button>}
    </section></main>;
  }
  if (session.data.deployment !== 'home_assistant_addon' || session.data.schemaVersion !== 1) {
    return <main className="centeredScreen">This Rhythm interface needs a compatible add-on version. Reload after updating.</main>;
  }
  return <LocalDeviceContext.Provider value={client}>
    <BrowserRouter basename={base.replace(/\/$/, '') || '/'}><ConfirmProvider>
      <div className="consoleShell localRhythm">
        <aside className="consoleNav" aria-label="Rhythm sections">
          <div className="consoleHubCard"><div className="consoleHubName"><Lightbulb size={22} /> Rhythm</div>
            <p className="consoleHubMeta">Your Home Assistant home</p></div>
          <nav className="consoleNavList">{pages.map(([path, label]) =>
            <NavLink key={path} to={`/${path}`} className={({isActive}) => `consoleNavItem${isActive ? ' active' : ''}`}>{label}</NavLink>)}</nav>
        </aside>
        <main className="consoleMain"><Routes>
          <Route path="/overview" element={<Overview status={session.data.status} refresh={session.refresh} />} />
          <Route path="/nodes" element={<NodesPage />} />
          <Route path="/managed-lights" element={<ManagedLights />} />
          <Route path="/profiles" element={<ProfilesPage />} />
          <Route path="/scenes" element={<ScenesPage />} />
          <Route path="/modes" element={<ModesPage />} />
          <Route path="/inputs" element={<InputsPage />} />
          <Route path="/environment" element={<Connection status={session.data.status} />} />
          <Route path="/history" element={<HistoryPage />} />
          <Route path="/system" element={<System status={session.data.status} />} />
          <Route path="*" element={<Navigate to="/overview" replace />} />
        </Routes></main>
      </div>
    </ConfirmProvider></BrowserRouter>
  </LocalDeviceContext.Provider>;
}

function Overview({status, refresh}: {status: Record<string, unknown> | null; refresh: () => Promise<void>}) {
  const client = useDeviceClient();
  const confirm = useConfirm();
  const breaker = usePolling(useCallback(() => getLightBreaker(client), [client]), {intervalMs: 10_000});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const enabled = breaker.data?.enabled === true;
  const connected = asRecord(status?.connection).connected === true;
  async function toggle() {
    if (!enabled && !(await confirm({title: 'Enable adaptive lighting',
      message: 'Review the rooms and lights you want Rhythm to manage first. Enabling allows configured profiles, modes and inputs to control those lights.',
      confirmLabel: 'Enable Rhythm'}))) return;
    setBusy(true); setError(null);
    try { await setLightBreaker(client, !enabled); await breaker.refresh(); await refresh(); }
    catch (e) { setError(errorMessage(e)); } finally { setBusy(false); }
  }
  return <div className="consolePage"><header className="pageHeader"><div>
    <div className="eyebrow">Rhythm for Home Assistant</div><h1>Lighting for your day</h1>
    <p className="pageIntro">Shape how your home feels with profiles, scenes and modes.</p>
  </div></header>
    <SectionCard title={connected ? 'Connected to Home Assistant' : 'Waiting for Home Assistant'}
      subtitle="The connection is managed automatically by your add-on." error={error ?? breaker.error}>
      <p>{enabled ? 'Adaptive lighting is enabled.' : 'Adaptive lighting is paused. Your lights keep their current state.'}</p>
      <div className="actionRow"><button className="consoleButton" disabled={busy || !breaker.data || (!enabled && !connected)} onClick={() => void toggle()}>
        {busy ? 'Updating…' : enabled ? 'Pause Rhythm' : 'Enable Rhythm'}</button>
        <Link className="consoleButton" to="/managed-lights">Choose managed lights</Link></div>
    </SectionCard>
    <SectionCard title="Get started"><ol>
      <li>Choose which Home Assistant lights Rhythm may control. New lights are excluded until selected.</li>
      <li>Choose their profiles and input behavior.</li>
      <li>Enable Rhythm when your selections are ready.</li>
    </ol><p>Manage device integrations and areas in Home Assistant. Use one lighting controller per light to avoid conflicting automations.</p></SectionCard>
  </div>;
}

type LightSelection = { entities: string[]; available: {entity_id: string; name: string}[] };
function ManagedLights() {
  const client = useDeviceClient();
  const confirm = useConfirm();
  const selection = usePolling(useCallback(() => client.get<LightSelection>('api/addon/lights'), [client]), {intervalMs: 30_000});
  const [draft, setDraft] = useState<string[] | null>(null);
  const [expected, setExpected] = useState<string[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const selected = draft ?? selection.data?.entities ?? [];
  async function save() {
    if (!draft || !expected || !(await confirm({title: 'Save managed lights', message: 'Rhythm will pause and save this selection. Review your profiles and enable Rhythm from Overview when ready.', confirmLabel: 'Pause and save'}))) return;
    setBusy(true); setError(null); setNotice(null);
    try {
      await setLightBreaker(client, false);
      await client.put('api/addon/lights', {body: {entities: draft, expected_entities: expected}});
      setDraft(null); setExpected(null); await selection.refresh(); setNotice('Selection saved. Rhythm is paused.');
    } catch (e) { setError(errorMessage(e)); } finally { setBusy(false); }
  }
  return <div className="consolePage"><header className="pageHeader"><h1>Managed lights</h1></header>
    <SectionCard title="Choose the lights Rhythm may control" error={error ?? selection.error}>
      <p>Only selected entities receive light commands. New entities stay excluded, even when added to an existing room. If you rename an entity in Home Assistant, review this selection again.</p>
      {selection.data?.available.map(light => <label className="managedLight" key={light.entity_id}>
        <input type="checkbox" disabled={busy} checked={selected.includes(light.entity_id)} onChange={event => {
          if (!draft) setExpected(selection.data!.entities);
          setDraft(event.target.checked ? [...selected, light.entity_id] : selected.filter(id => id !== light.entity_id));
        }} /><span>{light.name}<small>{light.entity_id}</small></span>
      </label>)}
      {!selection.data?.available.length && <p>Waiting for lights from Home Assistant. Add your light integrations and synchronize from the Home Assistant page.</p>}
      {selected.filter(id => !selection.data?.available.some(light => light.entity_id === id)).map(id => <label className="managedLight" key={id}>
        <input type="checkbox" disabled={busy} checked onChange={() => {if (!draft) setExpected(selection.data!.entities); setDraft(selected.filter(item => item !== id));}} />
        <span>{id}<small>Unavailable — remove if no longer needed</small></span></label>)}
      <div className="actionRow"><button className="consoleButton" disabled={busy || !draft} onClick={() => void save()}>{busy ? 'Saving…' : 'Save selection'}</button>
        <button className="consoleButton" disabled={busy || !draft} onClick={() => {setDraft(null); setExpected(null);}}>Discard changes</button></div>
      {notice && <p role="status">{notice}</p>}
    </SectionCard>
  </div>;
}

function Connection({status}: {status: Record<string, unknown> | null}) {
  const client = useDeviceClient();
  const state = usePolling(useCallback(() => getState(client), [client]), {intervalMs: 30_000});
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  async function sync() {
    setBusy(true); setError(null);
    try { await client.post('api/sync'); await state.refresh(); }
    catch (e) { setError(errorMessage(e)); } finally { setBusy(false); }
  }
  return <div className="consolePage"><header className="pageHeader"><h1>Home Assistant</h1></header>
    <SectionCard title="Your home connection" error={error ?? state.error}>
      <KeyValueGrid rows={[
        ['Connection', asRecord(status?.connection).connected === true ? 'Connected' : 'Waiting to reconnect'],
        ['Setup', 'Managed automatically'], ['Location and timezone', 'From Home Assistant']
      ]} />
      <p>Update areas, device integrations, home location and timezone in Home Assistant. Rhythm uses those settings here.</p>
      <button className="consoleButton" disabled={busy} onClick={() => void sync()}><RefreshCw size={16} />{busy ? 'Synchronizing…' : 'Synchronize now'}</button>
    </SectionCard>
  </div>;
}

function saveJson(name: string, value: unknown) {
  const url = URL.createObjectURL(new Blob([JSON.stringify(value, null, 2)], {type: 'application/json'}));
  const anchor = document.createElement('a'); anchor.href = url; anchor.download = name; anchor.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}

function System({status}: {status: Record<string, unknown> | null}) {
  const client = useDeviceClient();
  const confirm = useConfirm();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  async function run(action: () => Promise<void>) {
    setBusy(true); setError(null); setNotice(null);
    try { await action(); } catch (e) { setError(errorMessage(e)); } finally { setBusy(false); }
  }
  async function importProfiles(file: File) {
    if (file.size > 1024 * 1024) throw new Error('Choose a profile export smaller than 1 MB.');
    const value = JSON.parse(await file.text());
    if (value.format !== 'rhythm-ha-profiles' || value.version !== 1 ||
        !value.profiles || typeof value.profiles !== 'object' || Array.isArray(value.profiles)) {
      throw new Error('Choose a Rhythm Home Assistant profile export. Full device backups are restored through Home Assistant.');
    }
    if (!(await confirm({title: 'Restore lighting profiles', message: 'This replaces your lighting profile bundle and pauses adaptation for review. Home Assistant devices and areas are unchanged.',
      confirmLabel: 'Restore profiles', requireTypedText: 'RESTORE'}))) return;
    await setLightBreaker(client, false);
    await putProfileBundle(client, value.profiles);
    setNotice('Profiles restored. Review your rooms and enable Rhythm when ready.');
  }
  return <div className="consolePage"><header className="pageHeader"><h1>System</h1></header>
    {error && <ErrorNotice message={error} />}{notice && <p role="status">{notice}</p>}
    <SectionCard title="Version and updates"><p>Rhythm {asString(status?.version) ?? 'starting'}</p>
      <p>Start, stop, update and back up this add-on in Home Assistant. A Home Assistant backup includes all Rhythm configuration.</p>
      <button className="consoleButton" onClick={() => saveJson('rhythm-status.json', status)}>Download connection diagnostics</button>
    </SectionCard>
    <SectionCard title="Lighting profile export" subtitle="Transfer profile settings. Full configuration and device selections use Home Assistant backups.">
      <div className="actionRow"><button className="consoleButton" disabled={busy} onClick={() => void run(async () => {
        saveJson('rhythm-profiles.json', {format: 'rhythm-ha-profiles', version: 1, profiles: await getProfileBundle(client)});
      })}>Export profiles</button>
      <label className="consoleButton">Restore profiles<input type="file" accept="application/json,.json" disabled={busy} onChange={(event) => {
        const file = event.target.files?.[0]; event.target.value = '';
        if (file) void run(() => importProfiles(file));
      }} /></label></div>
    </SectionCard>
    <SectionCard title="Reset Rhythm configuration"><p>Remove Rhythm settings and reconnect automatically. Home Assistant devices, integrations and add-on options remain in place.</p>
      <button className="consoleButton danger" disabled={busy} onClick={() => void run(async () => {
        if (!(await confirm({title: 'Reset Rhythm configuration', message: 'Delete Rhythm profiles, scenes, bindings and local state. Home Assistant will restart the Rhythm service.',
          confirmLabel: 'Reset Rhythm', danger: true, requireTypedText: 'RESET RHYTHM'}))) return;
        await client.post('api/factory-reset'); setNotice('Rhythm is restarting. Reload shortly.');
      })}>Reset Rhythm</button>
    </SectionCard>
  </div>;
}
