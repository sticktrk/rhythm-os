import 'package:rhythm_sdk/rhythm_sdk.dart';

typedef ScheduleCloudCapture = void Function({
  Duration delay,
  String reason,
});

/// Delegates to [RhythmServerApi] and schedules a cloud snapshot after
/// structural mutations succeed.
class CloudBackedServerApi {
  CloudBackedServerApi({
    required RhythmServerApi delegate,
    required ScheduleCloudCapture scheduleCloudCapture,
  })  : _delegate = delegate,
        _scheduleCloudCapture = scheduleCloudCapture;

  final RhythmServerApi _delegate;
  final ScheduleCloudCapture _scheduleCloudCapture;

  static const Duration _defaultDelay = Duration(seconds: 2);
  static const Duration _syncDelay = Duration(seconds: 5);

  Future<RhythmRoomState?> roomAction({
    required String roomId,
    required String action,
  }) {
    return _delegate.roomAction(roomId: roomId, action: action);
  }

  Future<RhythmRoomState?> nodeAction({
    required String nodeId,
    required String action,
  }) {
    return _delegate.nodeAction(nodeId: nodeId, action: action);
  }

  Future<List<RhythmRoomState>> roomActionBatch(
    List<({String roomId, String action})> actions,
  ) {
    return _delegate.roomActionBatch(actions);
  }

  Future<List<RhythmRoomState>> nodeActionBatch(
    List<({String nodeId, String action})> actions,
  ) {
    return _delegate.nodeActionBatch(actions);
  }

  Future<void> roomBrightness({
    required String roomId,
    required int brightness,
  }) {
    return _delegate.roomBrightness(roomId: roomId, brightness: brightness);
  }

  Future<void> nodeBrightness({
    required String nodeId,
    required int brightness,
  }) {
    return _delegate.nodeBrightness(nodeId: nodeId, brightness: brightness);
  }

  Future<List<RhythmRoomState>> roomBrightnessBatch(
    List<({String roomId, int brightness})> items,
  ) {
    return _delegate.roomBrightnessBatch(items);
  }

  Future<List<RhythmRoomState>> nodeBrightnessBatch(
    List<({String nodeId, int brightness})> items,
  ) {
    return _delegate.nodeBrightnessBatch(items);
  }

  Future<void> roomOffset({
    required String roomId,
    required double timeOffset,
  }) async {
    await _delegate.roomOffset(roomId: roomId, timeOffset: timeOffset);
    _scheduleCloudCapture(reason: 'room_offset', delay: _defaultDelay);
  }

  Future<void> nodeOffset({
    required String nodeId,
    required double timeOffset,
  }) async {
    await _delegate.nodeOffset(nodeId: nodeId, timeOffset: timeOffset);
    _scheduleCloudCapture(reason: 'node_offset', delay: _defaultDelay);
  }

  Future<List<RhythmRoomState>> roomOffsetBatch(
    List<({String roomId, double timeOffset})> items,
  ) async {
    final states = await _delegate.roomOffsetBatch(items);
    _scheduleCloudCapture(reason: 'room_offset_batch', delay: _defaultDelay);
    return states;
  }

  Future<List<RhythmRoomState>> nodeOffsetBatch(
    List<({String nodeId, double timeOffset})> items,
  ) async {
    final states = await _delegate.nodeOffsetBatch(items);
    _scheduleCloudCapture(reason: 'node_offset_batch', delay: _defaultDelay);
    return states;
  }

  Future<void> roomPreferencesSet({
    required String roomId,
    bool? rhythmEnabled,
    bool? disabled,
    RoomModeState? state,
    bool? softOff,
    Map<String, dynamic>? profileSettings,
  }) async {
    await _delegate.roomPreferencesSet(
      roomId: roomId,
      rhythmEnabled: rhythmEnabled,
      disabled: disabled,
      state: state,
      softOff: softOff,
      profileSettings: profileSettings,
    );
    _scheduleCloudCapture(reason: 'room_preferences_set', delay: _defaultDelay);
  }

