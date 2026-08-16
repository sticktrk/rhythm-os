import { useCallback, useEffect, useMemo, useState } from 'react';
import { ArrowLeft, RefreshCw, Unplug, Wifi } from 'lucide-react';
import { Link, useParams } from 'react-router-dom';

import { SelectField, FormRow, TextField } from '../../components/controls/fields';
import { ToggleSwitch } from '../../components/controls/ToggleSwitch';
import { EmptyState, InlineSpinner, KeyValueGrid } from '../../components/ui/bits';
import { useConfirm } from '../../components/ui/ConfirmDialog';
import { SectionCard } from '../../components/ui/SectionCard';
import type { DeviceClient } from '../../device/client';
import { getState } from '../../device/state';
import {
  deleteHubCredentials,
  getHueAuthority,
  listCanonicalDevices,
  pairDevice,
  retryHub,
  syncAll,
  updateHueAuthority
} from '../../device/topology';
import { asArray, asBoolean, asRecord, asRecordArray, asString } from '../../device/values';
import { useDeviceCall } from '../../hooks/useDeviceCall';
import { useDeviceClient } from '../../hooks/useDeviceClient';
import { usePolling } from '../../hooks/usePolling';
import { useHub } from '../../state/HubContext';

import {
  canonicalDevicesForHub,
  configuredHubsFromState,
  HUB_LIST_BACK_TARGET,
  hubDisplayName,
  hueAuthorityBridgeForHub,
  hueAuthorityUpdateBody,
  type ConfiguredIntegrationHub
} from './hubManagement';

const topologySyncCapability = 'hue_room_topology_sync_v1';

export default function HubManagementPage() {
  const params = useParams<{ integrationType?: string; integrationAddress?: string }>();
  const type = params.integrationType;
  const encodedAddress = params.integrationAddress;
  if (type) {
    return (
      <HubDetail
        type={type}
        address={encodedAddress === '_' ? '' : (encodedAddress ?? '')}
      />
    );
  }
  return <HubList />;
}

