import 'package:rhythm_sdk/rhythm_sdk.dart';

/// Delegates to [RhythmServerApi].
///
/// Cloud backup is intentionally manual-only. This wrapper preserves the
/// existing call surface used by the app without scheduling captures.
class CloudBackedServerApi {
  CloudBackedServerApi({
    required RhythmServerApi delegate,
  }) : _delegate = delegate;

  final RhythmServerApi _delegate;

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
  }) {
    return _delegate.roomOffset(roomId: roomId, timeOffset: timeOffset);
  }

  Future<void> nodeOffset({
    required String nodeId,
    required double timeOffset,
  }) {
    return _delegate.nodeOffset(nodeId: nodeId, timeOffset: timeOffset);
  }

  Future<List<RhythmRoomState>> roomOffsetBatch(
    List<({String roomId, double timeOffset})> items,
  ) {
    return _delegate.roomOffsetBatch(items);
  }

  Future<List<RhythmRoomState>> nodeOffsetBatch(
    List<({String nodeId, double timeOffset})> items,
  ) {
    return _delegate.nodeOffsetBatch(items);
  }

  Future<void> roomPreferencesSet({
    required String roomId,
    bool? rhythmEnabled,
    bool? disabled,
    RoomModeState? state,
    bool? softOff,
    Map<String, dynamic>? profileSettings,
  }) {
    return _delegate.roomPreferencesSet(
      roomId: roomId,
      rhythmEnabled: rhythmEnabled,
      disabled: disabled,
      state: state,
      softOff: softOff,
      profileSettings: profileSettings,
    );
  }

  Future<void> nodePreferencesSet({
    required String nodeId,
    bool? rhythmEnabled,
    bool? disabled,
    RoomModeState? state,
    bool? softOff,
    Map<String, dynamic>? profileSettings,
  }) {
    return _delegate.nodePreferencesSet(
      nodeId: nodeId,
      rhythmEnabled: rhythmEnabled,
      disabled: disabled,
      state: state,
      softOff: softOff,
      profileSettings: profileSettings,
    );
  }

  Future<void> roomPreferencesBatchSet(List<Map<String, dynamic>> items) {
    return _delegate.roomPreferencesBatchSet(items);
  }

  Future<void> nodePreferencesBatchSet(List<Map<String, dynamic>> items) {
    return _delegate.nodePreferencesBatchSet(items);
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
  }) {
    return _delegate.absorbTimeOffset(offsetMinutes, id: id);
  }

  Future<RhythmCurveConfig?> resetConfig({String? id}) {
    return _delegate.resetConfig(id: id);
  }

  Future<bool> configSet(
    RhythmCurveConfig config, {
    String? id,
  }) {
    return _delegate.configSet(config, id: id);
  }

  Future<void> locationSet({
    required double lat,
    required double lon,
    double? utcOffset,
    String? timezoneName,
  }) {
    return _delegate.locationSet(
      lat: lat,
      lon: lon,
      utcOffset: utcOffset,
      timezoneName: timezoneName,
    );
  }

  Future<void> hubCredentials({
    required String hubType,
    required String address,
    required Map<String, dynamic> credentials,
  }) {
    return _delegate.hubCredentials(
      hubType: hubType,
      address: address,
      credentials: credentials,
    );
  }

  Future<void> hubDisconnect() {
    return _delegate.hubDisconnect();
  }

  Future<void> hubDisconnectOne({
    required String hubType,
    required String address,
  }) {
    return _delegate.hubDisconnectOne(hubType: hubType, address: address);
  }

  Future<bool> hubRetry({
    required String hubType,
    required String address,
  }) {
    return _delegate.hubRetry(hubType: hubType, address: address);
  }

  Future<void> motionTimeoutSet({
    String? nodeId,
    String? roomId,
    required int? timeoutSecs,
  }) {
    return _delegate.motionTimeoutSet(
      nodeId: nodeId,
      roomId: roomId,
      timeoutSecs: timeoutSecs,
    );
  }

  Future<RhythmSettings?> getSettings() {
    return _delegate.getSettings();
  }

  Future<void> sleep() {
    return _delegate.sleep();
  }

  Future<void> wake() {
    return _delegate.wake();
  }

  Future<RhythmModeResource?> getMode() {
    return _delegate.getMode();
  }

  Future<List<RhythmCurveConfig>> getProfiles() {
    return _delegate.getProfiles();
  }

  Future<void> setActiveMode(RhythmMode mode) {
    return _delegate.setActiveMode(mode);
  }

  Future<bool> settingsSet({
    bool? powerSave,
  }) {
    return _delegate.settingsSet(powerSave: powerSave);
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
  }) {
    return _delegate.setTopologyNodeControlTarget(
      nodeId: nodeId,
      controlKind: controlKind,
      targetId: targetId,
    );
  }

  Future<bool> resolveTriageMerge(String entryId, String canonicalId) {
    return _delegate.resolveTriageMerge(entryId, canonicalId);
  }

  Future<bool> modeSet({
    RhythmMode? active,
    List<RhythmModeConfig>? configs,
  }) {
    return _delegate.modeSet(active: active, configs: configs);
  }

  Future<List<RhythmModeTransitionConfig>> getTransitions() {
    return _delegate.getTransitions();
  }

  Future<bool> setTransitions(
    List<RhythmModeTransitionConfig> transitions,
  ) {
    return _delegate.setTransitions(transitions);
  }

  Future<bool> triggerTransition(String id) {
    return _delegate.triggerTransition(id);
  }

  Future<Map<String, dynamic>?> resolveTriageNewResult(String entryId) {
    return _delegate.resolveTriageNewResult(entryId);
  }

  Future<String?> resolveTriageNew(String entryId) async {
    final result = await resolveTriageNewResult(entryId);
    return result?['canonical_id'] as String?;
  }

  Future<bool> resolveTriageDismiss(String entryId) {
    return _delegate.resolveTriageDismiss(entryId);
  }

  Future<bool> resolveTriageBind(
    String entryId, {
    String? targetRoomId,
  }) {
    return _delegate.resolveTriageBind(
      entryId,
      targetRoomId: targetRoomId,
    );
  }

  Future<bool> resolveTriageRoom(String entryId, String roomId) {
    return _delegate.resolveTriageRoom(entryId, roomId);
  }

  Future<bool> topologyRenameRoom(String roomId, String name) {
    return _delegate.topologyRenameRoom(roomId, name);
  }

  Future<bool> topologyMergeRooms(String targetId, String sourceId) {
    return _delegate.topologyMergeRooms(targetId, sourceId);
  }

  Future<bool> topologyDeleteRoom(String roomId) {
    return _delegate.topologyDeleteRoom(roomId);
  }

  Future<bool> topologyMoveDevice({
    required String deviceId,
    required String fromRoomId,
    required String toRoomId,
  }) {
    return _delegate.topologyMoveDevice(
      deviceId: deviceId,
      fromRoomId: fromRoomId,
      toRoomId: toRoomId,
    );
  }

  Future<Map<String, dynamic>?> pairDevice({
    required String hubType,
    Map<String, dynamic> params = const {},
    Duration receiveTimeout = const Duration(seconds: 45),
  }) {
    return _delegate.pairDevice(
      hubType: hubType,
      params: params,
      receiveTimeout: receiveTimeout,
    );
  }

  Future<Map<String, dynamic>?> unpairDevice({
    required String hubType,
    required String deviceId,
    bool force = false,
  }) {
    return _delegate.unpairDevice(
      hubType: hubType,
      deviceId: deviceId,
      force: force,
    );
  }

  Future<bool> assignDeviceRoom(String deviceId, String? roomId) {
    return _delegate.assignDeviceRoom(deviceId, roomId);
  }

  Future<bool> assignDeviceParent(String deviceId, String? parentId) {
    return _delegate.assignDeviceParent(deviceId, parentId);
  }

  Future<Map<String, dynamic>?> createTopologyRoom(String name) {
    return _delegate.createTopologyRoom(name);
  }

  Future<Map<String, dynamic>?> triggerSync() {
    return _delegate.triggerSync();
  }

  Future<bool> ping() {
    return _delegate.ping();
  }
}
