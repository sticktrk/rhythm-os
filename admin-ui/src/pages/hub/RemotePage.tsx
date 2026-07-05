import { useCallback, useState } from 'react';
import { Globe, RefreshCw, Save, Trash2 } from 'lucide-react';

import { JsonEditor } from '../../components/controls/JsonEditor';
import { ToggleSwitch } from '../../components/controls/ToggleSwitch';
import { FormRow, TextField } from '../../components/controls/fields';
import { ErrorNotice, KeyValueGrid } from '../../components/ui/bits';
import { SectionCard } from '../../components/ui/SectionCard';
import { useConfirm } from '../../components/ui/ConfirmDialog';
import {
  deleteRemoteAccessConfig,
  getRemoteAccessStatus,
  setRemoteAccessConfig
} from '../../device/auth';
import {
  deleteActivityCloudConfig,
  getActivityCloudConfig,
  setActivityCloudConfig
} from '../../device/settings';
import { asBoolean, asNumber, asRecord, asString } from '../../device/values';
import { useDeviceCall } from '../../hooks/useDeviceCall';
import { useDeviceClient } from '../../hooks/useDeviceClient';
import { usePolling } from '../../hooks/usePolling';
import { isPlainObject, prettyJson } from '../../lib/json';

export default function RemotePage() {
  const client = useDeviceClient();
  const confirm = useConfirm();

  const statusQuery = usePolling(
    useCallback(() => getRemoteAccessStatus(client), [client]),
    { intervalMs: 15_000 }
  );
  const cloudQuery = usePolling(
    useCallback(() => getActivityCloudConfig(client).catch(() => ({})), [client])
  );

  const status = asRecord(statusQuery.data);

  const [tunnelEnabled, setTunnelEnabled] = useState<boolean | null>(null);
  const [hostname, setHostname] = useState<string | null>(null);
  const [connectorToken, setConnectorToken] = useState('');
  const [cloudText, setCloudText] = useState<string | null>(null);
  const [cloudParsed, setCloudParsed] = useState<unknown>(undefined);

  const statusRefresh = statusQuery.refresh;
  const saveTunnel = useDeviceCall(
    useCallback(async () => {
      await setRemoteAccessConfig(client, {
        enabled: tunnelEnabled ?? asBoolean(status.enabled) ?? true,
        hostname: hostname ?? asString(status.hostname) ?? '',
        ...(connectorToken.trim() ? { connector_token: connectorToken.trim() } : {})
      });
      await statusRefresh();
    }, [client, tunnelEnabled, hostname, connectorToken, status, statusRefresh])
  );

  const cloudRefresh = cloudQuery.refresh;
  const saveCloud = useDeviceCall(
    useCallback(async () => {
      if (!isPlainObject(cloudParsed)) {
        throw new Error('Activity cloud config must be a JSON object.');
      }
      await setActivityCloudConfig(client, cloudParsed);
      await cloudRefresh();
    }, [client, cloudParsed, cloudRefresh])
  );

  const shownEnabled = tunnelEnabled ?? asBoolean(status.enabled) ?? false;
  const shownHostname = hostname ?? asString(status.hostname) ?? '';

  return (
    <div className="consolePage">
      <header className="pageHeader">
        <div>
          <div className="eyebrow">Device console</div>
          <h2>Network &amp; Remote</h2>
          <p className="pageIntro">
            Cloudflare tunnel remote access and activity-cloud reporting.
          </p>
        </div>
        <div className="pageHeaderActions">
          <button
            className="consoleButton"
            type="button"
            onClick={() => {
              void statusQuery.refresh();
              void cloudQuery.refresh();
            }}
          >
            <RefreshCw size={15} />
            <span>Refresh</span>
          </button>
        </div>
      </header>

      {statusQuery.error && !statusQuery.data ? (
        <ErrorNotice message={statusQuery.error} />
      ) : null}

      <SectionCard
        title="Remote access status"
        subtitle="Tunnel supervisor telemetry"
        busy={statusQuery.refreshing}
        rawPayload={statusQuery.data ?? undefined}
      >
        <KeyValueGrid
          rows={[
            ['Enabled', asBoolean(status.enabled)],
            ['Configured', asBoolean(status.configured)],
            ['Service running', asBoolean(status.service_running)],
            ['Connector healthy', asBoolean(status.connector_healthy)],
            ['Hostname', asString(status.hostname)],
            ['Tunnel', asString(status.tunnel_name) ?? asString(status.tunnel_id)],
            ['Connections', asNumber(status.registered_connections)]
          ]}
        />
      </SectionCard>

      <SectionCard
        title="Tunnel config"
        subtitle="Writing this restarts the tunnel connector on the device"
        busy={saveTunnel.busy}
        error={saveTunnel.error}
      >
        <ToggleSwitch
          checked={shownEnabled}
          label="Remote access enabled"
          onChange={setTunnelEnabled}
        />
        <FormRow label="Hostname" hint="Public tunnel hostname">
          <TextField value={shownHostname} onChange={setHostname} mono />
        </FormRow>
        <FormRow
          label="Connector token"
          hint="Leave blank to keep the existing token"
        >
          <TextField value={connectorToken} onChange={setConnectorToken} mono />
        </FormRow>
        <div className="actionRow">
          <button
            className="consoleButton small primary"
            type="button"
            disabled={saveTunnel.busy}
            onClick={() => {
              void (async () => {
                const confirmed = await confirm({
                  title: 'Update remote access config',
                  message:
                    'Apply the tunnel configuration? If it is wrong, remote access to this device is lost until fixed locally.',
                  confirmLabel: 'Apply',
                  danger: true
                });
                if (confirmed) await saveTunnel.run();
              })();
            }}
          >
            <Save size={13} />
            <span>Apply config</span>
          </button>
          <button
            className="consoleButton small danger"
            type="button"
            onClick={() => {
              void (async () => {
                const confirmed = await confirm({
                  title: 'Remove remote access config',
                  message:
                    'Delete the tunnel configuration? The device will only be reachable on its LAN afterwards.',
                  confirmLabel: 'Delete',
                  danger: true,
                  requireTypedText: 'delete-remote'
                });
                if (!confirmed) return;
                await deleteRemoteAccessConfig(client);
                await statusQuery.refresh();
              })();
            }}
          >
            <Trash2 size={13} />
            <span>Delete config</span>
          </button>
        </div>
      </SectionCard>

      <SectionCard
        title="Activity cloud"
        subtitle="Cloud reporting configuration (free-form)"
        busy={saveCloud.busy || cloudQuery.refreshing}
        error={saveCloud.error}
        rawPayload={cloudQuery.data ?? undefined}
      >
        <JsonEditor
          value={cloudText ?? prettyJson(cloudQuery.data ?? {})}
          rows={8}
          onChange={(text, parsed) => {
            setCloudText(text);
            setCloudParsed(parsed);
          }}
        />
        <div className="actionRow">
          <button
            className="consoleButton small primary"
            type="button"
            disabled={saveCloud.busy || !isPlainObject(cloudParsed)}
            onClick={() => void saveCloud.run()}
          >
            <Save size={13} />
            <span>Save</span>
          </button>
          <button
            className="consoleButton small danger"
            type="button"
            onClick={() => {
              void (async () => {
                const confirmed = await confirm({
                  title: 'Delete activity cloud config',
                  message: 'Remove the activity-cloud configuration from the device?',
                  confirmLabel: 'Delete',
                  danger: true
                });
                if (!confirmed) return;
                await deleteActivityCloudConfig(client);
                setCloudText(null);
                await cloudQuery.refresh();
              })();
            }}
          >
            <Trash2 size={13} />
            <span>Delete</span>
          </button>
        </div>
        <p className="cardNote">
          <Globe size={12} /> Schema is device-version specific; the payload is
          validated server-side.
        </p>
      </SectionCard>
    </div>
  );
}
