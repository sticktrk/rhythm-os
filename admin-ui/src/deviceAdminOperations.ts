export type DeviceAdminMethod = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';

export type DeviceAdminOperation = {
  id: string;
  category: string;
  label: string;
  description: string;
  method: DeviceAdminMethod;
  path: string;
  query?: Record<string, string | number | boolean>;
  body?: unknown;
  danger?: boolean;
  timeoutSeconds?: number;
};

export const DEVICE_ADMIN_OPERATIONS: DeviceAdminOperation[] = [
  {
    id: 'custom-json',
    category: 'Custom',
    label: 'Custom JSON request',
    description: 'Run any relative JSON endpoint under /api/*.',
    method: 'GET',
    path: 'api/state'
  },
  {
    id: 'state',
    category: 'State',
    label: 'Read server state',
    description: 'Fetch /api/state for the selected device.',
    method: 'GET',
    path: 'api/state'
  },
  {
    id: 'state-authoritative',
    category: 'State',
    label: 'Read authoritative state',
    description: 'Fetch fresh authoritative server state.',
    method: 'GET',
    path: 'api/state',
    query: { authoritative: true }
  },
  {
    id: 'nodes-state',
    category: 'State',
    label: 'Read node runtime state',
    description: 'Fetch current node state snapshots.',
    method: 'GET',
    path: 'api/nodes/state'
  },
  {
    id: 'health',
    category: 'State',
    label: 'Read health',
    description: 'Fetch the device health endpoint.',
    method: 'GET',
    path: 'health'
  },
  {
    id: 'settings-get',
    category: 'Global Settings',
    label: 'Read settings',
    description: 'Fetch global server settings.',
    method: 'GET',
    path: 'api/settings'
  },
  {
    id: 'settings-set',
    category: 'Global Settings',
    label: 'Set settings',
    description: 'Patch global server settings.',
    method: 'PUT',
    path: 'api/settings',
    body: { auto_update: true }
  },
  {
    id: 'settings-update-channel',
    category: 'Global Settings',
    label: 'Set update channel',
    description: 'Switch the OTA release channel (beta or stable).',
    method: 'PUT',
    path: 'api/settings',
    body: { update_channel: 'stable' }
  },
  {
    id: 'light-breaker-get',
    category: 'Global Settings',
    label: 'Read light breaker',
    description: 'Fetch the autonomous light-control switch.',
    method: 'GET',
    path: 'api/light-breaker'
  },
  {
    id: 'light-breaker-set',
    category: 'Global Settings',
    label: 'Set light breaker',
    description: 'Enable or disable autonomous light control.',
    method: 'PUT',
    path: 'api/light-breaker',
    body: { enabled: true }
  },
  {
    id: 'mode-get',
    category: 'Global Settings',
    label: 'Read mode',
    description: 'Fetch mode state, profile routing, and mode config.',
    method: 'GET',
    path: 'api/mode'
  },
  {
    id: 'mode-set',
    category: 'Global Settings',
    label: 'Set active mode',
    description: 'Set day, sleep, or mode configs.',
    method: 'PUT',
    path: 'api/mode',
    body: { active: 'day' }
  },
  {
    id: 'light-runtime-get',
    category: 'Global Settings',
    label: 'Read light runtime',
    description: 'Fetch the active lighting runtime.',
    method: 'GET',
    path: 'api/light-runtime'
  },
  {
    id: 'light-runtime-set',
    category: 'Global Settings',
    label: 'Set light runtime',
    description: 'Select the active lighting runtime.',
    method: 'PUT',
    path: 'api/light-runtime',
    body: { runtime_id: 'rhythm_adaptive', transition_ms: 800 }
  },
  {
    id: 'profiles-get',
    category: 'Global Settings',
    label: 'Read profiles',
    description: 'Fetch available profile configs.',
    method: 'GET',
    path: 'api/profiles'
  },
  {
    id: 'transitions-get',
    category: 'Global Settings',
    label: 'Read transitions',
    description: 'Fetch transition config list.',
    method: 'GET',
    path: 'api/transitions'
  },
  {
    id: 'transitions-set',
    category: 'Global Settings',
    label: 'Set transitions',
    description: 'Replace transition config list.',
    method: 'PUT',
    path: 'api/transitions',
    body: { transitions: [] }
  },
  {
    id: 'transition-trigger',
    category: 'Global Settings',
    label: 'Trigger transition',
    description: 'Run a saved transition immediately.',
    method: 'POST',
    path: 'api/transitions/{transition_id}/trigger',
    body: {}
  },
  {
    id: 'input-bindings-get',
    category: 'Global Settings',
    label: 'Read input bindings',
    description: 'Fetch persisted physical input bindings.',
    method: 'GET',
    path: 'api/input-bindings'
  },
  {
    id: 'input-binding-create',
    category: 'Global Settings',
    label: 'Create input binding',
    description: 'Create a preset-backed input binding.',
    method: 'POST',
    path: 'api/input-bindings',
    body: {
      preset: 'day_sleep_toggle',
      source_node_id: '{button_node_id}',
      button_action: 'on_press',
      enabled: true
    }
  },
  {
    id: 'input-binding-set',
    category: 'Global Settings',
    label: 'Set input binding',
    description: 'Create or replace a physical input binding.',
    method: 'PUT',
    path: 'api/input-bindings/{binding_id}',
    body: {
      id: '{binding_id}',
      source_node_id: '{button_node_id}',
      preset: 'day_sleep_toggle',
      button_action: 'on_press',
      enabled: true
    }
  },
  {
    id: 'input-binding-delete',
    category: 'Global Settings',
    label: 'Delete input binding',
    description: 'Delete a persisted physical input binding.',
    method: 'DELETE',
    path: 'api/input-bindings/{binding_id}',
    danger: true
  },
  {
    id: 'location-set',
    category: 'Profiles & Curves',
    label: 'Set location',
    description: 'Set solar location and timezone.',
    method: 'PUT',
    path: 'api/location',
    body: {
      lat: 35.7796,
      lon: -78.6382,
      timezone_name: 'America/New_York'
    }
  },
  {
    id: 'config-get',
    category: 'Profiles & Curves',
    label: 'Read profile config',
    description: 'Fetch a stored profile config.',
    method: 'GET',
    path: 'api/config',
    query: { id: 'rhythm' }
  },
  {
    id: 'config-set',
    category: 'Profiles & Curves',
    label: 'Set profile config',
    description: 'Save a full profile config.',
    method: 'PUT',
    path: 'api/config',
    query: { id: 'rhythm', apply: true },
    body: { id: 'rhythm' }
  },
  {
    id: 'config-reset',
    category: 'Profiles & Curves',
    label: 'Reset profile config',
    description: 'Reset a profile to built-in defaults.',
    method: 'POST',
    path: 'api/config/reset',
    query: { id: 'rhythm' },
    danger: true
  },
  {
    id: 'curve-get',
    category: 'Profiles & Curves',
    label: 'Read curve preview',
    description: 'Preview a profile curve for a date.',
    method: 'GET',
    path: 'api/curve',
    query: {
      id: 'rhythm',
      date: '2026-07-04',
      samples_per_hour: 4,
      start_hour: 12
    }
  },
  {
    id: 'curve-post',
    category: 'Profiles & Curves',
    label: 'Preview draft curve',
    description: 'Preview a draft profile config without saving it.',
    method: 'POST',
    path: 'api/curve',
    query: {
      id: 'rhythm',
      date: '2026-07-04',
      samples_per_hour: 4,
      start_hour: 12
    },
    body: { id: 'rhythm' }
  },
  {
    id: 'curve-now',
    category: 'Profiles & Curves',
    label: 'Read curve now',
    description: 'Fetch the current sampled value for a profile.',
    method: 'GET',
    path: 'api/curve/now',
    query: { id: 'rhythm' }
  },
  {
    id: 'curve-solar',
    category: 'Profiles & Curves',
    label: 'Read solar curve context',
    description: 'Fetch solar data for a date.',
    method: 'GET',
    path: 'api/curve/solar',
    query: { date: '2026-07-04' }
  },
  {
    id: 'absorb-offset',
    category: 'Profiles & Curves',
    label: 'Absorb time offset',
    description: 'Bake a preview offset into a profile curve.',
    method: 'POST',
    path: 'api/config/absorb-offset',
    query: { id: 'rhythm' },
    body: { offset_minutes: 15 }
  },
  {
    id: 'profile-bundle-get',
    category: 'Profiles & Curves',
    label: 'Read profile bundle',
    description: 'Fetch the full profile bundle document.',
    method: 'GET',
    path: 'api/profile-bundle'
  },
  {
    id: 'profile-bundle-factory-default',
    category: 'Profiles & Curves',
    label: 'Read factory profile bundle',
    description: 'Fetch the factory-default profile bundle document.',
    method: 'GET',
    path: 'api/profile-bundle/factory-default'
  },
  {
    id: 'profile-bundle-put',
    category: 'Profiles & Curves',
    label: 'Restore profile bundle',
    description: 'Save a full profile bundle document.',
    method: 'PUT',
    path: 'api/profile-bundle',
    body: {}
  },
  {
    id: 'profile-bundle-reset',
    category: 'Profiles & Curves',
    label: 'Reset profile bundle',
    description: 'Reset profile bundle to factory defaults.',
    method: 'POST',
    path: 'api/profile-bundle/reset',
    danger: true
  },
  {
    id: 'share-bundle-get',
    category: 'Profiles & Curves',
    label: 'Read share bundle',
    description: 'Fetch the share bundle document.',
    method: 'GET',
    path: 'api/share-bundle'
  },
  {
    id: 'share-bundle-factory-default',
    category: 'Profiles & Curves',
    label: 'Read factory share bundle',
    description: 'Fetch the factory-default share bundle document.',
    method: 'GET',
    path: 'api/share-bundle/factory-default'
  },
  {
    id: 'share-bundle-put',
    category: 'Profiles & Curves',
    label: 'Restore share bundle',
    description: 'Save a share bundle document.',
    method: 'PUT',
    path: 'api/share-bundle',
    body: {}
  },
  {
    id: 'share-bundle-reset',
    category: 'Profiles & Curves',
    label: 'Reset share bundle',
    description: 'Reset share bundle to factory defaults.',
    method: 'POST',
    path: 'api/share-bundle/reset',
    danger: true
  },
  {
    id: 'backup-read',
    category: 'Profiles & Curves',
    label: 'Read backup bundle',
    description: 'Fetch the full device backup JSON bundle.',
    method: 'GET',
    path: 'api/backup',
    query: { include_secrets: false },
    timeoutSeconds: 60
  },
  {
    id: 'backup-restore',
    category: 'Profiles & Curves',
    label: 'Restore backup bundle',
    description: 'Restore a full device backup JSON bundle.',
    method: 'PUT',
    path: 'api/backup',
    body: {},
    danger: true,
    timeoutSeconds: 60
  },
  {
    id: 'node-action',
    category: 'Node Runtime',
    label: 'Run node action',
    description: 'Run lights_on, lights_off, toggle, or reset on a node.',
    method: 'PUT',
    path: 'api/nodes/action',
    body: { node_id: '{node_id}', action: 'lights_on' }
  },
  {
    id: 'node-brightness',
    category: 'Node Runtime',
    label: 'Set brightness',
    description: 'Set node or room brightness.',
    method: 'PUT',
    path: 'api/nodes/brightness',
    body: { node_id: '{node_id}', brightness: 80 }
  },
  {
    id: 'node-color',
    category: 'Node Runtime',
    label: 'Set color',
    description: 'Set node RGB color and optional brightness.',
    method: 'PUT',
    path: 'api/nodes/color',
    body: {
      node_id: '{node_id}',
      rgb: { r: 255, g: 180, b: 120 },
      brightness: 80,
      transition_ms: 500
    }
  },
  {
    id: 'node-curve',
    category: 'Node Runtime',
    label: 'Set curve modifier',
    description: 'Modify brightness or color temperature on the active curve.',
    method: 'PUT',
    path: 'api/nodes/curve',
    body: {
      node_id: '{node_id}',
      brightness: 80,
      color_temperature: 3000,
      preserve_brightness: true
    }
  },
  {
    id: 'node-offset',
    category: 'Node Runtime',
    label: 'Preview time offset',
    description: 'Apply a temporary time offset to selected nodes.',
    method: 'PUT',
    path: 'api/nodes/offset',
    body: { time_offset: 15, nodes: ['{node_id}'] }
  },
  {
    id: 'node-preferences',
    category: 'Node Runtime',
    label: 'Set node preferences',
    description: 'Set node rhythm, disabled, standby, mode, and profile values.',
    method: 'PUT',
    path: 'api/nodes/preferences',
    body: {
      node_id: '{node_id}',
      rhythm_enabled: true,
      disabled: false,
      standby_enabled: true,
      state: 'active',
      profile_settings: {}
    }
  },
  {
    id: 'node-motion-activation',
    category: 'Node Runtime',
    label: 'Set motion activation',
    description: 'Enable or disable motion-triggered activation for a node.',
    method: 'PUT',
    path: 'api/nodes/motion-activation',
    body: {
      node_id: '{node_id}',
      enabled: true,
      request_id: '{request_id}'
    }
  },
  {
    id: 'node-profile-overrides',
    category: 'Node Runtime',
    label: 'Set profile overrides',
    description: 'Patch per-node profile overrides.',
    method: 'PUT',
    path: 'api/nodes/profile-overrides',
    body: { node_id: '{node_id}', profile_overrides: {} }
  },
  {
    id: 'scenes-get',
    category: 'Scenes',
    label: 'Read scenes',
    description: 'Fetch saved scene definitions.',
    method: 'GET',
    path: 'api/scenes'
  },
  {
    id: 'scene-upsert',
    category: 'Scenes',
    label: 'Upsert scene',
    description: 'Create or replace a scene definition.',
    method: 'POST',
    path: 'api/scenes',
    body: { id: '{scene_id}', name: 'Scene name', layers: [] }
  },
  {
    id: 'scene-put',
    category: 'Scenes',
    label: 'Replace scene',
    description: 'Replace a scene by path ID.',
    method: 'PUT',
    path: 'api/scenes/{scene_id}',
    body: { id: '{scene_id}', name: 'Scene name', layers: [] }
  },
  {
    id: 'scene-apply',
    category: 'Scenes',
    label: 'Apply scene',
    description: 'Apply a saved scene to a node or room.',
    method: 'POST',
    path: 'api/scenes/{scene_id}/apply',
    body: { target_id: '{node_id}', transition_ms: 500 }
  },
  {
    id: 'scene-preview',
    category: 'Scenes',
    label: 'Preview scene',
    description: 'Preview a saved scene temporarily.',
    method: 'POST',
    path: 'api/scenes/{scene_id}/preview',
    body: { target_id: '{node_id}', transition_ms: 500, duration_ms: 30000 }
  },
  {
    id: 'scene-preview-draft',
    category: 'Scenes',
    label: 'Preview draft scene',
    description: 'Preview an unsaved draft scene temporarily.',
    method: 'POST',
    path: 'api/scenes/preview',
    body: {
      scene: { id: '{scene_id}', name: 'Scene name', layers: [] },
      target_id: '{node_id}',
      transition_ms: 500,
      duration_ms: 30000
    }
  },
  {
    id: 'scene-preview-commit',
    category: 'Scenes',
    label: 'Commit scene preview',
    description: 'Commit an active scene preview.',
    method: 'POST',
    path: 'api/scene-previews/{preview_id}/commit',
    body: {}
  },
  {
    id: 'scene-preview-cancel',
    category: 'Scenes',
    label: 'Cancel scene preview',
    description: 'Cancel an active scene preview.',
    method: 'POST',
    path: 'api/scene-previews/{preview_id}/cancel',
    body: {}
  },
  {
    id: 'scene-delete',
    category: 'Scenes',
    label: 'Delete scene',
    description: 'Delete a saved scene.',
    method: 'DELETE',
    path: 'api/scenes/{scene_id}',
    danger: true
  },
  {
    id: 'canonical-devices-get',
    category: 'Devices & Topology',
    label: 'Read canonical devices',
    description: 'Fetch all canonical devices.',
    method: 'GET',
    path: 'api/devices/canonical'
  },
  {
    id: 'canonical-device-get',
    category: 'Devices & Topology',
    label: 'Read canonical device',
    description: 'Fetch one canonical device by ID.',
    method: 'GET',
    path: 'api/devices/canonical/{device_id}'
  },
  {
    id: 'topology-nodes-get',
    category: 'Devices & Topology',
    label: 'Read topology nodes',
    description: 'Fetch the node topology graph.',
    method: 'GET',
    path: 'api/topology/nodes'
  },
  {
    id: 'assistant-contract-get',
    category: 'Devices & Topology',
    label: 'Read assistant contract',
    description: 'Fetch the runtime assistant operation contract.',
    method: 'GET',
    path: 'api/assistant/contract'
  },
  {
    id: 'assistant-topology-get',
    category: 'Devices & Topology',
    label: 'Read assistant topology',
    description: 'Fetch a freshness-bound assistant topology snapshot.',
    method: 'GET',
    path: 'api/assistant/topology'
  },
  {
    id: 'assistant-device-room-move-plan',
    category: 'Devices & Topology',
    label: 'Plan assistant device move',
    description: 'Prepare a non-mutating reviewed device-to-room move plan.',
    method: 'POST',
    path: 'api/assistant/plans/device-room-move',
    body: {
      device_id: '{device_id}',
      to_room_id: '{to_room_id}',
      correlation_id: 'admin-preview-001'
    }
  },
  {
    id: 'assistant-device-room-move-apply',
    category: 'Devices & Topology',
    label: 'Apply assistant device move',
    description: 'Apply one explicitly reviewed, freshness-bound device move plan.',
    method: 'POST',
    path: 'api/assistant/plans/device-room-move/apply',
    body: {
      plan_id: '{plan_id}',
      correlation_id: 'admin-preview-001',
      operation: 'apply_move_device_room_plan',
      contract_sha256: '{contract_sha256}',
      server_instance_id: '{server_instance_id}',
      topology_resource_sha256: '{topology_resource_sha256}',
      device_id: '{device_id}',
      from_room_id: '{from_room_id}',
      to_room_id: '{to_room_id}'
    },
    danger: true
  },
  {
    id: 'topology-room-create',
    category: 'Devices & Topology',
    label: 'Create room',
    description: 'Create a topology room.',
    method: 'POST',
    path: 'api/topology/rooms',
    body: { name: 'New room' }
  },
  {
    id: 'topology-room-rename',
    category: 'Devices & Topology',
    label: 'Rename room',
    description: 'Rename a topology room.',
    method: 'PUT',
    path: 'api/topology/rooms/{room_id}',
    body: { name: 'Room name' }
  },
  {
    id: 'topology-room-merge',
    category: 'Devices & Topology',
    label: 'Merge rooms',
    description: 'Merge one topology room into another.',
    method: 'PUT',
    path: 'api/topology/rooms/{target_room_id}/merge',
    body: { source_id: '{source_room_id}' },
    danger: true
  },
  {
    id: 'topology-room-delete',
    category: 'Devices & Topology',
    label: 'Delete room',
    description: 'Delete a topology room.',
    method: 'DELETE',
    path: 'api/topology/rooms/{room_id}',
    danger: true
  },
  {
    id: 'topology-move-device',
    category: 'Devices & Topology',
    label: 'Move device',
    description: 'Move a canonical device between topology rooms.',
    method: 'PUT',
    path: 'api/topology/rooms/{to_room_id}/devices/move',
    body: { device_id: '{device_id}', from_room: '{from_room_id}' }
  },
  {
    id: 'topology-control-target',
    category: 'Devices & Topology',
    label: 'Set control target',
    description: 'Set or clear an explicit topology control target.',
    method: 'PUT',
    path: 'api/topology/nodes/{node_id}/controls/{control_kind}',
    body: { target_id: '{target_id}' }
  },
  {
    id: 'device-rename',
    category: 'Devices & Topology',
    label: 'Rename canonical device',
    description: 'Rename a canonical device.',
    method: 'PUT',
    path: 'api/devices/canonical/{device_id}',
    body: { name: 'Device name' }
  },
  {
    id: 'device-assign-parent',
    category: 'Devices & Topology',
    label: 'Assign device parent',
    description: 'Assign or clear a canonical device parent/room.',
    method: 'PUT',
    path: 'api/devices/canonical/{device_id}/parent',
    body: { parent_id: '{room_id}' }
  },
  {
    id: 'device-flash',
    category: 'Devices & Topology',
    label: 'Flash canonical device',
    description: 'Pulse a light for physical identification.',
    method: 'POST',
    path: 'api/devices/canonical/{device_id}/flash',
    body: {}
  },
  {
    id: 'device-pair',
    category: 'Devices & Topology',
    label: 'Pair device',
    description: 'Start device pairing through a hub integration.',
    method: 'POST',
    path: 'api/devices/pair',
    body: {
      hub_type: 'matter',
      params: {
        setup_payload: '{matter_setup_payload}',
        network: 'wifi',
        rendezvous: 'auto'
      }
    },
    timeoutSeconds: 60
  },
  {
    id: 'device-pair-result',
    category: 'Devices & Topology',
    label: 'Read pairing result',
    description: 'Fetch a durable pairing result by session ID.',
    method: 'GET',
    path: 'api/devices/pair/{session_id}'
  },
  {
    id: 'device-pair-result-acknowledge',
    category: 'Devices & Topology',
    label: 'Acknowledge pairing result',
    description: 'Mark a durable pairing result as consumed.',
    method: 'DELETE',
    path: 'api/devices/pair/{session_id}',
    danger: true
  },
  {
    id: 'device-unpair',
    category: 'Devices & Topology',
    label: 'Unpair device',
    description: 'Remove a paired integration device.',
    method: 'POST',
    path: 'api/devices/unpair',
    body: {
      hub_type: 'matter',
      params: { device_id: '{device_id}', force: false }
    },
    danger: true
  },
  {
    id: 'matter-wifi-get',
    category: 'Devices & Topology',
    label: 'Read Wi-Fi status',
    description: 'Fetch appliance Wi-Fi status used by Matter pairing.',
    method: 'GET',
    path: 'api/wifi'
  },
  {
    id: 'matter-captures-get',
    category: 'Devices & Topology',
    label: 'Read Matter captures',
    description: 'Fetch Matter device capture summaries.',
    method: 'GET',
    path: 'api/matter/captures'
  },
  {
    id: 'matter-capture-get',
    category: 'Devices & Topology',
    label: 'Read Matter capture',
    description: 'Fetch a Matter capture by ID.',
    method: 'GET',
    path: 'api/matter/captures/{capture_id}'
  },
  {
    id: 'matter-bulb-test',
    category: 'Devices & Topology',
    label: 'Run Matter bulb test',
    description: 'Run a raw Matter bulb tester command.',
    method: 'POST',
    path: 'api/matter/bulb-test/run',
    body: { device_id: '{device_id}', test: 'on_off' }
  },
  {
    id: 'matter-bulb-report',
    category: 'Devices & Topology',
    label: 'Save Matter bulb report',
    description: 'Save and optionally apply a Matter bulb tester report.',
    method: 'POST',
    path: 'api/matter/bulb-test/report',
    body: { device_id: '{device_id}', apply_local: true }
  },
  {
    id: 'hub-credentials',
    category: 'Devices & Topology',
    label: 'Set hub credentials',
    description: 'Save integration hub credentials.',
    method: 'PUT',
    path: 'api/hub/credentials',
    body: {
      hub_type: 'hue',
      address: '{bridge_address}',
      credentials: {}
    }
  },
  {
    id: 'hub-disconnect-one',
    category: 'Devices & Topology',
    label: 'Disconnect hub',
    description: 'Disconnect one configured integration hub.',
    method: 'DELETE',
    path: 'api/hub/credentials',
    query: { hub_type: 'hue', address: '{bridge_address}' },
    danger: true
  },
  {
    id: 'hub-disconnect-all',
    category: 'Devices & Topology',
    label: 'Disconnect all hubs',
    description: 'Remove all configured integration hub credentials.',
    method: 'DELETE',
    path: 'api/hub/credentials',
    danger: true
  },
  {
    id: 'hub-retry',
    category: 'Devices & Topology',
    label: 'Retry hub bootstrap',
    description: 'Re-arm startup bootstrap for a configured hub.',
    method: 'POST',
    path: 'api/hub/retry',
    body: { hub_type: 'hue', address: '{bridge_address}' }
  },
  {
    id: 'triage-get',
    category: 'Devices & Topology',
    label: 'Read triage entries',
    description: 'Fetch pending device topology triage entries.',
    method: 'GET',
    path: 'api/triage'
  },
  {
    id: 'triage-count',
    category: 'Devices & Topology',
    label: 'Read triage count',
    description: 'Fetch pending triage counts.',
    method: 'GET',
    path: 'api/triage/count'
  },
  {
    id: 'triage-merge',
    category: 'Devices & Topology',
    label: 'Resolve triage merge',
    description: 'Merge a triage entry into a canonical device.',
    method: 'PUT',
    path: 'api/triage/{entry_id}/merge',
    body: { canonical_id: '{device_id}' },
    danger: true
  },
  {
    id: 'triage-new',
    category: 'Devices & Topology',
    label: 'Resolve triage new',
    description: 'Create a new canonical device from a triage entry.',
    method: 'PUT',
    path: 'api/triage/{entry_id}/new',
    body: {}
  },
  {
    id: 'triage-dismiss',
    category: 'Devices & Topology',
    label: 'Dismiss triage',
    description: 'Dismiss a triage entry.',
    method: 'PUT',
    path: 'api/triage/{entry_id}/dismiss',
    body: {},
    danger: true
  },
  {
    id: 'triage-bind',
    category: 'Devices & Topology',
    label: 'Approve triage binding',
    description: 'Approve a room binding triage entry.',
    method: 'PUT',
    path: 'api/triage/{entry_id}/bind',
    body: { target_room_id: '{room_id}' }
  },
  {
    id: 'triage-room',
    category: 'Devices & Topology',
    label: 'Assign triage room',
    description: 'Assign an unassigned-device triage entry to a room.',
    method: 'PUT',
    path: 'api/triage/{entry_id}/room',
    body: { room_id: '{room_id}' }
  },
  {
    id: 'sync',
    category: 'Devices & Topology',
    label: 'Trigger sync',
    description: 'Trigger server-side device and room sync.',
    method: 'POST',
    path: 'api/sync',
    timeoutSeconds: 45
  },
  {
    id: 'remote-access-status',
    category: 'Remote & Cloud',
    label: 'Read remote access',
    description: 'Fetch remote access service status.',
    method: 'GET',
    path: 'api/remote-access/status'
  },
  {
    id: 'remote-access-config',
    category: 'Remote & Cloud',
    label: 'Set remote access',
    description: 'Configure Cloudflare remote access on the device.',
    method: 'PUT',
    path: 'api/remote-access/config',
    body: {
      enabled: true,
      hostname: 'device.example.com',
      connector_token: '{connector_token}'
    }
  },
  {
    id: 'remote-access-clear',
    category: 'Remote & Cloud',
    label: 'Clear remote access',
    description: 'Remove remote access configuration.',
    method: 'DELETE',
    path: 'api/remote-access/config',
    danger: true
  },
  {
    id: 'activity-cloud-get',
    category: 'Remote & Cloud',
    label: 'Read activity cloud config',
    description: 'Fetch server activity cloud ingestion config.',
    method: 'GET',
    path: 'api/activity-cloud/config'
  },
  {
    id: 'activity-cloud-config',
    category: 'Remote & Cloud',
    label: 'Set activity cloud config',
    description: 'Configure server activity cloud ingestion.',
    method: 'PUT',
    path: 'api/activity-cloud/config',
    body: {}
  },
  {
    id: 'activity-cloud-clear',
    category: 'Remote & Cloud',
    label: 'Clear activity cloud config',
    description: 'Remove server activity cloud ingestion config.',
    method: 'DELETE',
    path: 'api/activity-cloud/config',
    danger: true
  },
  {
    id: 'cloud-join-proof',
    category: 'Remote & Cloud',
    label: 'Create cloud join proof',
    description: 'Create a short-lived owner-authenticated proof for joining this server to a cloud Home.',
    method: 'POST',
    path: 'api/cloud/join-proof',
    danger: true
  },
  {
    id: 'history-get',
    category: 'Remote & Cloud',
    label: 'Read history',
    description: 'Fetch recent device activity history.',
    method: 'GET',
    path: 'api/history',
    query: { limit: 100 }
  },
  {
    id: 'auth-status',
    category: 'Diagnostics & Appliance',
    label: 'Read API auth status',
    description: 'Fetch device API auth status.',
    method: 'GET',
    path: 'api/auth/status'
  },
  {
    id: 'auth-claim',
    category: 'Diagnostics & Appliance',
    label: 'Claim owner token',
    description: 'Claim an owner token when the device allows claiming.',
    method: 'POST',
    path: 'api/auth/claim',
    body: { label: 'Rhythm admin' },
    danger: true
  },
  {
    id: 'auth-settings',
    category: 'Diagnostics & Appliance',
    label: 'Set API auth',
    description: 'Enable or disable API auth on the device.',
    method: 'PUT',
    path: 'api/auth/settings',
    body: { require_api_auth: true, label: 'Rhythm admin' },
    danger: true
  },
  {
    id: 'support-token',
    category: 'Diagnostics & Appliance',
    label: 'Issue support token',
    description: 'Issue a device support token.',
    method: 'POST',
    path: 'api/auth/support-token',
    body: { label: 'Rhythm admin' }
  },
  {
    id: 'ota-status',
    category: 'Diagnostics & Appliance',
    label: 'Read OTA status',
    description: 'Fetch OTA state from the device.',
    method: 'GET',
    path: 'api/ota/status'
  },
  {
    id: 'ota-check',
    category: 'Diagnostics & Appliance',
    label: 'Check OTA update',
    description: 'Check for a server update.',
    method: 'GET',
    path: 'api/ota/check'
  },
  {
    id: 'ota-update',
    category: 'Diagnostics & Appliance',
    label: 'Apply OTA update',
    description: 'Request a server update.',
    method: 'POST',
    path: 'api/ota/update',
    danger: true,
    timeoutSeconds: 120
  },
  {
    id: 'wifi-reset',
    category: 'Diagnostics & Appliance',
    label: 'Reset Wi-Fi',
    description: 'Clear Wi-Fi credentials and return appliance to setup mode.',
    method: 'DELETE',
    path: 'api/wifi',
    danger: true
  },
  {
    id: 'wifi-change',
    category: 'Diagnostics & Appliance',
    label: 'Change Wi-Fi',
    description: 'Schedule Wi-Fi credential change.',
    method: 'PUT',
    path: 'api/wifi',
    body: { ssid: '{ssid}', password: '{password}' },
    danger: true
  },
  {
    id: 'factory-reset',
    category: 'Diagnostics & Appliance',
    label: 'Factory reset',
    description: 'Reset paired hubs and configuration to defaults.',
    method: 'POST',
    path: 'api/factory-reset',
    danger: true,
    timeoutSeconds: 60
  },
  {
    id: 'restart',
    category: 'Diagnostics & Appliance',
    label: 'Restart device',
    description: 'Restart the selected device.',
    method: 'POST',
    path: 'api/restart',
    danger: true
  }
];

export function operationToQueryText(operation: DeviceAdminOperation): string {
  return operation.query ? JSON.stringify(operation.query, null, 2) : '';
}

export function operationToBodyText(operation: DeviceAdminOperation): string {
  return operation.body === undefined ? '' : JSON.stringify(operation.body, null, 2);
}
