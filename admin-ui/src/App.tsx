import { FormEvent, useCallback, useEffect, useMemo, useState } from 'react';
import type { Session } from '@supabase/supabase-js';
import {
  Activity,
  AlertTriangle,
  Download,
  FileText,
  Home,
  KeyRound,
  Loader2,
  LogOut,
  MapPin,
  RefreshCw,
  Search,
  Server,
  ShieldCheck,
  Users,
  Wifi
} from 'lucide-react';

import {
  applyHubUpdate,
  checkHubUpdate,
  downloadDebugBundle,
  fetchHealth,
  fetchHubStatus,
  fetchHubLogSources,
  fetchHubLogTail,
  fetchMe,
  fetchReadiness,
  fetchSupportSnapshot,
  probeHub
} from './api';
import { isSupabaseConfigured, supabase } from './supabaseClient';
import type {
  HomeListItem,
  AdminApiHealth,
  AdminApiReadiness,
  DeviceLogSource,
  DeviceLogTail,
  DeviceOtaAction,
  DeviceStatus,
  MeResponse,
  ProbeResult,
  SupportHub,
  SupportSnapshot
} from './types';

type ProbeState = {
  loading: boolean;
  result?: ProbeResult;
  error?: string;
};

type BundleState = {
  loading: boolean;
  error?: string;
  fileName?: string;
  downloadedAt?: string;
  route?: 'remote' | 'local';
};

type LogState = {
  loading: boolean;
  opened: boolean;
  error?: string;
  sources?: DeviceLogSource[];
  selectedSourceId?: string;
  tail?: DeviceLogTail;
};

type StatusState = {
  loading: boolean;
  opened: boolean;
  error?: string;
  result?: DeviceStatus;
};

type OtaState = {
  checking: boolean;
  updating: boolean;
  error?: string;
  result?: DeviceOtaAction;
};

