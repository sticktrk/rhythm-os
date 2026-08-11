import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type DragEvent
} from 'react';
import {
  AlertTriangle,
  ArrowLeft,
  ArrowRight,
  Check,
  CheckCircle2,
  Clock3,
  GripVertical,
  Lightbulb,
  Link2,
  Pencil,
  RefreshCw,
  RotateCcw,
  Settings,
  ShieldCheck,
  Trash2,
  Wifi,
  X,
  Zap
} from 'lucide-react';

import { JsonEditor } from '../../components/controls/JsonEditor';
import {
  FormRow,
  SelectField,
  TextField
} from '../../components/controls/fields';
import { SegmentedControl } from '../../components/controls/SegmentedControl';
import { ToggleSwitch } from '../../components/controls/ToggleSwitch';
import { EmptyState, InlineSpinner, KeyValueGrid } from '../../components/ui/bits';
import { useConfirm } from '../../components/ui/ConfirmDialog';
import { Modal } from '../../components/ui/Modal';
import { RawPayloadToggle, SectionCard } from '../../components/ui/SectionCard';
import type { DeviceClient } from '../../device/client';
import {
  createRoom,
  deleteHubCredentials,
  deleteRoom,
  flashCanonicalDevice,
  getTopologyNodes,
  getTriageCount,
  getWifi,
  listCanonicalDevices,
  listMatterCaptures,
  listTriage,
  mergeRooms,
  pairDevice,
  putHubCredentials,
  renameCanonicalDevice,
  resetWifi,
  resolveTriage,
  retryHub,
  runBulbTest,
  setCanonicalDeviceParent,
  setWifi,
  syncAll,
  unpairDevice
} from '../../device/topology';
import { getNodesState, getState } from '../../device/state';
import {
  asArray,
  asBoolean,
  asRecord,
  asRecordArray,
  asString
} from '../../device/values';
import { useDeviceCall } from '../../hooks/useDeviceCall';
import { useDeviceClient } from '../../hooks/useDeviceClient';
import { usePolling } from '../../hooks/usePolling';
import { errorMessage } from '../../lib/format';
import { prettyJson } from '../../lib/json';
import { useHub } from '../../state/HubContext';

import {
  lightProfileOverrideSupport,
  lightSettingsProfilesFromState
} from './nodeLightSettings';
import { NodeDetail, parseNodes } from './NodesPage';
import {
  moveRoomBefore,
  moveRoomByOffset,
  parseStoredRoomOrder,
  reconcileRoomOrder,
  roomOrderStorageKey,
  segmentRoomDevices
} from './roomBoardLayout';
import {
  buildRoomMoveProposal,
  createRoomMoveRequestId,
  moveDeviceBetweenRoomsVerified,
  type RoomMoveProposal,
  type RoomMoveReceipt
} from './roomAssignment';
import {
  buildTopologyMembership,
  isBulbKind,
  topologyKindLabel,
  type TopologyItem
} from './topologyMembership';

import '../../styles/pages-phase6.css';

type Tab = 'devices' | 'rooms' | 'triage' | 'pairing' | 'hub' | 'wifi';

export default function TopologyPage() {
  const client = useDeviceClient();
  const { hub, hubId } = useHub();
  const [tab, setTab] = useState<Tab>('devices');

  const triageCountQuery = usePolling(
    useCallback(() => getTriageCount(client), [client]),
    { intervalMs: 60_000 }
  );
  const triageCount = asRecord(triageCountQuery.data).count;

  return (
    <div className="consolePage">
      <header className="pageHeader">
        <div>
          <div className="eyebrow">Device console</div>
          <h2>Devices &amp; Topology</h2>
          <p className="pageIntro">
            Canonical devices, rooms, discovery triage, pairing and network on{' '}
            {hub.name}.
          </p>
        </div>
      </header>

      <div className="tabBar">
        <SegmentedControl
          value={tab}
          onChange={(next) => setTab(next as Tab)}
          options={[
            { value: 'devices', label: 'Devices' },
            { value: 'rooms', label: 'Rooms' },
            {
              value: 'triage',
              label:
                typeof triageCount === 'number' && triageCount > 0
                  ? `Triage (${triageCount})`
                  : 'Triage'
            },
            { value: 'pairing', label: 'Pairing' },
            { value: 'hub', label: 'Hub' },
            { value: 'wifi', label: 'Wi-Fi' }
          ]}
        />
      </div>

      {tab === 'devices' ? <DevicesTab client={client} /> : null}
      {tab === 'rooms' ? <RoomsTab key={hubId} client={client} /> : null}
      {tab === 'triage' ? (
        <TriageTab
          client={client}
          onResolved={() => void triageCountQuery.refresh()}
        />
      ) : null}
      {tab === 'pairing' ? <PairingTab client={client} /> : null}
      {tab === 'hub' ? <HubTab client={client} /> : null}
      {tab === 'wifi' ? <WifiTab client={client} /> : null}
    </div>
  );
}

/* ------------------------------- Devices ------------------------------- */

function devicesFromPayload(data: unknown): Record<string, unknown>[] {
  if (Array.isArray(data)) return asRecordArray(data);
  const record = asRecord(data);
  return asRecordArray(
    asArray(record.devices).length > 0 ? record.devices : record.items
  );
}

function deviceId(device: Record<string, unknown>): string {
  return (
    asString(device.id) ??
    asString(device.device_id) ??
    asString(device.canonical_id) ??
    ''
  );
}

