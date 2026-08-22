import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;
import 'dart:ui' show Tristate;

import 'package:connectivity_plus/connectivity_plus.dart';
import 'package:dio/dio.dart';
import 'package:fake_async/fake_async.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_app/providers/subscription_provider.dart';
import 'package:rhythm_app/models/plan_tier.dart';
import 'package:rhythm_app/services/account_cloud_sync_service.dart';
import 'package:rhythm_app/services/demo_server_api.dart';
import 'package:rhythm_app/services/hue/hue_service_locator.dart';
import 'package:rhythm_app/services/remote_access_service.dart';
import 'package:rhythm_app/screens/settings/light_screen.dart';
import 'package:rhythm_app/screens/hubs/device_pairing_flow.dart';
import 'package:rhythm_app/screens/hubs/room_device_add_flow.dart';
import 'package:rhythm_app/widgets/device_detail_sheet.dart';
import 'package:rhythm_app/widgets/hub_picker_screen.dart';
import 'package:rhythm_app/widgets/room_settings_sheet.dart';
import 'package:rhythm_app/widgets/room_schedule_tab.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _TestSubscriptionProvider extends ChangeNotifier
    implements SubscriptionProvider {
  @override
  PlanTier get tier => PlanTier.pro;

  @override
  bool get isPro => true;

  @override
  bool has(Entitlement e) => !e.isComingSoon;

  @override
  bool isEligibleFor(Entitlement e) => true;

  @override
  bool get isDemoOverrideActive => false;

  @override
  PlanTier? get demoOverride => null;

  @override
  Future<void> setDemoOverride(PlanTier? tier) async {}

  @override
  Future<void> changePlan(PlanTier tier) async {}

  @override
  Future<void> refresh() async {}
}

class _TestHomeProvider extends HomeProvider {
  _TestHomeProvider(
    List<Hub> hubs, {
    Home? currentHome,
    List<AccountHomeServerHubs> accountHomes = const [],
  })  : _hubs = List<Hub>.of(hubs),
        _currentHome = currentHome,
        _accountHomes = accountHomes;

  final List<Hub> _hubs;
  final List<AccountHomeServerHubs> _accountHomes;
  Home? _currentHome;

  @override
  Home? get currentHome => _currentHome;

  @override
  List<Hub> get currentHomeHubs => _hubs;

  @override
  Hub? getFirstHubOfType(HubType type) {
    for (final hub in _hubs) {
      if (hub.type == type) return hub;
    }
    return null;
  }

  @override
  Future<bool> updateCurrentHome(Home home) async {
    _currentHome = home;
    notifyListeners();
    return true;
  }

  @override
  Future<List<AccountHomeServerHubs>> loadAccountHomes() async {
    return _accountHomes;
  }

  @override
  Future<bool> updateHub(
    Hub hub, {
    bool clearCloudRemoteEndpoint = false,
  }) async {
    final index = _hubs.indexWhere((saved) => saved.id == hub.id);
    if (index == -1) {
      _hubs.add(hub);
    } else {
      _hubs[index] = hub;
    }
    notifyListeners();
    return true;
  }
}

class _FakeRhythmServerApi extends RhythmServerApi {
  _FakeRhythmServerApi() : super(Dio());

  List<RhythmCurveConfig> profileConfigs = const [];
  RhythmModeResource? profileMode;
  bool profileConfigSetResult = true;
  bool profileModeSetResult = true;
  final List<({RhythmCurveConfig config, String? id, bool apply})>
      profileConfigSetCalls = [];
  final List<({RhythmMode? active, List<RhythmModeConfig>? configs})>
      profileModeSetCalls = [];
  int hubCredentialsCalls = 0;
  int hubRetryCalls = 0;
  bool hubCredentialsResult = true;
  String? lastHubType;
  String? lastAddress;
  Map<String, dynamic>? lastCredentials;
  List<RhythmTopologyNode> topologyNodes = const [];
  int assignDeviceParentCalls = 0;
  String? lastAssignedDeviceId;
  String? lastAssignedParentId;
  bool assignDeviceParentResult = true;
  Completer<bool>? assignDeviceParentCompleter;
  int flashCanonicalDeviceCalls = 0;
  bool flashCanonicalDeviceResult = true;
  Completer<bool>? flashCanonicalDeviceCompleter;
  List<Map<String, dynamic>>? triageEntries = const [];
  int resolveTriageNewCalls = 0;
  String? lastResolvedTriageEntryId;
  Map<String, dynamic>? resolveTriageNewResultValue = const {
    'status': 'standalone',
  };
  int setTopologyNodeControlTargetsCalls = 0;
  String? lastControlSourceNodeId;
  String? lastControlKind;
  List<String>? lastControlTargetIds;
  bool setTopologyNodeControlTargetsResult = true;
  FutureOr<void> Function()? beforeSetTopologyNodeControlTargets;
  int createTopologyRoomCalls = 0;
  String? lastCreatedRoomName;
  int topologyDeleteRoomCalls = 0;
  String? lastDeletedRoomId;
  bool topologyDeleteRoomResult = true;
  int topologyRenameRoomCalls = 0;
  String? lastRenamedRoomId;
  String? lastRenamedRoomName;
  bool topologyRenameRoomResult = true;
  int triggerSyncCalls = 0;
  final Map<String, Map<String, dynamic>?> canonicalDevices = {};
  int getCanonicalDevicesCalls = 0;
  bool getCanonicalDevicesFails = false;
  final List<
      ({
        String hubType,
        String deviceId,
        String? hubAddress,
        bool force,
      })> unpairCalls = [];
  Map<String, dynamic>? unpairResult = const {'status': 'complete'};
  List<Map<String, dynamic>?>? unpairResults;
  Completer<Map<String, dynamic>?>? unpairCompleter;
  final List<String?> unpairDeviceTypes = [];
  final List<String?> unpairCorrelationIds = [];
  bool removeCanonicalEndpointOnUnpair = true;
  List<RhythmInputBinding> inputBindings = const [];
  int createDaySleepToggleInputBindingCalls = 0;
  int setInputBindingCalls = 0;
  int deleteInputBindingCalls = 0;
  bool createReplacesPresetBindings = false;
  String? lastInputBindingSourceNodeId;
  RhythmButtonAction? lastInputBindingButtonAction;
  RhythmInputBinding? lastSetInputBinding;
  String? lastDeletedInputBindingId;
  List<RhythmModeTransitionConfig> transitions = const [];
  List<RhythmModeTransitionConfig>? lastSetTransitions;
  int setTransitionsCalls = 0;
  bool setTransitionsResult = true;
  bool lightBreakerEnabled = true;
  bool setLightBreakerResult = true;
  int setLightBreakerCalls = 0;
  bool? lastLightBreakerEnabled;
  int setLightRuntimeCalls = 0;
  RhythmLightRuntime? lastSetLightRuntime;
  int? lastSetLightRuntimeTransitionMs;
  RhythmLightRuntimeInitialApply? lightRuntimeInitialApply;
  final List<
      ({
        String nodeId,
        bool? rhythmEnabled,
        bool? disabled,
        bool? standbyEnabled,
        RoomModeState? state,
        bool? softOff,
        Map<String, dynamic>? profileSettings,
      })> nodePreferenceCalls = [];
  final List<
      ({
        String nodeId,
        Map<String, dynamic>? profileOverrides,
        bool replace,
        String? correlationId,
      })> nodeProfileOverrideCalls = [];
  bool nodeProfileOverridesSucceeds = true;
  Completer<bool>? nodeProfileOverridesCompleter;
  final List<
      ({
        String nodeId,
        int r,
        int g,
        int b,
        int? brightness,
        int? transitionMs,
        String? scope,
      })> nodeColorCalls = [];
  List<RhythmSceneDefinition> scenes = const [];
  final Map<String, List<RhythmSceneDefinition>> roomScenes = {};
  final List<String?> sceneTargetCalls = [];
  final Set<String?> failedSceneCatalogTargets = {};
  final Set<String> partialNativeDiscoveryFailureTargets = {};
  bool applySceneSucceeds = true;
  Completer<RhythmSceneActionResult?>? applySceneCompleter;
  final List<
      ({
        String sceneId,
        String targetId,
        int? transitionMs,
        String? correlationId,
      })> applySceneCalls = [];
  final List<({String nodeId, int brightness})> nodeBrightnessCalls = [];
  final List<({String nodeId, int brightness})> nodeCurveBrightnessCalls = [];
  final List<({String nodeId, bool enabled, String requestId})>
      motionActivationCalls = [];
  Completer<RhythmRoomState?>? motionActivationCompleter;
  bool motionActivationSucceeds = true;
  Completer<bool>? roomScheduleTestCompleter;
  bool roomScheduleTestSucceeds = true;
  bool roomScheduleSetSucceeds = true;
  bool roomScheduleSetThrows = false;
  RhythmRoomState? roomScheduleSetResponse;
  final List<Completer<RhythmRoomState?>> roomScheduleSetCompleters = [];
  final List<RhythmRoomSchedule> roomScheduleSetCalls = [];
  final List<String> roomScheduleSetRequestIds = [];
  final List<String> roomScheduleTestRequestIds = [];
  final List<
      ({
        String nodeId,
        int kelvin,
        bool preserveBrightness,
      })> nodeCurveColorTemperatureCalls = [];

  @override
  Future<RhythmModeResource?> getMode() async => profileMode;

  @override
  Future<List<RhythmCurveConfig>> getProfiles() async => profileConfigs;

  @override
  Future<RhythmCurveConfig?> getConfig({required String id}) async {
    for (final config in profileConfigs) {
      if (config.id == id) return config;
    }
    return null;
  }

  @override
  Future<bool> configSet(
    RhythmCurveConfig config, {
    String? id,
    bool apply = false,
  }) async {
    profileConfigSetCalls.add((config: config, id: id, apply: apply));
    if (!profileConfigSetResult) return false;
    final targetId = id ?? config.id;
    profileConfigs = [
      for (final existing in profileConfigs)
        if (existing.id != targetId) existing,
      config.copyWith(id: targetId),
    ];
    return true;
  }

  @override
  Future<bool> modeSet({
    RhythmMode? active,
    List<RhythmModeConfig>? configs,
  }) async {
    profileModeSetCalls.add((active: active, configs: configs));
    if (!profileModeSetResult) return false;
    profileMode = RhythmModeResource.fromJson({
      'active': (active ?? profileMode?.active ?? RhythmMode.day).wireValue,
      'configs': [
        for (final config in configs ?? profileMode?.configs ?? const [])
          config.toJson(),
      ],
    });
    return true;
  }

  @override
  Future<bool> hubCredentials({
    required String hubType,
    required String address,
    required Map<String, dynamic> credentials,
  }) async {
    hubCredentialsCalls++;
    lastHubType = hubType;
    lastAddress = address;
    lastCredentials = credentials;
    return hubCredentialsResult;
  }

  @override
  Future<bool> hubRetry({
    required String hubType,
    required String address,
  }) async {
    hubRetryCalls++;
    lastHubType = hubType;
    lastAddress = address;
    return true;
  }

  @override
  Future<Map<String, dynamic>?> getTriageCount() async => null;

  @override
  Future<List<Map<String, dynamic>>?> getTriageEntries() async => triageEntries;

  @override
  Future<Map<String, dynamic>?> resolveTriageNewResult(String entryId) async {
    resolveTriageNewCalls++;
    lastResolvedTriageEntryId = entryId;
    return resolveTriageNewResultValue;
  }

  @override
  Future<void> nodePreferencesSet({
    required String nodeId,
    bool? rhythmEnabled,
    bool? disabled,
    bool? standbyEnabled,
    RoomModeState? state,
    bool? softOff,
    Map<String, dynamic>? profileSettings,
  }) async {
    nodePreferenceCalls.add((
      nodeId: nodeId,
      rhythmEnabled: rhythmEnabled,
      disabled: disabled,
      standbyEnabled: standbyEnabled,
      state: state,
      softOff: softOff,
      profileSettings: profileSettings,
    ));
  }

  @override
  Future<RhythmRoomState?> nodeMotionActivationSet({
    required String nodeId,
    required bool enabled,
    required String requestId,
  }) async {
    motionActivationCalls.add((
      nodeId: nodeId,
      enabled: enabled,
      requestId: requestId,
    ));
    final pending = motionActivationCompleter;
    if (pending != null) return pending.future;
    if (!motionActivationSucceeds) return null;
    return RhythmRoomState.fromJson({
      'node_id': nodeId,
      'rhythm_enabled': true,
      'time_offset': 0.0,
      'brightness_offset': 0.0,
      'state': 'active',
      'profile_settings': {'motion_activation_enabled': enabled},
    });
  }

  @override
  Future<RhythmRoomState?> roomScheduleSet({
    required String roomId,
    required RhythmRoomSchedule schedule,
    required String requestId,
  }) async {
    final callIndex = roomScheduleSetCalls.length;
    roomScheduleSetCalls.add(schedule);
    roomScheduleSetRequestIds.add(requestId);
    if (roomScheduleSetThrows) throw StateError('schedule write failed');
    if (callIndex < roomScheduleSetCompleters.length) {
      return roomScheduleSetCompleters[callIndex].future;
    }
    if (!roomScheduleSetSucceeds) return null;
    return roomScheduleSetResponse ??
        RhythmRoomState.fromJson({
          'node_id': roomId,
          'state': 'active',
          'profile_settings': {'room_schedule': schedule.toJson()},
        });
  }

  @override
  Future<bool> roomScheduleTest({
    required String roomId,
    required RhythmMode mode,
    required String requestId,
  }) async {
    roomScheduleTestRequestIds.add(requestId);
    final pending = roomScheduleTestCompleter;
    return pending == null ? roomScheduleTestSucceeds : pending.future;
  }

  @override
  Future<bool> nodeProfileOverridesSet({
    required String nodeId,
    required Map<String, dynamic>? profileOverrides,
    bool replace = false,
    String? correlationId,
  }) async {
    nodeProfileOverrideCalls.add((
      nodeId: nodeId,
      profileOverrides: profileOverrides,
      replace: replace,
      correlationId: correlationId,
    ));
    final pending = nodeProfileOverridesCompleter;
    if (pending != null) return pending.future;
    return nodeProfileOverridesSucceeds;
  }

  @override
  Future<List<RhythmTopologyNode>> getTopologyNodes() async => topologyNodes;

  @override
  Future<List<RhythmInputBinding>> getInputBindings() async => inputBindings;

  @override
  Future<List<RhythmModeTransitionConfig>> getTransitions() async =>
      List<RhythmModeTransitionConfig>.unmodifiable(transitions);

  @override
  Future<RhythmLightBreaker?> getLightBreaker() async =>
      RhythmLightBreaker(enabled: lightBreakerEnabled);

  @override
  Future<bool> setLightBreaker(bool enabled) async {
    setLightBreakerCalls++;
    lastLightBreakerEnabled = enabled;
    if (!setLightBreakerResult) return false;
    lightBreakerEnabled = enabled;
    return true;
  }

  @override
  Future<RhythmLightRuntimeState?> setLightRuntime(
    RhythmLightRuntime runtime, {
    int? transitionMs,
  }) async {
    setLightRuntimeCalls++;
    lastSetLightRuntime = runtime;
    lastSetLightRuntimeTransitionMs = transitionMs;
    return RhythmLightRuntimeState(
      runtime: runtime,
      availableRuntimes: RhythmLightRuntime.values,
      initialApply: lightRuntimeInitialApply,
    );
  }

  @override
  Future<void> nodeColor({
    required String nodeId,
    required int r,
    required int g,
    required int b,
    int? brightness,
    int? transitionMs,
    String? scope,
    RhythmNodeColorScope? colorScope,
  }) async {
    nodeColorCalls.add((
      nodeId: nodeId,
      r: r,
      g: g,
      b: b,
      brightness: brightness,
      transitionMs: transitionMs,
      scope: scope,
    ));
  }

  @override
  Future<List<RhythmSceneDefinition>> getScenes({String? targetId}) async {
    final catalog = await getSceneCatalog(targetId: targetId);
    return catalog?.scenes ?? const [];
  }

  @override
  Future<RhythmSceneCatalogResult?> getSceneCatalog({String? targetId}) async {
    sceneTargetCalls.add(targetId);
    if (failedSceneCatalogTargets.contains(targetId)) return null;
    return RhythmSceneCatalogResult(
      scenes: targetId == null ? scenes : roomScenes[targetId] ?? scenes,
      nativeDiscoveryFailed: targetId != null &&
          partialNativeDiscoveryFailureTargets.contains(targetId),
    );
  }

  @override
  Future<RhythmSceneActionResult?> applyScene({
    required String sceneId,
    required String targetId,
    int? transitionMs,
    String? correlationId,
  }) async {
    applySceneCalls.add((
      sceneId: sceneId,
      targetId: targetId,
      transitionMs: transitionMs,
      correlationId: correlationId,
    ));
    final completer = applySceneCompleter;
    if (completer != null) return completer.future;
    if (!applySceneSucceeds) return null;
    return RhythmSceneActionResult(
      sceneId: sceneId,
      targetId: targetId,
      affectedNodeIds: [targetId],
    );
  }

  @override
  Future<RhythmRoomState?> nodeCurveBrightness({
    required String nodeId,
    required int brightness,
  }) async {
    nodeCurveBrightnessCalls.add((nodeId: nodeId, brightness: brightness));
    return null;
  }

  @override
  Future<void> nodeBrightness({
    required String nodeId,
    required int brightness,
  }) async {
    nodeBrightnessCalls.add((nodeId: nodeId, brightness: brightness));
  }

  @override
  Future<RhythmRoomState?> nodeCurveColorTemperature({
    required String nodeId,
    required int kelvin,
    bool preserveBrightness = true,
  }) async {
    nodeCurveColorTemperatureCalls.add((
      nodeId: nodeId,
      kelvin: kelvin,
      preserveBrightness: preserveBrightness,
    ));
    return null;
  }

  @override
  Future<bool> setTransitions(
    List<RhythmModeTransitionConfig> transitions,
  ) async {
    setTransitionsCalls++;
    lastSetTransitions = List<RhythmModeTransitionConfig>.from(transitions);
    if (!setTransitionsResult) return false;
    this.transitions = List<RhythmModeTransitionConfig>.from(transitions);
    return true;
  }

  @override
  Future<List<RhythmInputBinding>> createDaySleepToggleInputBinding({
    required String sourceNodeId,
    RhythmButtonAction? buttonAction = RhythmButtonAction.onPress,
    bool enabled = true,
  }) async {
    createDaySleepToggleInputBindingCalls++;
    lastInputBindingSourceNodeId = sourceNodeId;
    lastInputBindingButtonAction = buttonAction;
    final binding = RhythmInputBinding(
      id: 'day_sleep_toggle:$sourceNodeId:${buttonAction?.wireValue ?? 'any'}',
      preset: RhythmInputBindingPreset.daySleepToggle,
      sourceNodeId: sourceNodeId,
      trigger: RhythmInputBindingTrigger.button(buttonAction: buttonAction),
      action: const RhythmModeCycleAction(
        modes: [RhythmMode.day, RhythmMode.sleep],
      ),
      enabled: enabled,
    );
    inputBindings = [
      ...inputBindings.where((existing) {
        if (existing.id == binding.id) return false;
        return !createReplacesPresetBindings ||
            existing.preset != RhythmInputBindingPreset.daySleepToggle;
      }),
      binding,
    ];
    return inputBindings;
  }

  @override
  Future<List<RhythmInputBinding>> setInputBinding(
    RhythmInputBinding binding,
  ) async {
    setInputBindingCalls++;
    lastSetInputBinding = binding;
    inputBindings = [
      for (final existing in inputBindings)
        if (existing.id == binding.id) binding else existing,
      if (!inputBindings.any((existing) => existing.id == binding.id)) binding,
    ];
    return inputBindings;
  }

  @override
  Future<List<RhythmInputBinding>> deleteInputBinding(String id) async {
    deleteInputBindingCalls++;
    lastDeletedInputBindingId = id;
    inputBindings = [
      for (final binding in inputBindings)
        if (binding.id != id) binding,
    ];
    return inputBindings;
  }

  @override
  Future<bool> assignDeviceParent(String deviceId, String? parentId) async {
    assignDeviceParentCalls++;
    lastAssignedDeviceId = deviceId;
    lastAssignedParentId = parentId;
    final pending = assignDeviceParentCompleter;
    final result =
        pending == null ? assignDeviceParentResult : await pending.future;
    if (result) {
      topologyNodes = [
        for (final node in topologyNodes)
          if (node.id == deviceId)
            RhythmTopologyNode(
              id: node.id,
              name: node.name,
              kind: node.kind,
              parentId: parentId,
              placement: node.placement,
              controls: node.controls,
              hubRoomBindings: node.hubRoomBindings,
              manufacturer: node.manufacturer,
              model: node.model,
              userCustomized: node.userCustomized,
              bootstrapName: node.bootstrapName,
            )
          else
            node,
      ];
    }
    return result;
  }

  @override
  Future<bool> flashCanonicalDevice(String id) async {
    flashCanonicalDeviceCalls++;
    final pending = flashCanonicalDeviceCompleter;
    if (pending != null) return pending.future;
    return flashCanonicalDeviceResult;
  }

  @override
  Future<bool> setTopologyNodeControlTargets({
    required String nodeId,
    required String controlKind,
    required List<String> targetIds,
  }) async {
    setTopologyNodeControlTargetsCalls++;
    lastControlSourceNodeId = nodeId;
    lastControlKind = controlKind;
    lastControlTargetIds = List<String>.of(targetIds);
    await beforeSetTopologyNodeControlTargets?.call();
    if (!setTopologyNodeControlTargetsResult) return false;

    topologyNodes = [
      for (final node in topologyNodes)
        if (node.id == nodeId)
          RhythmTopologyNode(
            id: node.id,
            name: node.name,
            kind: node.kind,
            parentId: node.parentId,
            controls: [
              for (final targetId in targetIds)
                RhythmTopologyControlLink(
                  kind: controlKind,
                  targetId: targetId,
                ),
            ],
            manufacturer: node.manufacturer,
            model: node.model,
          )
        else
          node,
    ];
    return true;
  }

  @override
  Future<Map<String, dynamic>?> createTopologyRoom(String name) async {
    createTopologyRoomCalls++;
    lastCreatedRoomName = name;
    topologyNodes = [
      ...topologyNodes,
      RhythmTopologyNode.fromJson({
        'id': 'room-created',
        'name': name,
        'kind': 'room',
      }),
    ];
    return {
      'id': 'room-created',
      'name': name,
    };
  }

  @override
  Future<bool> topologyDeleteRoom(String roomId) async {
    topologyDeleteRoomCalls++;
    lastDeletedRoomId = roomId;
    return topologyDeleteRoomResult;
  }

  @override
  Future<bool> topologyRenameRoom(String roomId, String name) async {
    topologyRenameRoomCalls++;
    lastRenamedRoomId = roomId;
    lastRenamedRoomName = name;
    return topologyRenameRoomResult;
  }

  @override
  Future<Map<String, dynamic>?> triggerSync() async {
    triggerSyncCalls++;
    return const {};
  }

  @override
  Future<Map<String, dynamic>?> getCanonicalDevice(String id) async {
    return canonicalDevices[id];
  }

  @override
  Future<List<Map<String, dynamic>>?> getCanonicalDevices() async {
    getCanonicalDevicesCalls++;
    if (getCanonicalDevicesFails) return null;
    return canonicalDevices.values.whereType<Map<String, dynamic>>().toList();
  }

  @override
  Future<Map<String, dynamic>?> unpairDevice({
    required String hubType,
    required String deviceId,
    String? hubAddress,
    String? deviceType,
    String? correlationId,
    bool force = false,
    Duration receiveTimeout = const Duration(seconds: 90),
  }) async {
    unpairDeviceTypes.add(deviceType);
    unpairCorrelationIds.add(correlationId);
    unpairCalls.add((
      hubType: hubType,
      deviceId: deviceId,
      hubAddress: hubAddress,
      force: force,
    ));
    final pendingResult = unpairCompleter;
    final Map<String, dynamic>? result;
    if (pendingResult != null) {
      result = await pendingResult.future;
      if (identical(unpairCompleter, pendingResult)) {
        unpairCompleter = null;
      }
    } else {
      final queuedResults = unpairResults;
      result = queuedResults != null && queuedResults.isNotEmpty
          ? queuedResults.removeAt(0)
          : unpairResult;
    }
    if (result?['status'] == 'complete' && removeCanonicalEndpointOnUnpair) {
      for (final canonicalId in canonicalDevices.keys.toList()) {
        final canonical = canonicalDevices[canonicalId];
        if (canonical == null) continue;
        final endpoints = canonical['endpoints'] as List<dynamic>? ?? const [];
        final remaining = [
          for (final endpoint in endpoints)
            if (endpoint is! Map<String, dynamic> ||
                endpoint['native_id']?.toString() != deviceId ||
                (endpoint['hub_key'] as Map<String, dynamic>?)?['hub_type']
                        ?.toString() !=
                    hubType)
              endpoint,
        ];
        if (remaining.length != endpoints.length) {
          canonicalDevices[canonicalId] = {
            ...canonical,
            'endpoints': remaining,
          };
        }
      }
    }
    return result;
  }
}

class _FakeRhythmConnection extends RhythmConnection {
  _FakeRhythmConnection(this.fakeApi);

  final _FakeRhythmServerApi fakeApi;
  Completer<void>? connectBlocker;
  bool isConnected = true;
  int reconnectCalls = 0;
  bool? lastReconnectAuthoritative;
  FutureOr<void> Function(bool authoritative)? reconnectHandler;
  final List<
      ({
        String host,
        int port,
        bool useSsl,
        String? authToken,
      })> connectCalls = [];

  @override
  bool get connected => isConnected;

  @override
  RhythmServerApi get api => fakeApi;

  @override
  Future<void> connect(
    String host, {
    int port = 80,
    bool useSsl = false,
    String? webBaseUrl,
    String? authToken,
  }) async {
    connectCalls.add((
      host: host,
      port: port,
      useSsl: useSsl,
      authToken: authToken,
    ));
    await connectBlocker?.future;
  }

  @override
  Future<void> reconnect({bool authoritative = false}) async {
    reconnectCalls++;
    lastReconnectAuthoritative = authoritative;
    await reconnectHandler?.call(authoritative);
  }
}

class _FakeRhythmAuthApi extends RhythmAuthApi {
  _FakeRhythmAuthApi() : super(baseUrl: 'http://127.0.0.1');

  int statusCalls = 0;
  int claimCalls = 0;

  @override
  Future<RhythmAuthStatus> getStatus() async {
    statusCalls += 1;
    return const RhythmAuthStatus(
      requiresAuth: true,
      ownerConfigured: false,
      tokenCount: 0,
      claimAvailable: true,
    );
  }

  @override
  Future<RhythmOwnerClaim> claimOwnerToken({
    String label = 'Rhythm app',
  }) async {
    claimCalls += 1;
    return const RhythmOwnerClaim(
      tokenId: 'claimed-token-id',
      token: 'claimed-owner-token',
    );
  }
}

class _HelloRhythmConnection extends _FakeRhythmConnection {
  _HelloRhythmConnection(super.fakeApi);

  final _helloController = StreamController<RhythmHello>.broadcast();
  final _rhythmStateController = StreamController<RhythmRoomState>.broadcast();
  final _hubEventController = StreamController<
      ({String event, String? hubType, String? address})>.broadcast();
  final _motionTimerController =
      StreamController<RhythmMotionTimer>.broadcast();
  final _modeChangedController =
      StreamController<RhythmModeResource>.broadcast();
  final _settingsChangedController =
      StreamController<RhythmSettings>.broadcast();
  final _lightBreakerChangedController =
      StreamController<RhythmLightBreaker>.broadcast();
  final _newNodesController = StreamController<void>.broadcast();
  final _connectionStateController =
      StreamController<RhythmConnectionState>.broadcast();
  RhythmHello? helloOnReconnect;

  @override
  Future<void> reconnect({bool authoritative = false}) async {
    await super.reconnect(authoritative: authoritative);
    final hello = helloOnReconnect;
    if (hello != null) {
      emitHello(hello);
      await Future<void>.delayed(Duration.zero);
    }
  }

  @override
  Stream<RhythmHello> get helloEvents => _helloController.stream;

  @override
  Stream<RhythmRoomState> get rhythmStateEvents =>
      _rhythmStateController.stream;

  @override
  Stream<({String event, String? hubType, String? address})> get hubEvents =>
      _hubEventController.stream;

  @override
  Stream<RhythmMotionTimer> get motionTimerEvents =>
      _motionTimerController.stream;

  @override
  Stream<RhythmModeResource> get modeChangedEvents =>
      _modeChangedController.stream;

  @override
  Stream<RhythmSettings> get settingsChangedEvents =>
      _settingsChangedController.stream;

  @override
  Stream<RhythmLightBreaker> get lightBreakerChangedEvents =>
      _lightBreakerChangedController.stream;

  @override
  Stream<void> get newNodesDetected => _newNodesController.stream;

  @override
  Stream<Map<String, dynamic>> get triageChangedEvents =>
      const Stream<Map<String, dynamic>>.empty();

  @override
  Stream<RhythmConnectionState> get connectionStateStream =>
      _connectionStateController.stream;

  void emitConnectionState(RhythmConnectionState state) {
    _connectionStateController.add(state);
  }

  void emitHello(RhythmHello hello) {
    _helloController.add(hello);
  }

  void emitRhythmState(RhythmRoomState state) {
    _rhythmStateController.add(state);
  }

  void emitHubEvent({
    required String event,
    String? hubType,
    String? address,
  }) {
    _hubEventController.add((
      event: event,
      hubType: hubType,
      address: address,
    ));
  }

  void emitMotionTimer(RhythmMotionTimer timer) {
    _motionTimerController.add(timer);
  }

  void emitModeChanged(RhythmModeResource mode) {
    _modeChangedController.add(mode);
  }

  void emitSettingsChanged(RhythmSettings settings) {
    _settingsChangedController.add(settings);
  }

  void emitLightBreakerChanged(RhythmLightBreaker lightBreaker) {
    _lightBreakerChangedController.add(lightBreaker);
  }

  void emitNewNodesDetected() {
    _newNodesController.add(null);
  }

  @override
  void dispose() {
    _helloController.close();
    _rhythmStateController.close();
    _hubEventController.close();
    _motionTimerController.close();
    _modeChangedController.close();
    _settingsChangedController.close();
    _lightBreakerChangedController.close();
    _newNodesController.close();
    _connectionStateController.close();
    super.dispose();
  }
}

Widget _buildTestApp({
  required RoomProvider roomProvider,
  required ServerSyncProvider provider,
  required Widget child,
  String? fontFamily,
}) {
  return MultiProvider(
    providers: [
      ChangeNotifierProvider<RoomProvider>.value(value: roomProvider),
      ChangeNotifierProvider<ServerSyncProvider>.value(value: provider),
      ChangeNotifierProvider<SubscriptionProvider>(
        create: (_) => _TestSubscriptionProvider(),
      ),
      // The room Schedule tab reads the home location for its orbital clock.
      ChangeNotifierProvider<HomeProvider>(
        create: (_) => _TestHomeProvider(const []),
      ),
    ],
    child: MaterialApp(
      theme: ThemeData(fontFamily: fontFamily),
      home: Scaffold(body: child),
    ),
  );
}

Future<void> _pumpRoomSettingsSheet(
  WidgetTester tester, {
  required RoomProvider roomProvider,
  required ServerSyncProvider provider,
  required RoomDto room,
}) async {
  await tester.pumpWidget(
    _buildTestApp(
      roomProvider: roomProvider,
      provider: provider,
      child: TickerMode(
        enabled: false,
        child: RoomSettingsSheet(
          room: room,
          enableLivePreview: false,
        ),
      ),
    ),
  );
  await tester.pump(const Duration(milliseconds: 10));
}

Future<void> _selectRoomSettingsTab(
  WidgetTester tester,
  String label,
) async {
  final labelFinder = find
      .descendant(
        of: find.byKey(const ValueKey('room-settings-tabs')),
        matching: find.text(label),
      )
      .first;
  await tester.ensureVisible(labelFinder);
  final tapTarget = tester.getTopLeft(labelFinder) + const Offset(4, 4);
  await tester.tapAt(tapTarget);
  await tester.pump(const Duration(milliseconds: 250));
}

void _registerWidgetCleanup(WidgetTester tester) {
  addTearDown(() async {
    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pump();
  });
}

