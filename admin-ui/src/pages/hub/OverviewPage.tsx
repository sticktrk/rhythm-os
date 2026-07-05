import { useCallback } from 'react';
import { Moon, RefreshCw, Sun } from 'lucide-react';

import { SegmentedControl } from '../../components/controls/SegmentedControl';
import { ToggleSwitch } from '../../components/controls/ToggleSwitch';
import { ErrorNotice, KeyValueGrid } from '../../components/ui/bits';
import { SectionCard } from '../../components/ui/SectionCard';
import { getLightBreaker, setLightBreaker } from '../../device/settings';
import { getMode, setActiveMode } from '../../device/modes';
import { getState } from '../../device/state';
import {
  asArray,
  asBoolean,
  asNumber,
  asRecord,
  asString,
  pick
} from '../../device/values';
import { useDeviceCall } from '../../hooks/useDeviceCall';
import { useDeviceClient } from '../../hooks/useDeviceClient';
import { usePolling } from '../../hooks/usePolling';
import { formatEpochMs } from '../../lib/format';
import { useHub } from '../../state/HubContext';

export default function OverviewPage() {
  const client = useDeviceClient();
  const { hub, probe } = useHub();

  const stateQuery = usePolling(
    useCallback(() => getState(client), [client]),
    { intervalMs: 10_000 }
  );
  const modeQuery = usePolling(
    useCallback(() => getMode(client), [client]),
    { intervalMs: 15_000 }
  );
  const breakerQuery = usePolling(
    useCallback(() => getLightBreaker(client), [client]),
    { intervalMs: 15_000 }
  );

  const modeRefresh = modeQuery.refresh;
  const modeCall = useDeviceCall(
    useCallback(
      async (active: string) => {
        await setActiveMode(client, active);
        await modeRefresh();
      },
      [client, modeRefresh]
    )
  );

  const breakerRefresh = breakerQuery.refresh;
  const breakerCall = useDeviceCall(
    useCallback(
      async (enabled: boolean) => {
        await setLightBreaker(client, enabled);
        await breakerRefresh();
      },
      [client, breakerRefresh]
    )
  );

  const state = asRecord(stateQuery.data);
  const mode = asRecord(modeQuery.data);
  const breaker = asRecord(breakerQuery.data);

  const activeMode = asString(mode.active) ?? asString(state.active_mode);
  const breakerEnabled = asBoolean(breaker.enabled);
  const nodes = asArray(state.nodes);
  const rooms = asArray(state.rooms);
  const hubs = asArray(state.hubs);

  const offline =
    stateQuery.error !== null && stateQuery.data === null && !stateQuery.loading;

  return (
    <div className="consolePage">
      <header className="pageHeader">
        <div>
          <div className="eyebrow">Device console</div>
          <h2>Overview</h2>
          <p className="pageIntro">
            Live snapshot of {hub.name}. State refreshes every 10 seconds.
          </p>
        </div>
        <div className="pageHeaderActions">
          <button
            className="consoleButton"
            type="button"
            onClick={() => {
              void stateQuery.refresh();
              void modeQuery.refresh();
              void breakerQuery.refresh();
            }}
          >
            <RefreshCw size={15} />
            <span>Refresh</span>
          </button>
        </div>
      </header>

      {offline ? (
        <ErrorNotice
          message={`Device unreachable: ${stateQuery.error ?? 'unknown error'}`}
        />
      ) : null}

      <div className="cardGrid two">
        <SectionCard
          title="Mode"
          subtitle="Active lighting mode across the home"
          busy={modeCall.busy || modeQuery.refreshing}
          error={modeCall.error ?? modeQuery.error}
          rawPayload={modeQuery.data ?? undefined}
        >
          <SegmentedControl
            value={activeMode ?? ''}
            disabled={modeCall.busy || !modeQuery.data}
            onChange={(value) => void modeCall.run(value)}
            options={[
              { value: 'day', label: 'Day', icon: <Sun size={15} /> },
              { value: 'sleep', label: 'Sleep', icon: <Moon size={15} /> }
            ]}
          />
          <KeyValueGrid
            rows={[
              ['Active mode', activeMode],
              ['Configs', asArray(mode.configs).length || undefined],
              [
                'Light runtime',
                asString(state.light_runtime) ??
                  asString(pick(state, 'light_runtime', 'runtime_id'))
              ]
            ]}
          />
        </SectionCard>

        <SectionCard
          title="Light breaker"
          subtitle="Global kill switch for autonomous control"
          busy={breakerCall.busy || breakerQuery.refreshing}
          error={breakerCall.error ?? breakerQuery.error}
          rawPayload={breakerQuery.data ?? undefined}
        >
          <div className="breakerRow">
            <ToggleSwitch
              checked={breakerEnabled ?? false}
              busy={breakerCall.busy}
              disabled={breakerEnabled === undefined}
              onChange={(value) => void breakerCall.run(value)}
              label={
                breakerEnabled === undefined
                  ? 'Unknown'
                  : breakerEnabled
                    ? 'Autonomous control enabled'
                    : 'Autonomous control DISABLED'
              }
            />
          </div>
          <p className="cardNote">
            When disabled, the device stops driving lights entirely. Customers
            usually expect this to be on.
          </p>
        </SectionCard>
      </div>

      <SectionCard
        title="Identity"
        busy={stateQuery.refreshing}
        error={stateQuery.error}
        rawPayload={stateQuery.data ?? undefined}
      >
        <KeyValueGrid
          rows={[
            ['Server version', asString(state.server_version) ?? probe?.serverVersion],
            ['Instance', asString(state.server_instance_id) ?? hub.serverInstanceId],
            [
              'Platform',
              asString(pick(state, 'platform', 'type')) ??
                asString(state.platform_type)
            ],
            [
              'Context',
              asString(pick(state, 'platform', 'context')) ??
                asString(state.platform_context)
            ],
            ['Listen port', asNumber(state.listen_port)],
            ['Last tick', formatEpochMs(asNumber(state.last_tick_epoch_ms))],
            ['Nodes', nodes.length || undefined],
            ['Rooms', rooms.length || undefined],
            ['Hubs', hubs.length || undefined],
            ['Route', probe?.route],
            ['Base URL', probe?.baseUrl]
          ]}
        />
      </SectionCard>
    </div>
  );
}