  Future<void> nodePreferencesSet({
    required String nodeId,
    bool? rhythmEnabled,
    bool? disabled,
    RoomModeState? state,
    bool? softOff,
    Map<String, dynamic>? profileSettings,
  }) async {
    await _delegate.nodePreferencesSet(
      nodeId: nodeId,
      rhythmEnabled: rhythmEnabled,
      disabled: disabled,
      state: state,
      softOff: softOff,
      profileSettings: profileSettings,
    );
    _scheduleCloudCapture(reason: 'node_preferences_set', delay: _defaultDelay);
  }

  Future<void> roomPreferencesBatchSet(List<Map<String, dynamic>> items) async {
    await _delegate.roomPreferencesBatchSet(items);
    _scheduleCloudCapture(
      reason: 'room_preferences_batch_set',
      delay: _defaultDelay,
    );
  }

  Future<void> nodePreferencesBatchSet(List<Map<String, dynamic>> items) async {
    await _delegate.nodePreferencesBatchSet(items);
    _scheduleCloudCapture(
      reason: 'node_preferences_batch_set',
      delay: _defaultDelay,
    );
  }

  Future<List<RhythmRoomState>> fixMyLights({
    required Iterable<String> nodeIds,
  }) {
    return _delegate.fixMyLights(nodeIds: nodeIds);
  }

  Future<RhythmCurveConfig?> getConfig({required String id}) {
    return _delegate.getConfig(id: id);
  }

  Future<RhythmCurveData?> getCurveData({
    required String id,
    RhythmCurveConfig? overrides,
    DateTime? date,
    int samplesPerHour = 4,
    double startHour = 12,
    int? maxSteps,
  }) {
    return _delegate.getCurveData(
      id: id,
      overrides: overrides,
      date: date,
      samplesPerHour: samplesPerHour,
      startHour: startHour,
      maxSteps: maxSteps,
    );
  }

  Future<RhythmTimeInfo?> getCurveNow({
    required String id,
    double? hour,
  }) {
    return _delegate.getCurveNow(id: id, hour: hour);
  }

  Future<RhythmCurveConfig?> absorbTimeOffset(
    double offsetMinutes, {
    String? id,
  }) async {
    final result = await _delegate.absorbTimeOffset(offsetMinutes, id: id);
    if (result != null) {
      _scheduleCloudCapture(reason: 'absorb_time_offset', delay: _defaultDelay);
    }
    return result;
  }

  Future<RhythmCurveConfig?> resetConfig({String? id}) async {
    final result = await _delegate.resetConfig(id: id);
    if (result != null) {
      _scheduleCloudCapture(reason: 'reset_config', delay: _defaultDelay);
    }
    return result;
  }

  Future<bool> configSet(
    RhythmCurveConfig config, {
    String? id,
  }) async {
    final success = await _delegate.configSet(config, id: id);
    if (success) {
      _scheduleCloudCapture(reason: 'config_set', delay: _defaultDelay);
    }
    return success;
  }

  Future<void> locationSet({
    required double lat,
    required double lon,
    double? utcOffset,
    String? timezoneName,
  }) async {
    await _delegate.locationSet(
      lat: lat,
      lon: lon,
      utcOffset: utcOffset,
      timezoneName: timezoneName,
    );
    _scheduleCloudCapture(reason: 'location_set', delay: _defaultDelay);
  }

  Future<void> hubCredentials({
    required String hubType,
    required String address,
    required Map<String, dynamic> credentials,
  }) async {
    await _delegate.hubCredentials(
      hubType: hubType,
      address: address,
      credentials: credentials,
    );
    _scheduleCloudCapture(reason: 'hub_credentials', delay: _syncDelay);
  }

  Future<void> hubDisconnect() async {
    await _delegate.hubDisconnect();
    _scheduleCloudCapture(reason: 'hub_disconnect', delay: _syncDelay);
  }

  Future<void> hubDisconnectOne({
    required String hubType,
    required String address,
  }) async {
    await _delegate.hubDisconnectOne(hubType: hubType, address: address);
    _scheduleCloudCapture(reason: 'hub_disconnect_one', delay: _syncDelay);
  }