export default function App() {
  const [session, setSession] = useState<Session | null>(null);
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [authBusy, setAuthBusy] = useState(true);
  const [authError, setAuthError] = useState<string | null>(null);
  const [health, setHealth] = useState<AdminApiHealth | null>(null);
  const [readiness, setReadiness] = useState<AdminApiReadiness | null>(null);
  const [me, setMe] = useState<MeResponse | null>(null);
  const [snapshot, setSnapshot] = useState<SupportSnapshot | null>(null);
  const [loadingSnapshot, setLoadingSnapshot] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [selectedHomeId, setSelectedHomeId] = useState<string | null>(null);
  const [probeStates, setProbeStates] = useState<Record<string, ProbeState>>(
    {}
  );
  const [bundleStates, setBundleStates] = useState<Record<string, BundleState>>(
    {}
  );
  const [logStates, setLogStates] = useState<Record<string, LogState>>({});
  const [statusStates, setStatusStates] = useState<Record<string, StatusState>>(
    {}
  );
  const [otaStates, setOtaStates] = useState<Record<string, OtaState>>({});

  useEffect(() => {
    if (!supabase) {
      setAuthBusy(false);
      return;
    }

    let active = true;
    supabase.auth.getSession().then(({ data }) => {
      if (!active) return;
      setSession(data.session);
      setAuthBusy(false);
    });
    const {
      data: { subscription }
    } = supabase.auth.onAuthStateChange((_event, nextSession) => {
      setSession(nextSession);
      setAuthError(null);
      setHealth(null);
      setReadiness(null);
      setMe(null);
      setSnapshot(null);
      setSelectedHomeId(null);
      setProbeStates({});
      setBundleStates({});
      setLogStates({});
      setStatusStates({});
      setOtaStates({});
    });

    return () => {
      active = false;
      subscription.unsubscribe();
    };
  }, []);

  const loadDashboard = useCallback(async () => {
    if (!session) return;
    setLoadingSnapshot(true);
    setLoadError(null);
    try {
      const [nextHealth, nextReadiness, nextMe, nextSnapshot] = await Promise.all([
        fetchHealth().catch(() => null),
        fetchReadiness().catch(() => null),
        fetchMe(session.access_token),
        fetchSupportSnapshot(session.access_token)
      ]);
      setHealth(nextHealth);
      setReadiness(nextReadiness);
      setMe(nextMe);
      setSnapshot(nextSnapshot);
      setSelectedHomeId((current) => current ?? firstHomeId(nextSnapshot));
    } catch (error) {
      setLoadError(error instanceof Error ? error.message : String(error));
    } finally {
      setLoadingSnapshot(false);
    }
  }, [session]);

  useEffect(() => {
    void loadDashboard();
  }, [loadDashboard]);

  const homes = useMemo(() => flattenHomes(snapshot), [snapshot]);
  const filteredHomes = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return homes;
    return homes.filter((item) => item.searchText.includes(needle));
  }, [homes, query]);
  const selectedHome =
    filteredHomes.find((item) => item.home.id === selectedHomeId) ??
    filteredHomes[0] ??
    null;

  useEffect(() => {
    if (!selectedHome && filteredHomes.length === 0) return;
    if (selectedHomeId && filteredHomes.some((item) => item.home.id === selectedHomeId)) {
      return;
    }
    setSelectedHomeId(filteredHomes[0]?.home.id ?? null);
  }, [filteredHomes, selectedHome, selectedHomeId]);

  async function handleSignIn(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!supabase) return;
    setAuthBusy(true);
    setAuthError(null);
    const { error } = await supabase.auth.signInWithPassword({
      email: email.trim(),
      password
    });
    if (error) setAuthError(error.message);
    setPassword('');
    setAuthBusy(false);
  }

  async function handleSignOut() {
    if (!supabase) return;
    setAuthBusy(true);
    await supabase.auth.signOut();
    setAuthBusy(false);
  }

  async function handleProbe(hub: SupportHub) {
    if (!session) return;
    setProbeStates((current) => ({
      ...current,
      [hub.id]: { ...current[hub.id], loading: true, error: undefined }
    }));
    try {
      const result = await probeHub(session.access_token, hub.id);
      setProbeStates((current) => ({
        ...current,
        [hub.id]: { loading: false, result }
      }));
    } catch (error) {
      setProbeStates((current) => ({
        ...current,
        [hub.id]: {
          ...current[hub.id],
          loading: false,
          error: error instanceof Error ? error.message : String(error)
        }
      }));
    }
  }

  async function handleDownloadBundle(hub: SupportHub) {
    if (!session) return;
    setBundleStates((current) => ({
      ...current,
      [hub.id]: { ...current[hub.id], loading: true, error: undefined }
    }));
    try {
      const bundle = await downloadDebugBundle(session.access_token, hub.id);
      triggerBrowserDownload(bundle.blob, bundle.fileName);
      setBundleStates((current) => ({
        ...current,
        [hub.id]: {
          loading: false,
          fileName: bundle.fileName,
          downloadedAt: new Date().toISOString(),
          route: bundle.route
        }
      }));
    } catch (error) {
      setBundleStates((current) => ({
        ...current,
        [hub.id]: {
          ...current[hub.id],
          loading: false,
          error: error instanceof Error ? error.message : String(error)
        }
      }));
    }
  }

  async function handleLoadStatus(hub: SupportHub) {
    if (!session) return;
    setStatusStates((current) => ({
      ...current,
      [hub.id]: {
        ...current[hub.id],
        loading: true,
        opened: true,
        error: undefined
      }
    }));

    try {
      const result = await fetchHubStatus(session.access_token, hub.id);
      setStatusStates((current) => ({
        ...current,
        [hub.id]: {
          loading: false,
          opened: true,
          result
        }
      }));
    } catch (error) {
      setStatusStates((current) => ({
        ...current,
        [hub.id]: {
          ...current[hub.id],
          loading: false,
          opened: true,
          error: error instanceof Error ? error.message : String(error)
        }
      }));
    }
  }

  async function handleCheckUpdate(hub: SupportHub) {
    if (!session) return;
    setOtaStates((current) => ({
      ...current,
      [hub.id]: {
        ...current[hub.id],
        checking: true,
        updating: current[hub.id]?.updating ?? false,
        error: undefined
      }
    }));
    try {
      const result = await checkHubUpdate(session.access_token, hub.id);
      setOtaStates((current) => ({
        ...current,
        [hub.id]: {
          checking: false,
          updating: current[hub.id]?.updating ?? false,
          result
        }
      }));
      mergeOtaResultIntoStatus(hub.id, result);
    } catch (error) {
      setOtaStates((current) => ({
        ...current,
        [hub.id]: {
          ...current[hub.id],
          checking: false,
          updating: current[hub.id]?.updating ?? false,
          error: error instanceof Error ? error.message : String(error)
        }
      }));
    }
  }

  async function handleApplyUpdate(hub: SupportHub) {
    if (!session) return;
    const confirmed = window.confirm(`Run OTA update for ${hub.name}?`);
    if (!confirmed) return;
    setOtaStates((current) => ({
      ...current,
      [hub.id]: {
        ...current[hub.id],
        checking: current[hub.id]?.checking ?? false,
        updating: true,
        error: undefined
      }
    }));
    try {
      const result = await applyHubUpdate(session.access_token, hub.id);
      setOtaStates((current) => ({
        ...current,
        [hub.id]: {
          checking: current[hub.id]?.checking ?? false,
          updating: false,
          result
        }
      }));
      mergeOtaResultIntoStatus(hub.id, result);
    } catch (error) {
      setOtaStates((current) => ({
        ...current,
        [hub.id]: {
          ...current[hub.id],
          checking: current[hub.id]?.checking ?? false,
          updating: false,
          error: error instanceof Error ? error.message : String(error)
        }
      }));
    }
  }

  function mergeOtaResultIntoStatus(hubId: string, action: DeviceOtaAction) {
    setStatusStates((current) => {
      const existing = current[hubId];
      if (!existing?.opened || !existing.result) return current;
      return {
        ...current,
        [hubId]: {
          ...existing,
          result: {
            ...existing.result,
            ota: action.result
          }
        }
      };
    });
  }

  async function handleLoadLogs(hub: SupportHub, sourceId?: string) {
    if (!session) return;
    const existing = logStates[hub.id];
    setLogStates((current) => ({
      ...current,
      [hub.id]: {
        ...current[hub.id],
        loading: true,
        opened: true,
        error: undefined
      }
    }));

    try {
      const sources =
        existing?.sources ??
        (await fetchHubLogSources(session.access_token, hub.id)).sources;
      const selectedSourceId =
        sourceId ?? existing?.selectedSourceId ?? preferredLogSourceId(sources);
      const tail = selectedSourceId
        ? await fetchHubLogTail(session.access_token, hub.id, selectedSourceId)
        : undefined;

      setLogStates((current) => ({
        ...current,
        [hub.id]: {
          loading: false,
          opened: true,
          sources,
          selectedSourceId,
          tail
        }
      }));
    } catch (error) {
      setLogStates((current) => ({
        ...current,
        [hub.id]: {
          ...current[hub.id],
          loading: false,
          opened: true,
          error: error instanceof Error ? error.message : String(error)
        }
      }));
    }
  }

  if (!isSupabaseConfigured) {
    return <SetupScreen />;
  }

  if (authBusy && !session) {
    return <LoadingScreen label="Checking session" />;
  }

  if (!session) {
    return (
      <AuthScreen
        email={email}
        password={password}
        busy={authBusy}
        error={authError}
        onEmailChange={setEmail}
        onPasswordChange={setPassword}
        onSubmit={handleSignIn}
      />
    );
  }

  return (
    <div className="appShell">
      <header className="topbar">
        <div>
          <div className="eyebrow">Rhythm Staff</div>
          <h1>Customer Support</h1>
        </div>
        <div className="topbarActions">
          <div className="staffBadge">
            <ShieldCheck size={16} />
            <span>{me?.staffStatus.role ?? 'staff'}</span>
          </div>
          <button className="iconButton" type="button" onClick={loadDashboard}>
            <RefreshCw size={18} />
            <span>Refresh</span>
          </button>
          <button className="iconButton danger" type="button" onClick={handleSignOut}>
            <LogOut size={18} />
            <span>Sign out</span>
          </button>
        </div>
      </header>

      {loadError ? (
        <div className="notice error">
          <AlertTriangle size={18} />
          <span>{loadError}</span>
        </div>
      ) : null}

      <ApiCapabilityNotice health={health} readiness={readiness} />

      <section className="metrics" aria-label="Support totals">
        <Metric icon={<Users size={20} />} label="Customers" value={snapshot?.totals.customers ?? 0} />
        <Metric icon={<Home size={20} />} label="Homes" value={snapshot?.totals.homes ?? 0} />
        <Metric icon={<Server size={20} />} label="Light Boxes" value={snapshot?.totals.hubs ?? 0} />
        <Metric icon={<Activity size={20} />} label="Visible Results" value={filteredHomes.length} />
      </section>

      <main className="workspace">
        <aside className="sidebar" aria-label="Homes">
          <div className="searchBox">
            <Search size={18} />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Search customer, email, home, hub"
            />
          </div>
          <div className="homeList">
            {loadingSnapshot ? (
              <div className="listState">
                <Loader2 className="spin" size={18} />
                <span>Loading homes</span>
              </div>
            ) : filteredHomes.length === 0 ? (
              <div className="listState">No homes match this search.</div>
            ) : (
              filteredHomes.map((item) => (
                <button
                  type="button"
                  className={`homeListItem ${
                    item.home.id === selectedHome?.home.id ? 'selected' : ''
                  }`}
                  key={item.home.id}
                  onClick={() => setSelectedHomeId(item.home.id)}
                >
                  <span className="itemTitle">{item.home.name}</span>
                  <span className="itemMeta">{item.customer.customerLabel}</span>
                  <span className="itemMeta">
                    {item.hubs.length} Light Box{item.hubs.length === 1 ? '' : 'es'}
                  </span>
                </button>
              ))
            )}
          </div>
        </aside>

        <section className="detailPane" aria-label="Selected home">
          {selectedHome ? (
            <HomeDetail
              item={selectedHome}
              probeStates={probeStates}
              bundleStates={bundleStates}
              statusStates={statusStates}
              otaStates={otaStates}
              logStates={logStates}
              onProbe={handleProbe}
              onDownloadBundle={handleDownloadBundle}
              onLoadStatus={handleLoadStatus}
              onCheckUpdate={handleCheckUpdate}
              onApplyUpdate={handleApplyUpdate}
              onLoadLogs={handleLoadLogs}
            />
          ) : (
            <div className="emptyPane">
              <Home size={28} />
              <p>Select a home to inspect its Light Boxes.</p>
            </div>
          )}
        </section>
      </main>
    </div>
  );
}

