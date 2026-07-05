import { Loader2, RefreshCw } from 'lucide-react';

import {
  compactJoin,
  formatDateTime,
  formatEpochMs,
  statusBool,
  stringValue,
  yesNo
} from '../../lib/format';
import type { DeviceStatus } from '../../types';

export type StatusState = {
  loading: boolean;
  opened: boolean;
  error?: string;
  result?: DeviceStatus;
};

export function StatusPanel({
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

export function StatusTile({
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
