import { useCallback, useMemo, useState, type DragEvent } from 'react';
import {
  AlertTriangle,
  ArrowRight,
  Check,
  CheckCircle2,
  Clock3,
  GripVertical,
  Lightbulb,
  Link2,
  Pencil,
  RefreshCw,
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
  renameRoom,
  resetWifi,
  resolveTriage,
  retryHub,
  runBulbTest,
  setCanonicalDeviceParent,
  setWifi,
  syncAll,
  unpairDevice
} from '../../device/topology';
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
  buildRoomMoveProposal,
  createRoomMoveRequestId,
  moveDeviceBetweenRoomsVerified,
  type RoomMoveProposal,
  type RoomMoveReceipt
} from './roomAssignment';
import {
  buildTopologyMembership,
  topologyKindLabel,
  type TopologyItem
} from './topologyMembership';

import '../../styles/pages-phase6.css';

type Tab = 'devices' | 'rooms' | 'triage' | 'pairing' | 'hub' | 'wifi';

export default function TopologyPage() {
  const client = useDeviceClient();
  const { hub } = useHub();
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
      {tab === 'rooms' ? <RoomsTab client={client} /> : null}
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
  const topologyQuery = usePolling(
    useCallback(() => getTopologyNodes(client), [client]),
    { intervalMs: 0 }
  );

  const membership = useMemo(
    () => buildTopologyMembership(topologyQuery.data),
    [topologyQuery.data]
  );
  const rooms = useMemo(
    () => membership.rooms.map(({ room }) => room.raw),
    [membership.rooms]
  );
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
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draftName, setDraftName] = useState('');
  const [mergeTarget, setMergeTarget] = useState<Record<string, unknown> | null>(null);
  const [mergeSourceId, setMergeSourceId] = useState('');
  const [draggedDeviceId, setDraggedDeviceId] = useState<string | null>(null);
  const [dragOverRoomId, setDragOverRoomId] = useState<string | null>(null);
  const [movePickerDevice, setMovePickerDevice] = useState<TopologyItem | null>(null);
  const [movePickerTargetId, setMovePickerTargetId] = useState('');
  const [moveReceipt, setMoveReceipt] = useState<RoomMoveReceipt | null>(null);

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

  function roomId(room: Record<string, unknown>): string {
    return asString(room.id) ?? asString(room.room_id) ?? asString(room.node_id) ?? '';
  }
  function roomName(room: Record<string, unknown>): string {
    return asString(room.name) ?? asString(room.label) ?? roomId(room);
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
    event.dataTransfer.setData('text/plain', device.id);
    setDraggedDeviceId(device.id);
  }

  function allowRoomDrop(event: DragEvent<HTMLElement>, roomId: string) {
    if (!draggedDeviceId) return;
    if (!buildRoomMoveProposal(membership, draggedDeviceId, roomId)) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = 'move';
    setDragOverRoomId(roomId);
  }

  function dropInRoom(event: DragEvent<HTMLElement>, roomId: string) {
    event.preventDefault();
    const deviceId = event.dataTransfer.getData('text/plain') || draggedDeviceId;
    if (deviceId) proposeMove(deviceId, roomId);
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
        title="Room assignment"
        subtitle={`${rooms.length} room(s) · ${assignedDeviceCount} assigned device(s) · ${assignedBulbCount} bulb(s)`}
        busy={topologyQuery.refreshing || moveAction.busy}
        error={topologyQuery.error ?? moveAction.error}
        rawPayload={topologyQuery.data ?? undefined}
        actions={
          <button
            className="consoleButton small"
            type="button"
            onClick={() => void topologyQuery.refresh()}
          >
            <RefreshCw size={14} />
            <span>Reload</span>
          </button>
        }
      >
        <div className="roomAssignmentTrust">
          <ShieldCheck size={18} aria-hidden="true" />
          <div>
            <strong>Verified moves only</strong>
            <p>
              Drag a card or use Move. Every change is reviewed, checked against
              its current room on the appliance, and shown as complete only after
              fresh topology confirms it.
            </p>
          </div>
        </div>

        {membership.rooms.length === 0 ? (
          <EmptyState message="No rooms found in the topology payload — check Raw JSON." />
        ) : (
          <div className="roomAssignmentBoard" aria-label="Room assignment board">
            {membership.rooms.map(({ room, children }) => (
              <section
                className={`roomAssignmentLane${dragOverRoomId === room.id ? ' dropTarget' : ''}`}
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
                  <div>
                    <strong>{room.name}</strong>
                    <span className="mono" title={room.id}>{room.id}</span>
                  </div>
                  <span className="roomAssignmentCount">{children.length}</span>
                </header>
                <div className="roomAssignmentCards">
                  {children.length === 0 ? (
                    <div className="roomAssignmentEmpty">Drop a device here</div>
                  ) : (
                    children.map((device) => (
                      <article
                        className={`roomAssignmentDevice${draggedDeviceId === device.id ? ' dragging' : ''}`}
                        draggable={!moveAction.busy && membership.rooms.length > 1}
                        key={device.id}
                        onDragStart={(event) => startDrag(event, device)}
                        onDragEnd={() => {
                          setDraggedDeviceId(null);
                          setDragOverRoomId(null);
                        }}
                      >
                        <GripVertical className="roomAssignmentGrip" size={16} aria-hidden="true" />
                        <div className="roomAssignmentDeviceIdentity">
                          <strong>{device.name}</strong>
                          <span>
                            {topologyKindLabel(device.kind)} · <code>{device.id}</code>
                          </span>
                        </div>
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
                      </article>
                    ))
                  )}
                </div>
              </section>
            ))}

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
                  {membership.unassigned.map((device) => (
                    <article className="roomAssignmentDevice readonly" key={device.id}>
                      <AlertTriangle className="roomAssignmentWarning" size={15} aria-hidden="true" />
                      <div className="roomAssignmentDeviceIdentity">
                        <strong>{device.name}</strong>
                        <span>
                          {topologyKindLabel(device.kind)} · <code>{device.id}</code>
                        </span>
                      </div>
                    </article>
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
      </SectionCard>

      <SectionCard
        title="Room administration"
        subtitle="Create, rename, merge, and delete topology rooms"
        busy={action.busy}
        error={action.error}
      >
        <div className="buttonRow">
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
        </div>

        {rooms.length === 0 ? (
          <EmptyState message="No rooms available." />
        ) : (
          <div className="dataTableWrap">
            <table className="dataTable">
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Id</th>
                  <th>Assigned</th>
                  <th>Actions</th>
                </tr>
              </thead>
              <tbody>
                {rooms.map((room, index) => {
                  const id = roomId(room);
                  const name = roomName(room);
                  const isEditing = editingId === id;
                  const roomMembership = membership.rooms.find(
                    ({ room: memberRoom }) => memberRoom.id === id
                  );
                  const deviceCount = roomMembership?.children.length ?? 0;
                  const bulbCount = roomMembership?.bulbs.length ?? 0;
                  return (
                    <tr key={id || index}>
                      <td>
                        {isEditing ? (
                          <span className="inlineEdit">
                            <TextField value={draftName} onChange={setDraftName} />
                            <button
                              className="iconOnlyButton"
                              type="button"
                              aria-label="Save room name"
                              onClick={() => {
                                setEditingId(null);
                                if (draftName.trim() && draftName !== name) {
                                  void action.run(() => renameRoom(client, id, draftName.trim()));
                                }
                              }}
                            >
                              <Check size={14} />
                            </button>
                            <button
                              className="iconOnlyButton"
                              type="button"
                              aria-label="Cancel"
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
                        {deviceCount} device{deviceCount === 1 ? '' : 's'} · {bulbCount}{' '}
                        bulb{bulbCount === 1 ? '' : 's'}
                      </td>
                      <td>
                        <span className="rowActions">
                          <button
                            className="consoleButton small"
                            type="button"
                            onClick={() => {
                              setMergeTarget(room);
                              setMergeSourceId('');
                            }}
                          >
                            Merge into
                          </button>
                          <button
                            className="consoleButton small danger"
                            type="button"
                            onClick={() => {
                              void (async () => {
                                const ok = await confirm({
                                  title: 'Delete room',
                                  message: `Delete room "${name}"? Devices in it become roomless / go to triage.`,
                                  confirmLabel: 'Delete room',
                                  danger: true
                                });
                                if (ok) {
                                  await action.run(() => deleteRoom(client, id));
                                }
                              })();
                            }}
                          >
                            <Trash2 size={13} />
                            <span>Delete</span>
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
          title={`Merge a room into ${roomName(mergeTarget)}`}
          onClose={() => setMergeTarget(null)}
        >
          <FormRow label="Source room" hint="Its devices move here; the source room is removed">
            <SelectField
              value={mergeSourceId}
              onChange={setMergeSourceId}
              placeholder="Pick a room"
              options={rooms
                .filter((room) => roomId(room) !== roomId(mergeTarget))
                .map((room) => ({ value: roomId(room), label: roomName(room) }))}
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
                void action.run(() => mergeRooms(client, roomId(mergeTarget), mergeSourceId));
                setMergeTarget(null);
              }}
            >
              Merge
            </button>
          </div>
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
