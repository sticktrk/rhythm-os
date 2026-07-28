import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Link } from 'react-router-dom';
import {
  AlertTriangle,
  ChevronRight,
  Home,
  KeyRound,
  Loader2,
  LogOut,
  Mail,
  MapPin,
  RefreshCw,
  Search,
  ShieldCheck,
  Wifi
} from 'lucide-react';

import { probeHub } from '../../api';
import { errorMessage, shortId } from '../../lib/format';
import {
  flattenHomes,
  type HomeDirectoryItem
} from '../../lib/supportHomes';
import { useSession } from '../../state/SessionContext';
import { useSnapshot } from '../../state/SnapshotContext';
import type { ProbeResult } from '../../types';

const homeDirectoryProbeConcurrency = 3;

export default function DashboardPage() {
  const { accessToken, signOut } = useSession();
  const {
    snapshot,
    me,
    loading: loadingSnapshot,
    error: loadError,
    refresh
  } = useSnapshot();
  const [query, setQuery] = useState('');

  const homes = useMemo(() => flattenHomes(snapshot), [snapshot]);
  const filteredHomes = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return homes;
    return homes.filter((item) => item.searchText.includes(needle));
  }, [homes, query]);
  const [probes, setProbes] = useState<Record<string, ProbeResult>>({});
  const [probing, setProbing] = useState(false);
  const sweepGeneration = useRef(0);
  const automaticallyProbed = useRef<string | null>(null);
  const homesKey = homes
    .map((item) => `${item.home.id}:${item.hubs.map((hub) => hub.id).join(',')}`)
    .join('|');

  const runProbeSweep = useCallback(() => {
    const targets = homes.flatMap((item) =>
      item.hubs.map((hub) => ({ hub }))
    );
    if (targets.length === 0) return;

    const generation = ++sweepGeneration.current;
    setProbes({});
    setProbing(true);

    void (async () => {
      let nextTargetIndex = 0;
      const probeNextHub = async () => {
        while (nextTargetIndex < targets.length) {
          const target = targets[nextTargetIndex++];
          let result: ProbeResult;
          try {
            result = await probeHub(accessToken, target.hub.id);
          } catch (error) {
            result = {
              hubId: target.hub.id,
              status: 'offline',
              checkedAt: new Date().toISOString(),
              tokenAvailable: false,
              hasEncryptedToken: target.hub.hasEncryptedToken,
              message: errorMessage(error)
            };
          }
          if (sweepGeneration.current !== generation) return;
          setProbes((current) => ({ ...current, [target.hub.id]: result }));
        }
      };

      await Promise.all(
        Array.from(
          { length: Math.min(homeDirectoryProbeConcurrency, targets.length) },
          probeNextHub
        )
      );
      if (sweepGeneration.current === generation) setProbing(false);
    })();
  }, [accessToken, homes]);

  useEffect(() => {
    if (homes.length === 0 || automaticallyProbed.current === homesKey) return;
    automaticallyProbed.current = homesKey;
    runProbeSweep();
  }, [homes.length, homesKey, runProbeSweep]);

  useEffect(() => {
    const interval = window.setInterval(runProbeSweep, 60_000);
    return () => window.clearInterval(interval);
  }, [runProbeSweep]);

  const refreshHomes = useCallback(() => {
    void refresh();
    runProbeSweep();
  }, [refresh, runProbeSweep]);

  return (
    <HomeDirectoryView
      homes={homes}
      filteredHomes={filteredHomes}
      loading={loadingSnapshot}
      loadError={loadError}
      query={query}
      role={me?.staffStatus.role ?? 'staff'}
      probes={probes}
      probing={probing}
      onQueryChange={setQuery}
      onRefresh={refreshHomes}
      onSignOut={signOut}
    />
  );
}