  Future<void> motionTimeoutSet({
    String? nodeId,
    String? roomId,
    required int? timeoutSecs,
  }) async {
    await _delegate.motionTimeoutSet(
      nodeId: nodeId,
      roomId: roomId,
      timeoutSecs: timeoutSecs,
    );
    _scheduleCloudCapture(reason: 'motion_timeout_set', delay: _defaultDelay);
  }

  Future<RhythmSettings?> getSettings() {
    return _delegate.getSettings();
  }

  Future<void> sleep() async {
    await _delegate.sleep();
    _scheduleCloudCapture(reason: 'sleep', delay: _defaultDelay);
  }

  Future<void> wake() async {
    await _delegate.wake();
    _scheduleCloudCapture(reason: 'wake', delay: _defaultDelay);
  }

  Future<RhythmModeResource?> getMode() {
    return _delegate.getMode();
  }

  Future<List<RhythmCurveConfig>> getProfiles() {
    return _delegate.getProfiles();
  }

  Future<void> setActiveMode(RhythmMode mode) async {
    await _delegate.setActiveMode(mode);
    _scheduleCloudCapture(reason: 'set_active_mode', delay: _defaultDelay);
  }

  Future<bool> settingsSet({
    bool? powerSave,
  }) async {
    final success = await _delegate.settingsSet(powerSave: powerSave);
    if (success) {
      _scheduleCloudCapture(reason: 'settings_set', delay: _defaultDelay);
    }
    return success;
  }

  Future<Map<String, dynamic>?> getCanonicalDevice(String id) {
    return _delegate.getCanonicalDevice(id);
  }

  Future<List<Map<String, dynamic>>?> getCanonicalDevices() {
    return _delegate.getCanonicalDevices();
  }

  Future<List<Map<String, dynamic>>?> getTriageEntries() {
    return _delegate.getTriageEntries();
  }

  Future<Map<String, dynamic>?> getTriageCount() {
    return _delegate.getTriageCount();
  }

  Future<List<RhythmTopologyNode>> getTopologyNodes() {
    return _delegate.getTopologyNodes();
  }

  Future<bool> setTopologyNodeControlTarget({
    required String nodeId,
    required String controlKind,
    required String? targetId,
  }) async {
    final success = await _delegate.setTopologyNodeControlTarget(
      nodeId: nodeId,
      controlKind: controlKind,
      targetId: targetId,
    );
    if (success) {
      _scheduleCloudCapture(
        reason: 'topology_node_control_target',
        delay: _defaultDelay,
      );
    }
    return success;
  }

  Future<bool> resolveTriageMerge(String entryId, String canonicalId) async {
    final success = await _delegate.resolveTriageMerge(entryId, canonicalId);
    if (success) {
      _scheduleCloudCapture(reason: 'triage_merge', delay: _defaultDelay);
    }
    return success;
  }

  Future<bool> modeSet({
    RhythmMode? active,
    List<RhythmModeConfig>? configs,
  }) async {
    final success = await _delegate.modeSet(active: active, configs: configs);
    if (success) {
      _scheduleCloudCapture(reason: 'mode_set', delay: _defaultDelay);
    }
    return success;
  }

  Future<List<RhythmModeTransitionConfig>> getTransitions() {
    return _delegate.getTransitions();
  }

  Future<bool> setTransitions(
    List<RhythmModeTransitionConfig> transitions,
  ) async {
    final success = await _delegate.setTransitions(transitions);
    if (success) {
      _scheduleCloudCapture(reason: 'set_transitions', delay: _defaultDelay);
    }
    return success;
  }

  Future<bool> triggerTransition(String id) async {
    final success = await _delegate.triggerTransition(id);
    if (success) {
      _scheduleCloudCapture(reason: 'trigger_transition', delay: _defaultDelay);
    }
    return success;
  }

  Future<Map<String, dynamic>?> resolveTriageNewResult(String entryId) async {
    final result = await _delegate.resolveTriageNewResult(entryId);
    if (result != null) {
      _scheduleCloudCapture(reason: 'triage_new', delay: _defaultDelay);
    }
    return result;
  }