function ApiCapabilityNotice({
  health,
  readiness
}: {
  health: AdminApiHealth | null;
  readiness: AdminApiReadiness | null;
}) {
  if (readiness?.remoteDebugReady) return null;
  if (!readiness && !health) return null;
  if (
    !readiness &&
    health?.serviceRoleConfigured &&
    health.supportAccessConfigured
  ) {
    return null;
  }

  const missing =
    readiness?.missing ??
    [
      health && !health.serviceRoleConfigured ? 'SUPABASE_SERVICE_ROLE_KEY' : null,
      health && !health.supportAccessConfigured
        ? 'SUPPORT_ACCESS_ENCRYPTION_KEY'
        : null
    ].filter((value): value is string => value !== null);

  return (
    <div className="notice warning">
      <AlertTriangle size={18} />
      <span>
        admin-api remote debugging is not ready; missing {missing.join(' and ')}.{' '}
        {readiness?.notes[0] ?? 'Encrypted remote support needs matching server config.'}
      </span>
    </div>
  );
}

function SetupScreen() {
  return (
    <main className="centeredScreen">
      <section className="authPanel">
        <div className="panelIcon warning">
          <AlertTriangle size={24} />
        </div>
        <h1>Admin UI is not configured</h1>
        <p>
          Set <code>VITE_SUPABASE_URL</code>, <code>VITE_SUPABASE_ANON_KEY</code>,
          and <code>VITE_ADMIN_API_URL</code> in <code>admin-ui/.env</code>.
        </p>
      </section>
    </main>
  );
}