export function HomeDirectoryView({
  homes,
  filteredHomes,
  loading,
  loadError,
  query,
  role,
  probes,
  probing,
  onQueryChange,
  onRefresh,
  onSignOut
}: {
  homes: HomeDirectoryItem[];
  filteredHomes: HomeDirectoryItem[];
  loading: boolean;
  loadError: string | null;
  query: string;
  role: string;
  probes: Record<string, ProbeResult>;
  probing: boolean;
  onQueryChange: (value: string) => void;
  onRefresh: () => void;
  onSignOut: () => void;
}) {
  return (
    <div className="appShell">
      <header className="topbar">
        <div>
          <div className="eyebrow">Rhythm Staff</div>
          <h1>Homes</h1>
        </div>
        <div className="topbarActions">
          <div className="staffBadge">
            <ShieldCheck size={16} />
            <span>{role}</span>
          </div>
          <button
            className="iconButton"
            type="button"
            onClick={onRefresh}
          >
            <RefreshCw size={18} />
            <span>Refresh</span>
          </button>
          <button
            className="iconButton danger"
            type="button"
            onClick={onSignOut}
          >
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

      <main className="homeDirectory" aria-label="Homes">
        <div className="homeDirectoryHeader">
          <div>
            <div className="homeDirectoryTitle">All homes</div>
            <div className="homeDirectoryCount">
              {filteredHomes.length === homes.length
                ? `${homes.length} ${homes.length === 1 ? 'home' : 'homes'}`
                : `${filteredHomes.length} of ${homes.length} homes`}
            </div>
          </div>
          <label className="searchBox homeDirectorySearch">
            <Search size={18} />
            <span className="srOnly">Search homes</span>
            <input
              value={query}
              onChange={(event) => onQueryChange(event.target.value)}
              placeholder="Search homes or email"
            />
          </label>
        </div>

        <div className="homeDirectoryList" aria-live="polite">
          {loading ? (
            <div className="listState">
              <Loader2 className="spin" size={18} />
              <span>Loading homes</span>
            </div>
          ) : filteredHomes.length === 0 ? (
            <div className="listState">
              {homes.length === 0
                ? 'No homes are available.'
                : 'No homes match this search.'}
            </div>
          ) : (
            filteredHomes.map((item) => (
              <HomeDirectoryRow
                item={item}
                key={item.home.id}
                probes={probes}
                probing={probing}
              />
            ))
          )}
        </div>
      </main>
    </div>
  );
}

function HomeDirectoryRow({
  item,
  probes,
  probing
}: {
  item: HomeDirectoryItem;
  probes: Record<string, ProbeResult>;
  probing: boolean;
}) {
  const { home, hubs, email } = item;
  const destination =
    hubs.length === 1 ? `/hubs/${hubs[0].id}/overview` : `/homes/${home.id}`;
  return (
    <Link className="homeDirectoryRow" to={destination}>
      <div className="homeDirectoryIcon" aria-hidden="true">
        <Home size={20} />
      </div>
      <div className="homeDirectoryMain">
        <div className="homeDirectoryName">{home.name}</div>
        <div className="homeDirectoryMeta">
          {email ? (
            <span>
              <Mail size={15} />
              {email}
            </span>
          ) : null}
          {home.locationCity ? (
            <span>
              <MapPin size={15} />
              {home.locationCity}
            </span>
          ) : null}
          {home.timezone ? <span>{home.timezone}</span> : null}
        </div>
      </div>
      <HomeProbeSummary hubs={hubs} probes={probes} probing={probing} />
      <div className="homeDirectoryId">Home {shortId(home.id)}</div>
      <ChevronRight
        className="homeDirectoryChevron"
        aria-hidden="true"
        size={18}
      />
    </Link>
  );
}

function HomeProbeSummary({
  hubs,
  probes,
  probing
}: {
  hubs: HomeDirectoryItem['hubs'];
  probes: Record<string, ProbeResult>;
  probing: boolean;
}) {
  const results = hubs
    .map((hub) => probes[hub.id])
    .filter((result): result is ProbeResult => result !== undefined);
  const online = results.filter((result) => result.status === 'online').length;
  const authRequired = results.filter(
    (result) => result.status === 'auth_required'
  ).length;
  const versions = [
    ...new Set(
      results
        .filter((result) => result.status === 'online')
        .map((result) => result.serverVersion)
        .filter((version): version is string => Boolean(version))
    )
  ];

  if (hubs.length === 0) {
    return <span className="homeDirectoryStatus unknown">No hubs</span>;
  }
  if (probing && results.length < hubs.length) {
    return <span className="homeDirectoryStatus checking">Checking hubs</span>;
  }
  if (results.length === 0) {
    return <span className="homeDirectoryStatus unknown">Unknown</span>;
  }
  if (online === hubs.length) {
    return (
      <span className="homeDirectoryStatus online">
        <Wifi size={13} />
        {onlineStatusLabel(hubs.length, online, versions)}
      </span>
    );
  }
  if (authRequired > 0 && authRequired + online === hubs.length) {
    return (
      <span className="homeDirectoryStatus auth">
        <KeyRound size={13} />
        {authRequired === hubs.length
          ? 'Auth required'
          : `${online}/${hubs.length} online`}
      </span>
    );
  }
  return (
    <span className="homeDirectoryStatus offline">
      <AlertTriangle size={13} />
      {online > 0
        ? onlineStatusLabel(hubs.length, online, versions)
        : 'Offline'}
    </span>
  );
}

function onlineStatusLabel(
  hubCount: number,
  onlineCount: number,
  versions: string[]
): string {
  const status =
    hubCount === 1 ? 'Online' : `${onlineCount}/${hubCount} online`;
  if (versions.length === 1) return `${status} · v${versions[0]}`;
  if (versions.length > 1) return `${status} · ${versions.length} versions`;
  return status;
}