function HubList() {
  const client = useDeviceClient();
  const { hub } = useHub();
  const stateQuery = usePolling(
    useCallback(() => getState(client, true), [client]),
    { intervalMs: 10_000 }
  );
  const devicesQuery = usePolling(
    useCallback(() => listCanonicalDevices(client), [client]),
    { intervalMs: 0 }
  );
  const hubs = configuredHubsFromState(stateQuery.data);
  const [matterPayload, setMatterPayload] = useState('');
  const [matterResult, setMatterResult] = useState<unknown>(null);

  const matterCall = useDeviceCall(
    useCallback(
      async (setupPayload: string) => {
        const result = await pairDevice(client, {
          hubType: 'matter',
          params: {
            setup_payload: setupPayload,
            network: 'wifi',
            rendezvous: 'auto'
          }
        });
        setMatterResult(result);
        await Promise.all([stateQuery.refresh(), devicesQuery.refresh()]);
        return result;
      },
      [client, stateQuery.refresh, devicesQuery.refresh]
    )
  );
  const syncCall = useDeviceCall(
    useCallback(async () => {
      await syncAll(client);
      await Promise.all([stateQuery.refresh(), devicesQuery.refresh()]);
    }, [client, stateQuery.refresh, devicesQuery.refresh])
  );

  const state = asRecord(stateQuery.data);
  const capabilityHubs = asRecordArray(asRecord(state.capabilities).hubs);
  const matterCapability = capabilityHubs.find(
    (capability) => asString(capability.type) === 'matter'
  );
  const explicitCapabilities = capabilityHubs.length > 0;
  const matterSupported = !explicitCapabilities || matterCapability !== undefined;

  return (
    <div className="consolePage">
      <header className="pageHeader">
        <div>
          <div className="eyebrow">Device console</div>
          <h2>Hubs &amp; Devices</h2>
          <p className="pageIntro">
            Every configured integration on {hub.name}. Open a hub for its
            devices and settings.
          </p>
        </div>
        <div className="pageHeaderActions">
          <button
            className="consoleButton"
            type="button"
            disabled={stateQuery.refreshing || devicesQuery.refreshing}
            onClick={() => {
              void stateQuery.refresh();
              void devicesQuery.refresh();
            }}
          >
            <RefreshCw size={15} />
            <span>Refresh</span>
          </button>
        </div>
      </header>

      <SectionCard
        title="Configured hubs"
        subtitle={`${hubs.length} integration${hubs.length === 1 ? '' : 's'}`}
        busy={stateQuery.refreshing || devicesQuery.refreshing}
        error={stateQuery.error ?? devicesQuery.error}
        rawPayload={stateQuery.data ?? undefined}
      >
        {hubs.length === 0 ? (
          <EmptyState message="No configured integration hubs reported." />
        ) : (
          <div className="dataTableWrap">
            <table className="dataTable">
              <thead>
                <tr>
                  <th>Hub</th>
                  <th>Address</th>
                  <th>Status</th>
                  <th>Devices</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {hubs.map((integration) => {
                  const devices = canonicalDevicesForHub(
                    devicesQuery.data,
                    integration
                  );
                  return (
                    <tr key={`${integration.type}\u0000${integration.address}`}>
                      <td>{hubDisplayName(integration.type)}</td>
                      <td className="mono dim">{integration.address || 'local'}</td>
                      <td>
                        <span
                          className={`consoleStatus ${integration.connected ? 'online' : 'offline'}`}
                        >
                          {integration.connected ? <Wifi size={12} /> : <Unplug size={12} />}
                          {integration.connected ? 'Connected' : 'Disconnected'}
                        </span>
                      </td>
                      <td>{devices.length}</td>
                      <td>
                        <Link
                          className="consoleButton small"
                          to={`${encodeURIComponent(integration.type)}/${encodeURIComponent(integration.address || '_')}`}
                        >
                          Manage
                        </Link>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </SectionCard>

      <div className="cardGrid two">
        {matterSupported ? (
          <SectionCard
            title="Add Matter device"
            subtitle="Commission with an MT payload or manual pairing code"
            busy={matterCall.busy}
            error={matterCall.error}
            rawPayload={matterResult ?? undefined}
          >
            <FormRow label="Setup payload">
              <TextField
                value={matterPayload}
                onChange={setMatterPayload}
                placeholder="MT:… or manual code"
                mono
              />
            </FormRow>
            <div className="buttonRow">
              <button
                className="consoleButton primary"
                type="button"
                disabled={matterCall.busy || matterPayload.trim() === ''}
                onClick={() => void matterCall.run(matterPayload.trim())}
              >
                {matterCall.busy ? <InlineSpinner /> : null}
                <span>{matterCall.busy ? 'Pairing…' : 'Add Matter Device'}</span>
              </button>
            </div>
          </SectionCard>
        ) : null}

        <SectionCard
          title="Sync all hubs"
          subtitle="Refresh devices and integration state"
          busy={syncCall.busy}
          error={syncCall.error}
        >
          <div className="buttonRow">
            <button
              className="consoleButton"
              type="button"
              disabled={syncCall.busy || hubs.length === 0}
              onClick={() => void syncCall.run()}
            >
              Sync everything
            </button>
          </div>
        </SectionCard>
      </div>
    </div>
  );
}

function HubDetail({ type, address }: { type: string; address: string }) {
  const client = useDeviceClient();
  const confirm = useConfirm();
  const stateQuery = usePolling(
    useCallback(() => getState(client, true), [client]),
    { intervalMs: 10_000 }
  );
  const devicesQuery = usePolling(
    useCallback(() => listCanonicalDevices(client), [client]),
    { intervalMs: 0 }
  );
  const hubs = configuredHubsFromState(stateQuery.data);
  const integration = hubs.find(
    (candidate) =>
      candidate.type.toLowerCase() === type.toLowerCase() &&
      candidate.address.toLowerCase() === address.toLowerCase()
  );
  const devices = integration
    ? canonicalDevicesForHub(devicesQuery.data, integration)
    : [];

  const action = useDeviceCall(
    useCallback(
      async (kind: 'retry' | 'disconnect' | 'sync') => {
        if (kind === 'retry') await retryHub(client, type, address);
        if (kind === 'disconnect') {
          await deleteHubCredentials(client, { hubType: type, address });
        }
        if (kind === 'sync') await syncAll(client);
        await Promise.all([stateQuery.refresh(), devicesQuery.refresh()]);
      },
      [client, type, address, stateQuery.refresh, devicesQuery.refresh]
    )
  );

  const disconnect = async () => {
    const ok = await confirm({
      title: `Disconnect ${hubDisplayName(type)}`,
      message: `Remove the ${hubDisplayName(type)} integration at ${address || 'local'} from this Light Box?`,
      confirmLabel: 'Disconnect',
      danger: true
    });
    if (ok) await action.run('disconnect');
  };

  if (stateQuery.loading && stateQuery.data === null) {
    return <div className="consolePage"><InlineSpinner label="Loading hubs…" /></div>;
  }

  if (!integration) {
    return (
      <div className="consolePage">
        <Link
          className="consoleButton small"
          to={HUB_LIST_BACK_TARGET}
          relative="path"
        >
          <ArrowLeft size={14} /> Back to hubs
        </Link>
        <EmptyState message="This configured hub is no longer reported by the Light Box." />
      </div>
    );
  }

  return (
    <div className="consolePage">
      <header className="pageHeader">
        <div>
          <Link
            className="consoleButton small"
            to={HUB_LIST_BACK_TARGET}
            relative="path"
          >
            <ArrowLeft size={14} /> Back to hubs
          </Link>
          <div className="eyebrow">Integration hub</div>
          <h2>{hubDisplayName(type)}</h2>
          <p className="pageIntro">{address || 'Local controller'}</p>
        </div>
      </header>

      <SectionCard
        title="Hub status"
        busy={action.busy || stateQuery.refreshing || devicesQuery.refreshing}
        error={action.error ?? stateQuery.error ?? devicesQuery.error}
        rawPayload={integration.raw}
        actions={
          <span className="rowActions">
            <button className="consoleButton small" type="button" onClick={() => void action.run('retry')}>
              Retry
            </button>
            <button className="consoleButton small" type="button" onClick={() => void action.run('sync')}>
              Sync
            </button>
            <button className="consoleButton small danger" type="button" onClick={() => void disconnect()}>
              Disconnect
            </button>
          </span>
        }
      >
        <KeyValueGrid
          rows={[
            ['Type', type],
            ['Address', address || 'local'],
            ['Connected', integration.connected ? 'yes' : 'no'],
            ['Devices', devices.length]
          ]}
        />
      </SectionCard>

      <SectionCard
        title="Devices"
        subtitle={`${devices.length} device${devices.length === 1 ? '' : 's'} on this exact hub`}
        rawPayload={devicesQuery.data ?? undefined}
      >
        {devices.length === 0 ? (
          <EmptyState message="No canonical devices belong to this hub." />
        ) : (
          <div className="dataTableWrap">
            <table className="dataTable">
              <thead><tr><th>Name</th><th>Id</th><th>Type</th><th>Room</th></tr></thead>
              <tbody>
                {devices.map((device, index) => (
                  <tr key={asString(device.id) ?? index}>
                    <td>{asString(device.name) ?? asString(device.id) ?? 'Unnamed'}</td>
                    <td className="mono dim">{asString(device.id) ?? '—'}</td>
                    <td>{asString(device.device_type) ?? asString(device.type) ?? '—'}</td>
                    <td>{asString(device.room_name) ?? asString(device.room_id) ?? '—'}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </SectionCard>

      {type === 'hue' ? (
        <HueAuthorityPanel client={client} integration={integration} state={stateQuery.data} />
      ) : null}
    </div>
  );
}

function HueAuthorityPanel({
  client,
  integration,
  state
}: {
  client: DeviceClient;
  integration: ConfiguredIntegrationHub;
  state: unknown;
}) {
  const confirm = useConfirm();
  const query = usePolling(
    useCallback(() => getHueAuthority(client), [client]),
    { intervalMs: 0 }
  );
  const bridge = useMemo(
    () => hueAuthorityBridgeForHub(query.data, integration.address),
    [query.data, integration.address]
  );
  const bridgeKey = `${asString(bridge?.address) ?? ''}\u0000${asString(bridge?.revision) ?? ''}`;
  const [owners, setOwners] = useState<Record<string, 'hue' | 'rhythm'>>({});
  const [topologySyncEnabled, setTopologySyncEnabled] = useState(false);
  const rooms = asRecordArray(bridge?.rooms);

  useEffect(() => {
    const nextOwners: Record<string, 'hue' | 'rhythm'> = {};
    for (const room of rooms) {
      const roomId = asString(room.room_id);
      if (!roomId) continue;
      nextOwners[roomId] = asString(room.owner) === 'rhythm' ? 'rhythm' : 'hue';
    }
    setOwners(nextOwners);
    setTopologySyncEnabled(asBoolean(bridge?.topology_sync_enabled) ?? false);
  }, [bridgeKey]);

  const features = asArray(asRecord(asRecord(state).capabilities).features).filter(
    (feature): feature is string => typeof feature === 'string'
  );
  const topologySyncSupported = features.includes(topologySyncCapability);
  const allRhythm = rooms.length > 0 && rooms.every((room) => {
    const roomId = asString(room.room_id) ?? '';
    return owners[roomId] === 'rhythm';
  });

  const save = useDeviceCall(
    useCallback(async () => {
      if (!bridge) return;
      const ok = await confirm({
        title: 'Save Hue automation authority',
        message: allRhythm
          ? 'All Hue rooms will use Rhythm automation. Hue automation fields may be paused under the saved recovery manifest.'
          : 'Mixed or Hue-owned rooms keep bridge automation suppression blocked. Manual controls continue to work.',
        confirmLabel: 'Save choices'
      });
      if (!ok) return;
      const correlationId = `admin-hue-authority-${globalThis.crypto?.randomUUID?.() ?? Date.now()}`;
      await updateHueAuthority(
        client,
        hueAuthorityUpdateBody({
          bridge,
          owners,
          correlationId,
          ...(topologySyncSupported ? { topologySyncEnabled } : {})
        })
      );
      await query.refresh();
    }, [client, bridge, owners, topologySyncEnabled, topologySyncSupported, allRhythm, confirm, query.refresh])
  );

  return (
    <SectionCard
      title="Hue room automation"
      subtitle="Permanent authority and Rhythm room-sync controls"
      busy={query.refreshing || save.busy}
      error={query.error ?? save.error}
      rawPayload={query.data ?? undefined}
      actions={
        <button className="consoleButton small" type="button" onClick={() => void query.refresh()}>
          <RefreshCw size={14} /> Reload
        </button>
      }
    >
      {!bridge ? (
        <EmptyState message="Hue authority is not ready for this bridge." />
      ) : (
        <>
          {rooms.map((room) => {
            const roomId = asString(room.room_id) ?? '';
            return (
              <FormRow key={roomId} label={asString(room.name) ?? roomId} hint={roomId}>
                <SelectField
                  value={owners[roomId] ?? 'hue'}
                  onChange={(owner) =>
                    setOwners((current) => ({
                      ...current,
                      [roomId]: owner === 'rhythm' ? 'rhythm' : 'hue'
                    }))
                  }
                  options={[
                    { value: 'hue', label: 'Hue automation' },
                    { value: 'rhythm', label: 'Rhythm automation' }
                  ]}
                />
              </FormRow>
            );
          })}
          {topologySyncSupported ? (
            <FormRow
              label="Sync Rhythm rooms to Hue"
              hint={
                allRhythm
                  ? 'Moves only Hue lights into explicitly owned Rhythm mirror rooms.'
                  : 'Saved now, but remains blocked until every room uses Rhythm automation.'
              }
            >
              <ToggleSwitch
                checked={topologySyncEnabled}
                onChange={setTopologySyncEnabled}
                label={topologySyncEnabled ? 'Enabled' : 'Disabled'}
              />
            </FormRow>
          ) : null}
          <KeyValueGrid
            rows={[
              ['Topology sync status', asString(bridge.topology_sync_status)],
              ['Rooms', bridge.topology_sync_room_count],
              ['Lights', bridge.topology_sync_light_count]
            ]}
          />
          <div className="buttonRow">
            <button
              className="consoleButton primary"
              type="button"
              disabled={save.busy || rooms.length === 0}
              onClick={() => void save.run()}
            >
              Save Hue choices
            </button>
          </div>
        </>
      )}
    </SectionCard>
  );
}
