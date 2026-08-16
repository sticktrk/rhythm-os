import { useCallback, useEffect, useRef, useState } from 'react';
import { Link, NavLink, Outlet, useParams } from 'react-router-dom';
import {
  Activity,
  AlertTriangle,
  ArrowLeft,
  CloudSun,
  Cpu,
  Gauge,
  Globe,
  History,
  KeyRound,
  Lightbulb,
  Loader2,
  Moon,
  Network,
  Palette,
  RefreshCw,
  Settings,
  ShieldCheck,
  Terminal,
  ToggleLeft,
  Wifi
} from 'lucide-react';

import { probeHub } from '../../api';
import { ConfirmProvider } from '../../components/ui/ConfirmDialog';
import { DeviceClient } from '../../device/client';
import { getTriageCount } from '../../device/topology';
import { asNumber, asRecord } from '../../device/values';
import { usePolling } from '../../hooks/usePolling';
import { HubProvider } from '../../state/HubContext';
import { useSession } from '../../state/SessionContext';
import { useSnapshot } from '../../state/SnapshotContext';
import type { ProbeResult } from '../../types';

const NAV_ITEMS: Array<{
  to: string;
  label: string;
  icon: React.ReactNode;
}> = [
  { to: 'overview', label: 'Overview', icon: <Gauge size={17} /> },
  { to: 'nodes', label: 'Rooms & Nodes', icon: <Lightbulb size={17} /> },
  { to: 'profiles', label: 'Profiles & Curves', icon: <Activity size={17} /> },
  { to: 'scenes', label: 'Scenes', icon: <Palette size={17} /> },
  { to: 'modes', label: 'Modes & Transitions', icon: <Moon size={17} /> },
  { to: 'inputs', label: 'Inputs & Controls', icon: <ToggleLeft size={17} /> },
  { to: 'topology', label: 'Devices & Topology', icon: <Network size={17} /> },
  { to: 'integrations', label: 'Hubs & Devices', icon: <Cpu size={17} /> },
  { to: 'environment', label: 'Environment', icon: <CloudSun size={17} /> },
  { to: 'history', label: 'Activity & History', icon: <History size={17} /> },
  { to: 'remote', label: 'Network & Remote', icon: <Globe size={17} /> },
  { to: 'security', label: 'Security', icon: <ShieldCheck size={17} /> },
  { to: 'system', label: 'System', icon: <Settings size={17} /> },
  { to: 'console', label: 'Console', icon: <Terminal size={17} /> }
];

export default function HubConsoleLayout() {
  const params = useParams<{ hubId: string }>();
  const hubId = params.hubId ?? '';
  const { accessToken } = useSession();
  const { snapshot, loading, error, findHub, refresh } = useSnapshot();

  const [probe, setProbe] = useState<ProbeResult | null>(null);
  const [probing, setProbing] = useState(false);
  const probedRef = useRef<string | null>(null);

  const lookup = findHub(hubId);

  const triageQuery = usePolling(
    useCallback(() => {
      const client = new DeviceClient(accessToken, hubId);
      return getTriageCount(client).catch(() => ({}));
    }, [accessToken, hubId]),
    { intervalMs: 60_000, enabled: Boolean(lookup) }
  );
  const triageCount =
    asNumber(asRecord(triageQuery.data).count) ??
    asNumber(triageQuery.data) ??
    0;

  const runProbe = useCallback(() => {
    if (!hubId) return;
    setProbing(true);
    probeHub(accessToken, hubId)
      .then((result) => setProbe(result))
      .catch(() => setProbe(null))
      .finally(() => setProbing(false));
  }, [accessToken, hubId]);

  useEffect(() => {
    if (!lookup) return;
    if (probedRef.current === hubId) return;
    probedRef.current = hubId;
    runProbe();
  }, [lookup, hubId, runProbe]);

  if (!lookup) {
    return (
      <div className="consoleShell">
        <div className="consoleMissing">
          {loading || !snapshot ? (
            <>
              <Loader2 className="spin" size={22} />
              <span>Loading hub…</span>
            </>
          ) : (
            <>
              <AlertTriangle size={22} />
              <span>
                {error ?? `No Light Box with id ${hubId} is visible to support.`}
              </span>
              <button
                className="consoleButton"
                type="button"
                onClick={() => void refresh()}
              >
                <RefreshCw size={16} />
                <span>Retry</span>
              </button>
              <Link className="consoleButton" to="/">
                <ArrowLeft size={16} />
                <span>Back to dashboard</span>
              </Link>
            </>
          )}
        </div>
      </div>
    );
  }

  const { hub, home, customer } = lookup;

  return (
    <HubProvider
      value={{ hubId, hub, home, customer, probe, probing, reprobe: runProbe }}
    >
      <ConfirmProvider>
      <div className="consoleShell">
        <aside className="consoleNav" aria-label="Device console sections">
          <Link className="consoleBack" to="/">
            <ArrowLeft size={16} />
            <span>Dashboard</span>
          </Link>
          <div className="consoleHubCard">
            <div className="consoleHubName">{hub.name}</div>
            <div className="consoleHubMeta">
              <span>{home.name}</span>
              <span>{customer.customerLabel}</span>
            </div>
            <div className="consoleHubBadges">
              <ConsoleProbeBadge probe={probe} probing={probing} />
              {hub.hasLegacyToken || hub.hasEncryptedToken ? (
                <span className="consoleToken">
                  <KeyRound size={12} />
                  {hub.hasLegacyToken ? 'legacy' : 'encrypted'}
                </span>
              ) : null}
            </div>
          </div>
          <nav className="consoleNavList">
            {NAV_ITEMS.map((item) => (
              <NavLink
                key={item.to}
                to={item.to}
                className={({ isActive }) =>
                  `consoleNavItem${isActive ? ' active' : ''}`
                }
              >
                {item.icon}
                <span>{item.label}</span>
                {item.to === 'topology' && triageCount > 0 ? (
                  <span className="navBadge">{triageCount}</span>
                ) : null}
              </NavLink>
            ))}
          </nav>
        </aside>
        {/* Keyed by hub so mount-only pollers refetch when the :hubId param
            changes without the route unmounting. */}
        <main className="consoleMain" key={hubId}>
          <Outlet />
        </main>
      </div>
      </ConfirmProvider>
    </HubProvider>
  );
}

function ConsoleProbeBadge({
  probe,
  probing
}: {
  probe: ProbeResult | null;
  probing: boolean;
}) {
  if (probing) {
    return (
      <span className="consoleStatus checking">
        <Loader2 className="spin" size={12} />
        Checking
      </span>
    );
  }
  if (!probe) {
    return <span className="consoleStatus idle">Unknown</span>;
  }
  if (probe.status === 'online') {
    return (
      <span className="consoleStatus online">
        <Wifi size={12} />
        Online{probe.route ? ` · ${probe.route}` : ''}
        {probe.serverVersion ? ` · v${probe.serverVersion}` : ''}
      </span>
    );
  }
  if (probe.status === 'auth_required') {
    return (
      <span className="consoleStatus auth">
        <KeyRound size={12} />
        Auth required
      </span>
    );
  }
  return (
    <span className="consoleStatus offline">
      <AlertTriangle size={12} />
      Offline
    </span>
  );
}