  Future<String?> resolveTriageNew(String entryId) async {
    final result = await resolveTriageNewResult(entryId);
    return result?['canonical_id'] as String?;
  }

  Future<bool> resolveTriageDismiss(String entryId) async {
    final success = await _delegate.resolveTriageDismiss(entryId);
    if (success) {
      _scheduleCloudCapture(reason: 'triage_dismiss', delay: _defaultDelay);
    }
    return success;
  }

  Future<bool> resolveTriageBind(
    String entryId, {
    String? targetRoomId,
  }) async {
    final success = await _delegate.resolveTriageBind(
      entryId,
      targetRoomId: targetRoomId,
    );
    if (success) {
      _scheduleCloudCapture(reason: 'triage_bind', delay: _defaultDelay);
    }
    return success;
  }

  Future<bool> resolveTriageRoom(String entryId, String roomId) async {
    final success = await _delegate.resolveTriageRoom(entryId, roomId);
    if (success) {
      _scheduleCloudCapture(reason: 'triage_room', delay: _defaultDelay);
    }
    return success;
  }

  Future<bool> topologyRenameRoom(String roomId, String name) async {
    final success = await _delegate.topologyRenameRoom(roomId, name);
    if (success) {
      _scheduleCloudCapture(
          reason: 'topology_rename_room', delay: _defaultDelay);
    }
    return success;
  }

  Future<bool> topologyMergeRooms(String targetId, String sourceId) async {
    final success = await _delegate.topologyMergeRooms(targetId, sourceId);
    if (success) {
      _scheduleCloudCapture(
          reason: 'topology_merge_rooms', delay: _defaultDelay);
    }
    return success;
  }

  Future<bool> topologyMoveDevice({
    required String deviceId,
    required String fromRoomId,
    required String toRoomId,
  }) async {
    final success = await _delegate.topologyMoveDevice(
      deviceId: deviceId,
      fromRoomId: fromRoomId,
      toRoomId: toRoomId,
    );
    if (success) {
      _scheduleCloudCapture(
          reason: 'topology_move_device', delay: _defaultDelay);
    }
    return success;
  }

  Future<Map<String, dynamic>?> pairDevice({
    required String hubType,
    Map<String, dynamic> params = const {},
    Duration receiveTimeout = const Duration(seconds: 45),
  }) async {
    final result = await _delegate.pairDevice(
      hubType: hubType,
      params: params,
      receiveTimeout: receiveTimeout,
    );
    if (result != null) {
      _scheduleCloudCapture(reason: 'pair_device', delay: _syncDelay);
    }
    return result;
  }

  Future<Map<String, dynamic>?> unpairDevice({
    required String hubType,
    required String deviceId,
    bool force = false,
  }) async {
    final result = await _delegate.unpairDevice(
      hubType: hubType,
      deviceId: deviceId,
      force: force,
    );
    if (result != null) {
      _scheduleCloudCapture(reason: 'unpair_device', delay: _syncDelay);
    }
    return result;
  }

  Future<bool> assignDeviceRoom(String deviceId, String? roomId) async {
    final success = await _delegate.assignDeviceRoom(deviceId, roomId);
    if (success) {
      _scheduleCloudCapture(reason: 'assign_device_room', delay: _defaultDelay);
    }
    return success;
  }

  Future<bool> assignDeviceParent(String deviceId, String? parentId) async {
    final success = await _delegate.assignDeviceParent(deviceId, parentId);
    if (success) {
      _scheduleCloudCapture(
        reason: 'assign_device_parent',
        delay: _defaultDelay,
      );
    }
    return success;
  }

  Future<Map<String, dynamic>?> createTopologyRoom(String name) async {
    final result = await _delegate.createTopologyRoom(name);
    if (result != null) {
      _scheduleCloudCapture(
          reason: 'create_topology_room', delay: _defaultDelay);
    }
    return result;
  }

  Future<Map<String, dynamic>?> triggerSync() async {
    final result = await _delegate.triggerSync();
    if (result != null) {
      _scheduleCloudCapture(reason: 'trigger_sync', delay: _syncDelay);
    }
    return result;
  }

  Future<bool> ping() {
    return _delegate.ping();
  }
}