function LoadingScreen({ label }: { label: string }) {
  return (
    <main className="centeredScreen">
      <div className="loadingBlock">
        <Loader2 className="spin" size={22} />
        <span>{label}</span>
      </div>
    </main>
  );
}

function AuthScreen({
  email,
  password,
  busy,
  error,
  onEmailChange,
  onPasswordChange,
  onSubmit
}: {
  email: string;
  password: string;
  busy: boolean;
  error: string | null;
  onEmailChange: (value: string) => void;
  onPasswordChange: (value: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
}) {
  return (
    <main className="centeredScreen">
      <form className="authPanel" onSubmit={onSubmit}>
        <div className="panelIcon">
          <ShieldCheck size={24} />
        </div>
        <h1>Rhythm Admin</h1>
        <label>
          <span>Email</span>
          <input
            type="email"
            value={email}
            onChange={(event) => onEmailChange(event.target.value)}
            autoComplete="email"
            required
          />
        </label>
        <label>
          <span>Password</span>
          <input
            type="password"
            value={password}
            onChange={(event) => onPasswordChange(event.target.value)}
            autoComplete="current-password"
            required
          />
        </label>
        {error ? <div className="notice error">{error}</div> : null}
        <button className="primaryButton" type="submit" disabled={busy}>
          {busy ? <Loader2 className="spin" size={18} /> : <ShieldCheck size={18} />}
          <span>Sign in</span>
        </button>
      </form>
    </main>
  );
}

function Metric({
  icon,
  label,
  value
}: {
  icon: React.ReactNode;
  label: string;
  value: number;
}) {
  return (
    <div className="metric">
      <div className="metricIcon">{icon}</div>
      <div>
        <div className="metricValue">{value}</div>
        <div className="metricLabel">{label}</div>
      </div>
    </div>
  );
}

function HomeDetail({
  item,
  probeStates,
  bundleStates,
  statusStates,
  otaStates,
  logStates,
  onProbe,
  onDownloadBundle,
  onLoadStatus,
  onCheckUpdate,
  onApplyUpdate,
  onLoadLogs
}: {
  item: HomeListItem;
  probeStates: Record<string, ProbeState>;
  bundleStates: Record<string, BundleState>;
  statusStates: Record<string, StatusState>;
  otaStates: Record<string, OtaState>;
  logStates: Record<string, LogState>;
  onProbe: (hub: SupportHub) => void;
  onDownloadBundle: (hub: SupportHub) => void;
  onLoadStatus: (hub: SupportHub) => void;
  onCheckUpdate: (hub: SupportHub) => void;
  onApplyUpdate: (hub: SupportHub) => void;
  onLoadLogs: (hub: SupportHub, sourceId?: string) => void;
}) {
  return (
    <div className="homeDetail">
      <div className="homeHeader">
        <div>
          <div className="eyebrow">{item.customer.customerLabel}</div>
          <h2>{item.home.name}</h2>
          <div className="homeMeta">
            {item.home.locationCity ? (
              <span>
                <MapPin size={15} />
                {item.home.locationCity}
              </span>
            ) : null}
            {item.home.timezone ? <span>{item.home.timezone}</span> : null}
            {item.customer.secondaryLabel ? (
              <span>{item.customer.secondaryLabel}</span>
            ) : null}
          </div>
        </div>
        <div className="homeIds">
          <span>Home {shortId(item.home.id)}</span>
          <span>Owner {shortId(item.home.ownerId)}</span>
        </div>
      </div>

      <div className="sectionHeader">
        <div>
          <h3>Light Boxes</h3>
          <p>{item.hubs.length} server hub record{item.hubs.length === 1 ? '' : 's'}</p>
        </div>
      </div>

      {item.hubs.length === 0 ? (
        <div className="emptyPane compact">No Light Boxes are synced for this home.</div>
      ) : (
        <div className="hubTable">
          {item.hubs.map((hub) => (
            <HubRow
              key={hub.id}
              hub={hub}
              probeState={probeStates[hub.id]}
              bundleState={bundleStates[hub.id]}
              statusState={statusStates[hub.id]}
              otaState={otaStates[hub.id]}
              logState={logStates[hub.id]}
              onProbe={() => onProbe(hub)}
              onDownloadBundle={() => onDownloadBundle(hub)}
              onLoadStatus={() => onLoadStatus(hub)}
              onCheckUpdate={() => onCheckUpdate(hub)}
              onApplyUpdate={() => onApplyUpdate(hub)}
              onLoadLogs={(sourceId) => onLoadLogs(hub, sourceId)}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function HubRow({
  hub,
  probeState,
  bundleState,
  statusState,
  otaState,
  logState,
  onProbe,
  onDownloadBundle,
  onLoadStatus,
  onCheckUpdate,
  onApplyUpdate,
  onLoadLogs
}: {
  hub: SupportHub;
  probeState?: ProbeState;
  bundleState?: BundleState;
  statusState?: StatusState;
  otaState?: OtaState;
  logState?: LogState;
  onProbe: () => void;
  onDownloadBundle: () => void;
  onLoadStatus: () => void;
  onCheckUpdate: () => void;
  onApplyUpdate: () => void;
  onLoadLogs: (sourceId?: string) => void;
}) {
  const result = probeState?.result;
  const otaBusy = Boolean(otaState?.checking || otaState?.updating);
  return (
    <div className="hubRow">
      <div className="hubMain">
        <div className="hubIcon">
          <Server size={18} />
        </div>
        <div className="hubText">
          <div className="hubTitle">{hub.name}</div>
          <div className="hubMeta">
            {hub.remoteEndpoint ? (
              <span>Remote {hub.remoteEndpoint.baseUrl}</span>
            ) : null}
            <span>Local {hub.endpoint.baseUrl}</span>
            {hub.serverInstanceId ? <span>{shortId(hub.serverInstanceId)}</span> : null}
            {hub.lastConnected ? <span>Seen {formatDateTime(hub.lastConnected)}</span> : null}
          </div>
        </div>
      </div>

      <div className="tokenStrip">
        <span className={hub.hasLegacyToken ? 'token ok' : 'token'}>
          <KeyRound size={14} />
          Legacy
        </span>
        <span className={hub.hasEncryptedToken ? 'token encrypted' : 'token'}>
          <KeyRound size={14} />
          Encrypted
        </span>
      </div>

      <ProbeBadge state={probeState} />

      <div className="hubActions">
        <button className="probeButton" type="button" onClick={onProbe} disabled={probeState?.loading}>
          {probeState?.loading ? <Loader2 className="spin" size={16} /> : <Wifi size={16} />}
          <span>Probe</span>
        </button>
        <button
          className="probeButton"
          type="button"
          onClick={onCheckUpdate}
          disabled={otaBusy}
        >
          {otaState?.checking ? <Loader2 className="spin" size={16} /> : <RefreshCw size={16} />}
          <span>Check</span>
        </button>
        <button
          className="probeButton"
          type="button"
          onClick={onApplyUpdate}
          disabled={otaBusy}
        >
          {otaState?.updating ? <Loader2 className="spin" size={16} /> : <Download size={16} />}
          <span>Update</span>
        </button>
        <button
          className="probeButton"
          type="button"
          onClick={onLoadStatus}
          disabled={statusState?.loading}
        >
          {statusState?.loading ? <Loader2 className="spin" size={16} /> : <Activity size={16} />}
          <span>Status</span>
        </button>
        <button
          className="probeButton"
          type="button"
          onClick={onDownloadBundle}
          disabled={bundleState?.loading}
        >
          {bundleState?.loading ? <Loader2 className="spin" size={16} /> : <Download size={16} />}
          <span>Bundle</span>
        </button>
        <button
          className="probeButton"
          type="button"
          onClick={() => onLoadLogs()}
          disabled={logState?.loading}
        >
          {logState?.loading ? <Loader2 className="spin" size={16} /> : <FileText size={16} />}
          <span>Logs</span>
        </button>
      </div>

      {result ? (
        <div className="inventory">
          {result.baseUrl ? <span>{result.route ?? 'route'} {result.baseUrl}</span> : null}
          {result.serverVersion ? <span>v{result.serverVersion}</span> : null}
          {result.checkedAt ? <span>Checked {formatDateTime(result.checkedAt)}</span> : null}
          {result.inventory ? (
            <>
              <span>{result.inventory.lights} lights</span>
              <span>{result.inventory.buttons} buttons</span>
              <span>{result.inventory.motionSensors} motion</span>
            </>
          ) : null}
        </div>
      ) : null}

      {probeState?.error || result?.message || bundleState?.error || otaState?.error ? (
        <div className="hubMessage">
          {otaState?.error ?? bundleState?.error ?? probeState?.error ?? result?.message}
        </div>
      ) : null}

      {otaState?.result ? (
        <div className="hubMessage success">
          {otaActionMessage(otaState.result)}
          {' via '}
          {otaState.result.route}
          {otaState.result.completedAt ? ` at ${formatDateTime(otaState.result.completedAt)}` : ''}
        </div>
      ) : null}

      {bundleState?.fileName ? (
        <div className="hubMessage success">
          Downloaded {bundleState.fileName}
          {bundleState.route ? ` from ${bundleState.route}` : ''}
          {bundleState.downloadedAt ? ` at ${formatDateTime(bundleState.downloadedAt)}` : ''}
        </div>
      ) : null}

      {statusState?.opened ? (
        <StatusPanel state={statusState} onRefresh={onLoadStatus} />
      ) : null}

      {logState?.opened ? (
        <LogPanel
          state={logState}
          onSourceChange={(sourceId) => onLoadLogs(sourceId)}
          onRefresh={() => onLoadLogs(logState.selectedSourceId)}
        />
      ) : null}
    </div>
  );
}

function StatusPanel({
  state,
  onRefresh
}: {
  state: StatusState;
  onRefresh: () => void;
}) {
  const result = state.result;
  const errors = Object.entries(result?.errors ?? {});
  return (
    <div className="statusPanel">
      <div className="logToolbar">
        <div className="logMeta">
          {result ? (
            <>
              <span>{result.route} {result.baseUrl}</span>
              <span>{formatDateTime(result.checkedAt)}</span>
            </>
          ) : (
            <span>Status</span>
          )}
        </div>
        <button className="probeButton" type="button" onClick={onRefresh} disabled={state.loading}>
          {state.loading ? <Loader2 className="spin" size={16} /> : <RefreshCw size={16} />}
          <span>Refresh</span>
        </button>
      </div>

      {state.error ? <div className="hubMessage">{state.error}</div> : null}

      {result ? (
        <div className="statusGrid">
          <StatusTile
            title="Health"
            status={String(result.health?.status ?? 'unknown')}
            rows={[
              ['Token', result.tokenAvailable ? 'available' : 'missing'],
              ['Encrypted', result.hasEncryptedToken ? 'yes' : 'no']
            ]}
          />
          <StatusTile
            title="Runtime"
            status={result.state?.serverVersion ?? 'unavailable'}
            rows={[
              ['Platform', compactJoin([result.state?.platformType, result.state?.platformContext], ' / ')],
              ['Instance', result.state?.serverInstanceId],
              ['Nodes', result.state?.nodeCount],
              ['Hubs', result.state?.hubCount],
              ['Last tick', formatEpochMs(result.state?.lastTickEpochMs)],
              ['Mode', result.state?.activeMode],
              ['Runtime', result.state?.lightRuntime]
            ]}
          />
          <StatusTile
            title="Remote Access"
            status={statusBool(result.remoteAccess?.connector_healthy)}
            rows={[
              ['Enabled', yesNo(result.remoteAccess?.enabled)],
              ['Configured', yesNo(result.remoteAccess?.configured)],
              ['Service', yesNo(result.remoteAccess?.service_running)],
              ['Connections', result.remoteAccess?.registered_connections],
              ['Hostname', result.remoteAccess?.hostname]
            ]}
          />
          <StatusTile
            title="Auth"
            status={yesNo(result.auth?.requires_auth) ?? 'unknown'}
            rows={[
              ['Owner', yesNo(result.auth?.owner_configured)],
              ['Tokens', result.auth?.token_count],
              ['Remote request', yesNo(result.auth?.via_remote_access)]
            ]}
          />
          <StatusTile
            title="OTA"
            status={stringValue(result.ota?.state)}
            rows={[
              ['Current', result.ota?.current_version],
              ['Target', result.ota?.target_version],
              ['Latest', result.ota?.latest_version],
              ['Update', yesNo(result.ota?.update_available)],
              ['Error', result.ota?.last_error]
            ]}
          />
          {result.state?.inventory ? (
            <StatusTile
              title="Inventory"
              status={`${result.state.inventory.total} devices`}
              rows={[
                ['Lights', result.state.inventory.lights],
                ['Buttons', result.state.inventory.buttons],
                ['Motion', result.state.inventory.motionSensors],
                ['Other', result.state.inventory.otherDevices]
              ]}
            />
          ) : null}
        </div>
      ) : null}

      {errors.length > 0 ? (
        <div className="statusErrors">
          {errors.map(([key, value]) => (
            <div key={key}>
              <strong>{key}</strong>: {value}
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}

function StatusTile({
  title,
  status,
  rows
}: {
  title: string;
  status: string;
  rows: Array<[string, unknown]>;
}) {
  return (
    <div className="statusTile">
      <div className="statusTileHeader">
        <span>{title}</span>
        <strong>{status}</strong>
      </div>
      <dl>
        {rows
          .filter(([, value]) => value !== undefined && value !== null && value !== '')
          .map(([label, value]) => (
            <div key={label}>
              <dt>{label}</dt>
              <dd>{stringValue(value)}</dd>
            </div>
          ))}
      </dl>
    </div>
  );
}

function LogPanel({
  state,
  onSourceChange,
  onRefresh
}: {
  state: LogState;
  onSourceChange: (sourceId: string) => void;
  onRefresh: () => void;
}) {
  const sources = state.sources ?? [];
  return (
    <div className="logPanel">
      <div className="logToolbar">
        <div className="logMeta">
          {state.tail ? (
            <>
              <span>{state.tail.route} {state.tail.baseUrl}</span>
              <span>{state.tail.returnedLines} lines</span>
              <span>{formatDateTime(state.tail.fetchedAt)}</span>
            </>
          ) : (
            <span>Logs</span>
          )}
        </div>
        <div className="logControls">
          <select
            className="logSelect"
            value={state.selectedSourceId ?? ''}
            disabled={state.loading || sources.length === 0}
            onChange={(event) => onSourceChange(event.target.value)}
          >
            {sources.length === 0 ? (
              <option value="">No sources</option>
            ) : (
              sources.map((source) => (
                <option key={source.id} value={source.id}>
                  {source.fileName}
                </option>
              ))
            )}
          </select>
          <button
            className="probeButton"
            type="button"
            onClick={onRefresh}
            disabled={state.loading || !state.selectedSourceId}
          >
            {state.loading ? <Loader2 className="spin" size={16} /> : <RefreshCw size={16} />}
            <span>Refresh</span>
          </button>
        </div>
      </div>

      {state.error ? <div className="hubMessage">{state.error}</div> : null}

      {state.tail ? (
        <div className="logOutput" aria-label={`${state.tail.source.fileName} tail`}>
          {state.tail.lines.length === 0 ? (
            <div className="logEmpty">No lines returned.</div>
          ) : (
            state.tail.lines.map((line) => (
              <div className="logLine" key={`${line.source}-${line.lineNumber}`}>
                <span className="logLineNumber">{line.lineNumber}</span>
                <span className="logLineText">{line.text}</span>
              </div>
            ))
          )}
        </div>
      ) : null}
    </div>
  );
}

function ProbeBadge({ state }: { state?: ProbeState }) {
  if (state?.loading) {
    return (
      <span className="statusBadge checking">
        <Loader2 className="spin" size={14} />
        Checking
      </span>
    );
  }
  if (state?.error) {
    return (
      <span className="statusBadge offline">
        <AlertTriangle size={14} />
        Error
      </span>
    );
  }
  const result = state?.result;
  if (!result) return <span className="statusBadge idle">Not checked</span>;
  if (result.status === 'online') {
    return (
      <span className="statusBadge online">
        <Wifi size={14} />
        Online {result.route ? `(${result.route})` : ''}
      </span>
    );
  }
  if (result.status === 'auth_required') {
    return (
      <span className="statusBadge auth">
        <KeyRound size={14} />
        Auth required
      </span>
    );
  }
  return (
    <span className="statusBadge offline">
      <AlertTriangle size={14} />
      Offline
    </span>
  );
}

function otaActionMessage(action: DeviceOtaAction): string {
  const message = nonEmptyString(action.result.message);
  if (message) return message;

  const latest = nonEmptyString(action.result.latest_version);
  const current = nonEmptyString(action.result.current_version);
  if (action.action === 'check') {
    if (action.result.update_available === true && latest) {
      return `Update available: v${latest}`;
    }
    if (action.result.update_available === false) {
      return current ? `Already current: v${current}` : 'Already current';
    }
    return 'Update check completed';
  }

  return latest ? `Update requested: v${latest}` : 'Update requested';
}

function nonEmptyString(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim().length > 0
    ? value.trim()
    : undefined;
}

function flattenHomes(snapshot: SupportSnapshot | null): HomeListItem[] {
  if (!snapshot) return [];
  return snapshot.customers.flatMap((customer) =>
    customer.homes.map((entry) => {
      const searchable = [
        customer.customerLabel,
        customer.customerEmail,
        customer.customerName,
        customer.ownerId,
        entry.home.id,
        entry.home.name,
        entry.home.locationCity,
        entry.home.timezone,
        ...entry.hubs.flatMap((hub) => [
          hub.id,
          hub.name,
          hub.endpoint.host,
          hub.remoteEndpoint?.host,
          hub.serverInstanceId
        ])
      ];
      return {
        customer,
        home: entry.home,
        hubs: entry.hubs,
        searchText: searchable.filter(Boolean).join(' ').toLowerCase()
      };
    })
  );
}

function firstHomeId(snapshot: SupportSnapshot | null): string | null {
  return snapshot?.customers[0]?.homes[0]?.home.id ?? null;
}

function preferredLogSourceId(sources: DeviceLogSource[]): string | undefined {
  return (
    sources.find((source) => source.id === 'rhythm-server.log') ??
    sources.find((source) => source.fileName === 'rhythm-server.log') ??
    sources[0]
  )?.id;
}

function compactJoin(values: Array<string | undefined>, separator: string): string | undefined {
  const present = values.filter((value): value is string => Boolean(value));
  return present.length === 0 ? undefined : present.join(separator);
}

function yesNo(value: unknown): string | undefined {
  if (typeof value !== 'boolean') return undefined;
  return value ? 'yes' : 'no';
}

function statusBool(value: unknown): string {
  if (typeof value !== 'boolean') return 'unknown';
  return value ? 'healthy' : 'unhealthy';
}

function stringValue(value: unknown): string {
  if (value === undefined || value === null || value === '') return 'unknown';
  if (typeof value === 'number') return new Intl.NumberFormat().format(value);
  if (typeof value === 'boolean') return value ? 'yes' : 'no';
  return String(value);
}

function formatEpochMs(value: number | undefined): string | undefined {
  if (typeof value !== 'number' || value <= 0) return undefined;
  return formatDateTime(new Date(value).toISOString());
}

function shortId(value: string): string {
  return value.length <= 10 ? value : `${value.slice(0, 10)}...`;
}

function formatDateTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit'
  }).format(date);
}

function triggerBrowserDownload(blob: Blob, fileName: string) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = fileName;
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(url);
}