function DevicesTab({ client }: { client: DeviceClient }) {
  const confirm = useConfirm();
  const query = usePolling(
    useCallback(() => listCanonicalDevices(client), [client]),
    { intervalMs: 0 }
  );
  const devices = devicesFromPayload(query.data);

  const [editingId, setEditingId] = useState<string | null>(null);
  const [draftName, setDraftName] = useState('');
  const [parentTarget, setParentTarget] = useState<Record<string, unknown> | null>(null);
  const [parentId, setParentId] = useState('');
  const [unpairTarget, setUnpairTarget] = useState<Record<string, unknown> | null>(null);
  const [unpairHubType, setUnpairHubType] = useState('');
  const [unpairForce, setUnpairForce] = useState(false);

  const refresh = query.refresh;
  const action = useDeviceCall(
    useCallback(
      async (fn: () => Promise<unknown>) => {
        await fn();
        await refresh();
      },
      [refresh]
    )
  );

  async function handleUnpair() {
    if (!unpairTarget) return;
    const id = deviceId(unpairTarget);
    const ok = await confirm({
      title: 'Unpair device',
      message: `Unpair ${asString(unpairTarget.name) ?? id} from hub type "${unpairHubType}"${unpairForce ? ' (forced)' : ''}? The device will need to be re-commissioned.`,
      confirmLabel: 'Unpair',
      danger: true
    });
    if (!ok) return;
    await action.run(() =>
      unpairDevice(client, {
        hubType: unpairHubType,
        deviceId: id,
        force: unpairForce
      })
    );
    setUnpairTarget(null);
  }

  return (
    <SectionCard
      title="Canonical devices"
      subtitle={`${devices.length} device(s)`}
      busy={query.refreshing || action.busy}
      error={query.error ?? action.error}
      rawPayload={query.data ?? undefined}
      actions={
        <button className="consoleButton small" type="button" onClick={() => void query.refresh()}>
          <RefreshCw size={14} />
          <span>Reload</span>
        </button>
      }
    >
      {devices.length === 0 ? (
        <EmptyState message="No canonical devices reported." />
      ) : (
        <div className="dataTableWrap">
          <table className="dataTable">
            <thead>
              <tr>
                <th>Name</th>
                <th>Id</th>
                <th>Type</th>
                <th>Room</th>
                <th>Actions</th>
              </tr>
            </thead>
            <tbody>
              {devices.map((device, index) => {
                const id = deviceId(device);
                const name = asString(device.name) ?? id;
                const isEditing = editingId === id;
                return (
                  <tr key={id || index}>
                    <td>
                      {isEditing ? (
                        <span className="inlineEdit">
                          <TextField value={draftName} onChange={setDraftName} />
                          <button
                            className="iconOnlyButton"
                            type="button"
                            aria-label="Save name"
                            onClick={() => {
                              setEditingId(null);
                              if (draftName.trim() && draftName !== name) {
                                void action.run(() =>
                                  renameCanonicalDevice(client, id, draftName.trim())
                                );
                              }
                            }}
                          >
                            <Check size={14} />
                          </button>
                          <button
                            className="iconOnlyButton"
                            type="button"
                            aria-label="Cancel rename"
                            onClick={() => setEditingId(null)}
                          >
                            <X size={14} />
                          </button>
                        </span>
                      ) : (
                        <button
                          className="inlineEditName"
                          type="button"
                          onClick={() => {
                            setEditingId(id);
                            setDraftName(name);
                          }}
                        >
                          {name}
                          <Pencil size={12} />
                        </button>
                      )}
                    </td>
                    <td className="mono dim">{id}</td>
                    <td>
                      {asString(device.type) ??
                        asString(device.device_type) ??
                        asString(device.kind) ??
                        '—'}
                    </td>
                    <td>
                      {asString(device.room) ??
                        asString(device.room_id) ??
                        asString(device.room_name) ??
                        '—'}
                    </td>
                    <td>
                      <span className="rowActions">
                        <button
                          className="consoleButton small"
                          type="button"
                          title="Flash / identify the device"
                          onClick={() =>
                            void action.run(() => flashCanonicalDevice(client, id))
                          }
                        >
                          <Zap size={13} />
                          <span>Flash</span>
                        </button>
                        <button
                          className="consoleButton small"
                          type="button"
                          onClick={() => {
                            setParentTarget(device);
                            setParentId(asString(device.parent_id) ?? '');
                          }}
                        >
                          <Link2 size={13} />
                          <span>Parent</span>
                        </button>
                        <button
                          className="consoleButton small danger"
                          type="button"
                          onClick={() => {
                            setUnpairTarget(device);
                            setUnpairHubType(
                              asString(device.hub_type) ?? asString(device.hub) ?? 'matter'
                            );
                            setUnpairForce(false);
                          }}
                        >
                          <Trash2 size={13} />
                          <span>Unpair</span>
                        </button>
                      </span>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      {parentTarget ? (
        <Modal
          title={`Set parent for ${asString(parentTarget.name) ?? deviceId(parentTarget)}`}
          onClose={() => setParentTarget(null)}
        >
          <FormRow label="Parent device" hint="Leave empty to clear the parent">
            <SelectField
              value={parentId}
              onChange={setParentId}
              placeholder="(no parent)"
              options={devices
                .filter((device) => deviceId(device) !== deviceId(parentTarget))
                .map((device) => ({
                  value: deviceId(device),
                  label: asString(device.name) ?? deviceId(device)
                }))}
            />
          </FormRow>
          <div className="confirmActions">
            <button className="consoleButton" type="button" onClick={() => setParentTarget(null)}>
              Cancel
            </button>
            <button
              className="consoleButton primary"
              type="button"
              onClick={() => {
                const id = deviceId(parentTarget);
                void action.run(() =>
                  setCanonicalDeviceParent(client, id, parentId.trim() === '' ? null : parentId)
                );
                setParentTarget(null);
              }}
            >
              Set parent
            </button>
          </div>
        </Modal>
      ) : null}

      {unpairTarget ? (
        <Modal
          title={`Unpair ${asString(unpairTarget.name) ?? deviceId(unpairTarget)}`}
          onClose={() => setUnpairTarget(null)}
        >
          <FormRow label="Hub type" hint="Controller that owns this device, e.g. matter">
            <TextField value={unpairHubType} onChange={setUnpairHubType} mono />
          </FormRow>
          <FormRow label="Force" hint="Remove even if the device is unreachable">
            <ToggleSwitch checked={unpairForce} onChange={setUnpairForce} />
          </FormRow>
          <div className="confirmActions">
            <button className="consoleButton" type="button" onClick={() => setUnpairTarget(null)}>
              Cancel
            </button>
            <button
              className="consoleButton primary danger"
              type="button"
              disabled={unpairHubType.trim() === ''}
              onClick={() => void handleUnpair()}
            >
              Unpair…
            </button>
          </div>
        </Modal>
      ) : null}
    </SectionCard>
  );
}

/* -------------------------------- Rooms -------------------------------- */

function RoomsTab({ client }: { client: DeviceClient }) {
  const confirm = useConfirm();
  const { hubId } = useHub();
  const topologyQuery = usePolling(
    useCallback(() => getTopologyNodes(client), [client]),
    { intervalMs: 0 }
  );

  const membership = useMemo(
    () => buildTopologyMembership(topologyQuery.data),
    [topologyQuery.data]
  );
  const roomIds = useMemo(
    () => membership.rooms.map(({ room }) => room.id),
    [membership.rooms]
  );
  const roomIdsKey = roomIds.join('\u0000');
  const assignedBulbCount = useMemo(
    () => membership.rooms.reduce((total, room) => total + room.bulbs.length, 0),
    [membership.rooms]
  );
  const assignedDeviceCount = useMemo(
    () =>
      membership.rooms.reduce((total, room) => total + room.children.length, 0),
    [membership.rooms]
  );

  const [newRoomName, setNewRoomName] = useState('');
  const [mergeTarget, setMergeTarget] = useState<TopologyItem | null>(null);
  const [mergeSourceId, setMergeSourceId] = useState('');
  const [draggedRoomId, setDraggedRoomId] = useState<string | null>(null);
  const [draggedDeviceId, setDraggedDeviceId] = useState<string | null>(null);
  const [dragOverRoomId, setDragOverRoomId] = useState<string | null>(null);
  const [movePickerDevice, setMovePickerDevice] = useState<TopologyItem | null>(null);
  const [movePickerTargetId, setMovePickerTargetId] = useState('');
  const [moveReceipt, setMoveReceipt] = useState<RoomMoveReceipt | null>(null);
  const [settingsTargetId, setSettingsTargetId] = useState<string | null>(null);
  const [identifyReceipt, setIdentifyReceipt] = useState<{
    deviceId: string;
    deviceName: string;
    status: 'pending' | 'confirmed' | 'failed';
    error?: string;
  } | null>(null);

  const storageKey = roomOrderStorageKey(hubId);
  const [roomOrder, setRoomOrder] = useState<string[]>(() => {
    try {
      return parseStoredRoomOrder(window.localStorage.getItem(storageKey));
    } catch {
      return [];
    }
  });

  useEffect(() => {
    const authoritativeIds = roomIdsKey === '' ? [] : roomIdsKey.split('\u0000');
    setRoomOrder((current) => {
      const next = reconcileRoomOrder(authoritativeIds, current);
      try {
        window.localStorage.setItem(storageKey, JSON.stringify(next));
      } catch {
        // The board remains usable when browser storage is disabled.
      }
      return next.join('\u0000') === current.join('\u0000') ? current : next;
    });
  }, [roomIdsKey, storageKey]);

  const orderedRooms = useMemo(() => {
    const byId = new Map(
      membership.rooms.map((roomMembership) => [
        roomMembership.room.id,
        roomMembership
      ])
    );
    return reconcileRoomOrder(roomIds, roomOrder)
      .map((roomId) => byId.get(roomId))
      .filter((room): room is (typeof membership.rooms)[number] => Boolean(room));
  }, [membership.rooms, roomIds, roomOrder]);

  const settingsOpen = settingsTargetId !== null;
  const nodesQuery = usePolling(
    useCallback(() => getNodesState(client), [client]),
    { intervalMs: 5_000, enabled: settingsOpen }
  );
  const stateQuery = usePolling(
    useCallback(() => getState(client), [client]),
    { intervalMs: 0, enabled: settingsOpen }
  );
  const liveNodes = useMemo(() => parseNodes(nodesQuery.data), [nodesQuery.data]);
  const settingsNode = settingsTargetId
    ? liveNodes.find((node) => node.id === settingsTargetId) ?? null
    : null;
  const lightSettingsProfiles = useMemo(
    () => lightSettingsProfilesFromState(stateQuery.data),
    [stateQuery.data]
  );
  const lightSettingsSupport = useMemo(
    () => lightProfileOverrideSupport(stateQuery.data),
    [stateQuery.data]
  );

  const refreshTopology = topologyQuery.refresh;
  const action = useDeviceCall(
    useCallback(
      async (fn: () => Promise<unknown>) => {
        await fn();
        await refreshTopology();
      },
      [refreshTopology]
    )
  );
  const moveAction = useDeviceCall(
    useCallback(
      async (proposal: RoomMoveProposal, requestId: string) => {
        try {
          const receipt = await moveDeviceBetweenRoomsVerified(client, proposal, {
            requestId,
            onAccepted: setMoveReceipt
          });
          setMoveReceipt(receipt);
          await refreshTopology();
          return receipt;
        } catch (error) {
          throw new Error(`${errorMessage(error)} Request ${requestId}.`);
        }
      },
      [client, refreshTopology]
    )
  );
  const identifyAction = useDeviceCall(
    useCallback(
      async (device: TopologyItem) => {
        setIdentifyReceipt({
          deviceId: device.id,
          deviceName: device.name,
          status: 'pending'
        });
        try {
          await flashCanonicalDevice(client, device.id);
          setIdentifyReceipt({
            deviceId: device.id,
            deviceName: device.name,
            status: 'confirmed'
          });
        } catch (error) {
          setIdentifyReceipt({
            deviceId: device.id,
            deviceName: device.name,
            status: 'failed',
            error: errorMessage(error)
          });
          throw error;
        }
      },
      [client]
    )
  );

  function saveRoomOrder(next: string[]) {
    setRoomOrder(next);
    try {
      window.localStorage.setItem(storageKey, JSON.stringify(next));
    } catch {
      // Browser storage is optional; keep the in-memory visual order.
    }
  }

  async function refreshSettings() {
    await Promise.all([
      nodesQuery.refresh(),
      stateQuery.refresh(),
      refreshTopology()
    ]);
  }

  async function reviewMove(proposal: RoomMoveProposal) {
    const ok = await confirm({
      title: `Move ${proposal.deviceName}`,
      message: `Move ${proposal.deviceName} from ${proposal.fromRoomName} to ${proposal.toRoomName}? The card will stay in ${proposal.fromRoomName} until the appliance acknowledges the request and fresh topology confirms the new room.`,
      confirmLabel: 'Apply verified move'
    });
    if (!ok) return;
    const requestId = createRoomMoveRequestId();
    setMoveReceipt(null);
    await moveAction.run(proposal, requestId);
  }

  function proposeMove(deviceId: string, toRoomId: string) {
    const proposal = buildRoomMoveProposal(membership, deviceId, toRoomId);
    setDraggedDeviceId(null);
    setDragOverRoomId(null);
    if (proposal) void reviewMove(proposal);
  }

  function startDrag(event: DragEvent<HTMLElement>, device: TopologyItem) {
    event.dataTransfer.effectAllowed = 'move';
    event.dataTransfer.setData('application/x-rhythm-device', device.id);
    event.dataTransfer.setData('text/plain', device.id);
    setDraggedRoomId(null);
    setDraggedDeviceId(device.id);
  }

  function startRoomDrag(event: DragEvent<HTMLElement>, roomId: string) {
    event.stopPropagation();
    event.dataTransfer.effectAllowed = 'move';
    event.dataTransfer.setData('application/x-rhythm-room', roomId);
    event.dataTransfer.setData('text/plain', roomId);
    setDraggedDeviceId(null);
    setDraggedRoomId(roomId);
  }

  function allowRoomDrop(event: DragEvent<HTMLElement>, roomId: string) {
    if (draggedRoomId && draggedRoomId !== roomId) {
      event.preventDefault();
      event.dataTransfer.dropEffect = 'move';
      setDragOverRoomId(roomId);
      return;
    }
    if (!draggedDeviceId) return;
    if (!buildRoomMoveProposal(membership, draggedDeviceId, roomId)) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = 'move';
    setDragOverRoomId(roomId);
  }

  function dropInRoom(event: DragEvent<HTMLElement>, roomId: string) {
    event.preventDefault();
    const roomIdFromDrag =
      event.dataTransfer.getData('application/x-rhythm-room') || draggedRoomId;
    if (roomIdFromDrag) {
      saveRoomOrder(moveRoomBefore(roomOrder, roomIdFromDrag, roomId));
      setDraggedRoomId(null);
      setDragOverRoomId(null);
      return;
    }
    const deviceId =
      event.dataTransfer.getData('application/x-rhythm-device') ||
      draggedDeviceId;
    if (deviceId) proposeMove(deviceId, roomId);
  }

  async function reviewDeleteRoom(room: TopologyItem, childCount: number) {
    const ok = await confirm({
      title: 'Delete room',
      message: `Delete room "${room.name}"? ${childCount} assigned device${childCount === 1 ? '' : 's'} will become unassigned. If this room is backed by one authoritative integration room, that linked native room may also be deleted; a later integration sync may recreate rooms it still owns.`,
      confirmLabel: 'Delete room',
      danger: true
    });
    if (!ok) return;
    if (settingsTargetId === room.id) setSettingsTargetId(null);
    await action.run(() => deleteRoom(client, room.id));
  }

  function renderDeviceCard(device: TopologyItem, readonly = false) {
    const bulb = isBulbKind(device.kind);
    return (
      <article
        className={`roomAssignmentDevice${readonly ? ' readonly' : ''}${draggedDeviceId === device.id ? ' dragging' : ''}`}
        draggable={!readonly && !moveAction.busy && membership.rooms.length > 1}
        key={device.id}
        onDragStart={
          readonly ? undefined : (event) => startDrag(event, device)
        }
        onDragEnd={() => {
          setDraggedDeviceId(null);
          setDragOverRoomId(null);
        }}
      >
        {readonly ? (
          <AlertTriangle
            className="roomAssignmentWarning"
            size={15}
            aria-hidden="true"
          />
        ) : (
          <GripVertical
            className="roomAssignmentGrip"
            size={16}
            aria-hidden="true"
          />
        )}
        <button
          className={`roomAssignmentDeviceIdentity actionable${bulb ? ' identify' : ''}`}
          type="button"
          disabled={bulb && identifyAction.busy}
          aria-label={
            bulb
              ? `Identify ${device.name}`
              : `Open settings for ${device.name}`
          }
          onClick={() => {
            if (bulb) {
              void identifyAction.run(device);
            } else {
              setSettingsTargetId(device.id);
            }
          }}
        >
          <strong>{device.name}</strong>
          <span>
            {topologyKindLabel(device.kind)} ·{' '}
            {bulb ? 'Tap to identify' : 'Tap for settings'} ·{' '}
            <code>{device.id}</code>
          </span>
        </button>
        <span className="roomAssignmentDeviceActions">
          <button
            className="iconOnlyButton"
            type="button"
            aria-label={`Open settings for ${device.name}`}
            title="Settings"
            onClick={() => setSettingsTargetId(device.id)}
          >
            <Settings size={14} />
          </button>
          {!readonly ? (
            <button
              className="consoleButton small"
              type="button"
              disabled={moveAction.busy || membership.rooms.length < 2}
              aria-label={`Move ${device.name} to another room`}
              onClick={() => openMovePicker(device)}
            >
              <ArrowRight size={13} />
              <span>Move</span>
            </button>
          ) : null}
        </span>
      </article>
    );
  }

  function openMovePicker(device: TopologyItem) {
    setMovePickerDevice(device);
    setMovePickerTargetId('');
  }

  function closeMovePicker() {
    setMovePickerDevice(null);
    setMovePickerTargetId('');
  }

  function reviewPickedMove() {
    if (!movePickerDevice || !movePickerTargetId) return;
    const proposal = buildRoomMoveProposal(
      membership,
      movePickerDevice.id,
      movePickerTargetId
    );
    closeMovePicker();
    if (proposal) void reviewMove(proposal);
  }

  return (
    <div className="roomManagementStack">
      <SectionCard
        title="Rooms"
        subtitle={`${membership.rooms.length} room(s) · ${assignedDeviceCount} assigned device(s) · ${assignedBulbCount} bulb(s)`}
        busy={
          topologyQuery.refreshing ||
          moveAction.busy ||
          action.busy ||
          identifyAction.busy
        }
        error={
          topologyQuery.error ??
          moveAction.error ??
          action.error ??
          identifyAction.error
        }
        rawPayload={topologyQuery.data ?? undefined}
        actions={
          <span className="rowActions">
            <button
              className="consoleButton small"
              type="button"
              disabled={roomOrder.join('\u0000') === roomIds.join('\u0000')}
              title="Reset the browser-only visual room order"
              onClick={() => saveRoomOrder(roomIds)}
            >
              <RotateCcw size={13} />
              <span>Reset order</span>
            </button>
            <button
              className="consoleButton small"
              type="button"
              onClick={() => void topologyQuery.refresh()}
            >
              <RefreshCw size={14} />
              <span>Reload</span>
            </button>
          </span>
        }
      >
        <div className="roomAssignmentTrust">
          <ShieldCheck size={18} aria-hidden="true" />
          <div>
            <strong>Topology-backed, locally arranged</strong>
            <p>
              Drag room handles to arrange this browser only. Drag devices or use
              Move for distant rooms; every membership change is reviewed and
              confirmed from fresh appliance topology.
            </p>
          </div>
        </div>

        <div className="roomBoardToolbar">
          <TextField
            value={newRoomName}
            onChange={setNewRoomName}
            placeholder="New room name"
          />
          <button
            className="consoleButton"
            type="button"
            disabled={newRoomName.trim() === '' || action.busy}
            onClick={() => {
              void action.run(() => createRoom(client, newRoomName.trim()));
              setNewRoomName('');
            }}
          >
            Create room
          </button>
          <span className="roomBoardLocalNote">
            Room order is saved only in this browser.
          </span>
        </div>

        {membership.rooms.length === 0 ? (
          <EmptyState message="No rooms found in the topology payload — check Raw JSON." />
        ) : (
          <div className="roomAssignmentBoard" aria-label="Room assignment board">
            {orderedRooms.map(({ room, children }) => {
              const roomIndex = roomOrder.indexOf(room.id);
              const sections = segmentRoomDevices(children);
              const bulbCount = children.filter((device) =>
                isBulbKind(device.kind)
              ).length;
              return (
                <section
                  className={`roomAssignmentLane${dragOverRoomId === room.id ? ' dropTarget' : ''}${draggedRoomId === room.id ? ' dragging' : ''}`}
                  key={room.id}
                  aria-label={`${room.name} room, ${children.length} assigned device${children.length === 1 ? '' : 's'}`}
                  onDragEnter={(event) => allowRoomDrop(event, room.id)}
                  onDragOver={(event) => allowRoomDrop(event, room.id)}
                  onDragLeave={(event) => {
                    if (!event.currentTarget.contains(event.relatedTarget as Node)) {
                      setDragOverRoomId(null);
                    }
                  }}
                  onDrop={(event) => dropInRoom(event, room.id)}
                >
                <header className="roomAssignmentLaneHeader">
                  <div className="roomAssignmentOrderControls">
                    <button
                      className="roomAssignmentRoomGrip"
                      type="button"
                      draggable
                      aria-label={`Drag to reorder ${room.name}`}
                      title="Drag to reorder this room visually"
                      onDragStart={(event) => startRoomDrag(event, room.id)}
                      onDragEnd={() => {
                        setDraggedRoomId(null);
                        setDragOverRoomId(null);
                      }}
                    >
                      <GripVertical size={16} />
                    </button>
                    <button
                      className="iconOnlyButton"
                      type="button"
                      disabled={roomIndex <= 0}
                      aria-label={`Move ${room.name} earlier`}
                      onClick={() =>
                        saveRoomOrder(moveRoomByOffset(roomOrder, room.id, -1))
                      }
                    >
                      <ArrowLeft size={13} />
                    </button>
                    <button
                      className="iconOnlyButton"
                      type="button"
                      disabled={roomIndex < 0 || roomIndex >= roomOrder.length - 1}
                      aria-label={`Move ${room.name} later`}
                      onClick={() =>
                        saveRoomOrder(moveRoomByOffset(roomOrder, room.id, 1))
                      }
                    >
                      <ArrowRight size={13} />
                    </button>
                  </div>
                  <button
                    className="roomAssignmentRoomIdentity"
                    type="button"
                    aria-label={`Edit ${room.name} room and settings`}
                    onClick={() => setSettingsTargetId(room.id)}
                  >
                    <strong>{room.name}</strong>
                    <span className="mono" title={room.id}>
                      {room.id} · {bulbCount} bulb{bulbCount === 1 ? '' : 's'}
                    </span>
                  </button>
                  <span className="roomAssignmentLaneActions">
                    <span className="roomAssignmentCount">{children.length}</span>
                    <button
                      className="iconOnlyButton"
                      type="button"
                      title="Edit room and settings"
                      aria-label={`Edit ${room.name} room and settings`}
                      onClick={() => setSettingsTargetId(room.id)}
                    >
                      <Pencil size={13} />
                    </button>
                    <button
                      className="iconOnlyButton"
                      type="button"
                      title="Merge another room into this room"
                      aria-label={`Merge another room into ${room.name}`}
                      disabled={membership.rooms.length < 2}
                      onClick={() => {
                        setMergeTarget(room);
                        setMergeSourceId('');
                      }}
                    >
                      <Link2 size={13} />
                    </button>
                    <button
                      className="iconOnlyButton danger"
                      type="button"
                      title="Delete room"
                      aria-label={`Delete ${room.name} room`}
                      onClick={() => void reviewDeleteRoom(room, children.length)}
                    >
                      <Trash2 size={13} />
                    </button>
                  </span>
                </header>
                <div className="roomAssignmentCards">
                  {children.length === 0 ? (
                    <div className="roomAssignmentEmpty">Drop a device here</div>
                  ) : (
                    sections.map((section) => (
                      <section className="roomAssignmentDeviceSection" key={section.id}>
                        <header>
                          <span>{section.label}</span>
                          <span>{section.items.length}</span>
                        </header>
                        <div>
                          {section.items.map((device) => renderDeviceCard(device))}
                        </div>
                      </section>
                    ))
                  )}
                </div>
                </section>
              );
            })}

            {membership.unassigned.length > 0 ? (
              <section
                className="roomAssignmentLane readonly"
                aria-label={`Unassigned devices, ${membership.unassigned.length}`}
              >
                <header className="roomAssignmentLaneHeader">
                  <div>
                    <strong>Unassigned</strong>
                    <span>Read-only until guarded assignment is supported</span>
                  </div>
                  <span className="roomAssignmentCount">{membership.unassigned.length}</span>
                </header>
                <div className="roomAssignmentCards">
                  {segmentRoomDevices(membership.unassigned).map((section) => (
                    <section className="roomAssignmentDeviceSection" key={section.id}>
                      <header>
                        <span>{section.label}</span>
                        <span>{section.items.length}</span>
                      </header>
                      <div>
                        {section.items.map((device) => renderDeviceCard(device, true))}
                      </div>
                    </section>
                  ))}
                </div>
              </section>
            ) : null}
          </div>
        )}

        {moveReceipt ? (
          <div className={`roomMoveReceipt ${moveReceipt.status}`}>
            <div>
              {moveReceipt.status === 'confirmed' ? (
                <CheckCircle2 size={17} aria-hidden="true" />
              ) : moveReceipt.status === 'pending' ? (
                <Clock3 size={17} aria-hidden="true" />
              ) : (
                <AlertTriangle size={17} aria-hidden="true" />
              )}
              <span>
                {moveReceipt.status === 'confirmed'
                  ? `Confirmed: ${moveReceipt.candidate.deviceName} is in ${moveReceipt.candidate.roomName}.`
                  : moveReceipt.status === 'pending'
                    ? 'Accepted; waiting for authoritative topology confirmation.'
                    : 'The server read-back diverged from the proposed room. Review current topology before retrying.'}
              </span>
            </div>
            <code>{moveReceipt.requestId}</code>
            <RawPayloadToggle payload={moveReceipt} />
          </div>
        ) : null}

        {identifyReceipt ? (
          <div className={`roomIdentifyReceipt ${identifyReceipt.status}`}>
            {identifyReceipt.status === 'pending' ? (
              <Clock3 size={16} aria-hidden="true" />
            ) : identifyReceipt.status === 'failed' ? (
              <AlertTriangle size={16} aria-hidden="true" />
            ) : (
              <Zap size={16} aria-hidden="true" />
            )}
            <span>
              {identifyReceipt.status === 'pending'
                ? `Identifying ${identifyReceipt.deviceName}…`
                : identifyReceipt.status === 'failed'
                  ? `Identify failed for ${identifyReceipt.deviceName}: ${identifyReceipt.error ?? 'unknown error'}`
                  : `Identify command sent to ${identifyReceipt.deviceName}.`}
            </span>
            <code>{identifyReceipt.deviceId}</code>
          </div>
        ) : null}
      </SectionCard>

      {movePickerDevice ? (
        <Modal title={`Move ${movePickerDevice.name}`} onClose={closeMovePicker}>
          <FormRow
            label="Destination room"
            hint="The next step reviews the current and proposed room before writing"
          >
            <SelectField
              value={movePickerTargetId}
              onChange={setMovePickerTargetId}
              placeholder="Pick a different room"
              options={membership.rooms
                .filter(({ room }) => room.id !== movePickerDevice.parentId)
                .map(({ room }) => ({ value: room.id, label: room.name }))}
            />
          </FormRow>
          <div className="confirmActions">
            <button className="consoleButton" type="button" onClick={closeMovePicker}>
              Cancel
            </button>
            <button
              className="consoleButton primary"
              type="button"
              disabled={!movePickerTargetId}
              onClick={reviewPickedMove}
            >
              Review move
            </button>
          </div>
        </Modal>
      ) : null}

      {mergeTarget ? (
        <Modal
          title={`Merge a room into ${mergeTarget.name}`}
          onClose={() => setMergeTarget(null)}
        >
          <FormRow label="Source room" hint="Its devices move here; the source room is removed">
            <SelectField
              value={mergeSourceId}
              onChange={setMergeSourceId}
              placeholder="Pick a room"
              options={membership.rooms
                .map(({ room }) => room)
                .filter((room) => room.id !== mergeTarget.id)
                .map((room) => ({ value: room.id, label: room.name }))}
            />
          </FormRow>
          <div className="confirmActions">
            <button className="consoleButton" type="button" onClick={() => setMergeTarget(null)}>
              Cancel
            </button>
            <button
              className="consoleButton primary"
              type="button"
              disabled={mergeSourceId === ''}
              onClick={() => {
                void action.run(() => mergeRooms(client, mergeTarget.id, mergeSourceId));
                setMergeTarget(null);
              }}
            >
              Merge
            </button>
          </div>
        </Modal>
      ) : null}

      {settingsTargetId ? (
        <Modal
          title={`Settings · ${settingsNode?.name ?? settingsTargetId}`}
          onClose={() => setSettingsTargetId(null)}
          wide
        >
          {settingsNode ? (
            <NodeDetail
              key={settingsNode.id}
              node={settingsNode}
              parentProfileOverrides={
                liveNodes.find((node) => node.id === settingsNode.parentId)
                  ?.profileOverrides ?? {}
              }
              onWrite={refreshSettings}
              lightSettingsProfiles={lightSettingsProfiles}
              lightSettingsSupport={lightSettingsSupport}
              lightSettingsLoading={
                stateQuery.loading ||
                (stateQuery.refreshing && stateQuery.data === null)
              }
              lightSettingsError={stateQuery.error}
            />
          ) : (
            <EmptyState
              message={
                nodesQuery.error ??
                (nodesQuery.loading ||
                (nodesQuery.refreshing && nodesQuery.data === null)
                  ? 'Loading live device settings…'
                  : 'This topology item is not present in live node state.')
              }
              icon={<Settings size={20} />}
            />
          )}
        </Modal>
      ) : null}
    </div>
  );
}

/* -------------------------------- Triage -------------------------------- */

function TriageTab({
  client,
  onResolved
}: {
  client: DeviceClient;
  onResolved: () => void;
}) {
  const query = usePolling(
    useCallback(() => listTriage(client), [client]),
    { intervalMs: 0 }
  );
  const payload = query.data;
  const entries = Array.isArray(payload)
    ? asRecordArray(payload)
    : asRecordArray(asRecord(payload).entries);

  const [bodyModal, setBodyModal] = useState<{
    entryId: string;
    verb: 'bind' | 'room';
  } | null>(null);
  const [bodyText, setBodyText] = useState('{}');
  const [bodyParsed, setBodyParsed] = useState<unknown | undefined>({});

  const refresh = query.refresh;
  const action = useDeviceCall(
    useCallback(
      async (
        entryId: string,
        verb: 'merge' | 'new' | 'dismiss' | 'bind' | 'room',
        body?: Record<string, unknown>
      ) => {
        await resolveTriage(client, entryId, verb, body);
        await refresh();
        onResolved();
      },
      [client, refresh, onResolved]
    )
  );

  function entryId(entry: Record<string, unknown>): string {
    return asString(entry.id) ?? asString(entry.entry_id) ?? '';
  }

  return (
    <SectionCard
      title="Discovery triage"
      subtitle={`${entries.length} unresolved entr${entries.length === 1 ? 'y' : 'ies'}`}
      busy={query.refreshing || action.busy}
      error={query.error ?? action.error}
      rawPayload={query.data ?? undefined}
      actions={
        <button className="consoleButton small" type="button" onClick={() => void query.refresh()}>
          <RefreshCw size={14} />
          <span>Reload</span>
        </button>
      }
    >
      {entries.length === 0 ? (
        <EmptyState message="Triage queue is empty." />
      ) : (
        <div className="dataTableWrap">
          <table className="dataTable">
            <thead>
              <tr>
                <th>Entry</th>
                <th>Detail</th>
                <th>Resolve</th>
              </tr>
            </thead>
            <tbody>
              {entries.map((entry, index) => {
                const id = entryId(entry);
                return (
                  <tr key={id || index}>
                    <td className="mono dim">{id}</td>
                    <td className="mono">
                      {asString(entry.name) ??
                        asString(entry.label) ??
                        prettyJson(entry).slice(0, 200)}
                    </td>
                    <td>
                      <span className="rowActions">
                        <button
                          className="consoleButton small"
                          type="button"
                          onClick={() => void action.run(id, 'merge')}
                        >
                          Merge
                        </button>
                        <button
                          className="consoleButton small"
                          type="button"
                          onClick={() => void action.run(id, 'new')}
                        >
                          New
                        </button>
                        <button
                          className="consoleButton small"
                          type="button"
                          onClick={() => {
                            setBodyModal({ entryId: id, verb: 'bind' });
                            setBodyText('{}');
                            setBodyParsed({});
                          }}
                        >
                          Bind…
                        </button>
                        <button
                          className="consoleButton small"
                          type="button"
                          onClick={() => {
                            setBodyModal({ entryId: id, verb: 'room' });
                            setBodyText('{}');
                            setBodyParsed({});
                          }}
                        >
                          Room…
                        </button>
                        <button
                          className="consoleButton small danger"
                          type="button"
                          onClick={() => void action.run(id, 'dismiss')}
                        >
                          Dismiss
                        </button>
                      </span>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      {bodyModal ? (
        <Modal
          title={`Resolve ${bodyModal.entryId} → ${bodyModal.verb}`}
          onClose={() => setBodyModal(null)}
        >
          <p className="cardNote">
            Request body for PUT api/triage/{bodyModal.entryId}/{bodyModal.verb}
            {bodyModal.verb === 'bind'
              ? ' — typically {"device_id": "…"}'
              : ' — typically {"room_id": "…"} or {"name": "…"}'}
          </p>
          <JsonEditor
            value={bodyText}
            rows={6}
            onChange={(text, parsed) => {
              setBodyText(text);
              setBodyParsed(parsed);
            }}
          />
          <div className="confirmActions">
            <button className="consoleButton" type="button" onClick={() => setBodyModal(null)}>
              Cancel
            </button>
            <button
              className="consoleButton primary"
              type="button"
              disabled={bodyParsed === undefined}
              onClick={() => {
                void action.run(
                  bodyModal.entryId,
                  bodyModal.verb,
                  asRecord(bodyParsed)
                );
                setBodyModal(null);
              }}
            >
              Resolve
            </button>
          </div>
        </Modal>
      ) : null}
    </SectionCard>
  );
}

/* ------------------------------- Pairing ------------------------------- */

function PairingTab({ client }: { client: DeviceClient }) {
  const [hubType, setHubType] = useState('matter');
  const [setupPayload, setSetupPayload] = useState('');
  const [paramsText, setParamsText] = useState('{}');
  const [paramsParsed, setParamsParsed] = useState<unknown | undefined>({});
  const [pairResult, setPairResult] = useState<unknown>(null);

  const pairCall = useDeviceCall(
    useCallback(
      async (options: { hubType: string; params: Record<string, unknown> }) => {
        setPairResult(null);
        const result = await pairDevice(client, options);
        setPairResult(result);
      },
      [client]
    )
  );

  const capturesQuery = usePolling(
    useCallback(() => listMatterCaptures(client), [client]),
    { intervalMs: 0, enabled: hubType.trim().toLowerCase() === 'matter' }
  );

  const [bulbDeviceId, setBulbDeviceId] = useState('');
  const [bulbTest, setBulbTest] = useState('');
  const [bulbResult, setBulbResult] = useState<unknown>(null);
  const bulbCall = useDeviceCall(
    useCallback(
      async (deviceId: string, test: string) => {
        setBulbResult(null);
        const result = await runBulbTest(client, deviceId, test);
        setBulbResult(result);
      },
      [client]
    )
  );

  const isMatter = hubType.trim().toLowerCase() === 'matter';

  return (
    <>
      <SectionCard
        title="Pair a device"
        subtitle="Commissioning can take up to two minutes — leave the page open"
        busy={pairCall.busy}
        error={pairCall.error}
      >
        <FormRow label="Hub type">
          <TextField value={hubType} onChange={setHubType} mono />
        </FormRow>
        {isMatter ? (
          <FormRow
            label="Setup payload"
            hint="Matter QR payload (MT:…) or manual pairing code"
          >
            <TextField value={setupPayload} onChange={setSetupPayload} mono />
          </FormRow>
        ) : (
          <FormRow label="Params" hint="Raw params object for this hub type">
            <JsonEditor
              value={paramsText}
              rows={5}
              onChange={(text, parsed) => {
                setParamsText(text);
                setParamsParsed(parsed);
              }}
            />
          </FormRow>
        )}
        <div className="buttonRow">
          <button
            className="consoleButton primary"
            type="button"
            disabled={
              pairCall.busy ||
              (isMatter ? setupPayload.trim() === '' : paramsParsed === undefined)
            }
            onClick={() =>
              void pairCall.run({
                hubType: hubType.trim(),
                params: isMatter
                  ? {
                      setup_payload: setupPayload.trim(),
                      network: 'wifi',
                      rendezvous: 'auto'
                    }
                  : asRecord(paramsParsed)
              })
            }
          >
            {pairCall.busy ? <InlineSpinner /> : null}
            <span>{pairCall.busy ? 'Pairing…' : 'Pair device'}</span>
          </button>
        </div>
        {pairResult !== null ? <RawPayloadToggle payload={pairResult} /> : null}
      </SectionCard>

      {isMatter ? (
        <div className="cardGrid two">
          <SectionCard
            title="Matter captures"
            subtitle="Commissioning captures recorded by the device"
            busy={capturesQuery.refreshing}
            error={capturesQuery.error}
            rawPayload={capturesQuery.data ?? undefined}
            actions={
              <button
                className="consoleButton small"
                type="button"
                onClick={() => void capturesQuery.refresh()}
              >
                <RefreshCw size={14} />
                <span>Reload</span>
              </button>
            }
          >
            <KeyValueGrid
              rows={[
                [
                  'Captures',
                  Array.isArray(capturesQuery.data)
                    ? asArray(capturesQuery.data).length
                    : asArray(asRecord(capturesQuery.data).captures).length
                ]
              ]}
            />
          </SectionCard>

          <SectionCard
            title="Bulb tester"
            subtitle="Run a Matter bulb behaviour test"
            busy={bulbCall.busy}
            error={bulbCall.error}
          >
            <FormRow label="Device id">
              <TextField value={bulbDeviceId} onChange={setBulbDeviceId} mono />
            </FormRow>
            <FormRow label="Test" hint="Test name understood by the device">
              <TextField value={bulbTest} onChange={setBulbTest} mono />
            </FormRow>
            <div className="buttonRow">
              <button
                className="consoleButton"
                type="button"
                disabled={
                  bulbCall.busy || bulbDeviceId.trim() === '' || bulbTest.trim() === ''
                }
                onClick={() => void bulbCall.run(bulbDeviceId.trim(), bulbTest.trim())}
              >
                Run test
              </button>
            </div>
            {bulbResult !== null ? <RawPayloadToggle payload={bulbResult} /> : null}
          </SectionCard>
        </div>
      ) : null}
    </>
  );
}

/* --------------------------------- Hub --------------------------------- */

function HubTab({ client }: { client: DeviceClient }) {
  const confirm = useConfirm();
  const [hubType, setHubType] = useState('');
  const [address, setAddress] = useState('');
  const [credentialsText, setCredentialsText] = useState('{}');
  const [credentialsParsed, setCredentialsParsed] = useState<unknown | undefined>({});
  const [result, setResult] = useState<string | null>(null);

  const call = useDeviceCall(
    useCallback(async (fn: () => Promise<unknown>, message: string) => {
      setResult(null);
      await fn();
      setResult(message);
    }, [])
  );

  return (
    <div className="cardGrid two">
      <SectionCard
        title="Hub credentials"
        subtitle="Connect the device to a downstream controller (e.g. Hue bridge, HA)"
        busy={call.busy}
        error={call.error}
      >
        <FormRow label="Hub type">
          <TextField value={hubType} onChange={setHubType} mono placeholder="hue | home_assistant | …" />
        </FormRow>
        <FormRow label="Address">
          <TextField value={address} onChange={setAddress} mono placeholder="192.168.1.20" />
        </FormRow>
        <FormRow label="Credentials" hint="Free-form credentials object">
          <JsonEditor
            value={credentialsText}
            rows={5}
            onChange={(text, parsed) => {
              setCredentialsText(text);
              setCredentialsParsed(parsed);
            }}
          />
        </FormRow>
        <div className="buttonRow">
          <button
            className="consoleButton primary"
            type="button"
            disabled={
              call.busy ||
              hubType.trim() === '' ||
              address.trim() === '' ||
              credentialsParsed === undefined
            }
            onClick={() => {
              void (async () => {
                const ok = await confirm({
                  title: 'Save hub credentials',
                  message: `Store credentials for ${hubType} at ${address} on the device?`,
                  confirmLabel: 'Save credentials'
                });
                if (!ok) return;
                await call.run(
                  () =>
                    putHubCredentials(client, {
                      hubType: hubType.trim(),
                      address: address.trim(),
                      credentials: asRecord(credentialsParsed)
                    }),
                  'Credentials saved.'
                );
              })();
            }}
          >
            Save credentials
          </button>
          <button
            className="consoleButton danger"
            type="button"
            disabled={call.busy}
            onClick={() => {
              void (async () => {
                const scoped = hubType.trim() !== '' || address.trim() !== '';
                const ok = await confirm({
                  title: 'Delete hub credentials',
                  message: scoped
                    ? `Delete credentials for ${hubType || 'any type'} ${address || 'any address'}?`
                    : 'Delete ALL stored hub credentials on this device?',
                  confirmLabel: 'Delete',
                  danger: true
                });
                if (!ok) return;
                await call.run(
                  () =>
                    deleteHubCredentials(client, {
                      ...(hubType.trim() ? { hubType: hubType.trim() } : {}),
                      ...(address.trim() ? { address: address.trim() } : {})
                    }),
                  'Credentials deleted.'
                );
              })();
            }}
          >
            Delete credentials
          </button>
          <button
            className="consoleButton"
            type="button"
            disabled={call.busy || hubType.trim() === '' || address.trim() === ''}
            onClick={() =>
              void call.run(
                () => retryHub(client, hubType.trim(), address.trim()),
                'Retry requested.'
              )
            }
          >
            Retry connection
          </button>
        </div>
        {result ? <span className="resultNote">{result}</span> : null}
      </SectionCard>

      <SectionCard
        title="Sync"
        subtitle="Force re-synchronisation with downstream controllers"
        busy={call.busy}
      >
        <div className="buttonRow">
          <button
            className="consoleButton"
            type="button"
            disabled={call.busy}
            onClick={() => void call.run(() => syncAll(client), 'Full sync requested.')}
          >
            Sync everything
          </button>
        </div>
        <p className="cardNote">
          Syncs re-import devices and controls from connected hubs; new or
          changed devices land in the Triage tab.
        </p>
      </SectionCard>
    </div>
  );
}

/* -------------------------------- Wi-Fi -------------------------------- */

function WifiTab({ client }: { client: DeviceClient }) {
  const confirm = useConfirm();
  const query = usePolling(
    useCallback(() => getWifi(client), [client]),
    { intervalMs: 0 }
  );
  const wifi = asRecord(query.data);

  const [ssid, setSsid] = useState('');
  const [password, setPassword] = useState('');
  const [result, setResult] = useState<string | null>(null);

  const refresh = query.refresh;
  const call = useDeviceCall(
    useCallback(
      async (fn: () => Promise<unknown>, message: string) => {
        setResult(null);
        await fn();
        setResult(message);
        await refresh();
      },
      [refresh]
    )
  );

  return (
    <div className="cardGrid two">
      <SectionCard
        title="Wi-Fi status"
        busy={query.refreshing}
        error={query.error}
        rawPayload={query.data ?? undefined}
        actions={
          <button className="consoleButton small" type="button" onClick={() => void query.refresh()}>
            <RefreshCw size={14} />
            <span>Reload</span>
          </button>
        }
      >
        <KeyValueGrid
          rows={[
            [
              'Config present',
              asBoolean(wifi.config_present) === undefined
                ? undefined
                : asBoolean(wifi.config_present)
                  ? 'yes'
                  : 'no'
            ],
            [
              'Connected',
              asBoolean(wifi.connected) === undefined
                ? undefined
                : asBoolean(wifi.connected)
                  ? 'yes'
                  : 'no'
            ]
          ]}
        />
      </SectionCard>

      <SectionCard
        title="Change Wi-Fi"
        subtitle="Applying new credentials may take the device offline briefly"
        busy={call.busy}
        error={call.error}
      >
        <FormRow label="SSID">
          <TextField value={ssid} onChange={setSsid} />
        </FormRow>
        <FormRow label="Password">
          <TextField value={password} onChange={setPassword} mono />
        </FormRow>
        <div className="buttonRow">
          <button
            className="consoleButton primary"
            type="button"
            disabled={call.busy || ssid.trim() === ''}
            onClick={() => {
              void (async () => {
                const ok = await confirm({
                  title: 'Change Wi-Fi network',
                  message: `Point the device at "${ssid}"? If the credentials are wrong the device may drop offline until Wi-Fi is reset locally.`,
                  confirmLabel: 'Change Wi-Fi',
                  danger: true
                });
                if (!ok) return;
                await call.run(
                  () => setWifi(client, ssid.trim(), password),
                  'Wi-Fi credentials applied.'
                );
              })();
            }}
          >
            <Wifi size={15} />
            <span>Apply Wi-Fi</span>
          </button>
          <button
            className="consoleButton danger"
            type="button"
            disabled={call.busy}
            onClick={() => {
              void (async () => {
                const ok = await confirm({
                  title: 'Reset Wi-Fi',
                  message:
                    'Erase Wi-Fi configuration and drop the device into setup mode? It will disconnect and require local re-provisioning.',
                  confirmLabel: 'Reset Wi-Fi',
                  danger: true,
                  requireTypedText: 'reset-wifi'
                });
                if (!ok) return;
                await call.run(() => resetWifi(client), 'Wi-Fi reset requested.');
              })();
            }}
          >
            Reset Wi-Fi
          </button>
          {result ? <span className="resultNote">{result}</span> : null}
        </div>
      </SectionCard>
    </div>
  );
}
