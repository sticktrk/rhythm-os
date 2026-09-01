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
      {int? dispatchSpacingMs,
      String? correlationId}) {
    return _delegate.roomActionBatch(
      actions,
      dispatchSpacingMs: dispatchSpacingMs,
      correlationId: correlationId,
    );
  }

  Future<List<RhythmRoomState>> nodeActionBatch(
      List<({String nodeId, String action})> actions,
      {int? dispatchSpacingMs,
      String? correlationId}) {
    return _delegate.nodeActionBatch(
      actions,
      dispatchSpacingMs: dispatchSpacingMs,
      correlationId: correlationId,
    );
  }

  Future<RhythmDispatchResult> roomActionBatchResult(
      List<({String roomId, String action})> actions,
      {int? dispatchSpacingMs,
      String? correlationId}) {
    return _delegate.roomActionBatchResult(
      actions,
      dispatchSpacingMs: dispatchSpacingMs,
      correlationId: correlationId,
    );
  }

  Future<RhythmDispatchResult> nodeActionBatchResult(
      List<({String nodeId, String action})> actions,
      {int? dispatchSpacingMs,
      String? correlationId}) {
    return _delegate.nodeActionBatchResult(
      actions,
      dispatchSpacingMs: dispatchSpacingMs,
      correlationId: correlationId,
    );
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

  Future<RhythmRoomState?> roomCurveBrightness({
    required String roomId,
    required int brightness,
  }) {
    return _delegate.roomCurveBrightness(
      roomId: roomId,
      brightness: brightness,
    );
  }

  Future<RhythmRoomState?> nodeCurveBrightness({
    required String nodeId,
    required int brightness,
  }) {
    return _delegate.nodeCurveBrightness(
      nodeId: nodeId,
      brightness: brightness,
    );
  }

  Future<RhythmRoomState?> roomCurveColorTemperature({
    required String roomId,
    required int kelvin,
    bool preserveBrightness = true,
  }) {
    return _delegate.roomCurveColorTemperature(
      roomId: roomId,
      kelvin: kelvin,
      preserveBrightness: preserveBrightness,
    );
  }

  Future<RhythmRoomState?> nodeCurveColorTemperature({
    required String nodeId,
    required int kelvin,
    bool preserveBrightness = true,
  }) {
    return _delegate.nodeCurveColorTemperature(
      nodeId: nodeId,
      kelvin: kelvin,
      preserveBrightness: preserveBrightness,
    );
  }

  Future<List<RhythmRoomState>> roomBrightnessBatch(
      List<({String roomId, int brightness})> items,
      {int? dispatchSpacingMs}) {
    return _delegate.roomBrightnessBatch(
      items,
      dispatchSpacingMs: dispatchSpacingMs,
    );
  }

  Future<List<RhythmRoomState>> nodeBrightnessBatch(
      List<({String nodeId, int brightness})> items,
      {int? dispatchSpacingMs}) {
    return _delegate.nodeBrightnessBatch(
      items,
      dispatchSpacingMs: dispatchSpacingMs,
    );
  }

  Future<RhythmDispatchResult> roomBrightnessBatchResult(
      List<({String roomId, int brightness})> items,
      {int? dispatchSpacingMs}) {
    return _delegate.roomBrightnessBatchResult(
      items,
      dispatchSpacingMs: dispatchSpacingMs,
    );
  }

  Future<RhythmDispatchResult> nodeBrightnessBatchResult(
      List<({String nodeId, int brightness})> items,
      {int? dispatchSpacingMs}) {
    return _delegate.nodeBrightnessBatchResult(
      items,
      dispatchSpacingMs: dispatchSpacingMs,
    );
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
      {int? dispatchSpacingMs}) {
    return _delegate.roomOffsetBatch(
      items,
      dispatchSpacingMs: dispatchSpacingMs,
    );
  }

  Future<List<RhythmRoomState>> nodeOffsetBatch(
      List<({String nodeId, double timeOffset})> items,
      {int? dispatchSpacingMs}) {
    return _delegate.nodeOffsetBatch(
      items,
      dispatchSpacingMs: dispatchSpacingMs,
    );
  }

  Future<RhythmDispatchResult> roomOffsetBatchResult(
      List<({String roomId, double timeOffset})> items,
      {int? dispatchSpacingMs}) {
    return _delegate.roomOffsetBatchResult(
      items,
      dispatchSpacingMs: dispatchSpacingMs,
    );
  }

  Future<RhythmDispatchResult> nodeOffsetBatchResult(
      List<({String nodeId, double timeOffset})> items,
      {int? dispatchSpacingMs}) {
    return _delegate.nodeOffsetBatchResult(
      items,
      dispatchSpacingMs: dispatchSpacingMs,
    );
  }

  Future<RhythmDispatchResult> nodeOffsetPreviewResult({
    required double timeOffset,
    List<String>? nodes,
    int? dispatchSpacingMs,
  }) {
    return _delegate.nodeOffsetPreviewResult(
      timeOffset: timeOffset,
      nodes: nodes,
      dispatchSpacingMs: dispatchSpacingMs,
    );
  }

  Future<void> roomPreferencesSet({
    required String roomId,
    bool? rhythmEnabled,
    bool? disabled,
    bool? standbyEnabled,
    RoomModeState? state,
    bool? softOff,
    Map<String, dynamic>? profileSettings,
  }) {
    return _delegate.roomPreferencesSet(
      roomId: roomId,
      rhythmEnabled: rhythmEnabled,
      disabled: disabled,
      standbyEnabled: standbyEnabled,
      state: state,
      softOff: softOff,
      profileSettings: profileSettings,
    );
  }

  Future<RhythmWriteAck> nodePreferencesSet({
    required String nodeId,
    bool? rhythmEnabled,
    bool? disabled,
    bool? standbyEnabled,
    RoomModeState? state,
    bool? softOff,
    Map<String, dynamic>? profileSettings,
  }) {
    return _delegate.nodePreferencesSet(
      nodeId: nodeId,
      rhythmEnabled: rhythmEnabled,
      disabled: disabled,
      standbyEnabled: standbyEnabled,
      state: state,
      softOff: softOff,
      profileSettings: profileSettings,
    );
  }

  Future<RhythmRoomState?> nodeMotionActivationSet({
    required String nodeId,
    required bool enabled,
    required String requestId,
  }) {
    return _delegate.nodeMotionActivationSet(
      nodeId: nodeId,
      enabled: enabled,
      requestId: requestId,
    );
  }

  Future<RhythmRoomState?> roomScheduleSet({
    required String roomId,
    required RhythmRoomSchedule schedule,
    required String requestId,
  }) =>
      _delegate.roomScheduleSet(
        roomId: roomId,
        schedule: schedule,
        requestId: requestId,
      );

  Future<bool> roomScheduleTest({
    required String roomId,
    required RhythmMode mode,
    required String requestId,
  }) =>
      _delegate.roomScheduleTest(
        roomId: roomId,
        mode: mode,
        requestId: requestId,
      );

  Future<bool> nodeProfileOverridesSet({
    required String nodeId,
    required Map<String, dynamic>? profileOverrides,
    bool replace = false,
    String? correlationId,
  }) {
    return _delegate.nodeProfileOverridesSet(
      nodeId: nodeId,
      profileOverrides: profileOverrides,
      replace: replace,
      correlationId: correlationId,
    );
  }

  Future<void> roomPreferencesBatchSet(
    List<Map<String, dynamic>> items, {
    int? dispatchSpacingMs,
  }) {
    return _delegate.roomPreferencesBatchSet(
      items,
      dispatchSpacingMs: dispatchSpacingMs,
    );
  }

  Future<void> nodePreferencesBatchSet(
    List<Map<String, dynamic>> items, {
    int? dispatchSpacingMs,
  }) {
    return _delegate.nodePreferencesBatchSet(
      items,
      dispatchSpacingMs: dispatchSpacingMs,
    );
  }

  Future<RhythmDispatchResult> roomPreferencesBatchSetResult(
    List<Map<String, dynamic>> items, {
    int? dispatchSpacingMs,
  }) {
    return _delegate.roomPreferencesBatchSetResult(
      items,
      dispatchSpacingMs: dispatchSpacingMs,
    );
  }

  Future<RhythmDispatchResult> nodePreferencesBatchSetResult(
    List<Map<String, dynamic>> items, {
    int? dispatchSpacingMs,
  }) {
    return _delegate.nodePreferencesBatchSetResult(
      items,
      dispatchSpacingMs: dispatchSpacingMs,
    );
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

  Future<RhythmAbsorbTimeOffsetResult> absorbTimeOffsetResult(
    double offsetMinutes, {
    String? id,
  }) {
    return _delegate.absorbTimeOffsetResult(offsetMinutes, id: id);
  }

  Future<RhythmCurveConfig?> resetConfig({String? id}) {
    return _delegate.resetConfig(id: id);
  }

  Future<bool> configSet(
    RhythmCurveConfig config, {
    String? id,
    bool apply = false,
  }) {
    return _delegate.configSet(config, id: id, apply: apply);
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

  Future<bool> hubCredentials({
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

  Future<RhythmHueAuthority?> getHueAuthority() {
    return _delegate.getHueAuthority();
  }

  Future<RhythmHueAuthority?> updateHueAuthority({
    required RhythmHueBridgeAuthority bridge,
    required Map<String, RhythmHueRoomAuthorityOwner> owners,
    required String correlationId,
    bool? topologySyncEnabled,
  }) {
    return _delegate.updateHueAuthority(
      bridge: bridge,
      owners: owners,
      correlationId: correlationId,
      topologySyncEnabled: topologySyncEnabled,
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

  Future<RhythmLightBreaker?> getLightBreaker() {
    return _delegate.getLightBreaker();
  }

  Future<bool> setLightBreaker(bool enabled) {
    return _delegate.setLightBreaker(enabled);
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

  Future<RhythmLightRuntimeState?> getLightRuntime() {
    return _delegate.getLightRuntime();
  }

  Future<RhythmLightRuntimeState?> setLightRuntime(
    RhythmLightRuntime runtime, {
    int? transitionMs,
  }) {
    return _delegate.setLightRuntime(
      runtime,
      transitionMs: transitionMs,
    );
  }

  Future<List<RhythmCurveConfig>> getProfiles() {
    return _delegate.getProfiles();
  }

  Future<void> setActiveMode(RhythmMode mode) {
    return _delegate.setActiveMode(mode);
  }

  Future<bool> settingsSet({
    bool? powerSave,
    bool? autoUpdate,
  }) {
    return _delegate.settingsSet(
      powerSave: powerSave,
      autoUpdate: autoUpdate,
    );
  }

  Future<Map<String, dynamic>?> getCanonicalDevice(String id) {
    return _delegate.getCanonicalDevice(id);
  }

  Future<RhythmPairingRecoverySecret?> getMatterSetupCode(
    String nativeDeviceId,
  ) {
    return _delegate.getMatterSetupCode(nativeDeviceId);
  }

  Future<List<Map<String, dynamic>>?> getCanonicalDevices() {
    return _delegate.getCanonicalDevices();
  }

  Future<List<Map<String, dynamic>>?> getRemovedDevices() {
    return _delegate.getRemovedDevices();
  }

  Future<bool> permanentlyDeleteRemovedDevice(
    String id, {
    String? correlationId,
  }) {
    return _delegate.permanentlyDeleteRemovedDevice(
      id,
      correlationId: correlationId,
    );
  }

  Future<bool> renameCanonicalDevice(String id, String name) {
    return _delegate.renameCanonicalDevice(id, name);
  }

  Future<bool> flashCanonicalDevice(String id) {
    return _delegate.flashCanonicalDevice(id);
  }

  Future<Map<String, dynamic>?> runMatterBulbTest({
    required String deviceId,
    required String test,
  }) {
    return _delegate.runMatterBulbTest(deviceId: deviceId, test: test);
  }

  Future<Map<String, dynamic>?> saveMatterBulbTestReport(
    Map<String, dynamic> report, {
    bool applyLocal = true,
  }) {
    return _delegate.saveMatterBulbTestReport(
      report,
      applyLocal: applyLocal,
    );
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

  Future<bool> setTopologyNodeControlTargets({
    required String nodeId,
    required String controlKind,
    required List<String> targetIds,
  }) {
    return _delegate.setTopologyNodeControlTargets(
      nodeId: nodeId,
      controlKind: controlKind,
      targetIds: targetIds,
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

  Future<List<RhythmLightScheduleConfig>> getLightSchedules() {
    return _delegate.getLightSchedules();
  }

  Future<List<RhythmLightScheduleConfig>> setLightSchedules(
    List<RhythmLightScheduleConfig> schedules, {
    required List<RhythmLightScheduleConfig> expectedSchedules,
    String? correlationId,
  }) {
    return _delegate.setLightSchedules(
      schedules,
      expectedSchedules: expectedSchedules,
      correlationId: correlationId,
    );
  }

  Future<RhythmRoomState?> setLightScheduleAssignment({
    required String nodeId,
    required String? scheduleId,
    String? correlationId,
  }) {
    return _delegate.setLightScheduleAssignment(
      nodeId: nodeId,
      scheduleId: scheduleId,
      correlationId: correlationId,
    );
  }

  Future<RhythmRoomState?> clearLightScheduleAssignment({
    required String nodeId,
    String? correlationId,
  }) {
    return _delegate.clearLightScheduleAssignment(
      nodeId: nodeId,
      correlationId: correlationId,
    );
  }

  Future<RhythmRoomState?> setLightScheduleOverride({
    required String nodeId,
    required String scheduleId,
    required RhythmLightScheduleOverride? scheduleOverride,
    required Map<String, RhythmLightScheduleOverride>
        expectedEffectiveOverrides,
    required String correlationId,
  }) {
    return _delegate.setLightScheduleOverride(
      nodeId: nodeId,
      scheduleId: scheduleId,
      scheduleOverride: scheduleOverride,
      expectedEffectiveOverrides: expectedEffectiveOverrides,
      correlationId: correlationId,
    );
  }

  Future<List<RhythmInputBinding>> getInputBindings() {
    return _delegate.getInputBindings();
  }

  Future<List<RhythmInputBinding>> createPresetInputBinding({
    required RhythmInputBindingPreset preset,
    required String sourceNodeId,
    RhythmButtonAction? buttonAction,
    bool enabled = true,
  }) {
    return _delegate.createPresetInputBinding(
      preset: preset,
      sourceNodeId: sourceNodeId,
      buttonAction: buttonAction,
      enabled: enabled,
    );
  }

  Future<List<RhythmInputBinding>> createDaySleepToggleInputBinding({
    required String sourceNodeId,
    RhythmButtonAction? buttonAction = RhythmButtonAction.onPress,
    bool enabled = true,
  }) {
    return _delegate.createDaySleepToggleInputBinding(
      sourceNodeId: sourceNodeId,
      buttonAction: buttonAction,
      enabled: enabled,
    );
  }

  Future<List<RhythmInputBinding>> setPresetInputBinding({
    required String id,
    required RhythmInputBindingPreset preset,
    required String sourceNodeId,
    RhythmButtonAction? buttonAction,
    bool enabled = true,
  }) {
    return _delegate.setPresetInputBinding(
      id: id,
      preset: preset,
      sourceNodeId: sourceNodeId,
      buttonAction: buttonAction,
      enabled: enabled,
    );
  }

  Future<List<RhythmInputBinding>> setDaySleepToggleInputBinding({
    required String id,
    required String sourceNodeId,
    RhythmButtonAction? buttonAction = RhythmButtonAction.onPress,
    bool enabled = true,
  }) {
    return _delegate.setDaySleepToggleInputBinding(
      id: id,
      sourceNodeId: sourceNodeId,
      buttonAction: buttonAction,
      enabled: enabled,
    );
  }

  Future<List<RhythmInputBinding>> setInputBinding(
    RhythmInputBinding binding,
  ) {
    return _delegate.setInputBinding(binding);
  }

  Future<List<RhythmInputBinding>> deleteInputBinding(String id) {
    return _delegate.deleteInputBinding(id);
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
    String? sessionId,
  }) {
    return _delegate.pairDevice(
      hubType: hubType,
      params: params,
      receiveTimeout: receiveTimeout,
      sessionId: sessionId,
    );
  }

  Future<RhythmPairingResultStatus?> getPairingResult(String sessionId) {
    return _delegate.getPairingResult(sessionId);
  }

  Future<bool> acknowledgePairingResult(String sessionId) {
    return _delegate.acknowledgePairingResult(sessionId);
  }

  Future<Map<String, dynamic>?> unpairDevice({
    required String hubType,
    required String deviceId,
    String? hubAddress,
    String? deviceType,
    String? correlationId,
    bool force = false,
    bool archive = false,
    Duration receiveTimeout = const Duration(seconds: 90),
  }) {
    return _delegate.unpairDevice(
      hubType: hubType,
      deviceId: deviceId,
      hubAddress: hubAddress,
      deviceType: deviceType,
      correlationId: correlationId,
      force: force,
      archive: archive,
      receiveTimeout: receiveTimeout,
    );
  }

  Future<bool> assignDeviceRoom(String deviceId, String? roomId) {
    return _delegate.assignDeviceRoom(deviceId, roomId);
  }

  Future<bool> assignDeviceParent(String deviceId, String? parentId) {
    return _delegate.assignDeviceParent(deviceId, parentId);
  }

  Future<RhythmDeviceRoomAssignmentResult?> assignDeviceParentResult(
    String deviceId,
    String? parentId,
  ) {
    return _delegate.assignDeviceParentResult(deviceId, parentId);
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
