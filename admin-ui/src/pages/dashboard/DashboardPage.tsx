import { useMemo, useState } from 'react';
import { Link } from 'react-router-dom';
import {
  AlertTriangle,
  ChevronRight,
  Home,
  Loader2,
  LogOut,
  Mail,
  MapPin,
  RefreshCw,
  Search,
  ShieldCheck
} from 'lucide-react';

import { shortId } from '../../lib/format';
import {
  flattenHomes,
  type HomeDirectoryItem
} from '../../lib/supportHomes';
import { useSession } from '../../state/SessionContext';
import { useSnapshot } from '../../state/SnapshotContext';

export default function DashboardPage() {
  const { signOut } = useSession();
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

  return (
    <HomeDirectoryView
      homes={homes}
      filteredHomes={filteredHomes}
      loading={loadingSnapshot}
      loadError={loadError}
      query={query}
      role={me?.staffStatus.role ?? 'staff'}
      onQueryChange={setQuery}
      onRefresh={() => void refresh()}
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
              <HomeDirectoryRow item={item} key={item.home.id} />
            ))
          )}
        </div>
      </main>
    </div>
  );
}

function HomeDirectoryRow({ item }: { item: HomeDirectoryItem }) {
  const { home, email } = item;
  return (
    <Link className="homeDirectoryRow" to={`/homes/${home.id}`}>
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
      <div className="homeDirectoryId">Home {shortId(home.id)}</div>
      <ChevronRight
        className="homeDirectoryChevron"
        aria-hidden="true"
        size={18}
      />
    </Link>
  );
}
