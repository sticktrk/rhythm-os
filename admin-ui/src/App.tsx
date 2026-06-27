import { FormEvent, useCallback, useEffect, useMemo, useState } from 'react';
import type { Session } from '@supabase/supabase-js';
import {
  Activity,
  AlertTriangle,
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

import { fetchMe, fetchSupportSnapshot, probeHub } from './api';
import { isSupabaseConfigured, supabase } from './supabaseClient';
import type {
  HomeListItem,
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

export default function App() {
  const [session, setSession] = useState<Session | null>(null);
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [authBusy, setAuthBusy] = useState(true);
  const [authError, setAuthError] = useState<string | null>(null);
  const [me, setMe] = useState<MeResponse | null>(null);
  const [snapshot, setSnapshot] = useState<SupportSnapshot | null>(null);
  const [loadingSnapshot, setLoadingSnapshot] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [selectedHomeId, setSelectedHomeId] = useState<string | null>(null);
  const [probeStates, setProbeStates] = useState<Record<string, ProbeState>>(
    {}
  );

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
      setMe(null);
      setSnapshot(null);
      setSelectedHomeId(null);
      setProbeStates({});
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
      const [nextMe, nextSnapshot] = await Promise.all([
        fetchMe(session.access_token),
        fetchSupportSnapshot(session.access_token)
      ]);
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
              onProbe={handleProbe}
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
  onProbe
}: {
  item: HomeListItem;
  probeStates: Record<string, ProbeState>;
  onProbe: (hub: SupportHub) => void;
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
              onProbe={() => onProbe(hub)}
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
  onProbe
}: {
  hub: SupportHub;
  probeState?: ProbeState;
  onProbe: () => void;
}) {
  const result = probeState?.result;
  return (
    <div className="hubRow">
      <div className="hubMain">
        <div className="hubIcon">
          <Server size={18} />
        </div>
        <div className="hubText">
          <div className="hubTitle">{hub.name}</div>
          <div className="hubMeta">
            <span>{hub.remoteEndpoint ? hub.remoteEndpoint.baseUrl : hub.endpoint.baseUrl}</span>
            {hub.serverInstanceId ? <span>{shortId(hub.serverInstanceId)}</span> : null}
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

      <button className="probeButton" type="button" onClick={onProbe} disabled={probeState?.loading}>
        {probeState?.loading ? <Loader2 className="spin" size={16} /> : <Wifi size={16} />}
        <span>Probe</span>
      </button>

      {result?.inventory ? (
        <div className="inventory">
          <span>{result.inventory.lights} lights</span>
          <span>{result.inventory.buttons} buttons</span>
          <span>{result.inventory.motionSensors} motion</span>
        </div>
      ) : null}

      {probeState?.error || result?.message ? (
        <div className="hubMessage">{probeState?.error ?? result?.message}</div>
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

function shortId(value: string): string {
  return value.length <= 10 ? value : `${value.slice(0, 10)}...`;
}