Future<void> _captureRoomScheduleEvidence(
  WidgetTester tester,
  GlobalKey boundaryKey,
  String fileName,
) async {
  final outputDir = Platform.environment['CODEX_UI_SCREENSHOT_DIR'];
  if (outputDir == null || outputDir.isEmpty) return;
  final boundary = tester.renderObject<RenderRepaintBoundary>(
    find.byKey(boundaryKey),
  );
  boundary.markNeedsPaint();
  await tester.pump();
  await tester.runAsync(() async {
    final image = await boundary.toImage(pixelRatio: 2);
    final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
    await Directory(outputDir).create(recursive: true);
    await File('$outputDir/$fileName').writeAsBytes(
      bytes!.buffer.asUint8List(),
      flush: true,
    );
  });
}

Future<void> _loadRoomScheduleEvidenceFont() async {
  final executable = File(Platform.resolvedExecutable);
  final font = File(
    '${executable.parent.parent.parent.path}/material_fonts/Roboto-Regular.ttf',
  );
  final loader = FontLoader('CodexReadableRoboto')
    ..addFont(
      font.readAsBytes().then((bytes) => bytes.buffer.asByteData()),
    );
  await loader.load();
  final icons = File(
    '${executable.parent.parent.parent.path}/material_fonts/MaterialIcons-Regular.otf',
  );
  final iconLoader = FontLoader('MaterialIcons')
    ..addFont(
      icons.readAsBytes().then((bytes) => bytes.buffer.asByteData()),
    );
  await iconLoader.load();
}

RhythmSceneDefinition _testScene(String id) => RhythmSceneDefinition(
      id: id,
      name: 'Test Scene',
      light: const RhythmLightScene(
        defaultOutput: RhythmLightSceneOutput.on(
          brightness: 55,
          color:
              RhythmLightColor.rgb(RhythmSceneRgbColor(r: 240, g: 80, b: 24)),
        ),
      ),
    );

RhythmSceneDefinition _testPaletteScene(String id) => RhythmSceneDefinition(
      id: id,
      name: 'Palette Scene',
      light: const RhythmLightScene(
        palette: [
          RhythmLightSceneOutput.on(
            brightness: 42,
            color:
                RhythmLightColor.rgb(RhythmSceneRgbColor(r: 40, g: 90, b: 210)),
          ),
          RhythmLightSceneOutput.on(
            brightness: 80,
            color:
                RhythmLightColor.rgb(RhythmSceneRgbColor(r: 250, g: 80, b: 40)),
          ),
        ],
      ),
    );

RhythmSceneDefinition _testHuePaletteScene(String id) =>
    _testPaletteScene(id).copyWith(
      source: RhythmSceneSource.imported(
        provider: 'hue',
        externalId: id,
      ),
      extensions: const {'hue_palette_scene': true},
    );

RhythmSceneDefinition _testUnmarkedHueScene(String id) =>
    _testScene(id).copyWith(
      source: RhythmSceneSource.imported(
        provider: 'hue',
        externalId: id,
      ),
    );

RhythmSceneDefinition _testOffScene(String id) => RhythmSceneDefinition(
      id: id,
      name: 'Off Scene',
      light: const RhythmLightScene(
        defaultOutput: RhythmLightSceneOutput.off(),
      ),
    );

RhythmSceneDefinition _testPresetOnlyScene(String id) =>
    RhythmSceneDefinition.fromJson({
      'id': id,
      'name': 'Reset Scene',
      'light': {
        'entries': [
          {
            'target': {
              'kind': 'node',
              'node_id': 'room-1',
            },
            'value': {
              'kind': 'preset',
              'preset': 'reset',
            },
          },
        ],
      },
    });

