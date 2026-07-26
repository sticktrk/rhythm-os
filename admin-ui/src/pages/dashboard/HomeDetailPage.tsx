import { Link, useParams } from 'react-router-dom';
import {
  AlertTriangle,
  ArrowLeft,
  ChevronRight,
  Clock3,
  Cloud,
  Home,
  KeyRound,
  Loader2,
  LogOut,
  Mail,
  MapPin,
  RefreshCw,
  Server,
  ShieldCheck,
  Wifi
} from 'lucide-react';

import { formatDateTime, shortId } from '../../lib/format';
import { findHome } from '../../lib/supportHomes';
import { useSession } from '../../state/SessionContext';
import { useSnapshot } from '../../state/SnapshotContext';
import type { SupportHub } from '../../types';

export default function HomeDetailPage() {
  const { homeId } = useParams<{ homeId: string }>();
  const { signOut } = useSession();
  const { snapshot, me, error, refresh } = useSnapshot();
  const lookup = findHome(snapshot, homeId);
  const awaitingSnapshot = snapshot === null && error === null;

  return (
    <div className="appShell">
      <header className="topbar">
        <div>
          <div className="eyebrow">Rhythm Staff</div>
          <h1>{lookup?.home.name ?? 'Home'}</h1>
        </div>
        <div className="topbarActions">
          <div className="staffBadge">
            <ShieldCheck size={16} />
            <span>{me?.staffStatus.role ?? 'staff'}</span>
          </div>
          <button
            className="iconButton"
            type="button"
            onClick={() => void refresh()}
          >
            <RefreshCw size={18} />
            <span>Refresh</span>
          </button>
          <button className="iconButton danger" type="button" onClick={signOut}>
            <LogOut size={18} />
            <span>Sign out</span>
          </button>
        </div>
      </header>

      {error ? (
        <div className="notice error">
          <AlertTriangle size={18} />
          <span>{error}</span>
        </div>
      ) : null}

      <main className="homeDrilldown">
        <Link className="homeDrilldownBack" to="/">
          <ArrowLeft size={16} />
          All homes
        </Link>

        {awaitingSnapshot ? (
          <div className="homeDrilldownState">
            <Loader2 className="spin" size={20} />
            <span>Loading home</span>
          </div>
        ) : lookup ? (
          <>
            <section className="homeDrilldownHero">
              <div className="homeDrilldownIcon" aria-hidden="true">
                <Home size={24} />
              </div>
              <div className="homeDrilldownHeroMain">
                <h2>{lookup.home.name}</h2>
                <div className="homeDrilldownMeta">
                  {lookup.email ? (
                    <span>
                      <Mail size={15} />
                      {lookup.email}
                    </span>
                  ) : null}
                  {lookup.home.locationCity ? (
                    <span>
                      <MapPin size={15} />
                      {lookup.home.locationCity}
                    </span>
                  ) : null}
                  {lookup.home.timezone ? (
                    <span>{lookup.home.timezone}</span>
                  ) : null}
                </div>
              </div>
              <div className="homeDrilldownId">
                Home {shortId(lookup.home.id)}
              </div>
            </section>

            <section className="homeHubSection" aria-labelledby="home-hubs">
              <div className="homeHubSectionHeader">
                <div>
                  <h2 id="home-hubs">Hubs</h2>
                  <p>
                    {lookup.hubs.length}{' '}
                    {lookup.hubs.length === 1 ? 'hub' : 'hubs'}
                  </p>
                </div>
              </div>

              {lookup.hubs.length === 0 ? (
                <div className="homeDrilldownState">
                  No hubs are synced for this Home.
                </div>
              ) : (
                <div className="homeHubList">
                  {lookup.hubs.map((hub) => (
                    <HomeHubCard hub={hub} key={hub.id} />
                  ))}
                </div>
              )}
            </section>
          </>
        ) : (
          <div className="homeDrilldownState">
            <Home size={26} />
            <strong>Home not found</strong>
            <span>
              It may have been removed or may no longer be visible to this
              staff account.
            </span>
          </div>
        )}
      </main>
    </div>
  );
}

function HomeHubCard({ hub }: { hub: SupportHub }) {
  const hasSupportAccess = hub.hasEncryptedToken || hub.hasLegacyToken;
  return (
    <article className="homeHubCard">
      <div className="homeHubIcon" aria-hidden="true">
        <Server size={21} />
      </div>
      <div className="homeHubMain">
        <div className="homeHubTitleRow">
          <h3>{hub.name}</h3>
          <span
            className={`homeHubState ${hub.enabled ? 'enabled' : 'disabled'}`}
          >
            {hub.enabled ? 'Enabled' : 'Disabled'}
          </span>
        </div>
        <div className="homeHubMeta">
          <span>
            <Wifi size={14} />
            {hub.endpoint.baseUrl}
          </span>
          {hub.remoteEndpoint ? (
            <span>
              <Cloud size={14} />
              Remote configured
            </span>
          ) : null}
          <span>
            <KeyRound size={14} />
            {hasSupportAccess ? 'Support access saved' : 'No support access'}
          </span>
          {hub.lastConnected ? (
            <span>
              <Clock3 size={14} />
              Last connected {formatDateTime(hub.lastConnected)}
            </span>
          ) : null}
          {hub.serverInstanceId ? (
            <span>Server {shortId(hub.serverInstanceId)}</span>
          ) : null}
          <span>Hub {shortId(hub.id)}</span>
        </div>
      </div>
      <Link className="homeHubOpen" to={`/hubs/${hub.id}/overview`}>
        Open hub
        <ChevronRight size={16} />
      </Link>
    </article>
  );
}
