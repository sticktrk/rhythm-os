import { useEffect, useMemo, useState } from 'react';
import { Link } from 'react-router-dom';
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
  fetchHubStatus,
  fetchHubLogSources,
  fetchHubLogTail,
  probeHub
} from '../../api';
import {
  LogPanel,
  preferredLogSourceId,
  type LogState
} from '../../components/panels/LogPanel';
import {
  StatusPanel,
  type StatusState
} from '../../components/panels/StatusPanel';
import {
  errorMessage,
  formatDateTime,
  nonEmptyString,
  shortId,
  triggerBrowserDownload
} from '../../lib/format';
import { useSession } from '../../state/SessionContext';
import { useSnapshot } from '../../state/SnapshotContext';
import type {
  AdminApiHealth,
  AdminApiReadiness,
  DeviceOtaAction,
  HomeListItem,
  ProbeResult,
  SupportHub,
  SupportSnapshot
} from '../../types';

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

type OtaState = {
  checking: boolean;
  updating: boolean;
  error?: string;
  result?: DeviceOtaAction;
};

export default function DashboardPage() {
  const { accessToken, signOut } = useSession();
  const {
    snapshot,
    me,
    health,
    readiness,
    loading: loadingSnapshot,
    error: loadError,
    refresh
  } = useSnapshot();

  const [query, setQuery] = useState('');
  const [selectedHomeId, setSelectedHomeId] = useState<string | null>(null);
  const [probeStates, setProbeStates] = useState<Record<string, ProbeState>>({});
  const [bundleStates, setBundleStates] = useState<Record<string, BundleState>>({});
  const [logStates, setLogStates] = useState<Record<string, LogState>>({});
  const [statusStates, setStatusStates] = useState<Record<string, StatusState>>({});
  const [otaStates, setOtaStates] = useState<Record<string, OtaState>>({});

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

  async function handleProbe(hub: SupportHub) {
    setProbeStates((current) => ({
      ...current,
      [hub.id]: { ...current[hub.id], loading: true, error: undefined }
    }));
    try {
      const result = await probeHub(accessToken, hub.id);
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
          error: errorMessage(error)
        }
      }));
    }
  }

  async function handleDownloadBundle(hub: SupportHub) {
    setBundleStates((current) => ({
      ...current,
      [hub.id]: { ...current[hub.id], loading: true, error: undefined }
    }));
    try {
      const bundle = await downloadDebugBundle(accessToken, hub.id);
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
          error: errorMessage(error)
        }
      }));
    }
  }

  async function handleLoadStatus(hub: SupportHub) {
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
      const result = await fetchHubStatus(accessToken, hub.id);
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
          error: errorMessage(error)
        }
      }));
    }
  }

  async function handleCheckUpdate(hub: SupportHub) {
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
      const result = await checkHubUpdate(accessToken, hub.id);
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
          error: errorMessage(error)
        }
      }));
    }
  }

  async function handleApplyUpdate(hub: SupportHub) {
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
      const result = await applyHubUpdate(accessToken, hub.id);
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
          error: errorMessage(error)
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
        (await fetchHubLogSources(accessToken, hub.id)).sources;
      const selectedSourceId =
        sourceId ?? existing?.selectedSourceId ?? preferredLogSourceId(sources);
      const tail = selectedSourceId
        ? await fetchHubLogTail(accessToken, hub.id, selectedSourceId)
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
          error: errorMessage(error)
        }
      }));
    }
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
          <button className="iconButton" type="button" onClick={() => void refresh()}>
            <RefreshCw size={18} />
            <span>Refresh</span>
          </button>
          <button className="iconButton danger" type="button" onClick={signOut}>
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
        <Link className="probeButton enter" to={`/hubs/${encodeURIComponent(hub.id)}/overview`}>
          <Server size={16} />
          <span>Enter</span>
        </Link>
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