void main() {
  test('additional motion preserves explicit targets over physical placement',
      () {
    expect(
      additionalMotionTargetRoomIds(
        existingTargetRoomIds: const ['room-explicit'],
        physicalParentNodeId: 'room-physical',
        additionalRoomId: 'room-new',
      ),
      {'room-explicit', 'room-new'},
    );
  });

  group('ServerSyncProvider.applyProfileConfig', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _FakeRhythmConnection connection;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _FakeRhythmConnection(api);
    });

    tearDown(() {
      roomProvider.dispose();
      connection.dispose();
    });

    ServerSyncProvider buildProvider() {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      return provider;
    }

    test('inserts a profile config when none is cached', () {
      final provider = buildProvider();
      expect(provider.profiles, isEmpty);

      final config = RhythmCurveConfig(
        id: 'rhythm',
        name: 'Day',
        curve: const RhythmSuperGaussianCurve(widthRightBri: 0.85),
      );
      provider.applyProfileConfig(config);

      expect(provider.profiles, hasLength(1));
      expect(provider.profiles.single.id, 'rhythm');
      expect(
        provider.profiles.single.superGaussianCurve?.widthRightBri,
        closeTo(0.85, 1e-9),
      );
    });

    test('replaces an existing cached profile with absorbed widths', () {
      // Regression: after "Absorb", the Time Simulator rebuilds its curve graph
      // from ServerSyncProvider.profiles (via _dayConfig). Before the fix the
      // cache held the pre-absorb widths until the next async hello, so the
      // graph never reshaped — the user's "Absorb does nothing" symptom.
      final provider = buildProvider();

      final before = RhythmCurveConfig(
        id: 'rhythm',
        name: 'Day',
        curve: const RhythmSuperGaussianCurve(widthRightBri: 0.85),
      );
      provider.applyProfileConfig(before);

      var notified = 0;
      provider.addListener(() => notified++);

      final absorbed = RhythmCurveConfig(
        id: 'rhythm',
        name: 'Day',
        curve: const RhythmSuperGaussianCurve(widthRightBri: 1.70),
      );
      provider.applyProfileConfig(absorbed);

      expect(
        provider.profiles,
        hasLength(1),
        reason: 'should replace the profile in place, not append a duplicate',
      );
      final cached =
          provider.profiles.firstWhere((profile) => profile.id == 'rhythm');
      expect(
        cached.superGaussianCurve?.widthRightBri,
        closeTo(1.70, 1e-9),
        reason: 'cached profile must carry the freshly absorbed widths',
      );
      expect(
        notified,
        1,
        reason: 'listeners are notified so the curve graph reloads',
      );
    });
  });

  group('ServerSyncProvider.pushHubCredentials', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _HelloRhythmConnection connection;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _HelloRhythmConnection(api);
    });

    tearDown(() {
      roomProvider.dispose();
      connection.dispose();
    });

    test('pushes Hue credentials and reconnects immediately', () async {
      final homeProvider = _TestHomeProvider([
        Hub.hue(
          id: 'hue-1',
          homeId: 'home-1',
          name: 'Philips Hue',
          bridgeIp: '192.168.1.20',
          appKey: 'hue-user',
        ),
      ]);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      connection.emitHello(RhythmHello.fromJson({
        'capabilities': {
          'api_schema_version': 2,
          'features': const [
            RhythmFeature.hueRoomAuthorityConsent,
            RhythmFeature.hueRoomTopologySync,
          ],
          'hubs': const <dynamic>[],
        },
      }));
      await Future<void>.delayed(Duration.zero);

      expect(provider.hueRoomTopologySyncSupported, isTrue);

      final hubConnected = await provider.pushHubCredentials(RoomSourceDto.hue);

      expect(api.hubCredentialsCalls, 1);
      expect(api.lastHubType, 'hue');
      expect(api.lastAddress, '192.168.1.20:443');
      expect(api.lastCredentials, {'username': 'hue-user'});
      expect(hubConnected, isTrue);
      expect(connection.reconnectCalls, 1);
    });

    test('refuses Hue credentials when the server lacks consent support',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([
          Hub.hue(
            id: 'hue-1',
            homeId: 'home-1',
            name: 'Philips Hue',
            bridgeIp: '192.168.1.20',
            appKey: 'hue-user',
          ),
        ]),
      );
      addTearDown(provider.dispose);

      expect(await provider.pushHubCredentials(RoomSourceDto.hue), isFalse);
      expect(api.hubCredentialsCalls, 0);
      expect(connection.reconnectCalls, 0);
    });

    test('pushes Home Assistant credentials with token payload', () async {
      final homeProvider = _TestHomeProvider([
        Hub.homeAssistant(
          id: 'ha-1',
          homeId: 'home-1',
          name: 'Home Assistant',
          host: 'ha.local',
          port: 8123,
          useSsl: false,
          token: 'ha-token',
        ),
      ]);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      final hubConnected =
          await provider.pushHubCredentials(RoomSourceDto.homeAssistant);

      expect(api.hubCredentialsCalls, 1);
      expect(api.lastHubType, 'homeassistant');
      expect(api.lastAddress, 'ha.local:8123');
      expect(api.lastCredentials, {'token': 'ha-token'});
      expect(hubConnected, isTrue);
      expect(connection.reconnectCalls, 1);
    });

    test(
        'returns false when Home Assistant credentials do not connect on server',
        () async {
      api.hubCredentialsResult = false;
      final homeProvider = _TestHomeProvider([
        Hub.homeAssistant(
          id: 'ha-1',
          homeId: 'home-1',
          name: 'Home Assistant',
          host: 'homeassistant.local',
          port: 8123,
          useSsl: false,
          token: 'ha-token',
        ),
      ]);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      final hubConnected =
          await provider.pushHubCredentials(RoomSourceDto.homeAssistant);

      expect(api.hubCredentialsCalls, 1);
      expect(api.lastHubType, 'homeassistant');
      expect(api.lastAddress, 'homeassistant.local:8123');
      expect(hubConnected, isFalse);
      expect(connection.reconnectCalls, 1);
    });

    test('does nothing when the server is disconnected', () async {
      connection.isConnected = false;
      final homeProvider = _TestHomeProvider([
        Hub.hue(
          id: 'hue-1',
          homeId: 'home-1',
          name: 'Philips Hue',
          bridgeIp: '192.168.1.20',
          appKey: 'hue-user',
        ),
      ]);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      final hubConnected = await provider.pushHubCredentials(RoomSourceDto.hue);

      expect(api.hubCredentialsCalls, 0);
      expect(hubConnected, isFalse);
      expect(connection.reconnectCalls, 0);
    });
  });

  group('ServerSyncProvider motion activation', () {
    RhythmHello motionHello({bool supported = true, bool enabled = true}) {
      return RhythmHello.fromJson({
        'version': '0.6.509-beta',
        'capabilities': {
          'api_schema_version': 1,
          'features':
              supported ? [RhythmFeature.motionActivationToggle] : <String>[],
          'hubs': const <dynamic>[],
        },
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'profile_settings': {
              if (supported) 'motion_activation_enabled': enabled,
            },
          },
        ],
      });
    }

    testWidgets('older appliances keep the motion control non-actionable', (
      tester,
    ) async {
      final roomProvider = RoomProvider();
      final api = _FakeRhythmServerApi();
      final connection = _HelloRhythmConnection(api);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      addTearDown(roomProvider.dispose);
      addTearDown(connection.dispose);

      connection.emitHello(motionHello(supported: false));
      await tester.pump();

      expect(provider.motionActivationSupportedForNode('room-1'), isFalse);
      expect(
        await provider.setNodeMotionActivationEnabled('room-1', false),
        isFalse,
      );
      expect(api.motionActivationCalls, isEmpty);
      expect(provider.motionActivationEnabledForNode('room-1'), isTrue);
    });

    testWidgets('rejected write rolls back state and restores the timer', (
      tester,
    ) async {
      final roomProvider = RoomProvider();
      final api = _FakeRhythmServerApi()..motionActivationSucceeds = false;
      final connection = _HelloRhythmConnection(api);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      addTearDown(roomProvider.dispose);
      addTearDown(connection.dispose);

      connection.emitHello(motionHello());
      await tester.pump();
      final timer = MotionTimerInfo(
        motionActive: true,
        motionOwned: true,
        remainingSecs: null,
        timeoutSecs: 300,
        receivedAt: DateTime.now(),
      );
      roomProvider.updateNodeMotionTimer('room-1', timer);

      expect(
        await provider.setNodeMotionActivationEnabled('room-1', false),
        isFalse,
      );
      expect(provider.motionActivationEnabledForNode('room-1'), isTrue);
      expect(roomProvider.getMotionTimer('room-1'), timer);
      expect(provider.motionActivationPendingForNode('room-1'), isFalse);
    });

    testWidgets('serializes repeated taps until authoritative state returns', (
      tester,
    ) async {
      final roomProvider = RoomProvider();
      final completer = Completer<RhythmRoomState?>();
      final api = _FakeRhythmServerApi()..motionActivationCompleter = completer;
      final connection = _HelloRhythmConnection(api);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      addTearDown(roomProvider.dispose);
      addTearDown(connection.dispose);

      connection.emitHello(motionHello());
      await tester.pump();

      final first = provider.setNodeMotionActivationEnabled('room-1', false);
      await tester.pump();
      expect(provider.motionActivationPendingForNode('room-1'), isTrue);
      expect(
        await provider.setNodeMotionActivationEnabled('room-1', true),
        isFalse,
      );
      expect(api.motionActivationCalls, hasLength(1));

      completer.complete(
        RhythmRoomState.fromJson({
          'node_id': 'room-1',
          'rhythm_enabled': true,
          'time_offset': 0.0,
          'brightness_offset': 0.0,
          'state': 'active',
          'profile_settings': {'motion_activation_enabled': false},
        }),
      );
      expect(await first, isTrue);
      expect(provider.motionActivationPendingForNode('room-1'), isFalse);
      expect(provider.motionActivationEnabledForNode('room-1'), isFalse);
    });

    testWidgets('disconnected writes do not change optimistic state', (
      tester,
    ) async {
      final roomProvider = RoomProvider();
      final api = _FakeRhythmServerApi();
      final connection = _HelloRhythmConnection(api);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      addTearDown(roomProvider.dispose);
      addTearDown(connection.dispose);

      connection.emitHello(motionHello());
      await tester.pump();
      connection.isConnected = false;

      expect(
        await provider.setNodeMotionActivationEnabled('room-1', false),
        isFalse,
      );
      expect(provider.motionActivationEnabledForNode('room-1'), isTrue);
      expect(api.motionActivationCalls, isEmpty);
    });
  });

  group('ServerSyncProvider room schedule authority', () {
    RhythmHello scheduleHello({
      String wakeTime = '06:30',
      String name = 'Kitchen',
      String kind = 'room',
      String? parentId,
    }) {
      return RhythmHello.fromJson({
        'capabilities': {
          'api_schema_version': 2,
          'features': [RhythmFeature.roomScheduleV1],
          'hubs': const <dynamic>[],
        },
        'nodes': [
          {
            'id': 'room-1',
            'name': name,
            'kind': kind,
            if (parentId != null) 'parent_id': parentId,
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'profile_settings': {
              'room_schedule': {
                'source': 'follow_time',
                'wake_time': wakeTime,
                'sleep_time': '22:30',
              },
            },
          },
        ],
      });
    }

    RhythmRoomSchedule schedule(String wakeTime) => RhythmRoomSchedule(
          source: RhythmRoomScheduleSource.followTime,
          wakeTime: wakeTime,
          sleepTime: '22:30',
        );

    testWidgets('supports rooms and unassigned bulbs but not assigned bulbs',
        (tester) async {
      final roomProvider = RoomProvider();
      final connection = _HelloRhythmConnection(_FakeRhythmServerApi());
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      addTearDown(roomProvider.dispose);
      addTearDown(connection.dispose);

      connection.emitHello(scheduleHello());
      await tester.pump();
      expect(provider.roomScheduleSupportedForNode('room-1'), isTrue);

      connection.emitHello(scheduleHello(
        kind: 'light_device',
        name: 'Porch Bulb',
      ));
      await tester.pump();
      expect(provider.roomScheduleSupportedForNode('room-1'), isTrue);

      connection.emitHello(scheduleHello(
        kind: 'light_device',
        name: 'Porch Bulb',
        parentId: 'porch-room',
      ));
      await tester.pump();
      expect(provider.roomScheduleSupportedForNode('room-1'), isFalse);
    });

    testWidgets('explains that an assigned bulb inherits room settings',
        (tester) async {
      final roomProvider = RoomProvider();
      final connection = _HelloRhythmConnection(_FakeRhythmServerApi());
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      addTearDown(roomProvider.dispose);
      addTearDown(connection.dispose);

      connection.emitHello(scheduleHello(
        kind: 'light_device',
        name: 'Porch Bulb',
        parentId: 'porch-room',
      ));
      await tester.pump();
      await tester.pumpWidget(_buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const RoomScheduleTab(
          roomId: 'room-1',
          roomName: 'Porch Bulb',
          showRoomLightingOverride: false,
        ),
      ));
      await tester.pump();

      expect(find.byKey(const ValueKey('light-schedule-inherited')),
          findsOneWidget);
      expect(
        find.text(
            'This bulb uses the custom light settings from its assigned room.'),
        findsOneWidget,
      );
    });

    testWidgets('installs the full authoritative write response',
        (tester) async {
      final roomProvider = RoomProvider();
      final api = _FakeRhythmServerApi()
        ..roomScheduleSetResponse = RhythmRoomState.fromJson({
          'node_id': 'room-1',
          'name': 'Authoritative Kitchen',
          'state': 'active',
          'rhythm_enabled': true,
          'time_offset': 0.0,
          'brightness_offset': 4.0,
          'profile_settings': {
            'room_schedule': schedule('07:15').toJson(),
          },
        });
      final connection = _HelloRhythmConnection(api);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      addTearDown(roomProvider.dispose);
      addTearDown(connection.dispose);

      connection.emitHello(scheduleHello());
      await tester.pump();

      expect(
        await provider.setRoomSchedule(
          'room-1',
          schedule('07:15'),
          requestId: 'room-schedule-save-journey-1',
        ),
        isTrue,
      );
      expect(
        api.roomScheduleSetRequestIds.single,
        'room-schedule-save-journey-1',
      );
      expect(provider.nodeById('room-1')?.name, 'Authoritative Kitchen');
      expect(provider.nodeById('room-1')?.brightnessOffset, 4.0);
      expect(provider.scheduleForRoom('room-1').wakeTime, '07:15');
      expect(provider.roomSchedulePendingForRoom('room-1'), isFalse);
    });

    testWidgets('a reconnect snapshot wins over an older rejection',
        (tester) async {
      final roomProvider = RoomProvider();
      final pending = Completer<RhythmRoomState?>();
      final api = _FakeRhythmServerApi()
        ..roomScheduleSetCompleters.add(pending);
      final connection = _HelloRhythmConnection(api);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      addTearDown(roomProvider.dispose);
      addTearDown(connection.dispose);

      connection.emitHello(scheduleHello());
      await tester.pump();
      final write = provider.setRoomSchedule('room-1', schedule('07:15'));
      await tester.pump();
      connection.emitHello(
        scheduleHello(wakeTime: '09:00', name: 'Reconnected Kitchen'),
      );
      await tester.pump();
      pending.complete(null);

      expect(await write, isFalse);
      expect(provider.nodeById('room-1')?.name, 'Reconnected Kitchen');
      expect(provider.scheduleForRoom('room-1').wakeTime, '09:00');
      expect(provider.roomSchedulePendingForRoom('room-1'), isFalse);
    });

    testWidgets('rapid edits ignore the stale first completion',
        (tester) async {
      final roomProvider = RoomProvider();
      final first = Completer<RhythmRoomState?>();
      final second = Completer<RhythmRoomState?>();
      final api = _FakeRhythmServerApi()
        ..roomScheduleSetCompleters.addAll([first, second]);
      final connection = _HelloRhythmConnection(api);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      addTearDown(roomProvider.dispose);
      addTearDown(connection.dispose);

      connection.emitHello(scheduleHello());
      await tester.pump();
      final firstWrite = provider.setRoomSchedule('room-1', schedule('07:15'));
      await tester.pump();
      final secondWrite = provider.setRoomSchedule('room-1', schedule('08:00'));
      await tester.pump();

      first.complete(RhythmRoomState.fromJson({
        'node_id': 'room-1',
        'state': 'active',
        'profile_settings': {
          'room_schedule': schedule('07:15').toJson(),
        },
      }));
      expect(await firstWrite, isTrue);
      expect(provider.scheduleForRoom('room-1').wakeTime, '08:00');
      expect(provider.roomSchedulePendingForRoom('room-1'), isTrue);

      second.complete(RhythmRoomState.fromJson({
        'node_id': 'room-1',
        'state': 'active',
        'profile_settings': {
          'room_schedule': schedule('08:00').toJson(),
        },
      }));
      expect(await secondWrite, isTrue);
      expect(provider.scheduleForRoom('room-1').wakeTime, '08:00');
      expect(provider.roomSchedulePendingForRoom('room-1'), isFalse);
    });

    testWidgets('a thrown write rolls back and always clears pending',
        (tester) async {
      final roomProvider = RoomProvider();
      final api = _FakeRhythmServerApi()..roomScheduleSetThrows = true;
      final connection = _HelloRhythmConnection(api);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      addTearDown(roomProvider.dispose);
      addTearDown(connection.dispose);

      connection.emitHello(scheduleHello());
      await tester.pump();

      expect(
          await provider.setRoomSchedule('room-1', schedule('07:15')), isFalse);
      expect(provider.scheduleForRoom('room-1').wakeTime, '06:30');
      expect(provider.roomSchedulePendingForRoom('room-1'), isFalse);
    });
  });

  group('ServerSyncProvider room light profile overrides', () {
    RhythmHello roomLightHello({
      bool supported = true,
      bool dayIdleSupported = false,
      bool motionActivationEnabled = false,
    }) {
      return RhythmHello.fromJson({
        'version': '0.6.533-beta',
        'capabilities': {
          'api_schema_version': 2,
          'features': supported
              ? [
                  RhythmFeature.roomLightProfileOverrides,
                  if (dayIdleSupported)
                    RhythmFeature.roomDayIdleProfileOverrides,
                ]
              : <String>[],
          'hubs': const <dynamic>[],
        },
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'profile_settings': {
              'motion_activation_enabled': motionActivationEnabled,
              'profile_overrides': {
                'sleep': {
                  'motion_timeout_secs': {'mode': 'fixed', 'value': 900},
                },
              },
            },
          },
        ],
      });
    }

    testWidgets('capability gates writes and supported save preserves siblings',
        (tester) async {
      final roomProvider = RoomProvider();
      final api = _FakeRhythmServerApi();
      final connection = _HelloRhythmConnection(api);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      addTearDown(roomProvider.dispose);
      addTearDown(connection.dispose);

      connection.emitHello(roomLightHello(supported: false));
      await tester.pump();
      expect(
        provider.hasNodeLightProfileOverrides('room-1'),
        isFalse,
        reason: 'motion timeout overrides are not custom light settings',
      );
      expect(
        provider.lightProfileOverridesSupportedForNode('room-1'),
        isFalse,
      );
      expect(
        await provider.setNodeLightProfileOverride(
          'room-1',
          profileId: 'rhythm',
          profileOverride:
              const RhythmLightProfileNodeOverride(minBrightness: 8),
          correlationId: 'room-light-settings-unsupported',
        ),
        isFalse,
      );
      expect(api.nodeProfileOverrideCalls, isEmpty);

      connection.emitHello(roomLightHello());
      await tester.pump();
      expect(
        await provider.setNodeLightProfileOverride(
          'room-1',
          profileId: 'rhythm',
          profileOverride: const RhythmLightProfileNodeOverride(
            minBrightness: 8,
            maxBrightness: 72,
          ),
          correlationId: 'room-light-settings-123',
        ),
        isTrue,
      );
      expect(provider.hasNodeLightProfileOverrides('room-1'), isTrue);
      expect(
        provider
            .nodeById('room-1')
            ?.profileSettings
            ?.profileOverrides['sleep']
            ?.motionTimeoutSecs,
        900,
      );
      final call = api.nodeProfileOverrideCalls.single;
      expect(call.replace, isTrue);
      expect(call.correlationId, 'room-light-settings-123');
      expect(call.profileOverrides, {
        'rhythm': {
          'min_brightness': 8,
          'max_brightness': 72,
        },
      });
    });

    testWidgets('day idle writes require the additive room capability', (
      tester,
    ) async {
      final roomProvider = RoomProvider();
      final api = _FakeRhythmServerApi();
      final connection = _HelloRhythmConnection(api);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      addTearDown(roomProvider.dispose);
      addTearDown(connection.dispose);

      connection.emitHello(roomLightHello());
      await tester.pump();
      expect(
        provider.roomDayIdleProfileOverridesSupportedForNode('room-1'),
        isFalse,
      );
      expect(
        await provider.setNodeLightProfileOverride(
          'room-1',
          profileId: 'day_idle',
          profileOverride:
              const RhythmLightProfileNodeOverride(maxBrightness: 8),
          correlationId: 'room-low-glow-unsupported',
        ),
        isFalse,
      );
      expect(api.nodeProfileOverrideCalls, isEmpty);

      connection.emitHello(roomLightHello(dayIdleSupported: true));
      await tester.pump();
      expect(
        provider.roomDayIdleProfileOverridesSupportedForNode('room-1'),
        isTrue,
      );
      expect(
        await provider.setNodeLightProfileOverride(
          'room-1',
          profileId: 'day_idle',
          profileOverride:
              const RhythmLightProfileNodeOverride(maxBrightness: 8),
          correlationId: 'room-low-glow-supported',
        ),
        isTrue,
      );
      expect(api.nodeProfileOverrideCalls, hasLength(1));
    });

    testWidgets('existing override contract accepts light-device nodes',
        (tester) async {
      final roomProvider = RoomProvider();
      final api = _FakeRhythmServerApi();
      final connection = _HelloRhythmConnection(api);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      addTearDown(roomProvider.dispose);
      addTearDown(connection.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'version': '0.6.533-beta',
          'capabilities': {
            'api_schema_version': 2,
            'features': [RhythmFeature.roomLightProfileOverrides],
            'hubs': const <dynamic>[],
          },
          'nodes': [
            {
              'id': 'light-1',
              'name': 'Desk Lamp',
              'kind': 'light_device',
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'light_capabilities': {
                'individual_profile_overrides': true,
              },
            },
          ],
        }),
      );
      await tester.pump();

      expect(
        provider.lightProfileOverridesSupportedForNode('light-1'),
        isTrue,
      );
      expect(
        await provider.setNodeLightProfileOverride(
          'light-1',
          profileId: 'rhythm',
          profileOverride:
              const RhythmLightProfileNodeOverride(maxBrightness: 64),
          correlationId: 'bulb-light-settings-123',
        ),
        isTrue,
      );
      expect(provider.hasNodeLightProfileOverrides('light-1'), isTrue);
      expect(api.nodeProfileOverrideCalls, hasLength(1));
      expect(api.nodeProfileOverrideCalls.single.nodeId, 'light-1');
      expect(api.nodeProfileOverrideCalls.single.profileOverrides, {
        'rhythm': {'max_brightness': 64},
      });
    });

    testWidgets('group-routed light capability rejects individual overrides',
        (tester) async {
      final roomProvider = RoomProvider();
      final api = _FakeRhythmServerApi();
      final connection = _HelloRhythmConnection(api);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      addTearDown(roomProvider.dispose);
      addTearDown(connection.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'capabilities': {
            'api_schema_version': 2,
            'features': [RhythmFeature.roomLightProfileOverrides],
            'hubs': const <dynamic>[],
          },
          'nodes': [
            {
              'id': 'hue-light-1',
              'name': 'Grouped Hue Lamp',
              'kind': 'light_device',
              'parent_id': 'room-1',
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'light_capabilities': {
                'individual_profile_overrides': false,
              },
            },
          ],
        }),
      );
      await tester.pump();

      expect(
        provider.lightProfileOverridesSupportedForNode('hue-light-1'),
        isFalse,
      );
      expect(
        await provider.setNodeLightProfileOverride(
          'hue-light-1',
          profileId: 'rhythm',
          profileOverride:
              const RhythmLightProfileNodeOverride(maxBrightness: 31),
          correlationId: 'grouped-hue-light-settings',
        ),
        isFalse,
      );
      expect(api.nodeProfileOverrideCalls, isEmpty);
    });

    testWidgets(
        'pending save rejects overlap and does not roll back newer server state',
        (tester) async {
      final roomProvider = RoomProvider();
      final pending = Completer<bool>();
      final api = _FakeRhythmServerApi()
        ..nodeProfileOverridesCompleter = pending;
      final connection = _HelloRhythmConnection(api);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      addTearDown(roomProvider.dispose);
      addTearDown(connection.dispose);

      connection.emitHello(roomLightHello());
      await tester.pump();
      final firstSave = provider.setNodeLightProfileOverride(
        'room-1',
        profileId: 'rhythm',
        profileOverride: const RhythmLightProfileNodeOverride(minBrightness: 8),
        correlationId: 'room-light-settings-pending',
      );
      await tester.pump();

      expect(
        await provider.setNodeLightProfileOverride(
          'room-1',
          profileId: 'sleep',
          profileOverride:
              const RhythmLightProfileNodeOverride(maxBrightness: 72),
          correlationId: 'room-light-settings-overlap',
        ),
        isFalse,
      );
      expect(api.nodeProfileOverrideCalls, hasLength(1));

      connection.emitHello(
        roomLightHello(motionActivationEnabled: true),
      );
      await tester.pump();
      pending.complete(false);
      expect(await firstSave, isFalse);
      await tester.pump();

      expect(
        provider.nodeById('room-1')?.profileSettings?.motionActivationEnabled,
        isTrue,
      );
      expect(
        provider
            .nodeById('room-1')
            ?.profileSettings
            ?.profileOverrides
            .containsKey('rhythm'),
        isFalse,
      );
    });

    testWidgets('rejected save rolls optimistic override back', (tester) async {
      final roomProvider = RoomProvider();
      final api = _FakeRhythmServerApi()..nodeProfileOverridesSucceeds = false;
      final connection = _HelloRhythmConnection(api);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      addTearDown(roomProvider.dispose);
      addTearDown(connection.dispose);

      connection.emitHello(roomLightHello());
      await tester.pump();
      expect(
        await provider.setNodeLightProfileOverride(
          'room-1',
          profileId: 'rhythm',
          profileOverride: const RhythmLightProfileNodeOverride(
            minBrightness: 42,
            maxBrightness: 42,
          ),
          correlationId: 'room-light-settings-failed',
        ),
        isFalse,
      );
      expect(api.nodeProfileOverrideCalls.single.profileOverrides, {
        'rhythm': {
          'min_brightness': 42,
          'max_brightness': 42,
        },
      });
      expect(
        provider
            .nodeById('room-1')
            ?.profileSettings
            ?.profileOverrides
            .containsKey('rhythm'),
        isFalse,
      );
      expect(
        provider
            .nodeById('room-1')
            ?.profileSettings
            ?.profileOverrides['sleep']
            ?.motionTimeoutSecs,
        900,
      );
    });

    testWidgets('reset clears overrides but preserves other room preferences',
        (tester) async {
      final roomProvider = RoomProvider();
      final api = _FakeRhythmServerApi();
      final connection = _HelloRhythmConnection(api);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      addTearDown(roomProvider.dispose);
      addTearDown(connection.dispose);

      connection.emitHello(roomLightHello());
      await tester.pump();
      expect(
        await provider.resetNodeLightProfileOverrides(
          'room-1',
          correlationId: 'room-light-settings-reset',
        ),
        isTrue,
      );
      final settings = provider.nodeById('room-1')?.profileSettings;
      expect(settings?.profileOverrides, isEmpty);
      expect(settings?.motionActivationEnabled, isFalse);
      final call = api.nodeProfileOverrideCalls.single;
      expect(call.profileOverrides, isNull);
      expect(call.replace, isTrue);
    });
  });

  group('ServerSyncProvider.retryHub', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _FakeRhythmConnection connection;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _FakeRhythmConnection(api);
    });

    tearDown(() {
      roomProvider.dispose();
      connection.dispose();
    });

    test('calls /api/hub/retry and refreshes state once', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      final ok = await provider.retryHub('hue', '192.168.1.20:443');

      expect(ok, isTrue);
      expect(api.hubRetryCalls, 1);
      expect(api.lastHubType, 'hue');
      expect(api.lastAddress, '192.168.1.20:443');
      expect(connection.reconnectCalls, 1);
    });

    test('retries multiple hubs and reconnects once', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      final accepted = await provider.retryHubs([
        {
          'type': 'hue',
          'address': '192.168.1.20:443',
          'startup_retry': {'status': 'manual_retry_required'},
        },
        {
          'type': 'homeassistant',
          'address': 'ha.local:8123',
          'startup_retry': {'status': 'manual_retry_required'},
        },
      ]);

      expect(accepted, 2);
      expect(api.hubRetryCalls, 2);
      expect(connection.reconnectCalls, 1);
    });
  });

  group('ServerSyncProvider Matter capabilities', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _HelloRhythmConnection connection;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _HelloRhythmConnection(api);
    });

    tearDown(() {
      roomProvider.dispose();
      connection.dispose();
    });

    test('uses device_onboarding_methods from the hello capabilities payload',
        () async {
      final homeProvider = _TestHomeProvider(const []);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'rooms': const <Map<String, dynamic>>[],
          'location': const <String, dynamic>{},
          'capabilities': {
            'features': [RhythmFeature.matterSetupCodeRecovery],
            'hubs': [
              {
                'type': 'matter',
                'configurable': true,
                'device_onboarding_methods': [
                  'matter_on_network_setup_code',
                ],
                'supports_unpairing': true,
                'supports_roomless_devices': true,
              },
            ],
          },
        }),
      );

      await Future<void>.delayed(Duration.zero);

      expect(provider.canAddMatterDevice, isTrue);
      expect(provider.canAddMatterOnNetworkDevice, isTrue);
      expect(provider.canCommissionMatterBleWifi, isFalse);
      expect(provider.canUnpairMatterDevices, isTrue);
      expect(provider.canRecoverMatterSetupCode, isTrue);
      expect(provider.supportsMatterRoomlessDevices, isTrue);
    });

    test(
        'treats explicit hub capabilities as authoritative when Matter is absent',
        () async {
      final homeProvider = _TestHomeProvider(const []);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'rooms': const <Map<String, dynamic>>[],
          'location': const <String, dynamic>{},
          'capabilities': {
            'hubs': [
              {
                'type': 'hue',
                'configurable': true,
                'device_onboarding_methods': const <String>[],
                'supports_unpairing': false,
                'supports_roomless_devices': false,
              },
            ],
          },
        }),
      );

      await Future<void>.delayed(Duration.zero);

      expect(provider.canRecoverMatterSetupCode, isFalse);

      expect(provider.canConfigureHub('hue'), isTrue);
      expect(provider.canConfigureHub('homeassistant'), isFalse);
      expect(provider.canAddMatterDevice, isFalse);
      expect(provider.canAddMatterOnNetworkDevice, isFalse);
      expect(provider.canCommissionMatterBleWifi, isFalse);
    });

    test('exposes Hue BLE only for its advertised nearby-scan method',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'rooms': const <Map<String, dynamic>>[],
          'location': const <String, dynamic>{},
          'capabilities': {
            'hubs': [
              {
                'type': 'hue_ble',
                'configurable': false,
                'device_onboarding_methods': ['hue_ble_nearby_scan'],
                'supports_unpairing': true,
                'supports_roomless_devices': true,
              },
            ],
          },
        }),
      );

      await Future<void>.delayed(Duration.zero);

      expect(provider.canAddHueBleDevice, isTrue);
      expect(provider.canUnpairHueBleDevices, isTrue);
      expect(provider.supportsHueBleRoomlessDevices, isTrue);
      expect(provider.canScanToAddDevice, isTrue);
      expect(
        provider.canScanToAddDeviceType(RhythmDeviceType.light),
        isTrue,
      );
      expect(
        provider.canScanToAddDeviceType(RhythmDeviceType.button),
        isFalse,
      );
      expect(provider.canAddMatterDevice, isFalse);
    });

    test('exposes local-BLE QR only for an explicitly advertised profile',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(RhythmHello.fromJson({
        'rooms': const <Map<String, dynamic>>[],
        'location': const <String, dynamic>{},
        'capabilities': {
          'hubs': [
            {
              'type': 'local_ble',
              'configurable': false,
              'device_onboarding_methods': ['local_ble_qr'],
              'device_profiles': [
                {
                  'id': RhythmDeviceProfileId.oreinOc02001Button,
                  'device_type': 'button',
                  'display_name': 'Button',
                  'input_only': true,
                  'onboarding_methods': ['local_ble_qr'],
                },
              ],
              'supports_unpairing': true,
              'supports_roomless_devices': true,
              'blocks_room_readiness': false,
            },
          ],
        },
      }));
      await Future<void>.delayed(Duration.zero);

      expect(
        provider.canAddLocalBleProfile(
          RhythmDeviceProfileId.oreinOc02001Button,
        ),
        isTrue,
      );
      expect(provider.canUnpairLocalBleDevices, isTrue);
      expect(provider.supportsLocalBleRoomlessDevices, isTrue);
      expect(provider.canScanToAddDevice, isTrue);
      expect(
        provider.canScanToAddDeviceType(RhythmDeviceType.button),
        isTrue,
      );
      expect(
        provider.canScanToAddDeviceType(RhythmDeviceType.light),
        isFalse,
      );
      expect(
        provider.canScanToAddDeviceType(RhythmDeviceType.motion),
        isFalse,
      );
    });

    test('routes a known parser alias to the advertised current BLE profile',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(RhythmHello.fromJson({
        'rooms': const <Map<String, dynamic>>[],
        'location': const <String, dynamic>{},
        'capabilities': {
          'hubs': [
            {
              'type': 'local_ble',
              'configurable': false,
              'device_onboarding_methods': ['local_ble_qr'],
              'device_profiles': [
                {
                  'id': 'orein.oc02001.button.v2',
                  'compatible_profile_ids': [
                    RhythmDeviceProfileId.oreinOc02001Button,
                  ],
                  'device_type': 'button',
                  'display_name': 'Button',
                  'input_only': true,
                  'onboarding_methods': ['local_ble_qr'],
                },
              ],
            },
          ],
        },
      }));
      await Future<void>.delayed(Duration.zero);

      expect(
        provider.supportedLocalBleProfileIds,
        {RhythmDeviceProfileId.oreinOc02001Button},
      );
      expect(
        provider.canonicalLocalBleProfileId(
          RhythmDeviceProfileId.oreinOc02001Button,
        ),
        'orein.oc02001.button.v2',
      );
      expect(
          provider.canAddLocalBleProfile('orein.oc02001.button.v2'), isFalse);
    });

    test('requires the matching profile to advertise QR intake', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(RhythmHello.fromJson({
        'rooms': const <Map<String, dynamic>>[],
        'location': const <String, dynamic>{},
        'capabilities': {
          'hubs': [
            {
              'type': 'local_ble',
              'configurable': false,
              'device_onboarding_methods': ['local_ble_qr'],
              'device_profiles': [
                {
                  'id': RhythmDeviceProfileId.oreinOc02001Button,
                  'device_type': 'button',
                  'display_name': 'Button',
                  'input_only': true,
                },
              ],
            },
          ],
        },
      }));
      await Future<void>.delayed(Duration.zero);

      expect(provider.supportedLocalBleProfileIds, isEmpty);
      expect(provider.canAddLocalBleDevice, isFalse);
      expect(provider.canScanToAddDevice, isFalse);
    });

    test('does not expose a newer QR profile without a local parser', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(RhythmHello.fromJson({
        'rooms': const <Map<String, dynamic>>[],
        'location': const <String, dynamic>{},
        'capabilities': {
          'hubs': [
            {
              'type': 'local_ble',
              'configurable': false,
              'device_onboarding_methods': ['local_ble_qr'],
              'device_profiles': [
                {
                  'id': 'future.vendor.bulb.v1',
                  'device_type': 'light',
                  'display_name': 'BLE Bulb',
                  'input_only': false,
                  'onboarding_methods': ['local_ble_qr'],
                },
              ],
            },
          ],
        },
      }));
      await Future<void>.delayed(Duration.zero);

      expect(provider.supportedLocalBleProfileIds, isEmpty);
      expect(provider.canAddLocalBleDevice, isFalse);
      expect(provider.canScanToAddDevice, isFalse);
    });

    test('offers Hue serial intake only for an advertised connected bridge',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      RhythmHello hello({required bool connected}) => RhythmHello.fromJson({
            'rooms': const <Map<String, dynamic>>[],
            'location': const <String, dynamic>{},
            'hubs': [
              {
                'type': 'hue',
                'address': '192.168.1.20:443',
                'connected': connected,
              },
            ],
            'capabilities': {
              'hubs': [
                {
                  'type': 'hue',
                  'configurable': true,
                  'device_onboarding_methods': [
                    'hue_bridge_serial_search',
                    'hue_bridge_button_search',
                  ],
                  'supports_unpairing': true,
                  'unpairable_device_types': ['light', 'button', 'motion'],
                  'supports_roomless_devices': false,
                },
              ],
            },
          });

      connection.emitHello(hello(connected: false));
      await Future<void>.delayed(Duration.zero);
      expect(provider.canAddHueBridgeDeviceBySerial, isFalse);
      expect(provider.canAddHueBridgeButton, isFalse);
      expect(provider.canUnpairHueBridgeDevices, isTrue);
      expect(
        provider.canUnpairHueBridgeDeviceType(RhythmDeviceType.button),
        isTrue,
      );
      expect(provider.canScanToAddDevice, isFalse);

      connection.emitHello(hello(connected: true));
      await Future<void>.delayed(Duration.zero);
      expect(provider.canAddHueBridgeDeviceBySerial, isTrue);
      expect(provider.canAddHueBridgeButton, isTrue);
      expect(provider.canUnpairHueBridgeDevices, isTrue);
      expect(provider.canScanToAddDevice, isTrue);

      final legacy = RhythmHello.fromJson({
        'hubs': [
          {'type': 'hue', 'address': '192.168.1.20:443', 'connected': true},
        ],
        'capabilities': {
          'hubs': [
            {'type': 'hue', 'supports_unpairing': true},
          ],
        },
      });
      connection.emitHello(legacy);
      await Future<void>.delayed(Duration.zero);
      expect(provider.canAddHueBridgeButton, isFalse);
      expect(
        provider.canUnpairHueBridgeDeviceType(RhythmDeviceType.light),
        isTrue,
      );
      expect(
        provider.canUnpairHueBridgeDeviceType(RhythmDeviceType.button),
        isFalse,
      );
    });
  });

  group('ServerSyncProvider location reconciliation', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _HelloRhythmConnection connection;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _HelloRhythmConnection(api);
    });

    tearDown(() {
      roomProvider.dispose();
      connection.dispose();
    });

    test('accepts server location when the local home has none', () async {
      final home = Home.create(
        id: 'home-1',
        name: 'My Home',
        ownerId: 'user-1',
      );
      final homeProvider = _TestHomeProvider(
        const [],
        currentHome: home,
      );
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'rooms': const <Map<String, dynamic>>[],
          'location': {
            'latitude': 40.7128,
            'longitude': -74.0060,
            'timezone': 'America/New_York',
          },
        }),
      );

      await Future<void>.delayed(Duration.zero);

      final adoptedHome = homeProvider.currentHome!;
      expect(adoptedHome.location?.latitude, 40.7128);
      expect(adoptedHome.location?.longitude, -74.0060);
      expect(adoptedHome.timezone, 'America/New_York');
    });
  });

  group('ServerSyncProvider topology wiring', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _HelloRhythmConnection connection;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _HelloRhythmConnection(api);
    });

    tearDown(() {
      roomProvider.dispose();
      connection.dispose();
    });

    test('topology mutation refresh reports a preserved stale topology',
        () async {
      api.topologyNodes = [
        RhythmTopologyNode.fromJson({
          'id': 'room-1',
          'name': 'Kitchen',
          'kind': 'room',
        }),
      ];
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(Duration.zero);
      await Future<void>.delayed(Duration.zero);
      expect(provider.topologyNodes, hasLength(1));

      // RhythmServerApi uses an empty list as the failure result for this
      // endpoint. Preserve the last good graph, but do not report the move as
      // authoritatively reconciled.
      api.topologyNodes = const [];
      final refreshed = await provider.refreshAfterTopologyMutation();

      expect(refreshed, isFalse);
      expect(provider.topologyNodes.single.id, 'room-1');
      expect(connection.lastReconnectAuthoritative, isTrue);
    });

    test('marks and updates multiple motion-target nodes', () async {
      api.topologyNodes = [
        RhythmTopologyNode.fromJson({
          'id': 'room-1',
          'name': 'Kitchen',
          'kind': 'room',
        }),
        RhythmTopologyNode.fromJson({
          'id': 'room-2',
          'name': 'Hall',
          'kind': 'room',
        }),
        RhythmTopologyNode.fromJson({
          'id': 'sensor-1',
          'name': 'Kitchen Motion',
          'kind': 'motion_sensor',
          'controls': [
            {
              'kind': 'motion',
              'target_id': 'room-1',
              'inherited': false,
            },
            {
              'kind': 'motion',
              'target_id': 'room-2',
              'inherited': false,
            },
          ],
        }),
      ];

      final homeProvider = _TestHomeProvider(const []);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': [
            {
              'id': 'room-1',
              'name': 'Kitchen',
              'kind': 'room',
              'hub_types': ['hue'],
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'lights_on': true,
            },
            {
              'id': 'room-2',
              'name': 'Hall',
              'kind': 'room',
              'hub_types': ['hue'],
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'lights_on': true,
            },
          ],
          'location': const <String, dynamic>{},
        }),
      );

      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(roomProvider.hasMotionSensor('room-1'), isTrue);
      expect(roomProvider.hasMotionSensor('room-2'), isTrue);
      expect(provider.nodeHasMotionControlTarget('room-1'), isTrue);
      expect(provider.nodeHasMotionControlTarget('room-2'), isTrue);
      expect(
        provider.controlTargetNodeId(
          sourceNodeId: 'sensor-1',
          controlKind: 'motion',
        ),
        'room-1',
      );
      expect(
        provider.controlTargetNodeIds(
          sourceNodeId: 'sensor-1',
          controlKind: 'motion',
        ),
        ['room-1', 'room-2'],
      );

      final success = await provider.setNodeControlTargets(
        sourceNodeId: 'sensor-1',
        controlKind: 'motion',
        targetNodeIds: ['room-2', 'room-1', 'room-2', ''],
      );

      expect(success, isTrue);
      expect(api.setTopologyNodeControlTargetsCalls, 1);
      expect(api.lastControlSourceNodeId, 'sensor-1');
      expect(api.lastControlKind, 'motion');
      expect(api.lastControlTargetIds, ['room-1', 'room-2']);
    });

    test('control target save fails when authoritative refresh fails',
        () async {
      api.topologyNodes = [
        RhythmTopologyNode.fromJson({
          'id': 'button-1',
          'name': 'Kitchen Button',
          'kind': 'button',
          'parent_id': 'room-1',
        }),
      ];
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));
      expect(provider.topologyNodes, hasLength(1));

      api.topologyNodes = const [];
      final success = await provider.setNodeControlTargets(
        sourceNodeId: 'button-1',
        controlKind: 'button',
        targetNodeIds: const ['room-1', 'room-2'],
      );

      expect(success, isFalse);
      expect(provider.topologyNodes.single.id, 'button-1');
    });

    test('control target save survives the nodes-changed reconnect race',
        () async {
      api.topologyNodes = [
        RhythmTopologyNode.fromJson({
          'id': 'room-1',
          'name': 'Kitchen',
          'kind': 'room',
        }),
        RhythmTopologyNode.fromJson({
          'id': 'button-1',
          'name': 'Kitchen Button',
          'kind': 'button',
          'parent_id': 'room-1',
        }),
      ];
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      connection.reconnectHandler = (authoritative) {
        connection.isConnected = authoritative;
      };
      api.beforeSetTopologyNodeControlTargets = () async {
        connection.emitNewNodesDetected();
        await Future<void>.delayed(Duration.zero);
        expect(connection.isConnected, isFalse);
      };

      final success = await provider.setNodeControlTargets(
        sourceNodeId: 'button-1',
        controlKind: 'button',
        targetNodeIds: const ['room-1'],
      );

      expect(success, isTrue);
      expect(connection.reconnectCalls, 2);
      expect(connection.lastReconnectAuthoritative, isTrue);
      expect(
        provider.controlTargetNodeIds(
          sourceNodeId: 'button-1',
          controlKind: 'button',
        ),
        ['room-1'],
      );
    });

    test('keeps roomless light-device nodes in the room provider', () async {
      final homeProvider = _TestHomeProvider(const []);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': [
            {
              'id': 'room-1',
              'name': 'Kitchen',
              'kind': 'room',
              'hub_types': ['matter'],
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'lights_on': true,
            },
            {
              'id': 'light-1',
              'name': 'Desk Lamp',
              'kind': 'light_device',
              'hub_types': ['matter'],
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'lights_on': true,
            },
          ],
          'location': const <String, dynamic>{},
        }),
      );

      await Future<void>.delayed(const Duration(milliseconds: 10));

      final node = roomProvider.getNode('light-1');
      expect(node, isNotNull);
      expect(node!.kind, RoomNodeKind.lightDevice);
      expect(node.parentId, isNull);
      expect(node.deviceIds, ['light-1']);
      expect(
        roomProvider.enabledRooms.map((room) => room.id),
        contains('light-1'),
      );
    });

    test('uses observed_power lights_on for room visuals', () async {
      final homeProvider = _TestHomeProvider(const []);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': [
            {
              'id': 'room-1',
              'name': 'Kitchen',
              'kind': 'room',
              'hub_types': ['hue'],
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'lights_on': true,
              'observed_power': {'lights_on': false},
            },
          ],
          'location': const <String, dynamic>{},
        }),
      );

      await Future<void>.delayed(const Duration(milliseconds: 10));

      final room = roomProvider.getRoom('room-1');
      expect(room, isNotNull);
      expect(room!.lightsOn, isFalse);

      final helloRoom =
          provider.helloRooms.singleWhere((entry) => entry.id == 'room-1');
      expect(helloRoom.lightsOn, isFalse);
    });
  });

  group('ServerSyncProvider input bindings', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _HelloRhythmConnection connection;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _HelloRhythmConnection(api);
    });

    tearDown(() {
      roomProvider.dispose();
      connection.dispose();
    });

    test('reads day/sleep toggle bindings from hello', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'input_bindings': [
            {
              'id': 'day_sleep_toggle:button-1:on_press',
              'preset': 'day_sleep_toggle',
              'source_node_id': 'button-1',
              'trigger': {
                'kind': 'button',
                'button_action': 'on_press',
              },
              'action': {
                'kind': 'mode_cycle',
                'modes': ['day', 'sleep'],
                'transition': {'kind': 'auto'},
              },
              'enabled': true,
            },
          ],
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.daySleepToggleInputBinding?.sourceNodeId, 'button-1');
      expect(provider.daySleepToggleInputBinding?.enabled, isTrue);
    });

    test('binds one selected button as the day/sleep toggle', () async {
      api.inputBindings = [
        RhythmInputBinding(
          id: 'day_sleep_toggle:old:on_press',
          preset: RhythmInputBindingPreset.daySleepToggle,
          sourceNodeId: 'old',
          trigger: const RhythmInputBindingTrigger.button(
            buttonAction: RhythmButtonAction.onPress,
          ),
          action: const RhythmModeCycleAction(
            modes: [RhythmMode.day, RhythmMode.sleep],
          ),
        ),
      ];
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'input_bindings': api.inputBindings
              .map((binding) => binding.toJson())
              .toList(growable: false),
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final ok = await provider.bindDaySleepToggleButton('button-2');

      expect(ok, isTrue);
      expect(api.createDaySleepToggleInputBindingCalls, 1);
      expect(api.lastInputBindingSourceNodeId, 'button-2');
      expect(api.deleteInputBindingCalls, 1);
      expect(api.lastDeletedInputBindingId, 'day_sleep_toggle:old:on_press');
      expect(provider.daySleepToggleInputBinding?.sourceNodeId, 'button-2');
    });

    test('preserves the selected button action when binding', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(RhythmHello.fromJson({}));
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final ok = await provider.bindDaySleepToggleButton(
        'button-2',
        buttonAction: RhythmButtonAction.offPress,
      );

      expect(ok, isTrue);
      expect(api.lastInputBindingSourceNodeId, 'button-2');
      expect(api.lastInputBindingButtonAction, RhythmButtonAction.offPress);
      expect(
        provider.daySleepToggleInputBinding?.trigger.buttonAction,
        RhythmButtonAction.offPress,
      );
    });

    test('does not delete stale binding when create already replaced it',
        () async {
      api.createReplacesPresetBindings = true;
      api.inputBindings = [
        RhythmInputBinding(
          id: 'day_sleep_toggle:old:on_press',
          preset: RhythmInputBindingPreset.daySleepToggle,
          sourceNodeId: 'old',
          trigger: const RhythmInputBindingTrigger.button(
            buttonAction: RhythmButtonAction.onPress,
          ),
          action: const RhythmModeCycleAction(
            modes: [RhythmMode.day, RhythmMode.sleep],
          ),
        ),
      ];
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'input_bindings': api.inputBindings
              .map((binding) => binding.toJson())
              .toList(growable: false),
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final ok = await provider.bindDaySleepToggleButton('button-2');

      expect(ok, isTrue);
      expect(api.deleteInputBindingCalls, 0);
      expect(provider.daySleepToggleInputBinding?.sourceNodeId, 'button-2');
    });

    test('toggles the persisted day/sleep binding enabled flag', () async {
      final binding = RhythmInputBinding(
        id: 'day_sleep_toggle:button-1:on_press',
        preset: RhythmInputBindingPreset.daySleepToggle,
        sourceNodeId: 'button-1',
        trigger: const RhythmInputBindingTrigger.button(
          buttonAction: RhythmButtonAction.onPress,
        ),
        action: const RhythmModeCycleAction(
          modes: [RhythmMode.day, RhythmMode.sleep],
        ),
      );
      api.inputBindings = [binding];
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'input_bindings': api.inputBindings
              .map((entry) => entry.toJson())
              .toList(growable: false),
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final ok = await provider.setDaySleepToggleButtonEnabled(false);

      expect(ok, isTrue);
      expect(api.setInputBindingCalls, 1);
      expect(api.lastSetInputBinding?.enabled, isFalse);
      expect(provider.daySleepToggleInputBinding?.enabled, isFalse);
    });

    test('treats missing binding enable toggles as successful no-ops',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(RhythmHello.fromJson({}));
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final ok = await provider.setDaySleepToggleButtonEnabled(false);

      expect(ok, isTrue);
      expect(api.setInputBindingCalls, 0);
    });
  });

  group('ServerSyncProvider SSE sync', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _HelloRhythmConnection connection;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _HelloRhythmConnection(api);
    });

    tearDown(() {
      roomProvider.dispose();
      connection.dispose();
    });

    test('bootstraps motion timer state from hello nodes', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': [
            {
              'id': 'room-1',
              'name': 'Kitchen',
              'kind': 'room',
              'hub_types': ['hue'],
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'lights_on': true,
              'motion_active': false,
              'motion_owned': true,
              'remaining_secs': 42,
              'timeout_secs': 1200,
              'warning_active': true,
              'devices': [
                {
                  'id': 'sensor-1',
                  'type': 'motion',
                  'name': 'Kitchen Motion',
                },
              ],
            },
          ],
          'location': const <String, dynamic>{},
        }),
      );

      await Future<void>.delayed(const Duration(milliseconds: 10));

      final timer = roomProvider.getMotionTimer('room-1');
      expect(timer, isNotNull);
      expect(timer!.remainingSecs, 42);
      expect(timer.motionOwned, isTrue);
      expect(timer.warningActive, isTrue);
      expect(roomProvider.hasMotionSensor('room-1'), isTrue);
      expect(
        provider.helloRooms
            .singleWhere((room) => room.id == 'room-1')
            .warningActive,
        isTrue,
      );
    });

    test('applies node_state updates to hello room summaries', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': [
            {
              'id': 'room-1',
              'name': 'Kitchen',
              'kind': 'room',
              'hub_types': ['hue'],
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'lights_on': true,
              'light_capabilities': {
                'color_temperature': {
                  'min_kelvin': 1000,
                  'max_kelvin': 20000,
                },
              },
            },
          ],
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      connection.emitRhythmState(
        RhythmRoomState.fromJson({
          'id': 'room-1',
          'name': 'Kitchen Evening',
          'kind': 'room',
          'hub_types': ['hue'],
          'state': 'hard_off',
          'transitioning': true,
          'rhythm_enabled': false,
          'time_offset': 12.0,
          'brightness_offset': -8.0,
          'lights_on': false,
          'brightness': 9,
          'kelvin': 2100,
          'profile_settings': {
            'profile_id': 'sleep',
            'fade_ms': {'mode': 'fixed', 'value': 1500},
          },
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final room =
          provider.helloRooms.singleWhere((entry) => entry.id == 'room-1');
      expect(room.name, 'Kitchen Evening');
      expect(room.state, RoomModeState.hardOff);
      expect(room.transitioning, isTrue);
      expect(room.rhythmEnabled, isFalse);
      expect(room.lightsOn, isFalse);
      expect(room.brightness, 9);
      expect(room.kelvin, 2100);
      expect(room.profileSettings?.profileId, 'sleep');
      expect(room.profileSettings?.fadeMs, 1500);
      final colorTemperature =
          provider.colorTemperatureCapabilitiesForNode('room-1');
      expect(colorTemperature?.minKelvin, 1000);
      expect(colorTemperature?.maxKelvin, 20000);

      connection.emitRhythmState(
        RhythmRoomState.fromJson({
          'id': 'room-1',
          'state': 'hard_off',
          'rhythm_enabled': false,
          'time_offset': 12.0,
          'brightness_offset': -8.0,
          'light_capabilities': <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final knownCapabilities = provider.nodeById('room-1')?.lightCapabilities;
      expect(knownCapabilities, isNotNull);
      expect(knownCapabilities?.supportsColorTemperature, isFalse);
    });

    test('resolves persisted mood color from server mood profile', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': [
            {
              'id': 'room-1',
              'name': 'Kitchen',
              'kind': 'room',
              'hub_types': ['hue'],
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'lights_on': true,
              'profile_settings': {
                'mood_enabled': true,
                'mood_profile_id': 'node_mood_room-1',
              },
            },
          ],
          'profiles': [
            {
              'id': 'node_mood_room-1',
              'name': 'Kitchen Mood',
              'curve': {
                'type': 'constant',
                'brightness': 1,
                'color_temp': 0,
                'direct_color': {
                  'rgb': {'r': 20, 'g': 80, 'b': 240},
                  'xy': {'x': 0.16, 'y': 0.08},
                },
              },
            },
          ],
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.moodColorForNode('room-1'), (20, 80, 240));
      expect(roomProvider.getMoodColor('room-1'), (20, 80, 240));
    });

    test('applies mode_changed updates to active mode for main pills',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'mode': {'active': 'day'},
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.activeMode, RhythmMode.day);

      connection.emitModeChanged(
        RhythmModeResource.fromJson({
          'active': 'sleep',
          'cause': 'manual',
          'transition_id': 'day_to_sleep',
          'epoch_ms': 1778058932588,
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.activeMode, RhythmMode.sleep);
    });

    test('runtime selection survives incremental hello without runtime fields',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'light_runtime': 'rhythm-adaptive',
          'mode': {
            'active': 'day',
            'light_runtime': 'rhythm-adaptive',
          },
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.lightRuntime, RhythmLightRuntime.rhythmAdaptive);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'mode': {'active': 'day'},
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.lightRuntime, RhythmLightRuntime.rhythmAdaptive);
    });

    test('runtime switch waits for initial apply pacing metadata', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);
      api.lightRuntimeInitialApply = const RhythmLightRuntimeInitialApply(
        queued: true,
        dispatchCount: 2,
        dispatchSpacingMs: 500,
        estimatedDispatchMs: 1500,
      );

      var completed = false;
      final future = provider
          .dispatchSetLightRuntime(
        RhythmLightRuntime.rhythmAdaptive,
        transitionMs: 4321,
      )
          .then((value) {
        completed = true;
        return value;
      });

      await Future<void>.delayed(const Duration(milliseconds: 100));

      expect(api.setLightRuntimeCalls, 1);
      expect(api.lastSetLightRuntime, RhythmLightRuntime.rhythmAdaptive);
      expect(api.lastSetLightRuntimeTransitionMs, 4321);
      expect(provider.lightRuntime, RhythmLightRuntime.rhythmAdaptive);
      expect(completed, isFalse);

      await Future<void>.delayed(const Duration(milliseconds: 2600));

      expect(await future, isTrue);
      expect(completed, isTrue);
    });

    test('mode_changed without runtime keeps current light runtime', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'light_runtime': 'rhythm-adaptive',
          'mode': {
            'active': 'day',
            'light_runtime': 'rhythm-adaptive',
          },
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      connection.emitModeChanged(
        RhythmModeResource.fromJson({
          'active': 'sleep',
          'cause': 'manual',
          'transition_id': 'day_to_sleep',
          'epoch_ms': 1778058932588,
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.activeMode, RhythmMode.sleep);
      expect(provider.lightRuntime, RhythmLightRuntime.rhythmAdaptive);
    });

    test('does not infer runtime selection from day profile while asleep',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'mode': {
            'active': 'sleep',
            'configs': [
              {
                'mode': 'day',
                'active_profile_id': 'custom',
              },
              {
                'mode': 'sleep',
                'active_profile_id': 'sleep',
              },
            ],
          },
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.activeMode, RhythmMode.sleep);
      expect(provider.lightRuntime, RhythmLightRuntime.rhythmAdaptive);
    });

    test('preserves active mode across transient reconnect after hello',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'mode': {'active': 'day'},
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.activeMode, RhythmMode.day);
      expect(provider.hasBeenSynced, isTrue);

      connection.emitConnectionState(RhythmConnectionState.reconnecting);
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.activeMode, RhythmMode.day);
      expect(provider.hasBeenSynced, isTrue);
    });

    test('direct reconnect clears server identity until replacement hello',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitConnectionState(RhythmConnectionState.connected);
      connection.emitHello(RhythmHello.fromJson({
        'server_instance_id': 'srv-box-a',
      }));
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.connectedServerInstanceId, 'srv-box-a');

      connection.emitConnectionState(RhythmConnectionState.connecting);
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.connectedServerInstanceId, isNull);

      connection.emitConnectionState(RhythmConnectionState.connected);
      connection.emitHello(RhythmHello.fromJson({
        'server_instance_id': 'srv-box-b',
      }));
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.connectedServerInstanceId, 'srv-box-b');
    });

    test('settings_changed without power_save preserves legacy cache',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'settings': {
            'power_save': false,
            'auto_update': true,
          },
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.powerSave, isFalse);
      expect(provider.autoUpdate, isTrue);

      connection.emitSettingsChanged(
        RhythmSettings.fromJson({'auto_update': false}),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.powerSave, isFalse);
      expect(provider.autoUpdate, isFalse);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'settings': {
            'auto_update': true,
          },
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.powerSave, isFalse);
      expect(provider.autoUpdate, isTrue);
    });

    test('applies light breaker state from hello and SSE', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'light_breaker': {'enabled': false},
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.lightBreakerEnabled, isFalse);

      connection.emitLightBreakerChanged(
        const RhythmLightBreaker(enabled: true),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.lightBreakerEnabled, isTrue);
    });

    test('sets light breaker optimistically and rolls back on failure',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'light_breaker': {'enabled': true},
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final success = await provider.setLightBreakerEnabled(false);

      expect(success, isTrue);
      expect(api.setLightBreakerCalls, 1);
      expect(api.lastLightBreakerEnabled, isFalse);
      expect(provider.lightBreakerEnabled, isFalse);

      api.setLightBreakerResult = false;
      final failed = await provider.setLightBreakerEnabled(true);

      expect(failed, isFalse);
      expect(provider.lightBreakerEnabled, isFalse);
    });

    test('applies motion timer SSE updates to room provider and hello rooms',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': [
            {
              'id': 'room-1',
              'name': 'Kitchen',
              'kind': 'room',
              'hub_types': ['hue'],
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'lights_on': true,
            },
          ],
          'location': const <String, dynamic>{},
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      connection.emitMotionTimer(
        const RhythmMotionTimer.node(
          nodeId: 'room-1',
          motionActive: false,
          motionOwned: true,
          remainingSecs: 18,
          timeoutSecs: 1200,
          warningActive: true,
        ),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final timer = roomProvider.getMotionTimer('room-1');
      expect(timer, isNotNull);
      expect(timer!.remainingSecs, 18);
      expect(timer.warningActive, isTrue);

      final room =
          provider.helloRooms.singleWhere((entry) => entry.id == 'room-1');
      expect(room.motionOwned, isTrue);
      expect(room.remainingSecs, 18);
      expect(room.timeoutSecs, 1200);
      expect(room.warningActive, isTrue);
    });

    test('updates only the addressed hub when multiple hubs share a type',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': const <Map<String, dynamic>>[],
          'location': const <String, dynamic>{},
          'hubs': [
            {
              'type': 'hue',
              'address': '192.168.1.10:443',
              'connected': true,
            },
            {
              'type': 'hue',
              'address': '192.168.1.11:443',
              'connected': true,
            },
          ],
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      connection.emitHubEvent(
        event: 'disconnected',
        hubType: 'hue',
        address: '192.168.1.11:443',
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final hubsByAddress = {
        for (final hub in provider.serverHubInfos)
          hub['address'] as String: hub['connected'] as bool? ?? false,
      };
      expect(hubsByAddress['192.168.1.10:443'], isTrue);
      expect(hubsByAddress['192.168.1.11:443'], isFalse);
    });
  });

  group('ServerSyncProvider curve modifiers', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _FakeRhythmConnection connection;
    late ServerSyncProvider provider;

    setUp(() async {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _FakeRhythmConnection(api);
      provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      await roomProvider.addRoom(const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.hue,
        kind: RoomNodeKind.room,
        deviceIds: ['light-1'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ));
    });

    tearDown(() {
      provider.dispose();
      roomProvider.dispose();
      connection.dispose();
    });

    test('dispatches brightness as a curve modifier', () {
      final dispatched = provider.dispatchNodeCurveBrightness('room-1', 61);

      expect(dispatched, isTrue);
      expect(api.nodeCurveBrightnessCalls, hasLength(1));
      final call = api.nodeCurveBrightnessCalls.single;
      expect(call.nodeId, 'room-1');
      expect(call.brightness, 61);
    });

    test('dispatches direct brightness without editing the curve modifier', () {
      final dispatched = provider.dispatchNodeBrightness('room-1', 37);

      expect(dispatched, isTrue);
      expect(api.nodeBrightnessCalls, [
        (nodeId: 'room-1', brightness: 37),
      ]);
      expect(api.nodeCurveBrightnessCalls, isEmpty);
    });

    test('dispatches color temperature as a curve modifier', () {
      final dispatched = provider.dispatchNodeCurveColorTemperature(
        'room-1',
        3200,
      );

      expect(dispatched, isTrue);
      expect(api.nodeCurveColorTemperatureCalls, hasLength(1));
      final call = api.nodeCurveColorTemperatureCalls.single;
      expect(call.nodeId, 'room-1');
      expect(call.kelvin, 3200);
      expect(call.preserveBrightness, isTrue);
    });
  });

  group('ServerSyncProvider mood scenes', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _HelloRhythmConnection connection;
    late ServerSyncProvider provider;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _HelloRhythmConnection(api);
      provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
    });

    tearDown(() {
      provider.dispose();
      roomProvider.dispose();
      connection.dispose();
    });

    test('hides generated and non-lit scenes from picker data', () async {
      api.scenes = [
        _testScene('evening-glow'),
        _testScene('node-mood-scene-room-1'),
        _testOffScene('all-off'),
        _testPresetOnlyScene('reset-room'),
      ];

      final fetched = await provider.fetchScenes();

      expect(fetched.map((scene) => scene.id), ['evening-glow']);
      expect(provider.scenes.map((scene) => scene.id), ['evening-glow']);
    });

    test('keeps native scene catalogs isolated by room', () async {
      api.scenes = [_testScene('rhythm-scene')];
      api.roomScenes['room-1'] = [
        _testScene('rhythm-scene'),
        _testHuePaletteScene('native-hue-room-1'),
      ];
      api.roomScenes['room-2'] = [
        _testScene('rhythm-scene'),
        _testHuePaletteScene('native-hue-room-2'),
      ];

      await provider.fetchScenes(roomId: 'room-1');
      await provider.fetchScenes(roomId: 'room-2');

      expect(api.sceneTargetCalls, ['room-1', 'room-2']);
      expect(
        provider.scenesForRoom('room-1').map((scene) => scene.id),
        ['rhythm-scene', 'native-hue-room-1'],
      );
      expect(
        provider.scenesForRoom('room-2').map((scene) => scene.id),
        ['rhythm-scene', 'native-hue-room-2'],
      );
      expect(
        provider.scenesForRoom('room-1').map((scene) => scene.id),
        isNot(contains('native-hue-room-2')),
      );
    });

    test('retains cached native scenes across a partial discovery failure',
        () async {
      api.roomScenes['room-1'] = [
        _testScene('stored-old'),
        _testHuePaletteScene('native-hue-aurora'),
      ];
      await provider.fetchScenes(roomId: 'room-1');

      api.roomScenes['room-1'] = [_testScene('stored-current')];
      api.partialNativeDiscoveryFailureTargets.add('room-1');
      final fetched = await provider.fetchScenes(roomId: 'room-1');

      expect(
        fetched.map((scene) => scene.id),
        ['stored-current', 'native-hue-aurora'],
      );
    });

    test('clears a room cache after a successful empty refresh', () async {
      api.roomScenes['room-1'] = [_testHuePaletteScene('native-hue-aurora')];
      await provider.fetchScenes(roomId: 'room-1');

      api.roomScenes['room-1'] = const [];
      final fetched = await provider.fetchScenes(roomId: 'room-1');

      expect(fetched, isEmpty);
      expect(provider.scenesForRoom('room-1'), isEmpty);
    });

    test('retains a room cache when the catalog request fails', () async {
      api.roomScenes['room-1'] = [_testHuePaletteScene('native-hue-aurora')];
      await provider.fetchScenes(roomId: 'room-1');

      api.failedSceneCatalogTargets.add('room-1');
      final fetched = await provider.fetchScenes(roomId: 'room-1');

      expect(fetched.map((scene) => scene.id), ['native-hue-aurora']);
    });

    test('keeps marked and ordinary Hue scenes available for tab grouping',
        () async {
      api.roomScenes['room-1'] = [
        _testScene('rhythm-scene'),
        _testHuePaletteScene('native-hue-aurora'),
        _testUnmarkedHueScene('legacy-hue-relax'),
      ];

      final fetched = await provider.fetchScenes(roomId: 'room-1');

      expect(
        fetched.map((scene) => scene.id),
        ['rhythm-scene', 'native-hue-aurora', 'legacy-hue-relax'],
      );
      expect(
        provider.scenesForRoom('room-1').map((scene) => scene.id),
        containsAll(['native-hue-aurora', 'legacy-hue-relax']),
      );
    });

    test('detects Hue bindings in mixed authoritative room topology', () async {
      connection.emitHello(
        RhythmHello.fromJson({
          'nodes': [
            {
              'id': 'mixed-room',
              'name': 'Studio',
              'kind': 'room',
              'hub_types': ['matter', 'hue'],
            },
            {
              'id': 'matter-room',
              'name': 'Office',
              'kind': 'room',
              'hub_types': ['matter'],
            },
          ],
        }),
      );

      await Future<void>.delayed(Duration.zero);

      expect(provider.roomHasHueBinding('mixed-room'), isTrue);
      expect(provider.roomHasHueBinding('matter-room'), isFalse);
    });

    test('applies a mood scene without issuing a duplicate preferences write',
        () async {
      api.scenes = [_testScene('evening-glow')];
      await provider.fetchScenes();

      final dispatched = await provider.applyMoodScene(
        'room-1',
        'evening-glow',
        color: (240, 80, 24),
        transitionMs: 450,
      );

      expect(dispatched, isTrue);
      expect(api.applySceneCalls, hasLength(1));
      expect(api.applySceneCalls.single.sceneId, 'evening-glow');
      expect(api.applySceneCalls.single.targetId, 'room-1');
      expect(api.applySceneCalls.single.transitionMs, 450);
      expect(
        api.applySceneCalls.single.correlationId,
        startsWith('mood-scene-'),
      );
      expect(api.nodePreferenceCalls, isEmpty);
      expect(provider.moodSceneIdForRoom('room-1'), 'evening-glow');
      expect(roomProvider.getMoodColor('room-1'), (240, 80, 24));
      expect(roomProvider.getMoodBrightness('room-1'), 55);
    });

    test('uses palette scenes for representative mood brightness', () async {
      api.scenes = [_testPaletteScene('color-carnival')];
      await provider.fetchScenes();

      final dispatched = await provider.applyMoodScene(
        'room-1',
        'color-carnival',
      );

      expect(dispatched, isTrue);
      expect(provider.moodSceneIdForRoom('room-1'), 'color-carnival');
      expect(roomProvider.getMoodBrightness('room-1'), 42);
    });

    test('rolls back optimistic scene selection when apply fails', () async {
      api.scenes = [_testScene('evening-glow')];
      api.applySceneSucceeds = false;
      await provider.fetchScenes();
      roomProvider.setRoomColorLocal(
        'room-1',
        12,
        34,
        56,
        rememberAsMood: true,
      );
      roomProvider.setMoodBrightnessLocal('room-1', 27);
      roomProvider.setMoodEnabledLocal('room-1', false);

      final applied = await provider.applyMoodScene(
        'room-1',
        'evening-glow',
        color: (240, 80, 24),
      );

      expect(applied, isFalse);
      expect(provider.moodSceneIdForRoom('room-1'), isNull);
      expect(roomProvider.getRoomColor('room-1'), (12, 34, 56));
      expect(roomProvider.getMoodColor('room-1'), (12, 34, 56));
      expect(roomProvider.getMoodBrightness('room-1'), 27);
      expect(roomProvider.isMoodEnabled('room-1'), isFalse);
    });

    test('stale scene failure does not roll back a newer custom mood',
        () async {
      api.scenes = [_testScene('evening-glow')];
      api.applySceneCompleter = Completer<RhythmSceneActionResult?>();
      await provider.fetchScenes();

      final pending = provider.applyMoodScene(
        'room-1',
        'evening-glow',
        color: (240, 80, 24),
      );
      expect(provider.moodSceneIdForRoom('room-1'), 'evening-glow');

      provider.dispatchNodeColor(
        'room-1',
        10,
        20,
        30,
        scope: 'mood',
        brightness: 31,
      );
      api.applySceneCompleter!.complete(null);

      expect(await pending, isFalse);
      expect(provider.moodSceneIdForRoom('room-1'), isNull);
      expect(roomProvider.getMoodColor('room-1'), (10, 20, 30));
      expect(roomProvider.getMoodBrightness('room-1'), 31);
    });

    test('does not change optimistic scene state while disconnected', () async {
      api.scenes = [_testScene('evening-glow')];
      connection.isConnected = false;

      expect(
        await provider.applyMoodScene('room-1', 'evening-glow'),
        isFalse,
      );
      expect(provider.moodSceneIdForRoom('room-1'), isNull);
      expect(api.applySceneCalls, isEmpty);
    });
  });

  group('ServerSyncProvider generated mood scene state', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _HelloRhythmConnection connection;
    late ServerSyncProvider provider;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _HelloRhythmConnection(api);
      provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
    });

    tearDown(() {
      provider.dispose();
      roomProvider.dispose();
      connection.dispose();
    });

    test('treats generated mood scene ids as custom color state', () async {
      connection.emitHello(RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'state': 'mood',
            'profile_settings': {
              'mood_scene_id': 'node-mood-scene-room-1',
            },
          },
        ],
      }));

      await Future<void>.delayed(Duration.zero);

      expect(provider.moodSceneIdForRoom('room-1'), isNull);
    });

    test('keeps public mood scene ids selectable', () async {
      connection.emitHello(RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'state': 'mood',
            'profile_settings': {
              'mood_scene_id': 'evening-glow',
            },
          },
        ],
      }));

      await Future<void>.delayed(Duration.zero);

      expect(provider.moodSceneIdForRoom('room-1'), 'evening-glow');
    });

    test('clears public scene selection after direct mood color', () async {
      connection.emitHello(RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'state': 'mood',
            'profile_settings': {
              'mood_scene_id': 'evening-glow',
            },
          },
        ],
      }));

      await Future<void>.delayed(Duration.zero);
      expect(provider.moodSceneIdForRoom('room-1'), 'evening-glow');

      final dispatched = provider.dispatchNodeColor(
        'room-1',
        20,
        80,
        240,
        scope: 'mood',
      );

      expect(dispatched, isTrue);
      expect(provider.moodSceneIdForRoom('room-1'), isNull);
    });

    test('shows selected scene before server state catches up', () async {
      api.scenes = [_testScene('evening-glow')];
      await provider.fetchScenes();
      connection.emitHello(RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'state': 'mood',
            'profile_settings': {
              'mood_scene_id': 'node-mood-scene-room-1',
            },
          },
        ],
      }));

      await Future<void>.delayed(Duration.zero);
      expect(provider.moodSceneIdForRoom('room-1'), isNull);

      final dispatched = await provider.applyMoodScene(
        'room-1',
        'evening-glow',
        color: (240, 80, 24),
      );

      expect(dispatched, isTrue);
      expect(provider.moodSceneIdForRoom('room-1'), 'evening-glow');
    });
  });

  group('ServerSyncProvider.dispatchNodeColor', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _FakeRhythmConnection connection;
    late ServerSyncProvider provider;

    setUp(() async {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _FakeRhythmConnection(api);
      provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      await roomProvider.addRoom(const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.hue,
        kind: RoomNodeKind.room,
        deviceIds: ['light-1'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ));
    });

    tearDown(() {
      provider.dispose();
      roomProvider.dispose();
      connection.dispose();
    });

    test('preserves current brightness for mood-scoped color writes', () async {
      await roomProvider.applyServerNodeState(
        'room-1',
        rhythmEnabled: true,
        timeOffset: 0,
        brightnessOffset: 0,
        state: RoomModeState.mood,
        lightsOn: true,
        brightness: 7,
      );

      final dispatched = provider.dispatchNodeColor(
        'room-1',
        20,
        80,
        240,
        scope: 'mood',
      );

      expect(dispatched, isTrue);
      expect(api.nodeColorCalls, hasLength(1));
      final call = api.nodeColorCalls.single;
      expect(call.scope, 'mood');
      expect(call.brightness, 7);
      expect(call.r, 20);
      expect(call.g, 80);
      expect(call.b, 240);
      expect(roomProvider.getMoodColor('room-1'), (20, 80, 240));
    });

    test('uses mood default brightness when no server brightness is known', () {
      final dispatched = provider.dispatchNodeColor(
        'room-1',
        255,
        149,
        0,
        scope: 'mood',
      );

      expect(dispatched, isTrue);
      expect(api.nodeColorCalls.single.brightness, 1);
    });

    test('does not reuse active brightness for mood-scoped color writes',
        () async {
      await roomProvider.applyServerNodeState(
        'room-1',
        rhythmEnabled: true,
        timeOffset: 0,
        brightnessOffset: 0,
        state: RoomModeState.active,
        lightsOn: true,
        brightness: 44,
      );

      final dispatched = provider.dispatchNodeColor(
        'room-1',
        255,
        149,
        0,
        scope: 'mood',
      );

      expect(dispatched, isTrue);
      expect(api.nodeColorCalls.single.brightness, 1);
    });

    test('does not inject brightness for non-mood color writes', () {
      final dispatched = provider.dispatchNodeColor(
        'room-1',
        255,
        149,
        0,
      );

      expect(dispatched, isTrue);
      expect(api.nodeColorCalls.single.brightness, isNull);
    });
  });

  group('ServerSyncProvider fullRefresh', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _FakeRhythmConnection connection;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _FakeRhythmConnection(api);
    });

    tearDown(() {
      roomProvider.dispose();
      connection.dispose();
    });

    test('requests authoritative state on reconnect', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      await provider.fullRefresh();

      expect(connection.reconnectCalls, 1);
      expect(connection.lastReconnectAuthoritative, isTrue);
    });

    test('fullRefresh gates room display until the next hello arrives',
        () async {
      final helloConnection = _HelloRhythmConnection(api);
      addTearDown(helloConnection.dispose);
      final provider = ServerSyncProvider(
        connection: helloConnection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([
          Hub.server(
            id: 'server-1',
            homeId: 'home-1',
            name: 'Kitchen Server',
            host: '127.0.0.1',
            port: 54448,
            token: 'owner-token',
          ),
        ]),
      );
      addTearDown(provider.dispose);

      await provider.retryActiveServerConnection(
        assumeLanReachable: true,
        assumeSavedAuth: true,
      );
      helloConnection.emitHello(RhythmHello.fromJson({}));
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.roomsReadyForDisplay, isTrue);

      await provider.fullRefresh();

      expect(provider.isRoomReadinessRefreshPending, isTrue);
      expect(provider.roomsReadyForDisplay, isFalse);
      expect(helloConnection.reconnectCalls, 1);
      expect(helloConnection.lastReconnectAuthoritative, isTrue);

      helloConnection.emitHello(RhythmHello.fromJson({}));
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.isRoomReadinessRefreshPending, isFalse);
      expect(provider.roomsReadyForDisplay, isTrue);
    });

    test('scheduled hub startup keeps rooms gated until a connected hello',
        () async {
      final helloConnection = _HelloRhythmConnection(api);
      addTearDown(helloConnection.dispose);
      final provider = ServerSyncProvider(
        connection: helloConnection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([
          Hub.server(
            id: 'server-1',
            homeId: 'home-1',
            name: 'Kitchen Server',
            host: '127.0.0.1',
            port: 54448,
            token: 'owner-token',
          ),
        ]),
      );
      addTearDown(provider.dispose);

      helloConnection.emitHello(RhythmHello.fromJson({
        'hubs': [
          {
            'type': 'matter',
            'address': 'matter',
            'connected': false,
            'startup_retry': {'status': 'scheduled'},
          },
        ],
      }));
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.hasBeenSynced, isTrue);
      expect(provider.hasPendingAutomaticHubStartup, isTrue);
      expect(provider.roomsReadyForDisplay, isFalse);

      helloConnection.emitHello(RhythmHello.fromJson({
        'hubs': [
          {
            'type': 'matter',
            'address': 'matter',
            'connected': true,
          },
        ],
      }));
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.hasPendingAutomaticHubStartup, isFalse);
      expect(provider.roomsReadyForDisplay, isTrue);
    });

    test('scheduled hub startup does not gate usable hello rooms', () async {
      final helloConnection = _HelloRhythmConnection(api);
      addTearDown(helloConnection.dispose);
      final provider = ServerSyncProvider(
        connection: helloConnection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      helloConnection.emitHello(RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
          },
        ],
        'hubs': [
          {
            'type': 'matter',
            'address': 'matter',
            'connected': false,
            'startup_retry': {'status': 'scheduled'},
          },
        ],
      }));
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.hasPendingAutomaticHubStartup, isFalse);
      expect(provider.roomsReadyForDisplay, isTrue);
    });

    test('scheduled hub startup cannot gate rooms past the bounded grace', () {
      fakeAsync((async) {
        final localRoomProvider = RoomProvider();
        final localConnection = _HelloRhythmConnection(_FakeRhythmServerApi());
        final provider = ServerSyncProvider(
          connection: localConnection,
          roomProvider: localRoomProvider,
          homeProvider: _TestHomeProvider(const []),
        );

        final scheduledHello = RhythmHello.fromJson({
          'hubs': [
            {
              'type': 'hue_ble',
              'address': 'local',
              'connected': false,
              'startup_retry': {
                'status': 'scheduled',
                'attempt_count': 1,
              },
            },
          ],
        });
        localConnection.emitHello(scheduledHello);
        async.flushMicrotasks();

        expect(provider.hasPendingAutomaticHubStartup, isTrue);
        expect(provider.roomsReadyForDisplay, isFalse);

        async.elapse(const Duration(seconds: 8));

        expect(provider.hasPendingAutomaticHubStartup, isFalse);
        expect(provider.roomsReadyForDisplay, isTrue);

        // Repeated authoritative state for the same failed hub must not restart
        // the grace window and put the app back on the Setting up screen.
        localConnection.emitHello(scheduledHello);
        async.flushMicrotasks();
        expect(provider.roomsReadyForDisplay, isTrue);

        provider.dispose();
        localConnection.dispose();
        localRoomProvider.dispose();
      });
    });

    test('explicit input-only local BLE startup never gates rooms', () async {
      final helloConnection = _HelloRhythmConnection(api);
      addTearDown(helloConnection.dispose);
      final provider = ServerSyncProvider(
        connection: helloConnection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      helloConnection.emitHello(RhythmHello.fromJson({
        'hubs': [
          {
            'type': 'local_ble',
            'address': 'default',
            'connected': false,
            'startup_retry': {'status': 'scheduled'},
          },
        ],
        'capabilities': {
          'hubs': [
            {
              'type': 'local_ble',
              'configurable': false,
              'blocks_room_readiness': false,
              'device_onboarding_methods': ['local_ble_qr'],
              'device_profiles': [
                {
                  'id': RhythmDeviceProfileId.oreinOc02001Button,
                  'device_type': 'button',
                  'display_name': 'Button',
                  'input_only': true,
                  'onboarding_methods': ['local_ble_qr'],
                },
              ],
            },
          ],
        },
      }));
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.hasPendingAutomaticHubStartup, isFalse);
      expect(provider.roomsReadyForDisplay, isTrue);
    });

    test('local BLE lighting profiles can retain the room readiness gate',
        () async {
      final helloConnection = _HelloRhythmConnection(api);
      addTearDown(helloConnection.dispose);
      final provider = ServerSyncProvider(
        connection: helloConnection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      helloConnection.emitHello(RhythmHello.fromJson({
        'hubs': [
          {
            'type': 'local_ble',
            'address': 'default',
            'connected': false,
            'startup_retry': {'status': 'scheduled'},
          },
        ],
        'capabilities': {
          'hubs': [
            {
              'type': 'local_ble',
              'configurable': false,
              'blocks_room_readiness': true,
              'device_onboarding_methods': ['local_ble_nearby_scan'],
              'device_profiles': [
                {
                  'id': 'future.vendor.bulb.v1',
                  'device_type': 'light',
                  'display_name': 'BLE Bulb',
                  'input_only': false,
                  'onboarding_methods': ['local_ble_nearby_scan'],
                },
              ],
            },
          ],
        },
      }));
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.hasPendingAutomaticHubStartup, isTrue);
      expect(provider.roomsReadyForDisplay, isFalse);
      expect(provider.canAddLocalBleDevice, isFalse);
      expect(provider.canScanToAddDevice, isFalse);
    });

    test('manual hub retry state allows rooms so recovery banner can render',
        () async {
      final helloConnection = _HelloRhythmConnection(api);
      addTearDown(helloConnection.dispose);
      final provider = ServerSyncProvider(
        connection: helloConnection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      helloConnection.emitHello(RhythmHello.fromJson({
        'hubs': [
          {
            'type': 'matter',
            'address': 'matter',
            'connected': false,
            'startup_retry': {'status': 'manual_retry_required'},
          },
        ],
      }));
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.hasPendingAutomaticHubStartup, isFalse);
      expect(provider.roomsReadyForDisplay, isTrue);
    });

    test('unclassified disconnected hub gates briefly then allows recovery',
        () {
      fakeAsync((async) {
        final localRoomProvider = RoomProvider();
        final localConnection = _HelloRhythmConnection(_FakeRhythmServerApi());
        final provider = ServerSyncProvider(
          connection: localConnection,
          roomProvider: localRoomProvider,
          homeProvider: _TestHomeProvider(const []),
        );

        localConnection.emitHello(RhythmHello.fromJson({
          'hubs': [
            {
              'type': 'matter',
              'address': 'matter',
              'connected': false,
            },
          ],
        }));
        async.flushMicrotasks();

        expect(provider.hasPendingAutomaticHubStartup, isTrue);
        expect(provider.roomsReadyForDisplay, isFalse);

        async.elapse(const Duration(seconds: 8));

        expect(provider.hasPendingAutomaticHubStartup, isFalse);
        expect(provider.roomsReadyForDisplay, isTrue);

        provider.dispose();
        localConnection.dispose();
        localRoomProvider.dispose();
      });
    });

    test('first owner claim queues tunnel before server connection completes',
        () async {
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'user-1',
      );
      final homeProvider = _TestHomeProvider(
        [
          Hub.server(
            id: 'server-1',
            homeId: home.id,
            name: 'Kitchen Server',
            host: '192.168.5.123',
            port: 54448,
          ),
        ],
        currentHome: home,
      );
      final authApi = _FakeRhythmAuthApi();
      final scheduledHubs = <Hub>[];
      connection.connectBlocker = Completer<void>();
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
        authApiFactory: ({required baseUrl}) {
          expect(baseUrl, 'http://192.168.5.123:54448');
          return authApi;
        },
        remoteAccessAutoEnableScheduler: ({
          required Home home,
          required Hub serverHub,
          required RemoteAccessHubSaver saveHub,
          RemoteAccessLatestHubResolver? resolveLatestHub,
          RemoteAccessEnabledCallback? onEnabled,
        }) {
          scheduledHubs.add(serverHub);
        },
      );
      addTearDown(provider.dispose);

      final retry = provider.retryActiveServerConnection();
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(authApi.statusCalls, 1);
      expect(authApi.claimCalls, 1);
      expect(homeProvider.currentHomeHubs.single.token, 'claimed-owner-token');
      expect(connection.connectCalls, hasLength(1));
      expect(connection.connectCalls.single.authToken, 'claimed-owner-token');
      expect(
        scheduledHubs.single.token,
        'claimed-owner-token',
        reason: 'tunnel provisioning must not wait for connect() to return',
      );

      connection.connectBlocker!.complete();
      await retry;
    });

    test('home entry refresh gates until authoritative hello arrives',
        () async {
      final helloConnection = _HelloRhythmConnection(api);
      addTearDown(helloConnection.dispose);
      final provider = ServerSyncProvider(
        connection: helloConnection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([
          Hub.server(
            id: 'server-1',
            homeId: 'home-1',
            name: 'Kitchen Server',
            host: '127.0.0.1',
            port: 54448,
            token: 'owner-token',
          ),
        ]),
      );
      addTearDown(provider.dispose);

      final refresh = provider.refreshForHomeEntry(
        homeName: 'Kitchen',
        timeout: const Duration(seconds: 1),
        allowWifiFastPath: false,
      );

      expect(provider.hasHomeEntryRefreshGate, isTrue);
      expect(provider.isHomeEntryRefreshPending, isTrue);

      await Future<void>.delayed(const Duration(milliseconds: 100));

      expect(helloConnection.reconnectCalls, 1);
      expect(helloConnection.lastReconnectAuthoritative, isTrue);
      expect(provider.hasHomeEntryRefreshGate, isTrue);

      helloConnection.emitHello(RhythmHello.fromJson({}));

      expect(await refresh, isTrue);
      expect(provider.hasHomeEntryRefreshGate, isFalse);
      expect(provider.homeEntryRefreshError, isNull);
    });

    test('wifi fast path refreshes an already synced Home without gate',
        () async {
      final helloConnection = _HelloRhythmConnection(api);
      addTearDown(helloConnection.dispose);
      final hub = Hub.server(
        id: 'server-1',
        homeId: 'home-1',
        name: 'Kitchen Server',
        host: '127.0.0.1',
        port: 54448,
        token: 'owner-token',
        remoteEndpoint: const HubEndpoint(
          host: 'server.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
      );
      final provider = ServerSyncProvider(
        connection: helloConnection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([hub]),
        connectivityCheck: () async => const [ConnectivityResult.wifi],
      );
      addTearDown(provider.dispose);

      await provider.retryActiveServerConnection(
        assumeLanReachable: true,
        assumeSavedAuth: true,
      );
      helloConnection.emitHello(RhythmHello.fromJson({}));
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final priorConnectCalls = helloConnection.connectCalls.length;
      final refresh = provider.refreshForHomeEntry(
        homeName: 'Kitchen',
        wifiFastPathTimeout: const Duration(milliseconds: 50),
      );
      await Future<void>.delayed(const Duration(milliseconds: 10));

      expect(provider.hasHomeEntryRefreshGate, isFalse);
      expect(helloConnection.connectCalls.length, priorConnectCalls + 1);
      expect(helloConnection.connectCalls.last.host, '127.0.0.1');

      helloConnection.emitHello(RhythmHello.fromJson({}));

      expect(await refresh, isTrue);
      expect(provider.hasHomeEntryRefreshGate, isFalse);
      expect(provider.homeEntryRefreshError, isNull);
    });

    test('wifi fast path miss falls back to visible Home entry gate', () async {
      final helloConnection = _HelloRhythmConnection(api);
      addTearDown(helloConnection.dispose);
      final hub = Hub.server(
        id: 'server-1',
        homeId: 'home-1',
        name: 'Kitchen Server',
        host: '127.0.0.1',
        port: 54448,
        token: 'owner-token',
        remoteEndpoint: const HubEndpoint(
          host: 'server.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
      );
      final provider = ServerSyncProvider(
        connection: helloConnection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([hub]),
        connectivityCheck: () async => const [ConnectivityResult.wifi],
      );
      addTearDown(provider.dispose);

      await provider.retryActiveServerConnection(
        assumeLanReachable: true,
        assumeSavedAuth: true,
      );
      helloConnection.emitHello(RhythmHello.fromJson({}));
      await Future<void>.delayed(const Duration(milliseconds: 10));

      final refresh = provider.refreshForHomeEntry(
        homeName: 'Kitchen',
        timeout: const Duration(seconds: 1),
        wifiFastPathTimeout: const Duration(milliseconds: 20),
      );
      await Future<void>.delayed(const Duration(milliseconds: 80));

      expect(provider.hasHomeEntryRefreshGate, isTrue);
      expect(provider.isHomeEntryRefreshPending, isTrue);

      helloConnection.emitHello(RhythmHello.fromJson({}));

      expect(await refresh, isTrue);
      expect(provider.hasHomeEntryRefreshGate, isFalse);
    });

    test('fails over from LAN to remote endpoint when LAN reconnects',
        () async {
      final api = _FakeRhythmServerApi();
      final connection = _HelloRhythmConnection(api);
      addTearDown(connection.dispose);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([
          Hub.server(
            id: 'server-1',
            homeId: 'home-1',
            name: 'Kitchen Server',
            host: '127.0.0.1',
            port: 54448,
            token: 'owner-token',
            remoteEndpoint: const HubEndpoint(
              host: 'server.rhythm.lighting',
              port: 443,
              useSsl: true,
            ),
          ),
        ]),
        endpointReachability: (endpoint, authToken) async {
          expect(authToken, 'owner-token');
          return endpoint.host == '127.0.0.1';
        },
      );
      addTearDown(provider.dispose);

      provider.connectIfAvailable();
      await Future<void>.delayed(const Duration(milliseconds: 100));

      expect(connection.connectCalls, hasLength(1));
      expect(connection.connectCalls.single.host, '127.0.0.1');
      expect(connection.connectCalls.single.authToken, 'owner-token');

      connection.emitConnectionState(RhythmConnectionState.reconnecting);
      await Future<void>.delayed(const Duration(milliseconds: 20));

      expect(connection.connectCalls, hasLength(2));
      expect(connection.connectCalls.last.host, 'server.rhythm.lighting');
      expect(connection.connectCalls.last.port, 443);
      expect(connection.connectCalls.last.useSsl, isTrue);
      expect(connection.connectCalls.last.authToken, 'owner-token');
    });

    test('retry reselects remote endpoint when LAN is unreachable', () async {
      final api = _FakeRhythmServerApi();
      final connection = _HelloRhythmConnection(api);
      addTearDown(connection.dispose);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([
          Hub.server(
            id: 'server-1',
            homeId: 'home-1',
            name: 'Kitchen Server',
            host: '127.0.0.1',
            port: 54448,
            token: 'owner-token',
            remoteEndpoint: const HubEndpoint(
              host: 'server.rhythm.lighting',
              port: 443,
              useSsl: true,
            ),
          ),
        ]),
        endpointReachability: (endpoint, authToken) async {
          expect(authToken, 'owner-token');
          return endpoint.host != '127.0.0.1';
        },
      );
      addTearDown(provider.dispose);

      await provider.retryActiveServerConnection();

      expect(connection.connectCalls, hasLength(1));
      expect(connection.connectCalls.single.host, 'server.rhythm.lighting');
      expect(connection.connectCalls.single.port, 443);
      expect(connection.connectCalls.single.useSsl, isTrue);
      expect(connection.connectCalls.single.authToken, 'owner-token');
      expect(provider.activeConnectionEndpoint?.host, 'server.rhythm.lighting');
    });

    test('cellular startup skips the LAN probe when remote access is saved',
        () async {
      final api = _FakeRhythmServerApi();
      final connection = _HelloRhythmConnection(api);
      addTearDown(connection.dispose);
      var lanProbeCalls = 0;
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([
          Hub.server(
            id: 'server-1',
            homeId: 'home-1',
            name: 'Kitchen Server',
            host: '192.168.1.20',
            port: 54448,
            token: 'owner-token',
            remoteEndpoint: const HubEndpoint(
              host: 'server.rhythm.lighting',
              port: 443,
              useSsl: true,
            ),
          ),
        ]),
        connectivityCheck: () async => const [ConnectivityResult.mobile],
        endpointReachability: (endpoint, authToken) async {
          lanProbeCalls += 1;
          return false;
        },
      );
      addTearDown(provider.dispose);

      await provider.retryActiveServerConnection();

      expect(lanProbeCalls, 0);
      expect(connection.connectCalls, hasLength(1));
      expect(connection.connectCalls.single.host, 'server.rhythm.lighting');
      expect(connection.connectCalls.single.authToken, 'owner-token');
    });

    test('retry does not use a remote endpoint without an owner token',
        () async {
      final api = _FakeRhythmServerApi();
      final connection = _HelloRhythmConnection(api);
      addTearDown(connection.dispose);
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([
          Hub.server(
            id: 'server-1',
            homeId: 'home-1',
            name: 'Kitchen Server',
            host: '127.0.0.1',
            port: 54448,
            token: null,
            remoteEndpoint: const HubEndpoint(
              host: 'server.rhythm.lighting',
              port: 443,
              useSsl: true,
            ),
          ),
        ]),
        endpointReachability: (endpoint, authToken) async {
          expect(authToken, isNull);
          return false;
        },
      );
      addTearDown(provider.dispose);

      await provider.retryActiveServerConnection(assumeSavedAuth: true);

      expect(connection.connectCalls, isEmpty);
      expect(provider.activeConnectionEndpoint, isNull);
    });

    test('retry uses the saved tunnel endpoint before cloud refresh', () async {
      final api = _FakeRhythmServerApi();
      final connection = _HelloRhythmConnection(api);
      addTearDown(connection.dispose);
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'user-1',
      );
      final localHub = Hub.server(
        id: 'server-1',
        homeId: home.id,
        name: 'Kitchen Server',
        host: '127.0.0.1',
        port: 54448,
        token: 'owner-token',
        remoteEndpoint: const HubEndpoint(
          host: 'old-server.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
      );
      final homeProvider = _TestHomeProvider(
        [localHub],
        currentHome: home,
        accountHomes: [
          AccountHomeServerHubs(
            home: home,
            serverHubs: [
              Hub.server(
                id: localHub.id,
                homeId: home.id,
                name: localHub.name,
                host: localHub.endpoint.host,
                port: localHub.endpoint.port,
                remoteEndpoint: const HubEndpoint(
                  host: 'new-server.rhythm.lighting',
                  port: 443,
                  useSsl: true,
                ),
              ),
            ],
          ),
        ],
      );
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
        endpointReachability: (endpoint, authToken) async {
          expect(authToken, 'owner-token');
          return endpoint.host != '127.0.0.1';
        },
      );
      addTearDown(provider.dispose);

      await provider.retryActiveServerConnection();

      expect(connection.connectCalls, hasLength(1));
      expect(connection.connectCalls.single.host, 'old-server.rhythm.lighting');
      expect(connection.connectCalls.single.port, 443);
      expect(connection.connectCalls.single.useSsl, isTrue);
      expect(connection.connectCalls.single.authToken, 'owner-token');
      expect(homeProvider.currentHomeHubs.single.remoteEndpoint?.host,
          'old-server.rhythm.lighting');
      expect(provider.activeConnectionEndpoint?.host,
          'old-server.rhythm.lighting');

      connection.emitConnectionState(RhythmConnectionState.reconnecting);
      await Future<void>.delayed(const Duration(milliseconds: 20));

      expect(connection.connectCalls, hasLength(2));
      expect(connection.connectCalls.last.host, 'new-server.rhythm.lighting');
      expect(homeProvider.currentHomeHubs.single.remoteEndpoint?.host,
          'new-server.rhythm.lighting');
    });

    test('retry does not await the selected Home cloud snapshot', () async {
      final api = _FakeRhythmServerApi();
      final connection = _HelloRhythmConnection(api);
      addTearDown(connection.dispose);
      final otherHome = Home.create(
        id: 'home-a',
        name: 'Kitchen',
        ownerId: 'user-1',
      );
      final selectedHome = Home.create(
        id: 'home-b',
        name: 'Cabin',
        ownerId: 'user-1',
      );
      final localHub = Hub.server(
        id: 'server-1',
        homeId: selectedHome.id,
        name: 'Cabin Server',
        host: '127.0.0.1',
        port: 54448,
        token: 'owner-token',
        remoteEndpoint: const HubEndpoint(
          host: 'old-cabin.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
      );
      final homeProvider = _TestHomeProvider(
        [localHub],
        currentHome: selectedHome,
        accountHomes: [
          AccountHomeServerHubs(
            home: otherHome,
            serverHubs: [
              Hub.server(
                id: localHub.id,
                homeId: otherHome.id,
                name: 'Kitchen Server',
                host: '192.168.5.10',
                remoteEndpoint: const HubEndpoint(
                  host: 'stale-kitchen.rhythm.lighting',
                  port: 443,
                  useSsl: true,
                ),
              ),
            ],
          ),
          AccountHomeServerHubs(
            home: selectedHome,
            serverHubs: [
              Hub.server(
                id: localHub.id,
                homeId: selectedHome.id,
                name: localHub.name,
                host: localHub.endpoint.host,
                port: localHub.endpoint.port,
                remoteEndpoint: const HubEndpoint(
                  host: 'fresh-cabin.rhythm.lighting',
                  port: 443,
                  useSsl: true,
                ),
              ),
            ],
          ),
        ],
      );
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
        endpointReachability: (endpoint, authToken) async {
          expect(authToken, 'owner-token');
          return endpoint.host != '127.0.0.1';
        },
      );
      addTearDown(provider.dispose);

      await provider.retryActiveServerConnection();

      expect(connection.connectCalls, hasLength(1));
      expect(connection.connectCalls.single.host, 'old-cabin.rhythm.lighting');
      expect(homeProvider.currentHomeHubs.single.remoteEndpoint?.host,
          'old-cabin.rhythm.lighting');
      expect(
          provider.activeConnectionEndpoint?.host, 'old-cabin.rhythm.lighting');
    });

    test('retry recovers a failed saved local endpoint from cloud refresh',
        () async {
      final api = _FakeRhythmServerApi();
      final connection = _HelloRhythmConnection(api);
      addTearDown(connection.dispose);
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'user-1',
      );
      final localHub = Hub.server(
        id: 'server-1',
        homeId: home.id,
        name: 'Kitchen Server',
        host: '100.64.0.12',
        port: 54448,
        token: 'owner-token',
      ).copyWith(
        updatedAt: DateTime.utc(2026, 6, 1),
        pendingSync: false,
      );
      final homeProvider = _TestHomeProvider(
        [localHub],
        currentHome: home,
        accountHomes: [
          AccountHomeServerHubs(
            home: home,
            serverHubs: [
              Hub.server(
                id: localHub.id,
                homeId: home.id,
                name: localHub.name,
                host: '192.168.5.123',
                port: localHub.endpoint.port,
              ).copyWith(updatedAt: DateTime.utc(2026, 6, 2)),
            ],
          ),
        ],
      );
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: homeProvider,
        endpointReachability: (endpoint, authToken) async {
          expect(authToken, 'owner-token');
          return endpoint.host == '100.64.0.12';
        },
      );
      addTearDown(provider.dispose);

      await provider.retryActiveServerConnection();

      expect(connection.connectCalls, hasLength(1));
      expect(connection.connectCalls.single.host, '100.64.0.12');
      expect(connection.connectCalls.single.port, 54448);
      expect(connection.connectCalls.single.authToken, 'owner-token');
      expect(
        homeProvider.currentHomeHubs.single.endpoint.host,
        '100.64.0.12',
      );
      expect(provider.activeConnectionEndpoint?.host, '100.64.0.12');

      connection.emitConnectionState(RhythmConnectionState.reconnecting);
      await Future<void>.delayed(const Duration(milliseconds: 20));

      expect(connection.connectCalls, hasLength(2));
      expect(connection.connectCalls.last.host, '192.168.5.123');
      expect(connection.connectCalls.last.port, 54448);
      expect(connection.connectCalls.last.authToken, 'owner-token');
      expect(
        homeProvider.currentHomeHubs.single.endpoint.host,
        '192.168.5.123',
      );
      expect(provider.activeConnectionEndpoint?.host, '192.168.5.123');
    });

    test('refreshes authoritative state when preview tick is missing',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      await roomProvider.addRoom(const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.hue,
        deviceIds: ['light-1'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ));

      await provider.ensureRoomPreviewStateFresh('room-1');

      expect(connection.reconnectCalls, 1);
      expect(connection.lastReconnectAuthoritative, isTrue);
    });

    test('skips preview refresh when tick is recent', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      await roomProvider.addRoom(const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.hue,
        deviceIds: ['light-1'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ));
      roomProvider.setLastTickTime('room-1', DateTime.now());

      await provider.ensureRoomPreviewStateFresh('room-1');

      expect(connection.reconnectCalls, 0);
    });
  });

  group('ServerSyncProvider transition saves', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _HelloRhythmConnection connection;

    setUp(() {
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _HelloRhythmConnection(api);
    });

    tearDown(() {
      roomProvider.dispose();
      connection.dispose();
    });

    List<RhythmModeTransitionConfig> seedTransitions() {
      return const [
        RhythmModeTransitionConfig(
          id: 'sleep_to_day',
          label: 'Sleep to Day',
          fromMode: RhythmMode.sleep,
          toMode: RhythmMode.day,
          trigger: RhythmTransitionTrigger.solar('sunrise'),
          duration: TransitionDuration.auto(),
          preserveHardOff: true,
        ),
        RhythmModeTransitionConfig(
          id: 'day_to_sleep',
          label: 'Day to Sleep',
          fromMode: RhythmMode.day,
          toMode: RhythmMode.sleep,
          trigger: RhythmTransitionTrigger.solar('sunset'),
          duration: TransitionDuration.auto(),
          preserveHardOff: true,
        ),
      ];
    }

    test('stores trigger-enabled edits in the server and local cache',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      final initial = seedTransitions();
      api.transitions = initial;
      connection.emitHello(RhythmHello.fromJson({
        'rooms': const <Map<String, dynamic>>[],
        'location': const <String, dynamic>{},
        'transitions':
            initial.map((transition) => transition.toJson()).toList(),
      }));
      await Future<void>.delayed(Duration.zero);

      final disabled = [
        for (final transition in provider.modeTransitions)
          transition.copyWith(triggerEnabled: false),
      ];

      final success = await provider.dispatchSetTransitions(disabled);

      expect(success, isTrue);
      expect(api.setTransitionsCalls, 1);
      expect(
        api.lastSetTransitions!
            .every((transition) => !transition.triggerEnabled),
        isTrue,
      );
      expect(
        provider.modeTransitions
            .every((transition) => !transition.triggerEnabled),
        isTrue,
      );
      expect(connection.reconnectCalls, 0);
    });

    test('rolls back optimistic transition cache on server failure', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider(const []),
      );
      addTearDown(provider.dispose);

      final initial = seedTransitions();
      api.transitions = initial;
      connection.emitHello(RhythmHello.fromJson({
        'rooms': const <Map<String, dynamic>>[],
        'location': const <String, dynamic>{},
        'transitions':
            initial.map((transition) => transition.toJson()).toList(),
      }));
      await Future<void>.delayed(Duration.zero);

      api.setTransitionsResult = false;

      final success = await provider.dispatchSetTransitions([
        for (final transition in provider.modeTransitions)
          transition.copyWith(triggerEnabled: false),
      ]);

      expect(success, isFalse);
      expect(
        provider.modeTransitions
            .every((transition) => transition.triggerEnabled),
        isTrue,
      );
    });
  });

  testWidgets('room settings live preview refreshes a missing tick on open',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _FakeRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);

    const room = RoomDto(
      id: 'room-1',
      name: 'Kitchen',
      source: RoomSourceDto.hue,
      deviceIds: ['light-1'],
      rhythmEnabled: true,
      disabled: false,
      lightsOn: true,
      timeOffsetMinutes: 0,
      brightnessOffset: 0,
    );
    await roomProvider.addRoom(room);

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const TickerMode(
          enabled: false,
          child: RoomSettingsSheet(room: room),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 10));

    expect(connection.reconnectCalls, 1);
    expect(connection.lastReconnectAuthoritative, isTrue);
  });

  testWidgets('room page groups controls and devices into dedicated tabs',
      (tester) async {
    _registerWidgetCleanup(tester);
    final semantics = tester.ensureSemantics();
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 1200));

    connection.emitHello(
      RhythmHello.fromJson({
        'capabilities': {
          'api_schema_version': 2,
          'features': [
            RhythmFeature.roomLightProfileOverrides,
            RhythmFeature.roomScheduleV1,
          ],
          'hubs': const <dynamic>[],
        },
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['matter'],
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'standby_enabled': true,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': true,
            'devices': [
              {'id': 'light-1', 'type': 'light', 'name': 'Ceiling Light'},
              {'id': 'button-1', 'type': 'button', 'name': 'Wall Button'},
              {'id': 'motion-1', 'type': 'motion', 'name': 'Entry Motion'},
              {'id': 'contact-1', 'type': 'contact', 'name': 'Patio Door'},
            ],
          },
        ],
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));
    roomProvider.markNodeHasSensor('room-1');

    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.matter,
        deviceIds: ['light-1', 'button-1', 'motion-1', 'contact-1'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );

    expect(find.text('Bulbs'), findsOneWidget);
    expect(find.text('Motion'), findsOneWidget);
    expect(find.text('Buttons'), findsOneWidget);
    expect(find.text('Lighting'), findsNWidgets(2));
    expect(find.text('Info'), findsNothing);
    final lightingTab = find.descendant(
      of: find.byKey(const ValueKey('room-settings-tabs')),
      matching: find.text('Lighting'),
    );
    final bulbsTab = find.descendant(
      of: find.byKey(const ValueKey('room-settings-tabs')),
      matching: find.text('Bulbs'),
    );
    final motionTab = find.descendant(
      of: find.byKey(const ValueKey('room-settings-tabs')),
      matching: find.text('Motion'),
    );
    final buttonsTab = find.descendant(
      of: find.byKey(const ValueKey('room-settings-tabs')),
      matching: find.text('Buttons'),
    );
    expect(tester.getTopLeft(lightingTab).dx,
        lessThan(tester.getTopLeft(bulbsTab).dx));
    expect(tester.getTopLeft(bulbsTab).dx,
        lessThan(tester.getTopLeft(motionTab).dx));
    expect(tester.getTopLeft(motionTab).dx,
        lessThan(tester.getTopLeft(buttonsTab).dx));
    expect(find.byKey(const ValueKey('lighting')), findsOneWidget);
    expect(
      find.byKey(const ValueKey('room-settings-rename')),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('room-settings-source')),
      findsOneWidget,
    );
    expect(find.text('Matter'), findsOneWidget);
    expect(find.text('Hide this room'), findsNothing);
    expect(find.text('Hide this light'), findsNothing);
    await _selectRoomSettingsTab(tester, 'Bulbs');
    await tester.pumpAndSettle();
    final bulbsContent = find.byKey(const ValueKey('bulbs'));
    expect(
      find.descendant(of: bulbsContent, matching: find.text('Low glow')),
      findsNothing,
    );
    expect(find.text('Add Bulb'), findsOneWidget);
    expect(find.text('BULBS'), findsOneWidget);
    expect(find.text('Ceiling Light'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('light-profile-override-badge-room-1')),
      findsNothing,
    );
    expect(
      find.byKey(const ValueKey('light-profile-override-badge-light-1')),
      findsNothing,
    );
    expect(
      find.descendant(
        of: bulbsContent,
        matching: find.byKey(const ValueKey('room-settings-add-bulb')),
      ),
      findsOneWidget,
    );
    final addBulbSemantics = tester
        .widget<Semantics>(
          find.byKey(const ValueKey('room-settings-add-bulb')),
        )
        .properties;
    expect(addBulbSemantics.label, 'Add Bulb');
    expect(addBulbSemantics.hint, 'Choose Scan or Select from existing');
    expect(find.text('Delete Room'), findsNothing);
    expect(find.text('Entry Motion'), findsNothing);
    expect(find.text('Wall Button'), findsNothing);
    expect(
      find.descendant(
        of: bulbsContent,
        matching: find.byKey(
          const ValueKey('room-settings-light-settings-room-1'),
        ),
      ),
      findsNothing,
    );
    final roomNameRect = tester.getRect(
      find.byKey(const ValueKey('room-settings-room-name')),
    );
    final renameRect = tester.getRect(
      find.byKey(const ValueKey('room-settings-rename')),
    );
    final sourceRect = tester.getRect(
      find.byKey(const ValueKey('room-settings-source')),
    );
    expect(renameRect.left, greaterThanOrEqualTo(roomNameRect.right));
    expect(renameRect.left - roomNameRect.right, lessThanOrEqualTo(8));
    expect(sourceRect.top, lessThan(roomNameRect.top));

    await _selectRoomSettingsTab(tester, 'Motion');

    final motionContent = find.byKey(const ValueKey('motion'));
    expect(
      find.descendant(
        of: motionContent,
        matching: find.byKey(const ValueKey('room-settings-add-motion')),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(
        of: motionContent,
        matching: find.byKey(const ValueKey('room-settings-add-button')),
      ),
      findsNothing,
    );
    expect(
      find.descendant(
        of: motionContent,
        matching: find.text('MOTION SENSORS'),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(of: motionContent, matching: find.text('Entry Motion')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: motionContent, matching: find.text('Motion Timeout')),
      findsNWidgets(2),
    );
    expect(
      find.descendant(
        of: motionContent,
        matching: find.text('CONTACT SENSORS'),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(of: motionContent, matching: find.text('Patio Door')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: motionContent, matching: find.text('Ceiling Light')),
      findsNothing,
    );
    expect(
      find.descendant(of: motionContent, matching: find.text('Wall Button')),
      findsNothing,
    );

    await _selectRoomSettingsTab(tester, 'Buttons');

    final buttonsContent = find.byKey(const ValueKey('buttons'));
    expect(
      find.descendant(
        of: buttonsContent,
        matching: find.byKey(const ValueKey('room-settings-add-button')),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(
        of: buttonsContent,
        matching: find.byKey(const ValueKey('room-settings-add-motion')),
      ),
      findsNothing,
    );
    expect(
      find.descendant(of: buttonsContent, matching: find.text('BUTTONS')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: buttonsContent, matching: find.text('Wall Button')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: buttonsContent, matching: find.text('Ceiling Light')),
      findsNothing,
    );
    expect(
      find.descendant(of: buttonsContent, matching: find.text('Entry Motion')),
      findsNothing,
    );

    await _selectRoomSettingsTab(tester, 'Lighting');
    final lightingContent = find.byKey(const ValueKey('lighting'));
    expect(lightingContent, findsOneWidget);
    expect(
      find.descendant(
        of: lightingContent,
        matching: find.byKey(
          const ValueKey('room-settings-light-settings-room-1'),
        ),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(
        of: lightingContent,
        matching: find.byKey(const ValueKey('room-settings-add-bulb')),
      ),
      findsNothing,
    );
    expect(
      find.byKey(const ValueKey('room-settings-low-glow-room-1')),
      findsOneWidget,
    );
    expect(
      tester
          .widget<Text>(
            find.byKey(
              const ValueKey('room-settings-light-status-room-1'),
            ),
          )
          .data,
      'Auto',
    );
    final lightingSemantics = tester.widget<Semantics>(
      find.byKey(
        const ValueKey('room-settings-light-settings-room-1'),
      ),
    );
    expect(lightingSemantics.properties.label, 'Lighting');
    expect(
      lightingSemantics.properties.value,
      'Using automatic settings',
    );

    expect(find.text('SCHEDULE'), findsOneWidget);
    expect(find.text('WAKE / SLEEP PRESETS'), findsOneWidget);
    expect(find.text('TEST YOUR PRESETS'), findsOneWidget);
    expect(
      tester
          .getSemantics(
            find.byKey(const ValueKey('segmented-tab-lighting')),
          )
          .getSemanticsData()
          .flagsCollection
          .isSelected,
      Tristate.isTrue,
    );
    expect(
      find.byKey(const ValueKey('room-schedule-source-presets')),
      findsOneWidget,
    );
    // Custom-times editor stays collapsed while the room follows the home
    // schedule — the dial and steppers only mount for Custom times.
    expect(
      find.byKey(const ValueKey('room-schedule-time-dial')),
      findsNothing,
    );
    expect(
      find.byKey(const ValueKey('room-schedule-wake-later')),
      findsNothing,
    );
    expect(
        find.byKey(const ValueKey('room-schedule-test-wake')), findsOneWidget);
    expect(
        find.byKey(const ValueKey('room-schedule-test-sleep')), findsOneWidget);
    expect(find.text('SCHEDULE BEHAVIOR'), findsNothing);

    connection.helloOnReconnect = RhythmHello.fromJson({
      'nodes': [
        {
          'id': 'room-1',
          'name': 'Dining Room',
          'kind': 'room',
          'hub_types': ['matter'],
          'state': 'active',
          'rhythm_enabled': true,
          'disabled': false,
          'time_offset': 0.0,
          'brightness_offset': 0.0,
          'lights_on': true,
        },
      ],
      'location': const <String, dynamic>{},
    });
    api.topologyNodes = [
      RhythmTopologyNode.fromJson({
        'id': 'room-1',
        'name': 'Dining Room',
        'kind': 'room',
      }),
    ];

    await tester.tap(find.byKey(const ValueKey('room-settings-rename')));
    await tester.pumpAndSettle();
    expect(find.text('Rename Room'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('room-settings-delete-room')),
      findsOneWidget,
    );
    final deleteActionRect = tester.getRect(
      find.byKey(const ValueKey('room-settings-delete-room')),
    );
    final cancelActionRect = tester.getRect(
      find.widgetWithText(TextButton, 'Cancel'),
    );
    expect(deleteActionRect.bottom, lessThan(cancelActionRect.top));

    await tester.enterText(find.byType(TextField), 'Dining Room');
    await tester.tap(find.widgetWithText(TextButton, 'Rename'));
    await tester.pumpAndSettle();

    expect(api.topologyRenameRoomCalls, 1);
    expect(api.lastRenamedRoomId, 'room-1');
    expect(api.lastRenamedRoomName, 'Dining Room');
    expect(api.triggerSyncCalls, 0);
    expect(connection.reconnectCalls, 1);
    expect(connection.lastReconnectAuthoritative, isTrue);
    expect(roomProvider.getRoom('room-1')?.name, 'Dining Room');
    expect(provider.helloRooms.single.name, 'Dining Room');
    expect(
      tester
          .widget<Text>(
            find.byKey(const ValueKey('room-settings-room-name')),
          )
          .data,
      'Dining Room',
    );
    semantics.dispose();
  });

  testWidgets('unsupported room schedule stays visible as an update state',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 1200));

    connection.emitHello(RhythmHello.fromJson({
      'nodes': [
        {
          'id': 'room-1',
          'name': 'Kitchen',
          'kind': 'room',
          'state': 'active',
          'rhythm_enabled': true,
          'disabled': false,
          'time_offset': 0.0,
          'brightness_offset': 0.0,
        },
      ],
    }));
    await tester.pump();
    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.matter,
        deviceIds: [],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );

    await _selectRoomSettingsTab(tester, 'Lighting');
    expect(
      find.byKey(const ValueKey('room-schedule-update-required')),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('room-schedule-source-follow-time')),
      findsNothing,
    );
    expect(api.roomScheduleSetCalls, isEmpty);
  });

  testWidgets('room schedule retries reuse one journey request id',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi()..roomScheduleSetSucceeds = false;
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 1200));

    connection.emitHello(
      RhythmHello.fromJson({
        'capabilities': {
          'api_schema_version': 2,
          'features': [RhythmFeature.roomScheduleV1],
          'hubs': const <dynamic>[],
        },
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'profile_settings': {
              'room_schedule': {
                'source': 'follow_time',
                'wake_time': '06:30',
                'sleep_time': '22:30',
              },
            },
          },
        ],
      }),
    );
    await tester.pump();
    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.matter,
        deviceIds: [],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );
    await _selectRoomSettingsTab(tester, 'Lighting');

    final presets = find.byKey(const ValueKey('room-schedule-source-presets'));
    await tester.tap(presets);
    await tester.pump();
    expect(find.byKey(const ValueKey('room-schedule-failure')), findsOneWidget);

    api.roomScheduleSetSucceeds = true;
    await tester.tap(presets);
    await tester.pump();
    expect(api.roomScheduleSetRequestIds, hasLength(2));
    expect(api.roomScheduleSetRequestIds[1], api.roomScheduleSetRequestIds[0]);
    expect(
      api.roomScheduleSetRequestIds.first,
      startsWith('room-schedule-save-'),
    );

    api.roomScheduleTestSucceeds = false;
    final wakeTest = find.byKey(const ValueKey('room-schedule-test-wake'));
    // The test toggle sits at the bottom of the (lazy) tab list — scroll it
    // into build range, then pin the list to its end so the toggle is fully
    // inside the viewport (not clipped at its bottom edge).
    await tester.dragUntilVisible(
      wakeTest,
      find.byKey(const ValueKey('lighting')),
      const Offset(0, -120),
    );
    await tester.drag(
      find.byKey(const ValueKey('lighting')),
      const Offset(0, -200),
    );
    await tester.pump();
    await tester.tap(wakeTest);
    await tester.pump();
    api.roomScheduleTestSucceeds = true;
    await tester.tap(wakeTest);
    await tester.pump();
    expect(api.roomScheduleTestRequestIds, hasLength(2));
    expect(
      api.roomScheduleTestRequestIds[1],
      api.roomScheduleTestRequestIds[0],
    );
    expect(
      api.roomScheduleTestRequestIds.first,
      startsWith('room-schedule-test-'),
    );
  });

  testWidgets(
      'room schedule presets stay live and apply the active mode to lights',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 1400));

    connection.emitHello(
      RhythmHello.fromJson({
        'capabilities': {
          'api_schema_version': 2,
          'features': [RhythmFeature.roomScheduleV1],
          'hubs': const <dynamic>[],
        },
        'mode': {
          'active': 'day',
          'configs': [
            {
              'mode': 'day',
              'active_profile_id': 'rhythm',
              'room_defaults': const <Map<String, dynamic>>[],
            },
            {
              'mode': 'sleep',
              'active_profile_id': 'sleep',
              'room_defaults': const <Map<String, dynamic>>[],
            },
          ],
        },
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': true,
            'profile_settings': {
              'room_schedule': {
                // Custom times must NOT freeze the presets — the schedule
                // source is only the WHEN; presets are the WHAT.
                'source': 'follow_time',
                'wake_time': '06:30',
                'sleep_time': '22:30',
              },
            },
          },
        ],
      }),
    );
    await tester.pump();
    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.matter,
        deviceIds: [],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );
    await _selectRoomSettingsTab(tester, 'Lighting');
    await tester.pumpAndSettle();

    // Tapping On in the Wake row (the active mode) records the room default
    // AND moves the room's lights immediately.
    final wakeRow =
        find.byKey(const ValueKey('room-schedule-presets-day-room-1'));
    await tester.dragUntilVisible(
      wakeRow,
      find.byKey(const ValueKey('lighting')),
      const Offset(0, -120),
    );
    await tester.tap(
      find.descendant(of: wakeRow, matching: find.text('On')),
    );
    await tester.pump();
    expect(
        provider.roomDefaultStateForMode('room-1', RhythmMode.day), 'active');
    expect(api.nodePreferenceCalls, hasLength(1));
    expect(api.nodePreferenceCalls.single.nodeId, 'room-1');
    expect(api.nodePreferenceCalls.single.state, RoomModeState.active);
    expect(api.nodePreferenceCalls.single.rhythmEnabled, isTrue);

    // Tapping Off in the Sleep row updates the default but leaves the lights
    // alone — Sleep is not the current mode.
    final sleepRow =
        find.byKey(const ValueKey('room-schedule-presets-night-room-1'));
    await tester.dragUntilVisible(
      sleepRow,
      find.byKey(const ValueKey('lighting')),
      const Offset(0, -120),
    );
    await tester.tap(
      find.descendant(of: sleepRow, matching: find.text('Off')),
    );
    await tester.pump();
    expect(
      provider.roomDefaultStateForMode('room-1', RhythmMode.sleep),
      'hard_off',
    );
    expect(api.nodePreferenceCalls, hasLength(1));

    // Let the debounced room-default persist fire before teardown.
    await tester.pump(const Duration(milliseconds: 801));
    await tester.pump();
  });

  testWidgets('room schedule renders deterministic visual evidence states',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 1200));
    await tester.runAsync(_loadRoomScheduleEvidenceFont);

    Map<String, dynamic> hello({
      bool followTime = false,
      String wakeTime = '06:30',
    }) =>
        {
          'capabilities': {
            'api_schema_version': 2,
            'features': [RhythmFeature.roomScheduleV1],
            'hubs': const <dynamic>[],
          },
          'nodes': [
            {
              'id': 'mock-room',
              'name': 'Sample Bulb',
              'kind': 'light_device',
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'standby_enabled': true,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'lights_on': true,
              'profile_settings': {
                'room_schedule': {
                  'source': followTime ? 'follow_time' : 'wake_sleep_presets',
                  'wake_time': wakeTime,
                  'sleep_time': '22:30',
                },
              },
            },
          ],
          'location': const <String, dynamic>{},
        };

    connection.emitHello(RhythmHello.fromJson(hello()));
    await tester.pump(const Duration(milliseconds: 10));
    const room = RoomDto(
      id: 'mock-room',
      name: 'Sample Bulb',
      source: RoomSourceDto.matter,
      deviceIds: [],
      rhythmEnabled: true,
      disabled: false,
      lightsOn: true,
      timeOffsetMinutes: 0,
      brightnessOffset: 0,
    );
    await roomProvider.addRoom(room);
    final boundaryKey = GlobalKey();
    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        fontFamily: 'CodexReadableRoboto',
        child: RepaintBoundary(
          key: boundaryKey,
          child: const RoomSettingsSheet(
            room: room,
            enableLivePreview: false,
          ),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 10));
    await _selectRoomSettingsTab(tester, 'Lighting');
    await tester.pump(const Duration(milliseconds: 250));
    await _captureRoomScheduleEvidence(
      tester,
      boundaryKey,
      '01-wake-sleep-presets.png',
    );

    connection.emitHello(
      RhythmHello.fromJson(hello(followTime: true, wakeTime: '07:45')),
    );
    await tester.pump(const Duration(milliseconds: 20));
    expect(find.text('07:45'), findsOneWidget);
    // Let the custom-times editor finish expanding before tapping into it.
    await tester.pumpAndSettle();
    await tester.tap(
      find.byKey(const ValueKey('room-schedule-wake-later')),
    );
    await tester.pumpAndSettle();
    expect(api.roomScheduleSetCalls.last.wakeTime, '08:00');
    expect(find.text('08:00'), findsOneWidget);
    // Presets stay live under Custom times — the schedule source only picks
    // WHEN triggers fire; presets are always the WHAT.
    expect(
      find.byKey(const ValueKey('room-schedule-presets-disabled')),
      findsNothing,
    );
    await _captureRoomScheduleEvidence(
      tester,
      boundaryKey,
      '02-follow-time.png',
    );

    await tester.drag(
      find.byKey(const ValueKey('lighting')),
      const Offset(0, -180),
    );
    await tester.pump();
    await _captureRoomScheduleEvidence(
      tester,
      boundaryKey,
      '03-disabled-inline-presets.png',
    );

    api.roomScheduleTestCompleter = Completer<bool>();
    await tester.dragUntilVisible(
      find.byKey(const ValueKey('room-schedule-test-wake')),
      find.byKey(const ValueKey('lighting')),
      const Offset(0, -120),
    );
    await tester.tap(find.byKey(const ValueKey('room-schedule-test-wake')));
    await tester.pump();
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('room-schedule-test-wake')),
        matching: find.byType(CircularProgressIndicator),
      ),
      findsOneWidget,
    );
    await _captureRoomScheduleEvidence(
      tester,
      boundaryKey,
      '04-test-pending.png',
    );

    api.roomScheduleTestCompleter!.complete(true);
    await tester.pump();
    api.roomScheduleTestCompleter = null;
    api.roomScheduleSetSucceeds = false;
    await tester.dragUntilVisible(
      find.byKey(const ValueKey('room-schedule-source-presets')),
      find.byKey(const ValueKey('lighting')),
      const Offset(0, 120),
    );
    await tester.tap(
      find.byKey(const ValueKey('room-schedule-source-presets')),
    );
    await tester.pump();
    await _captureRoomScheduleEvidence(
      tester,
      boundaryKey,
      '05-save-failure-retry.png',
    );
    expect(find.byKey(const ValueKey('room-schedule-failure')), findsOneWidget);
  });

  testWidgets('room tabs show Scan above the inline existing-device list',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 1200));

    connection.emitHello(
      RhythmHello.fromJson({
        'capabilities': {
          'hubs': [
            {
              'type': 'local_ble',
              'configurable': false,
              'device_onboarding_methods': ['local_ble_qr'],
              'device_profiles': [
                {
                  'id': RhythmDeviceProfileId.oreinOc02001Button,
                  'device_type': 'button',
                  'display_name': 'Button',
                  'input_only': true,
                  'onboarding_methods': ['local_ble_qr'],
                },
              ],
              'supports_roomless_devices': true,
            },
          ],
        },
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['local_ble'],
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': false,
            'devices': const <dynamic>[],
          },
        ],
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));
    expect(provider.canScanToAddDevice, isTrue);
    expect(
      provider.canScanToAddDeviceType(RhythmDeviceType.light),
      isFalse,
    );
    api.canonicalDevices['light-hall'] = {
      'id': 'light-hall',
      'name': 'Hall Lamp',
      'device_type': 'light',
      'room_id': 'room-2',
      'endpoints': const <Map<String, dynamic>>[],
    };
    api.canonicalDevices['light-unassigned'] = {
      'id': 'light-unassigned',
      'name': 'Zulu Lamp',
      'device_type': 'light',
      'endpoints': const <Map<String, dynamic>>[],
    };
    api.topologyNodes = [
      RhythmTopologyNode.fromJson({
        'id': 'room-1',
        'name': 'Kitchen',
        'kind': 'room',
      }),
      RhythmTopologyNode.fromJson({
        'id': 'room-2',
        'name': 'Hall',
        'kind': 'room',
      }),
      RhythmTopologyNode.fromJson({
        'id': 'light-hall',
        'name': 'Hall Lamp',
        'kind': 'light',
        'parent_id': 'room-2',
      }),
    ];

    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.bridge,
        deviceIds: [],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );

    await _selectRoomSettingsTab(tester, 'Bulbs');
    final addBulb = find.byKey(const ValueKey('room-settings-add-bulb'));
    expect(addBulb, findsOneWidget);
    expect(find.text('Add Bulb'), findsOneWidget);

    await tester.tap(addBulb);
    await tester.pumpAndSettle();

    expect(
      find.byKey(const ValueKey('room-device-add-sheet')),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('existing-room-device-light-hall')),
      findsOneWidget,
    );
    expect(
      tester
          .getTopLeft(
            find.byKey(const ValueKey('existing-room-device-light-unassigned')),
          )
          .dy,
      lessThan(
        tester
            .getTopLeft(
              find.byKey(const ValueKey('existing-room-device-light-hall')),
            )
            .dy,
      ),
    );
    expect(find.text('Unassigned'), findsOneWidget);
    expect(find.text('Wall Button'), findsNothing);
    if (const bool.fromEnvironment(
      'RHYTHM_CAPTURE_ROOM_DEVICE_ADD_EVIDENCE',
    )) {
      await expectLater(
        find.byKey(const ValueKey('room-device-add-sheet')),
        matchesGoldenFile(
          'goldens/room-device-add-unassigned-first.png',
        ),
      );
    }

    await tester.tap(find.byKey(const ValueKey('room-device-add-scan')));
    await tester.pumpAndSettle();
    expect(
      find.text('Adding bulbs is not available on this Rhythm Box yet.'),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('device-pairing-code-input')),
      findsNothing,
    );
    await tester.pump(const Duration(seconds: 5));
    await tester.pumpAndSettle();

    await tester.tap(addBulb);
    await tester.pumpAndSettle();
    await tester.tap(
      find.byKey(const ValueKey('existing-room-device-light-hall')),
    );
    await tester.pumpAndSettle();
    expect(api.assignDeviceParentCalls, 1);
    expect(api.lastAssignedDeviceId, 'light-hall');
    expect(api.lastAssignedParentId, 'room-1');
    expect(find.text('Moved Hall Lamp to Kitchen'), findsOneWidget);
    await tester.pump(const Duration(seconds: 5));
    await tester.pumpAndSettle();
    api.getCanonicalDevicesCalls = 0;

    bool pairingIntakeIsVisible() =>
        find.byTooltip('Back to Add & Review').evaluate().isNotEmpty ||
        find
            .byKey(const ValueKey('device-pairing-code-input'))
            .evaluate()
            .isNotEmpty;

    await _selectRoomSettingsTab(tester, 'Motion');
    final addMotion = find.byKey(const ValueKey('room-settings-add-motion'));
    expect(addMotion, findsOneWidget);
    expect(find.text('Add Motion Sensor'), findsOneWidget);

    await tester.tap(addMotion);
    await tester.pumpAndSettle();

    expect(
      find.byKey(const ValueKey('room-device-add-sheet')),
      findsOneWidget,
    );
    expect(find.text('Scan'), findsOneWidget);
    expect(find.text('Existing devices'), findsOneWidget);
    expect(find.text('Select from existing'), findsNothing);
    expect(api.getCanonicalDevicesCalls, 1);
    expect(
      tester.getTopLeft(find.byKey(const ValueKey('room-device-add-scan'))).dy,
      lessThan(
        tester
            .getTopLeft(
              find.byKey(const ValueKey('existing-room-devices-empty')),
            )
            .dy,
      ),
    );

    await tester.tap(find.byKey(const ValueKey('room-device-add-scan')));
    await tester.pumpAndSettle();
    expect(pairingIntakeIsVisible(), isFalse);
    expect(
      find.text(
        'Adding motion sensors is not available on this Rhythm Box yet.',
      ),
      findsOneWidget,
    );
    await tester.pump(const Duration(seconds: 5));
    await tester.pumpAndSettle();

    await _selectRoomSettingsTab(tester, 'Buttons');

    final addButton = find.byKey(const ValueKey('room-settings-add-button'));
    expect(addButton, findsOneWidget);
    expect(find.text('Add Button'), findsOneWidget);

    await tester.tap(addButton);
    await tester.pumpAndSettle();

    expect(find.text('Scan'), findsOneWidget);
    expect(find.text('Existing devices'), findsOneWidget);
    expect(find.text('Select from existing'), findsNothing);

    await tester.tap(find.byKey(const ValueKey('room-device-add-scan')));
    await tester.pumpAndSettle();
    expect(pairingIntakeIsVisible(), isTrue);
  });

  testWidgets(
      'room add actions keep existing devices usable when Scan is unavailable',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 1200));

    connection.emitHello(
      RhythmHello.fromJson({
        'capabilities': {'hubs': const <dynamic>[]},
        'nodes': const <dynamic>[],
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    const room = RoomDto(
      id: 'room-1',
      name: 'Kitchen',
      source: RoomSourceDto.unknown,
      deviceIds: [],
      rhythmEnabled: true,
      disabled: false,
      lightsOn: false,
      timeOffsetMinutes: 0,
      brightnessOffset: 0,
    );
    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: room,
    );

    await _selectRoomSettingsTab(tester, 'Buttons');
    await tester.tap(
      find.byKey(const ValueKey('room-settings-add-button')),
    );
    await tester.pumpAndSettle();

    expect(find.text('Scan'), findsOneWidget);
    expect(find.text('Existing devices'), findsOneWidget);
    expect(find.text('Select from existing'), findsNothing);

    await tester.tap(find.byKey(const ValueKey('room-device-add-scan')));
    await tester.pumpAndSettle();

    expect(
      find.text('Adding buttons is not available on this Rhythm Box yet.'),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('device-pairing-code-input')),
      findsNothing,
    );

    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'light-1',
        name: 'Desk Lamp',
        source: RoomSourceDto.matter,
        deviceIds: [],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
        kind: RoomNodeKind.lightDevice,
      ),
    );

    await _selectRoomSettingsTab(tester, 'Motion');
    expect(
      find.byKey(const ValueKey('room-settings-add-motion')),
      findsNothing,
    );
    await _selectRoomSettingsTab(tester, 'Buttons');
    expect(
      find.byKey(const ValueKey('room-settings-add-button')),
      findsNothing,
    );
    await _selectRoomSettingsTab(tester, 'Bulbs');
    expect(
      find.byKey(const ValueKey('room-settings-add-bulb')),
      findsNothing,
    );
  });

  testWidgets('room sheet adds existing motion as an additional room control',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 1200));

    api.topologyNodes = [
      RhythmTopologyNode.fromJson({
        'id': 'room-1',
        'name': 'Kitchen',
        'kind': 'room',
      }),
      RhythmTopologyNode.fromJson({
        'id': 'room-2',
        'name': 'Hall',
        'kind': 'room',
      }),
      RhythmTopologyNode.fromJson({
        'id': 'motion-current',
        'name': 'Kitchen Motion',
        'kind': 'motion_sensor',
        'parent_id': 'room-1',
      }),
      RhythmTopologyNode.fromJson({
        'id': 'motion-hall',
        'name': 'Hall Motion',
        'kind': 'motion_sensor',
        'parent_id': 'room-2',
      }),
      RhythmTopologyNode.fromJson({
        'id': 'button-1',
        'name': 'Wall Button',
        'kind': 'button',
      }),
    ];
    api.canonicalDevices.addAll({
      'motion-current': {
        'id': 'motion-current',
        'name': 'Kitchen Motion',
        'device_type': 'motion',
        'room_id': 'room-1',
        'endpoints': const <Map<String, dynamic>>[],
      },
      'motion-hall': {
        'id': 'motion-hall',
        'name': 'Hall Motion',
        'device_type': 'motion',
        'room_id': 'room-2',
        'endpoints': const <Map<String, dynamic>>[],
      },
      'button-1': {
        'id': 'button-1',
        'name': 'Wall Button',
        'device_type': 'button',
        'endpoints': const <Map<String, dynamic>>[],
      },
    });
    api.getCanonicalDevicesFails = true;
    connection.emitHello(
      RhythmHello.fromJson({
        'capabilities': {'hubs': const <dynamic>[]},
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': false,
          },
          {
            'id': 'room-2',
            'name': 'Hall',
            'kind': 'room',
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': false,
          },
        ],
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.unknown,
        deviceIds: [],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );
    await _selectRoomSettingsTab(tester, 'Motion');
    await tester.tap(
      find.byKey(const ValueKey('room-settings-add-motion')),
    );
    await tester.pumpAndSettle();

    expect(api.getCanonicalDevicesCalls, 1);
    expect(
      find.byKey(const ValueKey('existing-room-devices-error')),
      findsOneWidget,
    );
    expect(find.text('Try again'), findsOneWidget);

    api.getCanonicalDevicesFails = false;
    await tester.tap(find.text('Try again'));
    await tester.pumpAndSettle();

    expect(api.getCanonicalDevicesCalls, 2);
    expect(
      find.byKey(const ValueKey('existing-room-devices-list')),
      findsOneWidget,
    );
    expect(find.text('Select from existing'), findsNothing);
    expect(
      find.byKey(const ValueKey('existing-room-device-motion-current')),
      findsOneWidget,
    );
    expect(find.text('Already in Kitchen'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('existing-room-device-motion-hall')),
      findsOneWidget,
    );
    expect(find.text('Currently in Hall'), findsOneWidget);
    expect(find.text('Wall Button'), findsNothing);

    await tester.tap(
      find.byKey(const ValueKey('existing-room-device-motion-hall')),
    );
    await tester.pumpAndSettle();

    expect(api.assignDeviceParentCalls, 0);
    expect(api.setTopologyNodeControlTargetsCalls, 1);
    expect(api.lastControlSourceNodeId, 'motion-hall');
    expect(api.lastControlKind, 'motion');
    expect(api.lastControlTargetIds, ['room-1', 'room-2']);
    expect(
      find.text('Added Hall Motion as additional motion for Kitchen'),
      findsOneWidget,
    );
  });

  testWidgets(
      'room sheet materializes canonical-only motion before adding it',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 1200));

    api.topologyNodes = [
      RhythmTopologyNode.fromJson({
        'id': 'room-1',
        'name': 'Kitchen',
        'kind': 'room',
      }),
    ];
    api.canonicalDevices['motion-unassigned'] = {
      'id': 'motion-unassigned',
      'name': 'Camera Motion',
      'device_type': 'motion',
      'endpoints': const <Map<String, dynamic>>[],
    };
    connection.emitHello(
      RhythmHello.fromJson({
        'capabilities': {'hubs': const <dynamic>[]},
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': false,
          },
        ],
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.unknown,
        deviceIds: [],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );
    await _selectRoomSettingsTab(tester, 'Motion');
    await tester.tap(
      find.byKey(const ValueKey('room-settings-add-motion')),
    );
    await tester.pumpAndSettle();

    final assignment = Completer<bool>();
    api.assignDeviceParentCompleter = assignment;
    await tester.tap(
      find.byKey(const ValueKey('existing-room-device-motion-unassigned')),
    );
    await tester.pump();

    expect(api.assignDeviceParentCalls, 1);
    expect(api.lastAssignedDeviceId, 'motion-unassigned');
    expect(api.lastAssignedParentId, 'room-1');

    // The real server creates this source node as part of the canonical room
    // assignment. Publish that authoritative result before completing the
    // fake request so the app's mandatory refresh observes it.
    api.topologyNodes = [
      ...api.topologyNodes,
      RhythmTopologyNode.fromJson({
        'id': 'motion-unassigned',
        'name': 'Camera Motion',
        'kind': 'motion_sensor',
        'parent_id': 'room-1',
      }),
    ];
    assignment.complete(true);
    await tester.pumpAndSettle();

    expect(api.setTopologyNodeControlTargetsCalls, 0);
    expect(
      find.text('Added Camera Motion as additional motion for Kitchen'),
      findsOneWidget,
    );
  });

  testWidgets(
      'repeat scan resolves the existing canonical device before room assignment',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);

    api.topologyNodes = [
      RhythmTopologyNode.fromJson({
        'id': 'room-1',
        'name': 'Kitchen',
        'kind': 'room',
      }),
      RhythmTopologyNode.fromJson({
        'id': 'room-2',
        'name': 'Hall',
        'kind': 'room',
      }),
      RhythmTopologyNode.fromJson({
        'id': 'canonical-button-1',
        'name': 'Wall Button',
        'kind': 'button',
        'parent_id': 'room-2',
      }),
    ];
    api.canonicalDevices['canonical-button-1'] = {
      'id': 'canonical-button-1',
      'name': 'Wall Button',
      'device_type': 'button',
      'room_id': 'room-2',
      'endpoints': [
        {
          'hub_key': {'hub_type': 'local_ble'},
          'native_id': 'local-ble-existing-public-id',
          'preferred': true,
        },
      ],
    };

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: Builder(
          builder: (context) => TextButton(
            onPressed: () => continueRecoveredLocalBlePairingFlow(
              context,
              const RhythmPairedDevice(
                deviceId: 'local-ble-existing-public-id',
                name: 'Button',
                deviceType: 'button',
              ),
              analyticsSource: 'room_settings_buttons',
              roomAssignment: const DevicePairingRoomAssignment(
                roomId: 'room-1',
                roomName: 'Kitchen',
                expectedDeviceType: RhythmDeviceType.button,
              ),
            ),
            child: const Text('Complete scan'),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Complete scan'));
    await tester.pumpAndSettle();

    expect(api.getCanonicalDevicesCalls, 1);
    expect(api.assignDeviceParentCalls, 1);
    expect(api.lastAssignedDeviceId, 'canonical-button-1');
    expect(api.lastAssignedParentId, 'room-1');
    expect(find.text('Moved Wall Button to Kitchen'), findsOneWidget);
  });

  testWidgets('repeat scan already in the room avoids a redundant assignment',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);

    api.canonicalDevices['canonical-button-1'] = {
      'id': 'canonical-button-1',
      'name': 'Wall Button',
      'device_type': 'button',
      'room_id': 'room-1',
      'endpoints': [
        {
          'hub_key': {'hub_type': 'local_ble'},
          'native_id': 'local-ble-existing-public-id',
        },
      ],
    };

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: Builder(
          builder: (context) => TextButton(
            onPressed: () => continueRecoveredLocalBlePairingFlow(
              context,
              const RhythmPairedDevice(
                deviceId: 'local-ble-existing-public-id',
                name: 'Button',
                deviceType: 'button',
              ),
              roomAssignment: const DevicePairingRoomAssignment(
                roomId: 'room-1',
                roomName: 'Kitchen',
                expectedDeviceType: RhythmDeviceType.button,
              ),
            ),
            child: const Text('Complete scan'),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Complete scan'));
    await tester.pumpAndSettle();

    expect(api.assignDeviceParentCalls, 0);
    expect(find.text('Wall Button is already in Kitchen'), findsOneWidget);
  });

  testWidgets(
      'Hub picker always shows Matter and marks connected Hue and HA hubs',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);

    connection.emitHello(
      RhythmHello.fromJson({
        'rooms': const <Map<String, dynamic>>[],
        'location': const <String, dynamic>{},
        'hubs': [
          {
            'type': 'hue',
            'connected': true,
          },
          {
            'type': 'homeassistant',
            'connected': true,
          },
        ],
        'capabilities': {
          'hubs': [
            {
              'type': 'hue',
              'configurable': true,
              'device_onboarding_methods': const <String>[],
              'supports_unpairing': true,
              'supports_roomless_devices': false,
            },
          ],
        },
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    expect(provider.canAddMatterDevice, isFalse);

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const HubPickerScreen(),
      ),
    );
    await tester.pump(const Duration(milliseconds: 10));

    expect(find.text('Add Hubs'), findsOneWidget);
    expect(find.text('Home Assistant'), findsOneWidget);
    expect(find.text('Philips Hue'), findsOneWidget);
    expect(find.text('Matter'), findsOneWidget);
    expect(find.text('BETA'), findsNWidgets(2));
    expect(find.byIcon(Icons.check_circle_rounded), findsNWidgets(2));
  });

  group('ServerSyncProvider demo mode', () {
    late RoomProvider roomProvider;
    late _FakeRhythmServerApi api;
    late _HelloRhythmConnection connection;

    setUp(() {
      DemoServerApi.instance.reset();
      HueServiceLocator.setDemoMode(true);
      roomProvider = RoomProvider();
      api = _FakeRhythmServerApi();
      connection = _HelloRhythmConnection(api);
    });

    tearDown(() {
      HueServiceLocator.setDemoMode(false);
      roomProvider.dispose();
      connection.dispose();
    });

    test('populates topology and triage data from the shared demo state',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([
          Hub.create(
            id: 'demo-server',
            homeId: 'home-1',
            type: HubType.server,
            name: 'Demo Server',
            endpoint: const HubEndpoint(host: '127.0.0.1', port: 54448),
          ),
        ]),
      );
      addTearDown(provider.dispose);

      provider.connectIfAvailable();
      await Future<void>.delayed(const Duration(milliseconds: 20));

      expect(provider.synced, isTrue);
      expect(provider.helloRooms.map((room) => room.name),
          contains('Living Room'));
      expect(provider.devicesForRoom('hue_demo_1').length, 4);
      expect(provider.deviceSummaryForRoom('hue_demo_1'),
          '2 lights, 1 button, 1 sensor');
      expect(provider.triagePendingCount, 2);
      expect(provider.triagePendingDevices, 1);
      expect(provider.triagePendingRooms, 1);
      expect(provider.hasReviewAttention, isTrue);
      expect(provider.hubConfiguredConflicts, hasLength(1));
      expect(provider.reviewHistory, isNotEmpty);
      expect(provider.reviewHistory.first.isPending, isFalse);
    });

    test('refreshes demo topology after assigning an unassigned device',
        () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([
          Hub.create(
            id: 'demo-server',
            homeId: 'home-1',
            type: HubType.server,
            name: 'Demo Server',
            endpoint: const HubEndpoint(host: '127.0.0.1', port: 54448),
          ),
        ]),
      );
      addTearDown(provider.dispose);

      provider.connectIfAvailable();
      await Future<void>.delayed(const Duration(milliseconds: 20));

      final success = await provider.api
          .assignDeviceParent('demo_floor_lamp', 'hue_demo_2');
      await Future<void>.delayed(const Duration(milliseconds: 20));

      expect(success, isTrue);
      expect(provider.deviceSummaryForRoom('hue_demo_2'), '3 lights');
      expect(provider.triagePendingDevices, 0);
      expect(provider.triagePendingCount, 1);
    });

    test('stores demo transition trigger enabled updates', () async {
      final provider = ServerSyncProvider(
        connection: connection,
        roomProvider: roomProvider,
        homeProvider: _TestHomeProvider([
          Hub.create(
            id: 'demo-server',
            homeId: 'home-1',
            type: HubType.server,
            name: 'Demo Server',
            endpoint: const HubEndpoint(host: '127.0.0.1', port: 54448),
          ),
        ]),
      );
      addTearDown(provider.dispose);

      provider.connectIfAvailable();
      await Future<void>.delayed(const Duration(milliseconds: 20));

      expect(provider.modeTransitions, hasLength(2));
      expect(
        provider.modeTransitions
            .every((transition) => transition.triggerEnabled),
        isTrue,
      );

      final success = await provider.api.setTransitions([
        for (final transition in provider.modeTransitions)
          transition.copyWith(triggerEnabled: false),
      ]);
      await Future<void>.delayed(const Duration(milliseconds: 20));

      expect(success, isTrue);
      expect(
        provider.modeTransitions
            .every((transition) => transition.triggerEnabled),
        isFalse,
      );
    });
  });

  testWidgets(
      'room bulb row shows custom ranges and long press identifies once',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final identifyCompleter = Completer<bool>();
    final api = _FakeRhythmServerApi()
      ..flashCanonicalDeviceCompleter = identifyCompleter
      ..topologyNodes = [
        RhythmTopologyNode.fromJson({
          'id': 'room-1',
          'name': 'Kitchen',
          'kind': 'room',
        }),
        RhythmTopologyNode.fromJson({
          'id': 'light-1',
          'name': 'Desk Lamp',
          'kind': 'light_device',
          'parent_id': 'room-1',
        }),
      ];
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 900));

    connection.emitHello(
      RhythmHello.fromJson({
        'capabilities': {
          'api_schema_version': 2,
          'features': [RhythmFeature.roomLightProfileOverrides],
          'hubs': const <dynamic>[],
        },
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': true,
            'devices': [
              {'id': 'light-1', 'type': 'light', 'name': 'Desk Lamp'},
            ],
          },
          {
            'id': 'light-1',
            'name': 'Desk Lamp',
            'kind': 'light_device',
            'parent_id': 'room-1',
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': true,
            'profile_settings': {
              'profile_overrides': {
                'rhythm': {
                  'min_brightness': 12,
                  'max_color_temp': 4800,
                },
              },
            },
          },
        ],
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.matter,
        deviceIds: ['light-1'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );

    await _selectRoomSettingsTab(tester, 'Bulbs');
    await tester.pumpAndSettle();
    final row = find.byKey(const ValueKey('room-device-row-light-1'));
    expect(row, findsOneWidget);
    expect(
      find.byKey(const ValueKey('light-profile-override-badge-light-1')),
      findsOneWidget,
    );
    expect(find.text('BRI · CCT'), findsOneWidget);
    expect(
      tester.widget<Semantics>(row).properties.hint,
      'Tap for settings. Touch and hold to identify.',
    );

    await tester.longPress(find.text('Desk Lamp'));
    await tester.pump();
    expect(api.flashCanonicalDeviceCalls, 1);
    expect(
      find.byKey(const ValueKey('room-device-identify-progress')),
      findsOneWidget,
    );

    await tester.longPress(find.text('Desk Lamp'));
    await tester.pump();
    expect(api.flashCanonicalDeviceCalls, 1);

    identifyCompleter.complete(false);
    await tester.pumpAndSettle();
    expect(find.text('Could not identify Desk Lamp'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('room-device-identify-progress')),
      findsNothing,
    );
  });

  testWidgets(
      'move identifies, shows pending UI, and refreshes room membership',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final assignmentCompleter = Completer<bool>();
    final api = _FakeRhythmServerApi()
      ..assignDeviceParentCompleter = assignmentCompleter
      ..topologyNodes = [
        RhythmTopologyNode.fromJson({
          'id': 'room-1',
          'name': 'Kitchen',
          'kind': 'room',
        }),
        RhythmTopologyNode.fromJson({
          'id': 'room-2',
          'name': 'Dining',
          'kind': 'room',
        }),
        RhythmTopologyNode.fromJson({
          'id': 'light-1',
          'name': 'Desk Lamp',
          'kind': 'light_device',
          'parent_id': 'room-1',
        }),
      ];
    api.canonicalDevices['light-1'] = {
      'id': 'light-1',
      'name': 'Desk Lamp',
      'endpoints': [
        {
          'hub_key': {'hub_type': 'hue', 'address': 'bridge'},
          'native_id': 'hue-light-1',
          'preferred': true,
        },
      ],
    };
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 1000));

    Map<String, dynamic> roomNode(
      String id,
      String name,
      List<Map<String, dynamic>> devices,
    ) =>
        {
          'id': id,
          'name': name,
          'kind': 'room',
          'state': 'active',
          'rhythm_enabled': true,
          'disabled': false,
          'time_offset': 0.0,
          'brightness_offset': 0.0,
          'lights_on': true,
          'devices': devices,
        };

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          roomNode('room-1', 'Kitchen', [
            {'id': 'light-1', 'type': 'light', 'name': 'Desk Lamp'},
          ]),
          roomNode('room-2', 'Dining', const []),
        ],
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    connection.helloOnReconnect = RhythmHello.fromJson({
      'nodes': [
        roomNode('room-1', 'Kitchen', const []),
        roomNode('room-2', 'Dining', [
          {'id': 'light-1', 'type': 'light', 'name': 'Desk Lamp'},
        ]),
      ],
      'location': const <String, dynamic>{},
    });

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: Builder(
          builder: (context) => TextButton(
            onPressed: () => DeviceDetailSheet.show(
              context,
              const RhythmDevice(
                id: 'light-1',
                type: RhythmDeviceType.light,
                name: 'Desk Lamp',
              ),
              'room-1',
            ),
            child: const Text('Open device'),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Open device'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Move to Room...'));
    await tester.pumpAndSettle();

    expect(api.flashCanonicalDeviceCalls, 1);
    expect(find.text('Move to Room'), findsOneWidget);
    await tester.tap(find.text('Dining'));
    await tester.pump(const Duration(milliseconds: 300));

    expect(api.assignDeviceParentCalls, 1);
    expect(api.lastAssignedParentId, 'room-2');
    expect(find.text('Updating room…'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('device-room-move-progress')),
      findsOneWidget,
    );
    expect(
      tester
          .widget<ElevatedButton>(
            find.widgetWithText(ElevatedButton, 'DONE'),
          )
          .onPressed,
      isNull,
    );

    await tester.binding.handlePopRoute();
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.byType(DeviceDetailSheet), findsOneWidget);

    api.topologyNodes = [
      RhythmTopologyNode.fromJson({
        'id': 'room-1',
        'name': 'Kitchen',
        'kind': 'room',
      }),
      RhythmTopologyNode.fromJson({
        'id': 'room-2',
        'name': 'Dining',
        'kind': 'room',
      }),
      RhythmTopologyNode.fromJson({
        'id': 'light-1',
        'name': 'Desk Lamp',
        'kind': 'light_device',
        'parent_id': 'room-2',
      }),
    ];
    assignmentCompleter.complete(true);
    await tester.pumpAndSettle();

    expect(connection.reconnectCalls, 1);
    expect(connection.lastReconnectAuthoritative, isTrue);
    expect(find.byType(DeviceDetailSheet), findsNothing);
    expect(provider.devicesForRoom('room-1'), isEmpty);
    expect(
      provider.devicesForRoom('room-2').map((device) => device.id),
      contains('light-1'),
    );
  });

  testWidgets('Unassigned activates a new matter bulb as standalone',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    api.triageEntries = [
      {
        'id': 'unassigned-light-1',
        'kind': 'unassigned_device',
        'canonical_id': 'light-1',
      },
    ];
    final connection = _HelloRhythmConnection(api);
    final assignmentResult = Completer<bool>();
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['matter'],
            'state': 'active',
            'rhythm_enabled': false,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': false,
          },
        ],
        'location': const <String, dynamic>{},
      }),
    );

    await tester.pumpWidget(
      ChangeNotifierProvider<ServerSyncProvider>.value(
        value: provider,
        child: MaterialApp(
          home: Scaffold(
            body: Builder(
              builder: (context) {
                return TextButton(
                  onPressed: () async {
                    assignmentResult.complete(
                      await showDeviceNodeAssignmentFlow(
                        context,
                        device: const RhythmDevice(
                          id: 'light-1',
                          type: RhythmDeviceType.light,
                          name: 'Desk Lamp',
                        ),
                        currentParentNodeId: '',
                        allowNoRoom: true,
                      ),
                    );
                  },
                  child: const Text('Open'),
                );
              },
            ),
          ),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();

    expect(find.text('Unassigned'), findsOneWidget);
    expect(find.text('Kitchen'), findsOneWidget);

    await tester.tap(find.text('Unassigned'));
    await tester.pumpAndSettle();

    expect(await assignmentResult.future, isTrue);
    expect(api.assignDeviceParentCalls, 0);
    expect(api.resolveTriageNewCalls, 1);
    expect(api.lastResolvedTriageEntryId, 'unassigned-light-1');
    expect(connection.reconnectCalls, 1);
    expect(find.text('Desk Lamp is ready to use standalone'), findsOneWidget);
  });

  testWidgets('Move to Room sheet scrolls when many rooms are available',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final assignmentResult = Completer<bool>();
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 700));

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          for (int i = 1; i <= 20; i++)
            {
              'id': 'room-$i',
              'name': 'Room ${i.toString().padLeft(2, '0')}',
              'kind': 'room',
              'hub_types': ['matter'],
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'lights_on': true,
            },
        ],
        'location': const <String, dynamic>{},
      }),
    );

    await tester.pumpWidget(
      ChangeNotifierProvider<ServerSyncProvider>.value(
        value: provider,
        child: MaterialApp(
          home: Scaffold(
            body: Builder(
              builder: (context) {
                return TextButton(
                  onPressed: () async {
                    assignmentResult.complete(
                      await showDeviceNodeAssignmentFlow(
                        context,
                        device: const RhythmDevice(
                          id: 'light-1',
                          type: RhythmDeviceType.light,
                          name: 'Desk Lamp',
                        ),
                        currentParentNodeId: 'room-1',
                      ),
                    );
                  },
                  child: const Text('Open'),
                );
              },
            ),
          ),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();

    final scrollable = find.byType(SingleChildScrollView);
    await tester.dragUntilVisible(
      find.text('Room 20'),
      scrollable,
      const Offset(0, -250),
    );
    await tester.tap(find.text('Room 20'));
    await tester.pumpAndSettle();

    expect(await assignmentResult.future, isTrue);
    expect(api.assignDeviceParentCalls, 1);
    expect(api.lastAssignedDeviceId, 'light-1');
    expect(api.lastAssignedParentId, 'room-20');
    expect(connection.reconnectCalls, 1);
    expect(find.text('Moved Desk Lamp to Room 20'), findsOneWidget);
  });

  testWidgets('device assignment can create a room inline', (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final assignmentResult = Completer<bool>();
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);

    await tester.pumpWidget(
      ChangeNotifierProvider<ServerSyncProvider>.value(
        value: provider,
        child: MaterialApp(
          home: Scaffold(
            body: Builder(
              builder: (context) {
                return TextButton(
                  onPressed: () async {
                    assignmentResult.complete(
                      await showDeviceNodeAssignmentFlow(
                        context,
                        device: const RhythmDevice(
                          id: 'light-1',
                          type: RhythmDeviceType.light,
                          name: 'Desk Lamp',
                        ),
                        currentParentNodeId: '',
                      ),
                    );
                  },
                  child: const Text('Open'),
                );
              },
            ),
          ),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();

    expect(find.text('Create New Room'), findsOneWidget);

    await tester.tap(find.text('Create New Room'));
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField), 'Office');
    await tester.tap(find.text('Create'));
    await tester.pumpAndSettle();

    expect(await assignmentResult.future, isTrue);
    expect(api.createTopologyRoomCalls, 1);
    expect(api.lastCreatedRoomName, 'Office');
    expect(api.assignDeviceParentCalls, 1);
    expect(api.lastAssignedDeviceId, 'light-1');
    expect(api.lastAssignedParentId, 'room-created');
    expect(connection.reconnectCalls, 1);
    expect(find.text('Assigned Desk Lamp to Office'), findsOneWidget);
  });

  testWidgets('device detail header centers long device names', (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);

    const deviceName = 'GE Lighting, a Savant company Cync Full Color A19';

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const DeviceDetailSheet(
          device: RhythmDevice(
            id: 'light-1',
            type: RhythmDeviceType.light,
            name: deviceName,
          ),
          roomId: '',
        ),
      ),
    );
    await tester.pumpAndSettle();

    final headerText = tester.widget<Text>(find.text(deviceName));
    expect(headerText.textAlign, TextAlign.center);
  });

  testWidgets('Hue Bluetooth lights expose their removable connection',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 900));

    api.canonicalDevices['light-1'] = {
      'id': 'light-1',
      'name': 'Hue white lamp',
      'endpoints': [
        {
          'hub_key': {
            'hub_type': 'hue_ble',
            'address': 'local',
          },
          'native_id': '001788010c765ba7',
          'preferred': true,
        },
      ],
    };
    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': const <Map<String, dynamic>>[],
        'location': const <String, dynamic>{},
        'capabilities': {
          'hubs': [
            {
              'type': 'hue_ble',
              'configurable': false,
              'device_onboarding_methods': ['hue_ble_nearby_scan'],
              'supports_unpairing': true,
              'supports_roomless_devices': true,
            },
          ],
        },
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const DeviceDetailSheet(
          device: RhythmDevice(
            id: 'light-1',
            type: RhythmDeviceType.light,
            name: 'Hue white lamp',
          ),
          roomId: '',
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Network'));
    await tester.pumpAndSettle();

    expect(find.text('Hue Bluetooth'), findsOneWidget);
    expect(find.text('Remove Device'), findsOneWidget);

    await tester.tap(find.text('Remove Device'));
    await tester.pumpAndSettle();

    expect(
      find.textContaining('authenticated Bluetooth release'),
      findsOneWidget,
    );
    expect(
      find.textContaining('powered on and nearby'),
      findsOneWidget,
    );
    expect(
      find.textContaining('paired again without a factory reset'),
      findsOneWidget,
    );
  });

  testWidgets('Hue Bluetooth release failure offers retry before local forget',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi()
      ..unpairResults = [
        {'status': 'failed', 'error': 'Bulb is offline'},
        {'status': 'failed', 'error': 'Bulb is still offline'},
        {
          'status': 'complete',
          'completion_scope': 'local_bond_retained',
          'warning':
              'Rhythm retained the Bluetooth bond so a nearby scan can restore it.',
        },
      ];
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 1100));

    api.canonicalDevices['light-1'] = {
      'id': 'light-1',
      'name': 'Hue white lamp',
      'endpoints': [
        {
          'hub_key': {
            'hub_type': 'hue_ble',
            'address': 'local',
          },
          'native_id': 'hue-ble-001788010c765ba7',
          'preferred': true,
        },
      ],
    };
    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': const <Map<String, dynamic>>[],
        'location': const <String, dynamic>{},
        'capabilities': {
          'hubs': [
            {
              'type': 'hue_ble',
              'device_onboarding_methods': ['hue_ble_nearby_scan'],
              'supports_unpairing': true,
            },
          ],
        },
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const DeviceDetailSheet(
          device: RhythmDevice(
            id: 'light-1',
            type: RhythmDeviceType.light,
            name: 'Hue white lamp',
          ),
          roomId: '',
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Network'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Remove Device'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Remove'));
    await tester.pumpAndSettle();

    expect(find.text('Couldn’t release the bulb'), findsOneWidget);
    expect(find.textContaining('Bulb is offline'), findsOneWidget);
    expect(find.text('Try Again'), findsOneWidget);
    expect(find.text('Forget Anyway'), findsOneWidget);
    expect(
      find.textContaining('can explicitly re-adopt it later'),
      findsOneWidget,
    );
    expect(
      find.textContaining('may require a factory reset'),
      findsOneWidget,
    );
    expect(
      api.unpairCalls,
      [
        (
          hubType: 'hue_ble',
          deviceId: 'hue-ble-001788010c765ba7',
          hubAddress: null,
          force: false,
        ),
      ],
    );

    await tester.tap(find.text('Try Again'));
    await tester.pumpAndSettle();

    expect(find.text('Couldn’t release the bulb'), findsOneWidget);
    expect(find.textContaining('Bulb is still offline'), findsOneWidget);
    expect(
      api.unpairCalls.map((call) => call.force),
      [false, false],
    );

    await tester.tap(find.text('Forget Anyway'));
    await tester.pumpAndSettle();

    expect(
      api.unpairCalls.map((call) => call.force),
      [false, false, true],
    );
    expect(connection.reconnectCalls, 1);
    expect(find.text('Couldn’t release the bulb'), findsNothing);
    expect(find.textContaining('must be factory reset'), findsNothing);
    expect(find.textContaining('retained the Bluetooth bond'), findsOneWidget);
    await tester.tap(find.text('Done'));
    await tester.pumpAndSettle();
  });

  testWidgets(
      'Hue Bluetooth removal keeps its modal open and Done disabled while '
      'the request is in flight', (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final pendingRemoval = Completer<Map<String, dynamic>?>();
    final api = _FakeRhythmServerApi()..unpairCompleter = pendingRemoval;
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 1000));

    api.canonicalDevices['light-1'] = {
      'id': 'light-1',
      'name': 'Hue white lamp',
      'endpoints': [
        {
          'hub_key': {
            'hub_type': 'hue_ble',
            'address': 'local',
          },
          'native_id': 'hue-ble-001788010c765ba7',
          'preferred': true,
        },
      ],
    };
    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': const <Map<String, dynamic>>[],
        'location': const <String, dynamic>{},
        'capabilities': {
          'hubs': [
            {
              'type': 'hue_ble',
              'device_onboarding_methods': ['hue_ble_nearby_scan'],
              'supports_unpairing': true,
            },
          ],
        },
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: Builder(
          builder: (context) => TextButton(
            onPressed: () => DeviceDetailSheet.show(
              context,
              const RhythmDevice(
                id: 'light-1',
                type: RhythmDeviceType.light,
                name: 'Hue white lamp',
              ),
              '',
            ),
            child: const Text('Open device'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open device'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Network'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Remove Device'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Remove'));
    await tester.pump(const Duration(milliseconds: 300));

    expect(find.text('Removing...'), findsOneWidget);
    expect(
      tester
          .widget<ElevatedButton>(
            find.widgetWithText(ElevatedButton, 'DONE'),
          )
          .onPressed,
      isNull,
    );

    await tester.binding.handlePopRoute();
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.text('Removing...'), findsOneWidget);

    await tester.tapAt(const Offset(10, 10));
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.text('Removing...'), findsOneWidget);

    await tester.fling(
      find.byType(DeviceDetailSheet),
      const Offset(0, 600),
      1000,
    );
    await tester.pump(const Duration(milliseconds: 500));
    expect(find.text('Removing...'), findsOneWidget);

    pendingRemoval.complete(const {
      'status': 'complete',
      'completion_scope': 'local_bond_removed',
    });
    await tester.pumpAndSettle();

    expect(find.byType(DeviceDetailSheet), findsNothing);
    expect(find.text('Open device'), findsOneWidget);
  });

  testWidgets(
      'merged device enumerates removable endpoints and keeps detail open '
      'after Hue Bluetooth removal', (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 1000));

    api.canonicalDevices['light-1'] = {
      'id': 'light-1',
      'name': 'Merged Hue lamp',
      'endpoints': [
        {
          'hub_key': {
            'hub_type': 'hue_ble',
            'address': 'local',
          },
          'native_id': 'hue-ble-001788010c765ba7',
          'preferred': true,
        },
        {
          'hub_key': {
            'hub_type': 'matter',
            'address': 'local',
          },
          'native_id': 'matter-42',
          'preferred': false,
        },
      ],
    };
    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': const <Map<String, dynamic>>[],
        'location': const <String, dynamic>{},
        'capabilities': {
          'hubs': [
            {
              'type': 'hue_ble',
              'configurable': false,
              'device_onboarding_methods': ['hue_ble_nearby_scan'],
              'supports_unpairing': true,
              'supports_roomless_devices': true,
            },
            {
              'type': 'matter',
              'configurable': true,
              'device_onboarding_methods': [
                'matter_on_network_setup_code',
              ],
              'supports_unpairing': true,
              'supports_roomless_devices': true,
            },
          ],
        },
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const DeviceDetailSheet(
          device: RhythmDevice(
            id: 'light-1',
            type: RhythmDeviceType.light,
            name: 'Merged Hue lamp',
          ),
          roomId: '',
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Network'));
    await tester.pumpAndSettle();

    expect(find.text('Remove Hue Bluetooth Connection'), findsOneWidget);
    expect(find.text('Remove Matter Connection'), findsOneWidget);

    await tester.tap(find.text('Remove Hue Bluetooth Connection'));
    await tester.pumpAndSettle();

    expect(find.text('Remove Hue Bluetooth Connection?'), findsOneWidget);
    expect(
      find.textContaining(
        'The device will remain in Rhythm through its other connection.',
      ),
      findsOneWidget,
    );
    expect(
      find.textContaining('authenticated Bluetooth release'),
      findsOneWidget,
    );
    expect(
      find.textContaining('paired again without a factory reset'),
      findsOneWidget,
    );

    await tester.tap(find.text('Remove'));
    await tester.pumpAndSettle();

    expect(
      api.unpairCalls,
      [
        (
          hubType: 'hue_ble',
          deviceId: 'hue-ble-001788010c765ba7',
          hubAddress: null,
          force: false,
        ),
      ],
    );
    expect(connection.reconnectCalls, 1);
    expect(find.text('Merged Hue lamp'), findsOneWidget);
    expect(find.text('DONE'), findsOneWidget);
    expect(find.text('Hue Bluetooth'), findsNothing);
    expect(find.text('Matter'), findsOneWidget);
    expect(find.text('Remove Device'), findsOneWidget);
    expect(
      find.text(
        'Removed Hue Bluetooth connection from Merged Hue lamp',
      ),
      findsOneWidget,
    );
  });

  testWidgets('merged Hue Bluetooth lifecycle warnings require acknowledgement',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    const lifecycleWarning =
        'Rhythm retained the Bluetooth bond so a nearby scan can restore it.';
    final api = _FakeRhythmServerApi()
      ..unpairResult = const {
        'status': 'complete',
        'completion_scope': 'local_bond_retained',
        'warning': lifecycleWarning,
      };
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 1000));

    api.canonicalDevices['light-1'] = {
      'id': 'light-1',
      'name': 'Merged Hue lamp',
      'endpoints': [
        {
          'hub_key': {
            'hub_type': 'hue_ble',
            'address': 'local',
          },
          'native_id': 'hue-ble-001788010c765ba7',
          'preferred': true,
        },
        {
          'hub_key': {
            'hub_type': 'matter',
            'address': 'local',
          },
          'native_id': 'matter-42',
          'preferred': false,
        },
      ],
    };
    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': const <Map<String, dynamic>>[],
        'location': const <String, dynamic>{},
        'capabilities': {
          'hubs': [
            {
              'type': 'hue_ble',
              'device_onboarding_methods': ['hue_ble_nearby_scan'],
              'supports_unpairing': true,
            },
            {
              'type': 'matter',
              'device_onboarding_methods': [
                'matter_on_network_setup_code',
              ],
              'supports_unpairing': true,
            },
          ],
        },
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const DeviceDetailSheet(
          device: RhythmDevice(
            id: 'light-1',
            type: RhythmDeviceType.light,
            name: 'Merged Hue lamp',
          ),
          roomId: '',
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Network'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Remove Hue Bluetooth Connection'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Remove'));
    await tester.pumpAndSettle();

    expect(
      find.text('Removed Hue Bluetooth connection'),
      findsOneWidget,
    );
    expect(find.text(lifecycleWarning), findsOneWidget);
    expect(find.text('Done'), findsOneWidget);

    await tester.tapAt(const Offset(10, 10));
    await tester.pumpAndSettle();
    expect(find.text(lifecycleWarning), findsOneWidget);

    await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();
    expect(find.text(lifecycleWarning), findsOneWidget);

    await tester.tap(find.text('Done'));
    await tester.pumpAndSettle();

    expect(find.text(lifecycleWarning), findsNothing);
    expect(find.text('Merged Hue lamp'), findsOneWidget);
    expect(find.text('DONE'), findsOneWidget);
    expect(find.text('Hue Bluetooth'), findsNothing);
    expect(find.text('Matter'), findsOneWidget);
  });

  testWidgets('merged device removal targets the selected Matter endpoint',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 1000));

    api.canonicalDevices['light-1'] = {
      'id': 'light-1',
      'name': 'Merged Hue lamp',
      'endpoints': [
        {
          'hub_key': {
            'hub_type': 'hue_ble',
            'address': 'local',
          },
          'native_id': 'hue-ble-001788010c765ba7',
          'preferred': true,
        },
        {
          'hub_key': {
            'hub_type': 'matter',
            'address': 'local',
          },
          'native_id': 'matter-42',
          'preferred': false,
        },
      ],
    };
    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': const <Map<String, dynamic>>[],
        'location': const <String, dynamic>{},
        'capabilities': {
          'hubs': [
            {
              'type': 'hue_ble',
              'device_onboarding_methods': ['hue_ble_nearby_scan'],
              'supports_unpairing': true,
            },
            {
              'type': 'matter',
              'device_onboarding_methods': [
                'matter_on_network_setup_code',
              ],
              'supports_unpairing': true,
            },
          ],
        },
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const DeviceDetailSheet(
          device: RhythmDevice(
            id: 'light-1',
            type: RhythmDeviceType.light,
            name: 'Merged Hue lamp',
          ),
          roomId: '',
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Network'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Remove Matter Connection'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Remove'));
    await tester.pumpAndSettle();

    expect(
      api.unpairCalls,
      [
        (
          hubType: 'matter',
          deviceId: 'matter-42',
          hubAddress: null,
          force: false,
        ),
      ],
    );
    expect(find.text('DONE'), findsOneWidget);
    expect(find.text('Matter'), findsNothing);
    expect(find.text('Hue Bluetooth'), findsOneWidget);
    expect(find.text('Remove Device'), findsOneWidget);
  });

  testWidgets(
      'merged Hue Bridge removal targets its bridge and preserves Bluetooth',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 1000));

    api.canonicalDevices['light-1'] = {
      'id': 'light-1',
      'name': 'Dual Hue lamp',
      'endpoints': [
        {
          'hub_key': {
            'hub_type': 'hue',
            'address': '192.168.1.20',
          },
          'native_id': 'hue-light-7',
          'preferred': true,
        },
        {
          'hub_key': {
            'hub_type': 'hue_ble',
            'address': 'local',
          },
          'native_id': 'hue-ble-001788010c765ba7',
          'preferred': false,
        },
      ],
    };
    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': const <Map<String, dynamic>>[],
        'location': const <String, dynamic>{},
        'capabilities': {
          'hubs': [
            {
              'type': 'hue',
              'device_onboarding_methods': ['hue_bridge_serial_search'],
              'supports_unpairing': true,
            },
            {
              'type': 'hue_ble',
              'device_onboarding_methods': ['hue_ble_nearby_scan'],
              'supports_unpairing': true,
            },
          ],
        },
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const DeviceDetailSheet(
          device: RhythmDevice(
            id: 'light-1',
            type: RhythmDeviceType.light,
            name: 'Dual Hue lamp',
          ),
          roomId: '',
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Network'));
    await tester.pumpAndSettle();

    expect(find.text('Remove Hue Bridge Connection'), findsOneWidget);
    expect(find.text('Remove Hue Bluetooth Connection'), findsOneWidget);

    await tester.tap(find.text('Remove Hue Bridge Connection'));
    await tester.pumpAndSettle();

    expect(
      find.textContaining('ask your Hue Bridge to remove'),
      findsOneWidget,
    );
    expect(
      find.textContaining(
        'The device will remain in Rhythm through its other connection.',
      ),
      findsOneWidget,
    );
    expect(find.textContaining('factory-reset'), findsNothing);

    await tester.tap(find.text('Remove'));
    await tester.pumpAndSettle();

    expect(
      api.unpairCalls,
      [
        (
          hubType: 'hue',
          deviceId: 'hue-light-7',
          hubAddress: '192.168.1.20',
          force: false,
        ),
      ],
    );
    expect(connection.reconnectCalls, 1);
    expect(find.text('DONE'), findsOneWidget);
    expect(find.text('Hue Bridge'), findsNothing);
    expect(find.text('Hue Bluetooth'), findsOneWidget);
    expect(find.text('Remove Device'), findsOneWidget);
  });

  testWidgets('Hue Bridge switch removal uses typed capability and copy',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 900));

    api.canonicalDevices['switch-1'] = {
      'id': 'switch-1',
      'name': 'Kitchen Dimmer',
      'endpoints': [
        {
          'hub_key': {
            'hub_type': 'hue',
            'address': '192.168.1.20',
          },
          'native_id': 'hue-device-switch-1',
          'preferred': true,
        },
      ],
    };
    connection.emitHello(
      RhythmHello.fromJson({
        'capabilities': {
          'hubs': [
            {
              'type': 'hue',
              'supports_unpairing': true,
              'unpairable_device_types': ['light', 'button', 'motion'],
            },
          ],
        },
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const DeviceDetailSheet(
          device: RhythmDevice(
            id: 'switch-1',
            type: RhythmDeviceType.button,
            name: 'Kitchen Dimmer',
          ),
          roomId: '',
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Network'));
    await tester.pumpAndSettle();
    expect(find.text('Remove Device'), findsOneWidget);

    await tester.tap(find.text('Remove Device'));
    await tester.pumpAndSettle();
    expect(find.textContaining('whether the switch is already absent'),
        findsOneWidget);
    expect(find.textContaining('bulb'), findsNothing);

    await tester.tap(find.text('Remove'));
    await tester.pumpAndSettle();
    expect(api.unpairCalls.single.hubType, 'hue');
    expect(api.unpairCalls.single.deviceId, 'hue-device-switch-1');
    expect(api.unpairCalls.single.hubAddress, '192.168.1.20');
    expect(api.unpairDeviceTypes.single, 'button');
    expect(
      api.unpairCorrelationIds.single,
      startsWith('hue-bridge-remove-'),
    );
  });

  testWidgets('legacy Hue capability hides switch removal', (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 900));
    api.canonicalDevices['switch-legacy'] = {
      'id': 'switch-legacy',
      'name': 'Legacy Dimmer',
      'endpoints': [
        {
          'hub_key': {'hub_type': 'hue', 'address': '192.168.1.20'},
          'native_id': 'hue-device-switch-legacy',
        },
      ],
    };
    connection.emitHello(
      RhythmHello.fromJson({
        'capabilities': {
          'hubs': [
            {'type': 'hue', 'supports_unpairing': true},
          ],
        },
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));
    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const DeviceDetailSheet(
          device: RhythmDevice(
            id: 'switch-legacy',
            type: RhythmDeviceType.button,
            name: 'Legacy Dimmer',
          ),
          roomId: '',
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Network'));
    await tester.pumpAndSettle();
    expect(find.text('Remove Device'), findsNothing);
  });

  testWidgets('Hue Bridge fallback only finishes after absence confirmation',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi()
      ..unpairResults = [
        {'status': 'failed', 'error': 'Hue Bridge unreachable'},
        {'status': 'complete'},
      ];
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 900));

    api.canonicalDevices['light-1'] = {
      'id': 'light-1',
      'name': 'Bridge lamp',
      'endpoints': [
        {
          'hub_key': {
            'hub_type': 'hue',
            'address': '192.168.1.20',
          },
          'native_id': 'hue-light-8',
          'preferred': true,
        },
      ],
    };
    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': const <Map<String, dynamic>>[],
        'location': const <String, dynamic>{},
        'capabilities': {
          'hubs': [
            {
              'type': 'hue',
              'device_onboarding_methods': ['hue_bridge_serial_search'],
              'supports_unpairing': true,
            },
          ],
        },
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const DeviceDetailSheet(
          device: RhythmDevice(
            id: 'light-1',
            type: RhythmDeviceType.light,
            name: 'Bridge lamp',
          ),
          roomId: '',
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Network'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Remove Device'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Remove'));
    await tester.pumpAndSettle();

    expect(find.text('Removal Not Confirmed'), findsOneWidget);
    expect(find.textContaining('Hue Bridge unreachable'), findsOneWidget);
    expect(
      find.textContaining(
        'only after the bridge confirms the bulb is absent',
      ),
      findsOneWidget,
    );
    expect(find.textContaining('local-only'), findsNothing);
    expect(find.textContaining('without asking'), findsNothing);

    await tester.tap(find.text('Check and Finish Removal'));
    await tester.pumpAndSettle();

    expect(
      api.unpairCalls,
      [
        (
          hubType: 'hue',
          deviceId: 'hue-light-8',
          hubAddress: '192.168.1.20',
          force: false,
        ),
        (
          hubType: 'hue',
          deviceId: 'hue-light-8',
          hubAddress: '192.168.1.20',
          force: true,
        ),
      ],
    );
    expect(connection.reconnectCalls, 1);
    expect(find.text('DONE'), findsNothing);
  });

  testWidgets(
      'addressable bulb uses shared Low glow and opens custom Lighting overrides',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 1000));
    api.canonicalDevices['light-1'] = {
      'id': 'light-1',
      'name': 'Desk Lamp',
      'endpoints': const <Map<String, dynamic>>[],
    };
    connection.emitHello(
      RhythmHello.fromJson({
        'version': '0.6.533-beta',
        'capabilities': {
          'api_schema_version': 2,
          'features': [RhythmFeature.roomLightProfileOverrides],
          'hubs': const <dynamic>[],
        },
        'nodes': [
          {
            'id': 'light-1',
            'name': 'Desk Lamp',
            'kind': 'light_device',
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'standby_enabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'light_capabilities': {
              'individual_profile_overrides': true,
            },
            'profile_settings': {
              'profile_overrides': {
                'rhythm': {'max_brightness': 64},
              },
            },
          },
        ],
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const DeviceDetailSheet(
          device: RhythmDevice(
            id: 'light-1',
            type: RhythmDeviceType.light,
            name: 'Desk Lamp',
          ),
          roomId: '',
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Lighting'), findsOneWidget);
    expect(find.text('Custom'), findsOneWidget);
    expect(find.text('Low glow'), findsOneWidget);
    expect(find.text('Keep softly lit'), findsNothing);

    await tester.tap(find.text('Low glow'));
    await tester.pump();

    expect(api.nodePreferenceCalls, hasLength(1));
    expect(api.nodePreferenceCalls.single.nodeId, 'light-1');
    expect(api.nodePreferenceCalls.single.standbyEnabled, isTrue);
    expect(provider.standbyEnabledForNode('light-1'), isTrue);

    await tester.tap(
      find.byKey(
        const ValueKey('device-settings-light-settings-light-1'),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Light settings · Bulb override'), findsOneWidget);
    expect(find.text('Custom light settings'), findsOneWidget);
  });

  testWidgets('group-routed Hue bulb disables individual Lighting settings',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 1000));
    api.canonicalDevices['hue-light-1'] = {
      'id': 'hue-light-1',
      'name': 'Grouped Hue Lamp',
      'endpoints': const <Map<String, dynamic>>[],
    };
    connection.emitHello(
      RhythmHello.fromJson({
        'capabilities': {
          'api_schema_version': 2,
          'features': [RhythmFeature.roomLightProfileOverrides],
          'hubs': const <dynamic>[],
        },
        'nodes': [
          {
            'id': 'hue-light-1',
            'name': 'Grouped Hue Lamp',
            'kind': 'light_device',
            'parent_id': 'room-1',
            'hub_types': ['hue'],
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'standby_enabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'light_capabilities': {
              'individual_profile_overrides': false,
            },
          },
        ],
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const DeviceDetailSheet(
          device: RhythmDevice(
            id: 'hue-light-1',
            type: RhythmDeviceType.light,
            name: 'Grouped Hue Lamp',
          ),
          roomId: 'room-1',
        ),
      ),
    );
    await tester.pumpAndSettle();

    final lighting = find.byKey(
      const ValueKey('device-settings-light-settings-hue-light-1'),
    );
    expect(lighting, findsOneWidget);
    expect(
      tester
          .widget<Text>(
            find.byKey(
              const ValueKey(
                'device-settings-light-status-hue-light-1',
              ),
            ),
          )
          .data,
      'Room only',
    );
    final semantics = tester.getSemantics(lighting);
    expect(semantics.label, 'Lighting');
    expect(semantics.value, 'Controlled by room');
    expect(
      semantics.getSemanticsData().flagsCollection.isEnabled,
      Tristate.isFalse,
    );

    await tester.tap(lighting);
    await tester.pumpAndSettle();
    expect(find.byType(LightScreen), findsNothing);
    expect(find.textContaining('Update the Rhythm appliance'), findsNothing);
  });

  testWidgets('motion controls lightly select and save multiple rooms',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 900));
    api.topologyNodes = [
      RhythmTopologyNode.fromJson({
        'id': 'room-1',
        'name': 'Kitchen',
        'kind': 'room',
      }),
      RhythmTopologyNode.fromJson({
        'id': 'room-2',
        'name': 'Hall',
        'kind': 'room',
      }),
      RhythmTopologyNode.fromJson({
        'id': 'sensor-1',
        'name': 'Kitchen Motion',
        'kind': 'motion_sensor',
        'parent_id': 'room-1',
        'controls': [
          {
            'kind': 'motion',
            'target_id': 'room-1',
            'inherited': true,
          },
        ],
      }),
    ];
    api.canonicalDevices['sensor-1'] = {
      'id': 'sensor-1',
      'name': 'Kitchen Motion',
      'endpoints': const <Map<String, dynamic>>[],
    };
    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': true,
          },
          {
            'id': 'room-2',
            'name': 'Hall',
            'kind': 'room',
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': true,
          },
        ],
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const DeviceDetailSheet(
          device: RhythmDevice(
            id: 'sensor-1',
            type: RhythmDeviceType.motion,
            name: 'Kitchen Motion',
          ),
          roomId: 'room-1',
        ),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));

    expect(find.text('Motion controls'), findsOneWidget);
    expect(find.text('1 room'), findsOneWidget);

    await tester.tap(find.text('Motion controls'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));

    expect(find.text('Motion Controls'), findsOneWidget);
    expect(
      tester
          .widget<CheckboxListTile>(
            find.widgetWithText(CheckboxListTile, 'Kitchen'),
          )
          .value,
      isTrue,
    );

    await tester.tap(find.text('Kitchen'));
    await tester.pump();
    expect(
      tester
          .widget<ElevatedButton>(
            find.widgetWithText(ElevatedButton, 'SAVE'),
          )
          .onPressed,
      isNull,
    );

    await tester.tap(find.text('Kitchen'));
    await tester.tap(find.text('Hall'));
    await tester.pump();
    await tester.tap(find.widgetWithText(ElevatedButton, 'SAVE'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 500));

    expect(api.setTopologyNodeControlTargetsCalls, 1);
    expect(api.lastControlSourceNodeId, 'sensor-1');
    expect(api.lastControlKind, 'motion');
    expect(api.lastControlTargetIds, ['room-1', 'room-2']);
    expect(connection.reconnectCalls, 1);
    expect(connection.lastReconnectAuthoritative, isTrue);
    expect(find.text('2 rooms'), findsOneWidget);
    expect(
      find.text('Updated Kitchen Motion motion controls'),
      findsOneWidget,
    );
  });

  testWidgets(
      'button controls require capability and save multiple room targets',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 900));
    api.topologyNodes = [
      RhythmTopologyNode.fromJson({
        'id': 'room-1',
        'name': 'Kitchen',
        'kind': 'room',
      }),
      RhythmTopologyNode.fromJson({
        'id': 'room-2',
        'name': 'Hall',
        'kind': 'room',
      }),
      RhythmTopologyNode.fromJson({
        'id': 'button-1',
        'name': 'Kitchen Button',
        'kind': 'button',
        'parent_id': 'room-1',
        'controls': [
          {
            'kind': 'button',
            'target_id': 'room-1',
            'inherited': true,
          },
        ],
      }),
    ];
    api.canonicalDevices['button-1'] = {
      'id': 'button-1',
      'name': 'Kitchen Button',
      'endpoints': const <Map<String, dynamic>>[],
    };
    final helloJson = {
      'nodes': [
        {
          'id': 'room-1',
          'name': 'Kitchen',
          'kind': 'room',
          'state': 'active',
          'rhythm_enabled': true,
          'disabled': false,
          'time_offset': 0.0,
          'brightness_offset': 0.0,
          'lights_on': true,
        },
        {
          'id': 'room-2',
          'name': 'Hall',
          'kind': 'room',
          'state': 'active',
          'rhythm_enabled': true,
          'disabled': false,
          'time_offset': 0.0,
          'brightness_offset': 0.0,
          'lights_on': true,
        },
      ],
      'location': const <String, dynamic>{},
    };
    connection.emitHello(RhythmHello.fromJson(helloJson));
    await tester.pump(const Duration(milliseconds: 10));

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const DeviceDetailSheet(
          device: RhythmDevice(
            id: 'button-1',
            type: RhythmDeviceType.button,
            name: 'Kitchen Button',
          ),
          roomId: 'room-1',
        ),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));

    expect(provider.buttonMultiRoomControlsSupported, isFalse);
    expect(find.text('Button controls'), findsNothing);

    connection.emitHello(
      RhythmHello.fromJson({
        ...helloJson,
        'capabilities': {
          'features': [RhythmFeature.buttonMultiRoomControls],
        },
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    expect(provider.buttonMultiRoomControlsSupported, isTrue);
    expect(find.text('Button controls'), findsOneWidget);
    expect(find.text('1 room'), findsOneWidget);

    await tester.tap(find.text('Button controls'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));

    expect(find.text('Button Controls'), findsOneWidget);
    expect(
      tester
          .widget<CheckboxListTile>(
            find.widgetWithText(CheckboxListTile, 'Kitchen'),
          )
          .value,
      isTrue,
    );

    await tester.tap(find.text('Hall'));
    await tester.pump();
    await tester.tap(find.widgetWithText(ElevatedButton, 'SAVE'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 500));

    expect(api.setTopologyNodeControlTargetsCalls, 1);
    expect(api.lastControlSourceNodeId, 'button-1');
    expect(api.lastControlKind, 'button');
    expect(api.lastControlTargetIds, ['room-1', 'room-2']);
    expect(find.text('2 rooms'), findsOneWidget);
    expect(
      find.text('Updated Kitchen Button button controls'),
      findsOneWidget,
    );
  });

  testWidgets('room motion tab includes a sensor parented to another room',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 900));
    api.topologyNodes = [
      RhythmTopologyNode.fromJson({
        'id': 'room-1',
        'name': 'Drop Zone',
        'kind': 'room',
      }),
      RhythmTopologyNode.fromJson({
        'id': 'room-2',
        'name': 'Stairwell',
        'kind': 'room',
      }),
      RhythmTopologyNode.fromJson({
        'id': 'sensor-1',
        'name': 'Drop zone hallway',
        'kind': 'motion_sensor',
        'parent_id': 'room-2',
        'controls': [
          {'kind': 'motion', 'target_id': 'room-1', 'inherited': false},
          {'kind': 'motion', 'target_id': 'room-2', 'inherited': true},
        ],
      }),
    ];
    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Drop Zone',
            'kind': 'room',
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': true,
          },
          {
            'id': 'room-2',
            'name': 'Stairwell',
            'kind': 'room',
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': true,
            'devices': [
              {
                'id': 'sensor-1',
                'type': 'motion',
                'name': 'Drop zone hallway',
              },
            ],
          },
        ],
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));
    await tester.pump();
    expect(provider.topologyNodes, hasLength(3));
    expect(
      provider.controlSourceNodesForTarget(
        targetNodeId: 'room-1',
        controlKind: 'motion',
      ),
      hasLength(1),
    );
    expect(provider.deviceForNode('sensor-1')?.type, RhythmDeviceType.motion);

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const RoomSettingsSheet(
          enableLivePreview: false,
          room: RoomDto(
            id: 'room-1',
            name: 'Drop Zone',
            source: RoomSourceDto.matter,
            deviceIds: [],
            rhythmEnabled: true,
            disabled: false,
            lightsOn: true,
            timeOffsetMinutes: 0,
            brightnessOffset: 0,
          ),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 10));
    await _selectRoomSettingsTab(tester, 'Motion');
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('motion')), findsOneWidget);
    expect(
      find.text('MOTION SENSORS', skipOffstage: false),
      findsOneWidget,
    );
    expect(
      find.text('Drop zone hallway', skipOffstage: false),
      findsOneWidget,
    );

    expect(
      find.byWidgetPredicate(
        (widget) =>
            widget.runtimeType.toString() == '_DeviceRow' &&
            (widget as dynamic).roomId == 'room-2',
        skipOffstage: false,
      ),
      findsOneWidget,
    );
  });

  testWidgets('Room card device flow offers Remove from Room for Matter bulbs',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    api.triageEntries = [
      {
        'id': 'unassigned-light-1',
        'kind': 'unassigned_device',
        'canonical_id': 'light-1',
      },
    ];
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 900));

    api.topologyNodes = [
      RhythmTopologyNode.fromJson({
        'id': 'room-1',
        'name': 'Kitchen',
        'kind': 'room',
      }),
      RhythmTopologyNode.fromJson({
        'id': 'light-1',
        'name': 'Desk Lamp',
        'kind': 'light_device',
        'parent_id': 'room-1',
      }),
    ];
    api.canonicalDevices['light-1'] = {
      'endpoints': [
        {
          'hub_key': {'hub_type': 'matter'},
          'native_id': 'matter-light-1',
          'preferred': true,
        },
      ],
    };

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['matter'],
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': true,
          },
        ],
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.matter,
        deviceIds: ['light-1'],
        rhythmEnabled: false,
        disabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );

    await _selectRoomSettingsTab(tester, 'Bulbs');
    await tester.tap(find.text('Desk Lamp').last);
    await tester.pumpAndSettle();

    expect(find.text('Move or Remove...'), findsOneWidget);

    await tester.tap(find.text('Move or Remove...'));
    await tester.pumpAndSettle();

    expect(find.text('Remove from Room'), findsOneWidget);

    await tester.tap(find.text('Remove from Room'));
    await tester.pumpAndSettle();

    expect(api.assignDeviceParentCalls, 1);
    expect(api.lastAssignedDeviceId, 'light-1');
    expect(api.lastAssignedParentId, isNull);
    expect(api.resolveTriageNewCalls, 1);
    expect(api.lastResolvedTriageEntryId, 'unassigned-light-1');
    expect(connection.reconnectCalls, 1);
    expect(find.text('Desk Lamp is ready to use standalone'), findsOneWidget);
  });

  testWidgets('Room device flow removes motion sensors from a room',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 900));

    api.canonicalDevices['sensor-1'] = {
      'endpoints': [
        {
          'hub_key': {'hub_type': 'hue'},
          'native_id': 'hue-sensor-1',
          'preferred': true,
        },
      ],
    };

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['hue'],
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': true,
            'devices': [
              {
                'id': 'sensor-1',
                'type': 'motion',
                'name': 'Kitchen Motion',
              },
            ],
          },
        ],
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));

    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.hue,
        deviceIds: ['sensor-1'],
        rhythmEnabled: false,
        disabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );

    await _selectRoomSettingsTab(tester, 'Motion');
    await tester.tap(find.text('Kitchen Motion').last);
    await tester.pumpAndSettle();

    expect(find.text('Move or Remove...'), findsOneWidget);

    await tester.tap(find.text('Move or Remove...'));
    await tester.pumpAndSettle();

    expect(find.text('Remove from Room'), findsOneWidget);

    await tester.tap(find.text('Remove from Room'));
    await tester.pumpAndSettle();

    expect(api.assignDeviceParentCalls, 1);
    expect(api.lastAssignedDeviceId, 'sensor-1');
    expect(api.lastAssignedParentId, isNull);
    expect(connection.reconnectCalls, 1);
    expect(find.text('Removed Kitchen Motion from its room'), findsOneWidget);
  });

  testWidgets('Delete Room is available for a source-backed Hue room',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 1400));

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['hue'],
            'device_ids': ['light-1'],
            'state': 'active',
            'rhythm_enabled': false,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': false,
          },
        ],
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));
    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.hue,
        deviceIds: ['light-1'],
        rhythmEnabled: false,
        disabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );

    await tester.tap(find.byKey(const ValueKey('room-settings-rename')));
    await tester.pumpAndSettle();

    expect(find.text('Rename Room'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('room-settings-delete-room')),
      findsOneWidget,
    );

    await tester.tap(find.text('Delete Room'));
    await tester.pump(const Duration(milliseconds: 250));
    expect(
      find.textContaining('also be deleted there when supported'),
      findsOneWidget,
    );
    await tester.tap(find.widgetWithText(TextButton, 'Delete'));
    await tester.pump(const Duration(milliseconds: 500));

    expect(api.topologyDeleteRoomCalls, 1);
    expect(api.lastDeletedRoomId, 'room-1');
  });

  testWidgets('room page explains unavailable Lighting settings capability',
      (tester) async {
    _registerWidgetCleanup(tester);
    final semantics = tester.ensureSemantics();
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 1200));

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['matter'],
            'device_ids': ['light-1'],
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': true,
          },
        ],
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));
    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.matter,
        kind: RoomNodeKind.room,
        deviceIds: ['light-1'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );

    await _selectRoomSettingsTab(tester, 'Lighting');
    final lightSettings = find.byKey(
      const ValueKey('room-settings-light-settings-room-1'),
    );
    expect(lightSettings, findsOneWidget);
    expect(
      tester
          .widget<Text>(
            find.byKey(
              const ValueKey('room-settings-light-status-room-1'),
            ),
          )
          .data,
      'Update required',
    );
    final lightSettingsSemantics = tester.widget<Semantics>(lightSettings);
    expect(lightSettingsSemantics.properties.label, 'Lighting');
    expect(
      lightSettingsSemantics.properties.value,
      'Appliance update required',
    );

    await tester.tap(lightSettings);
    await tester.pump();
    expect(
      find.text(
        'Update the Rhythm appliance to customize light settings for Kitchen.',
      ),
      findsOneWidget,
    );
    semantics.dispose();
  });

  testWidgets('room settings standby switch pushes node preference',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 1200));

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['matter'],
            'device_ids': ['light-1'],
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'standby_enabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': true,
          },
        ],
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));
    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.matter,
        deviceIds: ['light-1'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );

    await _selectRoomSettingsTab(tester, 'Lighting');
    await tester.tap(
      find.descendant(
        of: find.byKey(
          const ValueKey('room-settings-low-glow-room-1'),
        ),
        matching: find.text('Low glow'),
      ),
    );
    await tester.pump();

    expect(api.nodePreferenceCalls, hasLength(1));
    final call = api.nodePreferenceCalls.single;
    expect(call.nodeId, 'room-1');
    expect(call.standbyEnabled, isTrue);
    expect(provider.standbyEnabledForNode('room-1'), isTrue);
  });

  testWidgets('room standby preference ignores stale hello after local change',
      (tester) async {
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);

    RhythmHello helloWithStandby(bool standbyEnabled) => RhythmHello.fromJson({
          'nodes': [
            {
              'id': 'room-1',
              'name': 'Kitchen',
              'kind': 'room',
              'hub_types': ['matter'],
              'device_ids': ['light-1'],
              'state': 'active',
              'rhythm_enabled': true,
              'disabled': false,
              'standby_enabled': standbyEnabled,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'lights_on': true,
            },
          ],
          'location': const <String, dynamic>{},
        });

    connection.emitHello(helloWithStandby(false));
    await tester.pump(const Duration(milliseconds: 10));
    expect(provider.standbyEnabledForNode('room-1'), isFalse);

    provider.setNodeStandbyEnabledLocal('room-1', true);
    expect(provider.standbyEnabledForNode('room-1'), isTrue);

    connection.emitHello(helloWithStandby(false));
    await tester.pump(const Duration(milliseconds: 10));
    expect(provider.standbyEnabledForNode('room-1'), isTrue);

    connection.emitHello(helloWithStandby(true));
    await tester.pump(const Duration(milliseconds: 10));
    expect(provider.standbyEnabledForNode('room-1'), isTrue);

    connection.emitHello(helloWithStandby(false));
    await tester.pump(const Duration(milliseconds: 10));
    expect(provider.standbyEnabledForNode('room-1'), isFalse);
  });

  testWidgets('room settings hides motion timeout rows without motion behavior',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 1200));

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['matter'],
            'device_ids': ['light-1'],
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': true,
          },
        ],
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));
    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.matter,
        deviceIds: ['light-1'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );

    await _selectRoomSettingsTab(tester, 'Lighting');

    expect(
      find.byKey(const ValueKey('room-settings-low-glow-room-1')),
      findsOneWidget,
    );
    expect(find.text('DAY PROFILE'), findsNothing);
    expect(find.text('SLEEP PROFILE'), findsNothing);
    expect(find.text('Motion Timeout'), findsNothing);
    expect(find.byType(Slider), findsNothing);
  });

  testWidgets('room settings Motion tab pushes day and sleep motion timeouts',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 1200));

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['matter'],
            'device_ids': ['light-1'],
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': true,
            'profile_settings': {
              'profile_overrides': {
                'sleep': {
                  'motion_timeout_secs': {'mode': 'fixed', 'value': 900},
                },
              },
            },
          },
        ],
        'mode': {
          'active': 'day',
          'configs': [
            {'mode': 'day', 'active_profile_id': 'rhythm'},
            {'mode': 'sleep', 'active_profile_id': 'sleep'},
          ],
        },
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));
    roomProvider.updateNodeMotionTimer(
      'room-1',
      MotionTimerInfo(
        motionActive: false,
        motionOwned: true,
        remainingSecs: 850,
        timeoutSecs: 900,
        receivedAt: DateTime.now(),
      ),
    );
    roomProvider.markNodeHasSensor('room-1');
    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.matter,
        deviceIds: ['light-1'],
        rhythmEnabled: true,
        disabled: false,
        lightsOn: true,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );

    await _selectRoomSettingsTab(tester, 'Motion');
    expect(find.text('Motion Timeout'), findsNWidgets(2));

    await tester.tap(find.text('10m').first);
    await tester.pump();

    expect(api.nodePreferenceCalls, isEmpty);
    expect(api.nodeProfileOverrideCalls, isEmpty);
    expect(find.byType(Slider), findsNWidgets(2));
    expect(
      provider
          .nodeById('room-1')
          ?.profileSettings
          ?.profileOverrides['rhythm']
          ?.motionTimeoutSetting,
      isNull,
    );
    expect(roomProvider.getMotionTimer('room-1')?.timeoutSecs, 900);
    expect(roomProvider.getMotionTimer('room-1')?.remainingSecs, 850);

    final daySlider = tester.widget<Slider>(find.byType(Slider).first);
    expect(daySlider.value, 600);
    expect(daySlider.onChanged, isNotNull);
    expect(daySlider.onChangeEnd, isNotNull);
    daySlider.onChanged!(720);
    await tester.pump();

    final draggedDaySlider = tester.widget<Slider>(find.byType(Slider).first);
    expect(draggedDaySlider.value, 720);
    expect(api.nodeProfileOverrideCalls, isEmpty);
    expect(
      provider
          .nodeById('room-1')
          ?.profileSettings
          ?.profileOverrides['rhythm']
          ?.motionTimeoutSetting,
      isNull,
    );

    draggedDaySlider.onChangeEnd!(720);
    await tester.pump();

    expect(
      provider
          .nodeById('room-1')
          ?.profileSettings
          ?.profileOverrides['rhythm']
          ?.motionTimeoutSetting
          ?.fixedValue,
      720,
    );
    expect(roomProvider.getMotionTimer('room-1')?.timeoutSecs, 720);
    expect(roomProvider.getMotionTimer('room-1')?.remainingSecs, 720);

    expect(api.nodeProfileOverrideCalls, hasLength(1));
    final dayCall = api.nodeProfileOverrideCalls.single;
    expect(dayCall.nodeId, 'room-1');
    expect(
      dayCall.profileOverrides,
      {
        'rhythm': {
          'motion_timeout_secs': {'mode': 'fixed', 'value': 720},
        },
      },
    );

    await tester.tap(find.text('Auto').last);
    await tester.pump();

    expect(api.nodeProfileOverrideCalls, hasLength(2));
    expect(roomProvider.getMotionTimer('room-1')?.timeoutSecs, 720);
    final sleepCall = api.nodeProfileOverrideCalls.last;
    expect(
      sleepCall.profileOverrides,
      {'sleep': null},
    );
    expect(
      provider
          .nodeById('room-1')
          ?.profileSettings
          ?.profileOverrides['sleep']
          ?.motionTimeoutSetting,
      isNull,
    );
  });

  testWidgets('Delete Room is shown for matter rooms and calls the SDK method',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi();
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.binding.setSurfaceSize(const Size(390, 1400));

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'hub_types': ['matter'],
            'device_ids': ['light-1'],
            'state': 'active',
            'rhythm_enabled': false,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'lights_on': false,
          },
        ],
        'location': const <String, dynamic>{},
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));
    await _pumpRoomSettingsSheet(
      tester,
      roomProvider: roomProvider,
      provider: provider,
      room: const RoomDto(
        id: 'room-1',
        name: 'Kitchen',
        source: RoomSourceDto.matter,
        deviceIds: ['light-1'],
        rhythmEnabled: false,
        disabled: false,
        lightsOn: false,
        timeOffsetMinutes: 0,
        brightnessOffset: 0,
      ),
    );

    expect(find.text('Delete Room'), findsNothing);

    await tester.tap(find.byKey(const ValueKey('room-settings-rename')));
    await tester.pumpAndSettle();

    expect(find.text('Rename Room'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('room-settings-delete-room')),
      findsOneWidget,
    );

    await tester.tap(find.text('Delete Room'));
    await tester.pump(const Duration(milliseconds: 250));
    await tester.tap(find.widgetWithText(TextButton, 'Delete'));
    await tester.pump(const Duration(milliseconds: 500));

    expect(api.topologyDeleteRoomCalls, 1);
    expect(api.lastDeletedRoomId, 'room-1');
    expect(connection.reconnectCalls, 1);
    expect(find.text('Deleted Kitchen'), findsOneWidget);
  });

  testWidgets(
      'global Lighting exposes Day-only Low Glow and saves profile before mode',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    final api = _FakeRhythmServerApi()
      ..profileConfigs = const [
        RhythmCurveConfig(id: 'rhythm', name: 'Day Profile'),
        RhythmCurveConfig(id: 'sleep', name: 'Sleep Profile'),
        RhythmCurveConfig(
          id: 'day_idle',
          name: 'Day Mood',
          minColorTemp: 0,
          maxColorTemp: 0,
          minBrightness: 1,
          maxBrightness: 1,
          curve: RhythmInheritActiveCurve(),
        ),
        RhythmCurveConfig(
          id: 'sleep_idle',
          name: 'Sleep Mood',
          minColorTemp: 0,
          maxColorTemp: 0,
          minBrightness: 1,
          maxBrightness: 1,
          curve: RhythmInheritActiveCurve(),
        ),
      ]
      ..profileMode = RhythmModeResource.fromJson({
        'active': 'sleep',
        'configs': [
          {'mode': 'day', 'active_profile_id': 'rhythm'},
          {'mode': 'sleep', 'active_profile_id': 'sleep'},
        ],
      });
    final screenshotPath = Platform.environment['RHYTHM_LOW_GLOW_SCREENSHOT'];
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 1100));

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': const <Map<String, dynamic>>[],
        'location': const <String, dynamic>{},
        'profiles': [
          for (final profile in api.profileConfigs) profile.toJson(),
        ],
        'mode': {
          'active': 'sleep',
          'configs': [
            {'mode': 'day', 'active_profile_id': 'rhythm'},
            {'mode': 'sleep', 'active_profile_id': 'sleep'},
          ],
        },
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));
    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const LightScreen(showBackButton: true),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Day'), findsOneWidget);
    expect(find.text('Sleep'), findsOneWidget);
    expect(find.text('Low Glow'), findsOneWidget);
    expect(find.text('Sleep Mood'), findsNothing);

    await tester.tap(find.text('Low Glow'));
    await tester.pumpAndSettle();
    expect(find.text('Brightness & Color'), findsOneWidget);
    expect(find.text('Custom Brightness'), findsOneWidget);
    expect(find.text('Custom Color'), findsOneWidget);

    if (screenshotPath != null && screenshotPath.isNotEmpty) {
      await expectLater(
        find.byType(LightScreen),
        matchesGoldenFile(screenshotPath),
      );
    }

    await tester.tap(
      find.byKey(const ValueKey('day-low-glow-custom-brightness')),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();

    expect(api.profileConfigSetCalls, hasLength(1));
    expect(api.profileConfigSetCalls.single.id, 'day_idle');
    expect(api.profileConfigSetCalls.single.config.minBrightness, 1);
    expect(api.profileModeSetCalls, hasLength(1));
    final dayConfig = api.profileModeSetCalls.single.configs!.singleWhere(
      (config) => config.mode == RhythmMode.day,
    );
    expect(dayConfig.idleProfileId, 'day_idle');

    api.profileConfigSetResult = false;
    await tester.tap(
      find.byKey(const ValueKey('day-low-glow-custom-color')),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();

    expect(api.profileConfigSetCalls, hasLength(2));
    expect(
      api.profileModeSetCalls,
      hasLength(1),
      reason: 'a rejected profile write must not update Day mode routing',
    );

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const LightScreen(
          showBackButton: true,
          roomId: 'room-1',
          roomName: 'Kitchen',
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Low Glow'), findsOneWidget);

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const LightScreen(
          showBackButton: true,
          roomId: 'bulb-1',
          roomName: 'Desk Lamp',
          overrideScope: LightOverrideScope.bulb,
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Low Glow'), findsNothing);
  });

  testWidgets(
      'room Low Glow saves an isolated day_idle override and returns to Auto',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    const profiles = [
      RhythmCurveConfig(id: 'rhythm', name: 'Day Profile'),
      RhythmCurveConfig(id: 'sleep', name: 'Sleep Profile'),
      RhythmCurveConfig(
        id: 'day_idle',
        name: 'Low Glow',
        minColorTemp: 0,
        maxColorTemp: 0,
        minBrightness: 1,
        maxBrightness: 1,
        curve: RhythmInheritActiveCurve(),
      ),
    ];
    final api = _FakeRhythmServerApi()
      ..profileConfigs = profiles
      ..profileMode = RhythmModeResource.fromJson({
        'active': 'day',
        'configs': [
          {'mode': 'day', 'active_profile_id': 'rhythm'},
          {'mode': 'sleep', 'active_profile_id': 'sleep'},
        ],
      });
    final screenshotDir =
        Platform.environment['RHYTHM_ROOM_LOW_GLOW_SCREENSHOT_DIR'];
    Future<void> captureState(String name) async {
      if (screenshotDir == null || screenshotDir.isEmpty) return;
      await expectLater(
        find.byType(LightScreen),
        matchesGoldenFile('$screenshotDir/$name.png'),
      );
    }

    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 1400));

    connection.emitHello(
      RhythmHello.fromJson({
        'version': '0.6.590-beta',
        'capabilities': {
          'api_schema_version': 2,
          'features': [
            RhythmFeature.roomLightProfileOverrides,
            RhythmFeature.roomDayIdleProfileOverrides,
          ],
          'hubs': const <dynamic>[],
        },
        'nodes': [
          {
            'id': 'room-1',
            'name': 'Kitchen',
            'kind': 'room',
            'state': 'active',
            'rhythm_enabled': true,
            'disabled': false,
            'time_offset': 0.0,
            'brightness_offset': 0.0,
            'profile_settings': {
              'profile_overrides': {
                'sleep': {
                  'min_brightness': 7,
                },
              },
            },
          },
        ],
        'profiles': [for (final profile in profiles) profile.toJson()],
        'mode': {
          'active': 'day',
          'configs': [
            {'mode': 'day', 'active_profile_id': 'rhythm'},
            {'mode': 'sleep', 'active_profile_id': 'sleep'},
          ],
        },
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));
    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const LightScreen(
          showBackButton: true,
          roomId: 'room-1',
          roomName: 'Kitchen',
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Low Glow'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('room-light-layer-custom-day_idle')),
      findsNothing,
    );
    await tester.tap(find.text('Low Glow'));
    await tester.pumpAndSettle();
    expect(find.text('Auto · 1%'), findsOneWidget);
    expect(
      tester
          .getSemantics(
            find.byKey(const ValueKey('light-layer-preview-day_idle')),
          )
          .label,
      contains('Auto, 1 percent, Day color'),
    );
    await captureState('room-low-glow-inherited');
    await tester.tap(
      find.byKey(const ValueKey('day-low-glow-custom-brightness')),
    );
    await tester.pumpAndSettle();
    expect(find.text('Custom · 1%'), findsOneWidget);
    await tester.tap(
      find.byKey(const ValueKey('day-low-glow-custom-brightness')),
    );
    await tester.pumpAndSettle();
    expect(find.text('Auto · 1%'), findsOneWidget);
    expect(
      tester
          .getSemantics(
            find.byKey(const ValueKey('light-layer-preview-day_idle')),
          )
          .label,
      contains('Auto, 1 percent, Day color'),
    );
    await tester.tap(
      find.byKey(const ValueKey('day-low-glow-custom-brightness')),
    );
    await tester.pumpAndSettle();
    await captureState('room-low-glow-custom');
    tester
        .widget<Slider>(
          find.byKey(const ValueKey('day-low-glow-brightness')),
        )
        .onChanged!(80);
    await tester.pumpAndSettle();
    await tester.tap(
      find.byKey(const ValueKey('day-low-glow-custom-color')),
    );
    await tester.pumpAndSettle();
    final spectrum = find.byKey(
      const ValueKey('day-low-glow-color-spectrum'),
    );
    final spectrumWidth = tester.getSize(spectrum).width;
    final spectrumGesture = tester.widget<GestureDetector>(spectrum);
    spectrumGesture.onTapDown!(
      TapDownDetails(localPosition: Offset(spectrumWidth * 0.08, 12)),
    );
    await tester.pumpAndSettle();
    expect(find.text('Custom · 80%'), findsOneWidget);
    await captureState('room-low-glow-warm-bright');
    spectrumGesture.onTapDown!(
      TapDownDetails(localPosition: Offset(spectrumWidth * 0.62, 12)),
    );
    await tester.pumpAndSettle();
    expect(
      tester
          .getSemantics(
            find.byKey(const ValueKey('light-layer-preview-day_idle')),
          )
          .label,
      contains('Custom, 80 percent, custom color'),
    );
    await captureState('room-low-glow-cool-bright');
    final pendingSave = Completer<bool>();
    api.nodeProfileOverridesCompleter = pendingSave;
    await tester.tap(find.text('Save'));
    await tester.pump();
    await captureState('room-low-glow-pending');
    pendingSave.complete(true);
    await tester.pumpAndSettle();
    api.nodeProfileOverridesCompleter = null;

    expect(api.profileConfigSetCalls, isEmpty);
    expect(api.profileModeSetCalls, isEmpty);
    expect(api.nodeProfileOverrideCalls, hasLength(1));
    expect(api.nodeProfileOverrideCalls.single.nodeId, 'room-1');
    expect(
      api.nodeProfileOverrideCalls.single.profileOverrides?.keys,
      contains('day_idle'),
    );
    expect(
      provider
          .nodeById('room-1')
          ?.profileSettings
          ?.profileOverrides
          .containsKey('sleep'),
      isTrue,
      reason: 'saving Low Glow must retain unrelated room overrides',
    );

    expect(
      await provider.setNodeLightProfileOverride(
        'room-1',
        profileId: 'day_idle',
        profileOverride: null,
        correlationId: 'room-low-glow-auto',
      ),
      isTrue,
    );
    await tester.pumpAndSettle();
    await tester.pump(const Duration(seconds: 5));

    expect(api.nodeProfileOverrideCalls, hasLength(2));
    expect(api.nodeProfileOverrideCalls.last.profileOverrides, {
      'day_idle': null,
    });
    expect(
      provider
          .nodeById('room-1')
          ?.profileSettings
          ?.profileOverrides
          .containsKey('day_idle'),
      isFalse,
    );
    expect(
      provider
          .nodeById('room-1')
          ?.profileSettings
          ?.profileOverrides
          .containsKey('sleep'),
      isTrue,
    );

    await tester.tap(
      find.byKey(const ValueKey('day-low-glow-custom-brightness')),
    );
    await tester.pumpAndSettle();
    api.nodeProfileOverridesSucceeds = false;
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();
    await captureState('room-low-glow-failed-retry');
    expect(api.nodeProfileOverrideCalls, hasLength(3));
    expect(
      provider
          .nodeById('room-1')
          ?.profileSettings
          ?.profileOverrides
          .containsKey('day_idle'),
      isFalse,
      reason: 'a rejected retry must restore the authoritative Auto state',
    );

    api.nodeProfileOverridesSucceeds = true;
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();
    expect(api.nodeProfileOverrideCalls, hasLength(4));
  });

  testWidgets('returning Low Glow to Auto only clears the Day idle mapping',
      (tester) async {
    _registerWidgetCleanup(tester);
    final roomProvider = RoomProvider();
    const initialProfiles = [
      RhythmCurveConfig(id: 'rhythm', name: 'Day Profile'),
      RhythmCurveConfig(id: 'sleep', name: 'Sleep Profile'),
      RhythmCurveConfig(
        id: 'day_idle',
        name: 'Day Mood',
        minColorTemp: 0,
        maxColorTemp: 0,
        minBrightness: 20,
        maxBrightness: 20,
        curve: RhythmConstantCurve(
          brightness: 1,
          colorTemp: 0,
          directColor: RhythmDirectColor(
            rgb: RhythmRgbColor(r: 255, g: 149, b: 41),
            xy: RhythmXyColor(x: 0.61, y: 0.37),
          ),
        ),
      ),
      RhythmCurveConfig(
        id: 'sleep_idle',
        name: 'Sleep Mood',
        minColorTemp: 0,
        maxColorTemp: 0,
        minBrightness: 1,
        maxBrightness: 1,
        curve: RhythmInheritActiveCurve(),
      ),
    ];
    final api = _FakeRhythmServerApi()
      ..profileConfigs = initialProfiles
      ..profileMode = RhythmModeResource.fromJson({
        'active': 'sleep',
        'configs': [
          {
            'mode': 'day',
            'active_profile_id': 'rhythm',
            'idle_profile_id': 'day_idle',
          },
          {
            'mode': 'sleep',
            'active_profile_id': 'sleep',
            'idle_profile_id': 'sleep_idle',
          },
        ],
      });
    final connection = _HelloRhythmConnection(api);
    final provider = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _TestHomeProvider(const []),
    );
    addTearDown(provider.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(connection.dispose);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.binding.setSurfaceSize(const Size(390, 1100));

    connection.emitHello(
      RhythmHello.fromJson({
        'nodes': const <Map<String, dynamic>>[],
        'location': const <String, dynamic>{},
        'profiles': [for (final profile in initialProfiles) profile.toJson()],
        'mode': {
          'active': 'sleep',
          'configs': [
            {
              'mode': 'day',
              'active_profile_id': 'rhythm',
              'idle_profile_id': 'day_idle',
            },
            {
              'mode': 'sleep',
              'active_profile_id': 'sleep',
              'idle_profile_id': 'sleep_idle',
            },
          ],
        },
      }),
    );
    await tester.pump(const Duration(milliseconds: 10));
    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const LightScreen(showBackButton: true),
      ),
    );
    await tester.pumpAndSettle();

    await tester.tap(find.text('Low Glow'));
    await tester.pumpAndSettle();
    await tester.tap(
      find.byKey(const ValueKey('day-low-glow-custom-brightness')),
    );
    await tester.pumpAndSettle();
    await tester.tap(
      find.byKey(const ValueKey('day-low-glow-custom-color')),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();

    expect(
      api.profileConfigSetCalls,
      isEmpty,
      reason: 'returning to Auto must not rewrite or remove stored profiles',
    );
    expect(api.profileModeSetCalls, hasLength(1));
    final savedConfigs = api.profileModeSetCalls.single.configs!;
    final savedDay = savedConfigs.singleWhere(
      (config) => config.mode == RhythmMode.day,
    );
    final savedSleep = savedConfigs.singleWhere(
      (config) => config.mode == RhythmMode.sleep,
    );
    expect(savedDay.activeProfileId, 'rhythm');
    expect(savedDay.idleProfileId, isNull);
    expect(savedSleep.activeProfileId, 'sleep');
    expect(savedSleep.idleProfileId, 'sleep_idle');
    expect(api.profileConfigs, initialProfiles);

    Map<String, dynamic> roomHello({required bool explicitDayIdle}) => {
          'version': '0.6.590-beta',
          'capabilities': {
            'api_schema_version': 2,
            'features': [
              RhythmFeature.roomLightProfileOverrides,
              RhythmFeature.roomDayIdleProfileOverrides,
            ],
            'hubs': const <dynamic>[],
          },
          'nodes': [
            {
              'id': 'inheriting-room',
              'name': 'Kitchen',
              'kind': 'room',
              'state': 'standby',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
            },
            {
              'id': 'custom-room',
              'name': 'Nursery',
              'kind': 'room',
              'state': 'standby',
              'rhythm_enabled': true,
              'disabled': false,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'profile_settings': {
                'profile_overrides': {
                  'day_idle': {
                    'min_brightness': 7,
                    'max_brightness': 7,
                  },
                },
              },
            },
          ],
          'profiles': [for (final profile in initialProfiles) profile.toJson()],
          'mode': {
            'active': 'day',
            'configs': [
              {
                'mode': 'day',
                'active_profile_id': 'rhythm',
                if (explicitDayIdle) 'idle_profile_id': 'day_idle',
              },
              {
                'mode': 'sleep',
                'active_profile_id': 'sleep',
                'idle_profile_id': 'sleep_idle',
              },
            ],
          },
        };

    connection.emitHello(
      RhythmHello.fromJson(roomHello(explicitDayIdle: false)),
    );
    await tester.pump(const Duration(milliseconds: 10));
    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const LightScreen(
          key: ValueKey('inheriting-auto-room'),
          roomId: 'inheriting-room',
          roomName: 'Kitchen',
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Auto · 1%'), findsOneWidget);
    expect(
      tester
          .getSemantics(
            find.byKey(const ValueKey('light-layer-preview-day_idle')),
          )
          .label,
      contains('Auto, 1 percent, Day color'),
    );
    await tester.tap(find.text('Low Glow'));
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<Switch>(
            find.byKey(
              const ValueKey('day-low-glow-custom-brightness'),
            ),
          )
          .value,
      isFalse,
      reason: 'the room editor must ignore the stale stored custom profile',
    );

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const LightScreen(
          key: ValueKey('custom-auto-room'),
          roomId: 'custom-room',
          roomName: 'Nursery',
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Custom · 7%'), findsOneWidget);
    expect(
      tester
          .getSemantics(
            find.byKey(const ValueKey('light-layer-preview-day_idle')),
          )
          .label,
      contains('Custom, 7 percent, Day color'),
    );

    api.profileMode = RhythmModeResource.fromJson(
      roomHello(explicitDayIdle: true)['mode'] as Map<String, dynamic>,
    );
    connection.emitHello(
      RhythmHello.fromJson(roomHello(explicitDayIdle: true)),
    );
    await tester.pump(const Duration(milliseconds: 10));
    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const LightScreen(
          key: ValueKey('inheriting-explicit-room'),
          roomId: 'inheriting-room',
          roomName: 'Kitchen',
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Auto · 20%'), findsOneWidget);
    expect(
      tester
          .getSemantics(
            find.byKey(const ValueKey('light-layer-preview-day_idle')),
          )
          .label,
      contains('Auto, 20 percent, custom color'),
    );
    await tester.tap(find.text('Low Glow'));
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<Switch>(
            find.byKey(
              const ValueKey('day-low-glow-custom-brightness'),
            ),
          )
          .value,
      isTrue,
      reason: 'an explicit mapping must load the stored custom profile',
    );

    await tester.pumpWidget(
      _buildTestApp(
        roomProvider: roomProvider,
        provider: provider,
        child: const LightScreen(
          key: ValueKey('custom-explicit-room'),
          roomId: 'custom-room',
          roomName: 'Nursery',
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Custom · 7%'), findsOneWidget);
    expect(
      tester
          .getSemantics(
            find.byKey(const ValueKey('light-layer-preview-day_idle')),
          )
          .label,
      contains('Custom, 7 percent, custom color'),
    );
    await tester.tap(find.text('Low Glow'));
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<Slider>(
            find.byKey(const ValueKey('day-low-glow-brightness')),
          )
          .value,
      7,
      reason: 'the room delta must remain isolated atop explicit inheritance',
    );
    expect(api.profileConfigs, initialProfiles);
  });
}
