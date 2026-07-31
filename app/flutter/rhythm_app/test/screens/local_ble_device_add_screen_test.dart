import 'dart:async';

import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_app/screens/hubs/local_ble_device_add_screen.dart';
import 'package:rhythm_app/services/analytics_service.dart';
import 'package:rhythm_app/services/device_pairing_code.dart';
import 'package:rhythm_app/services/settings_service.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../helpers/capturing_analytics_backend.dart';

class _FakePendingPairingStore implements LocalBlePendingPairingStore {
  _FakePendingPairingStore({
    this.pending,
    this.clearSucceeds = true,
    this.loadFails = false,
    this.saveGate,
  });

  PendingLocalBlePairing? pending;
  final bool clearSucceeds;
  final bool loadFails;
  final Completer<void>? saveGate;
  final saved = <PendingLocalBlePairing>[];
  final cleared = <String?>[];

  @override
  Future<PendingLocalBlePairing?> load() async {
    if (loadFails) throw const FormatException('corrupt pointer');
    return pending;
  }

  @override
  Future<bool> save(PendingLocalBlePairing pairing) async {
    pending = pairing;
    saved.add(pairing);
    await saveGate?.future;
    return true;
  }

  @override
  Future<bool> clear({String? sessionId}) async {
    cleared.add(sessionId);
    if (!clearSucceeds) return false;
    if (sessionId == null || pending?.sessionId == sessionId) pending = null;
    return true;
  }
}

class _PairingApi extends RhythmServerApi {
  _PairingApi() : super(Dio());

  int pairRequests = 0;
  int statusRequests = 0;
  int acknowledgementRequests = 0;

  @override
  Future<Map<String, dynamic>?> pairDevice({
    required String hubType,
    Map<String, dynamic> params = const {},
    Duration receiveTimeout = const Duration(seconds: 45),
    String? sessionId,
  }) async {
    pairRequests += 1;
    return null;
  }

  @override
  Future<RhythmPairingResultStatus?> getPairingResult(String sessionId) async {
    statusRequests += 1;
    return null;
  }

  @override
  Future<bool> acknowledgePairingResult(String sessionId) async {
    acknowledgementRequests += 1;
    return true;
  }
}

class _PairingConnection extends RhythmConnection {
  _PairingConnection(this._api);

  final RhythmServerApi _api;

  @override
  RhythmServerApi get api => _api;
}

class _MutableServerSyncProvider extends ServerSyncProvider {
  _MutableServerSyncProvider({
    required super.connection,
    required super.roomProvider,
    required super.homeProvider,
    required Hub hub,
  }) : _hub = hub;

  final Hub _hub;
  String? _serverInstanceId;

  @override
  Hub? get connectedServerHub => _hub;

  @override
  String? get connectedServerInstanceId => _serverInstanceId;

  void authenticateAs(String serverInstanceId) {
    _serverInstanceId = serverInstanceId;
    notifyListeners();
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const qr = 'B:0A0B0C0D0E0F%G\$S:SYNTHETIC000001\$M:TESTMODEL001';
  late LocalBleSetup setup;
  late CapturingAnalyticsBackend analytics;

  setUp(() async {
    setup = parseLocalBleSetupCode(qr)!;
    analytics = CapturingAnalyticsBackend();
    await analytics.initialize();
    BackendProvider.setInstanceForTesting(
      auth: OfflineAuthBackend(),
      analytics: analytics,
    );
    await AnalyticsService().initialize();
  });
  tearDown(BackendProvider.resetForTesting);

  test('fallback route latches first server instance and rejects replacement',
      () {
    const fallbackScope = 'home-1:hub-record-1';
    var evaluation = evaluatePinnedLocalBleServerScope(
      routeScope: fallbackScope,
      routeUsesFallbackIdentity: true,
      currentFallbackScope: fallbackScope,
      currentCanonicalScope: fallbackScope,
      currentHasInstanceIdentity: false,
    );
    expect(evaluation.matches, isFalse);
    expect(evaluation.latchedInstanceScope, isNull);

    evaluation = evaluatePinnedLocalBleServerScope(
      routeScope: fallbackScope,
      routeUsesFallbackIdentity: true,
      currentFallbackScope: fallbackScope,
      currentCanonicalScope: 'home-1:server-instance-a',
      currentHasInstanceIdentity: true,
      latchedInstanceScope: evaluation.latchedInstanceScope,
    );
    expect(evaluation.matches, isTrue);
    expect(evaluation.latchedInstanceScope, 'home-1:server-instance-a');

    final replacement = evaluatePinnedLocalBleServerScope(
      routeScope: fallbackScope,
      routeUsesFallbackIdentity: true,
      currentFallbackScope: fallbackScope,
      currentCanonicalScope: 'home-1:server-instance-b',
      currentHasInstanceIdentity: true,
      latchedInstanceScope: evaluation.latchedInstanceScope,
    );
    expect(replacement.matches, isFalse);

    final identityMissingAgain = evaluatePinnedLocalBleServerScope(
      routeScope: fallbackScope,
      routeUsesFallbackIdentity: true,
      currentFallbackScope: fallbackScope,
      currentCanonicalScope: fallbackScope,
      currentHasInstanceIdentity: false,
      latchedInstanceScope: evaluation.latchedInstanceScope,
    );
    expect(identityMissingAgain.matches, isFalse);
  });

  testWidgets('pre-hello route persists a new operation under durable hello A',
      (tester) async {
    final api = _PairingApi();
    final connection = _PairingConnection(api);
    final roomProvider = RoomProvider();
    final homeProvider = HomeProvider();
    final hub = Hub.create(
      id: 'hub-record-1',
      homeId: 'home-1',
      type: HubType.server,
      name: 'Rhythm Box',
      endpoint: const HubEndpoint(host: '192.0.2.10', port: 54448),
      serverInstanceId: 'endpoint:http://192.0.2.10:54448',
    );
    final serverSync = _MutableServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
      hub: hub,
    );
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(homeProvider.dispose);
    final createdStores = <String, _FakePendingPairingStore>{};

    await tester.pumpWidget(
      ChangeNotifierProvider<ServerSyncProvider>.value(
        value: serverSync,
        child: MaterialApp(
          home: LocalBleDeviceAddScreen(
            setup: setup,
            inputMethod: 'camera',
            journeyId: 'journey-durable-scope-a',
            pendingPairingStoreFactory: (scope) =>
                createdStores.putIfAbsent(scope, _FakePendingPairingStore.new),
            pairingDeadline: const Duration(hours: 1),
            statusPollInterval: const Duration(days: 1),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(createdStores, isEmpty,
        reason: 'the live fallback route never creates durable storage');

    serverSync.authenticateAs('SERVER-A');
    await tester.pump();
    await tester.tap(find.text('Find Device'));
    await tester.pump();
    await tester.pump();

    expect(createdStores.keys, {'home-1:server-a'});
    final store = createdStores['home-1:server-a']!;
    expect(store.saved, hasLength(1));
    expect(store.saved.single.serverScope, 'home-1:server-a');
    expect(api.pairRequests, 1);
    expect(
      LocalBlePairingRouteOwnership.ownedKeys.value,
      contains(
        LocalBlePairingRouteOwnership.key(
          'home-1:server-a',
          'journey-durable-scope-a',
        ),
      ),
    );
    expect(
      LocalBlePairingRouteOwnership.ownedKeys.value,
      isNot(
        contains(
          LocalBlePairingRouteOwnership.key(
            'home-1:hub-record-1',
            'journey-durable-scope-a',
          ),
        ),
      ),
    );

    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pump();
  });

  testWidgets(
      'replacement B before POST cannot poll or acknowledge pointer for A',
      (tester) async {
    final api = _PairingApi();
    final connection = _PairingConnection(api);
    final roomProvider = RoomProvider();
    final homeProvider = HomeProvider();
    final hub = Hub.create(
      id: 'hub-record-1',
      homeId: 'home-1',
      type: HubType.server,
      name: 'Rhythm Box',
      endpoint: const HubEndpoint(host: '192.0.2.10', port: 54448),
      serverInstanceId: 'endpoint:http://192.0.2.10:54448',
    );
    final serverSync = _MutableServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
      hub: hub,
    );
    addTearDown(serverSync.dispose);
    addTearDown(connection.dispose);
    addTearDown(roomProvider.dispose);
    addTearDown(homeProvider.dispose);
    final saveGate = Completer<void>();
    final createdStores = <String, _FakePendingPairingStore>{};

    await tester.pumpWidget(
      ChangeNotifierProvider<ServerSyncProvider>.value(
        value: serverSync,
        child: MaterialApp(
          home: LocalBleDeviceAddScreen(
            setup: setup,
            inputMethod: 'camera',
            journeyId: 'journey-replacement-after-save',
            pendingPairingStoreFactory: (scope) => createdStores.putIfAbsent(
              scope,
              () => _FakePendingPairingStore(saveGate: saveGate),
            ),
            pairingDeadline: const Duration(hours: 1),
            statusPollInterval: const Duration(days: 1),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    serverSync.authenticateAs('SERVER-A');
    await tester.pump();
    await tester.tap(find.text('Find Device'));
    await tester.pump();

    final store = createdStores['home-1:server-a']!;
    expect(store.saved.single.serverScope, 'home-1:server-a');
    expect(api.pairRequests, 0);

    serverSync.authenticateAs('SERVER-B');
    await tester.pump();
    saveGate.complete();
    await tester.pump();
    await tester.pump();
    await tester.pump();

    expect(createdStores.keys, {'home-1:server-a'});
    expect(store.pending?.serverScope, 'home-1:server-a');
    expect(store.cleared, isEmpty);
    expect(api.pairRequests, 0);
    expect(api.statusRequests, 0);
    expect(api.acknowledgementRequests, 0);

    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pump();
  });

  testWidgets('unreadable recovery pointer blocks a new pairing admission',
      (tester) async {
    final store = _FakePendingPairingStore(loadFails: true);
    var pairingRequests = 0;
    await tester.pumpWidget(
      MaterialApp(
        home: LocalBleDeviceAddScreen(
          setup: setup,
          inputMethod: 'camera',
          journeyId: 'journey-corrupt-pointer',
          pendingPairingStore: store,
          pairingRequest: ({
            required hubType,
            required params,
            required receiveTimeout,
            required sessionId,
          }) async {
            pairingRequests += 1;
            return null;
          },
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(
      find.textContaining('could not read the saved pairing recovery record'),
      findsOneWidget,
    );
    final button = tester.widget<FilledButton>(
      find.byKey(const ValueKey('check-local-ble-pairing-status')),
    );
    expect(button.onPressed, isNull);
    expect(pairingRequests, 0);
    expect(store.saved, isEmpty);
  });

  testWidgets('submits the canonical profile while retaining parsed fields',
      (tester) async {
    const canonicalProfileId = 'orein.oc02001.button.v2';
    String? capturedHubType;
    Map<String, dynamic>? capturedParams;
    Duration? capturedTimeout;
    RhythmPairedDevice? result;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await Navigator.of(context).push(
                MaterialPageRoute<RhythmPairedDevice>(
                  builder: (_) => LocalBleDeviceAddScreen(
                    setup: setup,
                    pairingProfileId: canonicalProfileId,
                    inputMethod: 'manual_code',
                    analyticsSource: 'test',
                    journeyId: 'journey-1',
                    pairingRequest: ({
                      required hubType,
                      required params,
                      required receiveTimeout,
                      required sessionId,
                    }) async {
                      capturedHubType = hubType;
                      capturedParams = params;
                      capturedTimeout = receiveTimeout;
                      return {
                        'hub_type': 'local_ble',
                        'status': 'complete',
                        'device': {
                          'device_id': 'local-ble-button-1',
                          'name': 'Button',
                          'device_type': 'button',
                        },
                      };
                    },
                  ),
                ),
              );
            },
            child: const Text('Open'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();

    expect(find.text('Put the device in pairing mode'), findsOneWidget);
    expect(find.text('Find Device'), findsOneWidget);
    expect(find.text(qr), findsNothing);
    expect(capturedParams, isNull);

    await tester.tap(find.text('Find Device'));
    await tester.pumpAndSettle();

    expect(capturedHubType, 'local_ble');
    expect(setup.profileId, RhythmDeviceProfileId.oreinOc02001Button);
    expect(capturedParams, {
      ...setup.pairingParams,
      'profile_id': canonicalProfileId,
    });
    expect(capturedParams.toString(), isNot(contains(qr)));
    expect(capturedTimeout, const Duration(seconds: 135));
    expect(result?.deviceId, 'local-ble-button-1');
    final pairingEvents = analytics.events.where(
      (event) => event.name.startsWith('local_ble_pairing_'),
    );
    expect(pairingEvents, hasLength(2));
    for (final event in pairingEvents) {
      expect(event.properties['journey_id'], 'journey-1');
      expect(event.properties['input_method'], 'manual_code');
      expect(
        event.properties['profile_id'],
        canonicalProfileId,
      );
      expect(event.properties.values, isNot(contains(qr)));
      expect(
        event.properties.values,
        isNot(contains(setup.setupFields['ble_identity'])),
      );
    }
  });

  testWidgets('blocks dismissal and reconciles a correlated terminal event',
      (tester) async {
    final request = Completer<Map<String, dynamic>?>();
    final progress = StreamController<RhythmPairingProgress>();
    addTearDown(progress.close);
    RhythmPairedDevice? result;

    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await Navigator.of(context).push(
                MaterialPageRoute<RhythmPairedDevice>(
                  builder: (_) => LocalBleDeviceAddScreen(
                    setup: setup,
                    inputMethod: 'camera',
                    journeyId: 'journey-2',
                    pairingRequest: ({
                      required hubType,
                      required params,
                      required receiveTimeout,
                      required sessionId,
                    }) =>
                        request.future,
                    progressEvents: progress.stream,
                  ),
                ),
              );
            },
            child: const Text('Open'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Find Device'));
    await tester.pump();

    await tester.binding.handlePopRoute();
    await tester.pump();
    expect(
        find.byKey(const ValueKey('local-ble-pairing-screen')), findsOneWidget);
    expect(
      find.textContaining('Pairing status is still being checked'),
      findsOneWidget,
    );

    progress.add(
      const RhythmPairingProgress(
        hubType: 'local_ble',
        sessionId: 'journey-2',
        status: RhythmPairingStatus.complete,
        stage: RhythmPairingStage.complete,
        message: 'Complete',
        device: RhythmPairedDevice(
          deviceId: 'local-ble-button-2',
          name: 'Button',
          deviceType: 'button',
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(result?.deviceId, 'local-ble-button-2');
    request.complete(null);
    await tester.pump();
  });

  testWidgets('late SSE success wins after an early request exception',
      (tester) async {
    final progress = StreamController<RhythmPairingProgress>();
    addTearDown(progress.close);
    RhythmPairedDevice? result;

    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await Navigator.of(context).push(
                MaterialPageRoute<RhythmPairedDevice>(
                  builder: (_) => LocalBleDeviceAddScreen(
                    setup: setup,
                    inputMethod: 'camera',
                    journeyId: 'journey-3',
                    pairingRequest: ({
                      required hubType,
                      required params,
                      required receiveTimeout,
                      required sessionId,
                    }) async {
                      throw StateError('sensitive transport detail');
                    },
                    progressEvents: progress.stream,
                  ),
                ),
              );
            },
            child: const Text('Open'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Find Device'));
    await tester.pump();

    expect(
      find.textContaining('Waiting for final confirmation'),
      findsOneWidget,
    );
    expect(find.textContaining('sensitive transport detail'), findsNothing);
    final pairingButton = tester.widget<FilledButton>(
      find.byKey(const ValueKey('find-local-ble-device')),
    );
    expect(pairingButton.onPressed, isNull);

    await tester.binding.handlePopRoute();
    await tester.pump();
    expect(
      find.byKey(const ValueKey('local-ble-pairing-screen')),
      findsOneWidget,
    );

    progress.add(
      const RhythmPairingProgress(
        hubType: 'local_ble',
        sessionId: 'journey-3',
        status: RhythmPairingStatus.complete,
        stage: RhythmPairingStage.complete,
        message: 'Complete',
        device: RhythmPairedDevice(
          deviceId: 'local-ble-button-3',
          name: 'Button',
          deviceType: 'button',
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(result?.deviceId, 'local-ble-button-3');
    final pairingEvents = analytics.events
        .where((event) => event.name.startsWith('local_ble_pairing_'))
        .toList(growable: false);
    expect(pairingEvents, hasLength(2));
    expect(pairingEvents.last.properties['outcome'], 'succeeded');
    for (final event in pairingEvents) {
      expect(
        event.properties.values,
        isNot(contains('sensitive transport detail')),
      );
    }
  });

  testWidgets('late SSE success wins after an early gateway timeout',
      (tester) async {
    final progress = StreamController<RhythmPairingProgress>();
    addTearDown(progress.close);
    RhythmPairedDevice? result;

    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await Navigator.of(context).push(
                MaterialPageRoute<RhythmPairedDevice>(
                  builder: (_) => LocalBleDeviceAddScreen(
                    setup: setup,
                    inputMethod: 'camera',
                    journeyId: 'journey-504',
                    pairingRequest: ({
                      required hubType,
                      required params,
                      required receiveTimeout,
                      required sessionId,
                    }) async =>
                        {
                      'http_status': 504,
                      'error': 'sensitive gateway detail',
                    },
                    progressEvents: progress.stream,
                  ),
                ),
              );
            },
            child: const Text('Open'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Find Device'));
    await tester.pump();

    expect(
      find.textContaining('Waiting for final confirmation'),
      findsOneWidget,
    );
    expect(find.textContaining('sensitive gateway detail'), findsNothing);
    expect(
      tester
          .widget<FilledButton>(
            find.byKey(const ValueKey('find-local-ble-device')),
          )
          .onPressed,
      isNull,
    );

    progress.add(
      const RhythmPairingProgress(
        hubType: 'local_ble',
        sessionId: 'journey-504',
        status: RhythmPairingStatus.complete,
        stage: RhythmPairingStage.complete,
        message: 'Complete',
        device: RhythmPairedDevice(
          deviceId: 'local-ble-button-504',
          name: 'Button',
          deviceType: 'button',
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(result?.deviceId, 'local-ble-button-504');
    final pairingEvents = analytics.events
        .where((event) => event.name.startsWith('local_ble_pairing_'))
        .toList(growable: false);
    expect(pairingEvents, hasLength(2));
    expect(pairingEvents.last.properties['outcome'], 'succeeded');
  });

  testWidgets('definitive HTTP conflict unlocks immediately', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () {
              Navigator.of(context).push(
                MaterialPageRoute<RhythmPairedDevice>(
                  builder: (_) => LocalBleDeviceAddScreen(
                    setup: setup,
                    inputMethod: 'manual_code',
                    journeyId: 'journey-409',
                    pairingRequest: ({
                      required hubType,
                      required params,
                      required receiveTimeout,
                      required sessionId,
                    }) async =>
                        {
                      'http_status': 409,
                      'error': 'conflict detail',
                    },
                  ),
                ),
              );
            },
            child: const Text('Open'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Find Device'));
    await tester.pump();

    expect(
      find.byKey(const ValueKey('local-ble-pairing-error')),
      findsOneWidget,
    );
    expect(
      tester
          .widget<FilledButton>(
            find.byKey(const ValueKey('find-local-ble-device')),
          )
          .onPressed,
      isNotNull,
    );
    final completed = analytics.events.singleWhere(
      (event) => event.name == 'local_ble_pairing_completed',
    );
    expect(completed.properties['outcome'], 'failed');
    expect(completed.properties['failure_stage'], 'server_rejected');

    await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();
  });

  testWidgets('ambiguous response remains recoverable after route deadline',
      (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () {
              Navigator.of(context).push(
                MaterialPageRoute<RhythmPairedDevice>(
                  builder: (_) => LocalBleDeviceAddScreen(
                    setup: setup,
                    inputMethod: 'manual_code',
                    journeyId: 'journey-4',
                    pairingDeadline: const Duration(seconds: 1),
                    pairingRequest: ({
                      required hubType,
                      required params,
                      required receiveTimeout,
                      required sessionId,
                    }) async =>
                        null,
                  ),
                ),
              );
            },
            child: const Text('Open'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Find Device'));
    await tester.pump();

    await tester.pump(const Duration(milliseconds: 999));
    expect(
      find.byKey(const ValueKey('local-ble-pairing-error')),
      findsNothing,
    );
    expect(
      tester
          .widget<FilledButton>(
            find.byKey(const ValueKey('find-local-ble-device')),
          )
          .onPressed,
      isNull,
    );

    await tester.pump(const Duration(milliseconds: 1));
    expect(
      find.byKey(const ValueKey('local-ble-pairing-unresolved')),
      findsOneWidget,
    );
    expect(
      tester
          .widget<FilledButton>(
            find.byKey(
              const ValueKey('check-local-ble-pairing-status'),
            ),
          )
          .onPressed,
      isNotNull,
    );
    expect(
      analytics.events.where(
        (event) => event.name == 'local_ble_pairing_completed',
      ),
      isEmpty,
    );

    await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();
  });

  testWidgets('polls durable status after an ambiguous response',
      (tester) async {
    final store = _FakePendingPairingStore();
    RhythmPairedDevice? result;
    var statusCalls = 0;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await Navigator.of(context).push(
                MaterialPageRoute<RhythmPairedDevice>(
                  builder: (_) => LocalBleDeviceAddScreen(
                    setup: setup,
                    inputMethod: 'manual_code',
                    journeyId: 'journey-poll',
                    pendingPairingStore: store,
                    pairingRequest: ({
                      required hubType,
                      required params,
                      required receiveTimeout,
                      required sessionId,
                    }) async =>
                        null,
                    pairingStatusRequest: (sessionId) async {
                      statusCalls += 1;
                      return const RhythmPairingResultStatus(
                        sessionId: 'journey-poll',
                        state: RhythmPairingResultState.terminal,
                        hubType: 'local_ble',
                        result: RhythmPairingSessionResult(
                          hubType: 'local_ble',
                          status: RhythmPairingStatus.complete,
                          device: RhythmPairedDevice(
                            deviceId: 'local-ble-polled',
                            name: 'Button',
                            deviceType: 'button',
                          ),
                        ),
                      );
                    },
                    statusPollInterval: const Duration(milliseconds: 20),
                  ),
                ),
              );
            },
            child: const Text('Open'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Find Device'));
    await tester.pumpAndSettle();

    expect(result?.deviceId, 'local-ble-polled');
    expect(statusCalls, greaterThanOrEqualTo(1));
    expect(store.saved.first.sessionId, 'journey-poll');
    expect(store.saved.first.profileId, setup.profileId);
    expect(
      store.saved.last.terminalResult?.status,
      PendingLocalBleTerminalResult.complete,
    );
    expect(store.cleared, contains('journey-poll'));
  });

  testWidgets('reopened screen reconciles a recent persisted session',
      (tester) async {
    final store = _FakePendingPairingStore(
      pending: PendingLocalBlePairing(
        sessionId: 'journey-before-restart',
        journeyId: 'original-journey-before-restart',
        attemptNumber: 3,
        profileId: setup.profileId,
        serverScope: 'injected-server',
        startedAtEpochMs: DateTime.now().millisecondsSinceEpoch,
      ),
    );
    RhythmPairedDevice? result;
    var postCalls = 0;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await Navigator.of(context).push(
                MaterialPageRoute<RhythmPairedDevice>(
                  builder: (_) => LocalBleDeviceAddScreen(
                    setup: setup,
                    inputMethod: 'camera',
                    journeyId: 'new-journey-after-restart',
                    pendingPairingStore: store,
                    pairingRequest: ({
                      required hubType,
                      required params,
                      required receiveTimeout,
                      required sessionId,
                    }) async {
                      postCalls += 1;
                      return null;
                    },
                    pairingStatusRequest: (sessionId) async {
                      expect(sessionId, 'journey-before-restart');
                      return const RhythmPairingResultStatus(
                        sessionId: 'journey-before-restart',
                        state: RhythmPairingResultState.terminal,
                        hubType: 'local_ble',
                        result: RhythmPairingSessionResult(
                          hubType: 'local_ble',
                          status: RhythmPairingStatus.complete,
                          device: RhythmPairedDevice(
                            deviceId: 'local-ble-after-restart',
                            name: 'Button',
                            deviceType: 'button',
                          ),
                        ),
                      );
                    },
                    statusPollInterval: const Duration(milliseconds: 20),
                  ),
                ),
              );
            },
            child: const Text('Open'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();

    expect(result?.deviceId, 'local-ble-after-restart');
    expect(postCalls, 0);
    expect(store.cleared, contains('journey-before-restart'));
    final completed = analytics.events.singleWhere(
      (event) => event.name == 'local_ble_pairing_completed',
    );
    expect(
        completed.properties['journey_id'], 'original-journey-before-restart');
    expect(completed.properties['attempt_number'], 3);
    expect(
      completed.properties['\$insert_id'],
      'local_ble_pairing_completed:journey-before-restart',
    );
  });

  testWidgets('caches typed not-found before acknowledgement and clearing',
      (tester) async {
    final store = _FakePendingPairingStore(
      pending: PendingLocalBlePairing(
        sessionId: 'journey-overtaking-get',
        journeyId: 'journey-overtaking-get',
        attemptNumber: 1,
        profileId: setup.profileId,
        serverScope: 'injected-server',
        startedAtEpochMs: DateTime.now().millisecondsSinceEpoch,
      ),
    );
    await tester.pumpWidget(
      MaterialApp(
        home: LocalBleDeviceAddScreen(
          setup: setup,
          inputMethod: 'camera',
          journeyId: 'unused-new-journey',
          pairingDeadline: const Duration(hours: 1),
          pendingPairingStore: store,
          pairingRequest: ({
            required hubType,
            required params,
            required receiveTimeout,
            required sessionId,
          }) async =>
              null,
          pairingStatusRequest: (_) async => const RhythmPairingResultStatus(
            sessionId: 'journey-overtaking-get',
            state: RhythmPairingResultState.notFound,
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(store.cleared, isEmpty);
    expect(store.saved.single.terminalResult?.status,
        PendingLocalBleTerminalResult.notFound);
    expect(find.textContaining('no record of the previous request'),
        findsOneWidget);
    await tester.tap(
      find.byKey(
        const ValueKey('acknowledge-local-ble-pairing-failure'),
      ),
    );
    await tester.pumpAndSettle();
    expect(store.cleared, ['journey-overtaking-get']);
  });

  testWidgets('rejects a terminal status for another hub type', (tester) async {
    final store = _FakePendingPairingStore(
      pending: PendingLocalBlePairing(
        sessionId: 'journey-hub-collision',
        journeyId: 'journey-hub-collision',
        attemptNumber: 1,
        profileId: setup.profileId,
        serverScope: 'injected-server',
        startedAtEpochMs: DateTime.now().millisecondsSinceEpoch,
      ),
    );
    await tester.pumpWidget(
      MaterialApp(
        home: LocalBleDeviceAddScreen(
          setup: setup,
          inputMethod: 'camera',
          journeyId: 'unused-new-journey',
          pendingPairingStore: store,
          pairingRequest: ({
            required hubType,
            required params,
            required receiveTimeout,
            required sessionId,
          }) async =>
              null,
          pairingStatusRequest: (_) async => const RhythmPairingResultStatus(
            sessionId: 'journey-hub-collision',
            state: RhythmPairingResultState.terminal,
            hubType: 'hue_ble',
            result: RhythmPairingSessionResult(
              hubType: 'hue_ble',
              status: RhythmPairingStatus.complete,
              device: RhythmPairedDevice(
                deviceId: 'hue-device',
                name: 'Wrong device',
                deviceType: 'light',
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(store.cleared, isEmpty);
    expect(find.textContaining('different pairing'), findsOneWidget);
  });

  testWidgets('pins server scope for the lifetime of the route',
      (tester) async {
    final store = _FakePendingPairingStore();
    var serverScope = 'home-1:server-before-hello';
    late StateSetter rebuild;
    await tester.pumpWidget(
      MaterialApp(
        home: StatefulBuilder(
          builder: (context, setState) {
            rebuild = setState;
            return LocalBleDeviceAddScreen(
              setup: setup,
              inputMethod: 'camera',
              journeyId: 'journey-scope-pin',
              serverScope: serverScope,
              pendingPairingStore: store,
              pairingRequest: ({
                required hubType,
                required params,
                required receiveTimeout,
                required sessionId,
              }) async =>
                  {
                'hub_type': 'local_ble',
                'status': 'complete',
                'device': {
                  'device_id': 'scope-pin-device',
                  'name': 'Button',
                  'device_type': 'button',
                },
              },
            );
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    rebuild(() => serverScope = 'home-1:server-after-hello');
    await tester.pump();
    await tester.tap(find.text('Find Device'));
    await tester.pumpAndSettle();

    expect(
      store.saved.map((pairing) => pairing.serverScope),
      everyElement('home-1:server-before-hello'),
    );
  });

  for (final response in <String, Map<String, dynamic>>{
    'malformed failed': {
      'status': 'failed',
      'error': 'untyped failure',
    },
    'foreign-hub complete': {
      'hub_type': 'hue_ble',
      'status': 'complete',
      'device': {
        'device_id': 'foreign-device',
        'name': 'Wrong device',
        'device_type': 'light',
      },
    },
  }.entries) {
    testWidgets('${response.key} POST body remains unresolved', (tester) async {
      final store = _FakePendingPairingStore();
      await tester.pumpWidget(
        MaterialApp(
          home: LocalBleDeviceAddScreen(
            setup: setup,
            inputMethod: 'camera',
            journeyId: 'journey-${response.key.replaceAll(' ', '-')}',
            pairingDeadline: const Duration(hours: 1),
            pendingPairingStore: store,
            pairingRequest: ({
              required hubType,
              required params,
              required receiveTimeout,
              required sessionId,
            }) async =>
                response.value,
            pairingStatusRequest: (_) async => null,
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('Find Device'));
      await tester.pump();

      expect(store.saved, hasLength(1));
      expect(store.cleared, isEmpty);
      expect(find.textContaining('Waiting for final confirmation'),
          findsOneWidget);
      await tester.pumpWidget(const SizedBox.shrink());
    });
  }

  for (final terminalStatus in ['failed', 'complete']) {
    testWidgets('HTTP 504 with $terminalStatus body remains unresolved',
        (tester) async {
      final store = _FakePendingPairingStore();
      await tester.pumpWidget(
        MaterialApp(
          home: LocalBleDeviceAddScreen(
            setup: setup,
            inputMethod: 'camera',
            journeyId: 'journey-504-$terminalStatus',
            pairingDeadline: const Duration(hours: 1),
            pendingPairingStore: store,
            pairingRequest: ({
              required hubType,
              required params,
              required receiveTimeout,
              required sessionId,
            }) async =>
                {
              'hub_type': 'local_ble',
              'status': terminalStatus,
              'http_status': 504,
              'request_delivery': 'accepted_or_unknown',
              if (terminalStatus == 'complete')
                'device': {
                  'device_id': 'gateway-device',
                  'name': 'Button',
                  'device_type': 'button',
                },
            },
            pairingStatusRequest: (_) async => null,
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('Find Device'));
      await tester.pump();

      expect(store.saved, hasLength(1));
      expect(store.cleared, isEmpty);
      expect(find.textContaining('Waiting for final confirmation'),
          findsOneWidget);
      await tester.pumpWidget(const SizedBox.shrink());
    });
  }

  testWidgets(
      'complete body with ambiguous delivery and no HTTP status reconciles',
      (tester) async {
    final store = _FakePendingPairingStore();
    await tester.pumpWidget(
      MaterialApp(
        home: LocalBleDeviceAddScreen(
          setup: setup,
          inputMethod: 'camera',
          journeyId: 'journey-ambiguous-complete-body',
          pairingDeadline: const Duration(hours: 1),
          pendingPairingStore: store,
          pairingRequest: ({
            required hubType,
            required params,
            required receiveTimeout,
            required sessionId,
          }) async =>
              {
            'hub_type': 'local_ble',
            'status': 'complete',
            'request_delivery': 'accepted_or_unknown',
            'device': {
              'device_id': 'ambiguous-device',
              'name': 'Button',
              'device_type': 'button',
            },
          },
          pairingStatusRequest: (_) async => null,
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Find Device'));
    await tester.pump();

    expect(store.saved, hasLength(1));
    expect(store.cleared, isEmpty);
    expect(find.text('Device added'), findsNothing);
    expect(
      find.textContaining('Waiting for final confirmation'),
      findsOneWidget,
    );
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets('HTTP 200 failed admission is cached, shown, ACKed, and cleared',
      (tester) async {
    final store = _FakePendingPairingStore();
    final ordering = <String>[];
    final requestedSessionIds = <String>[];
    await tester.pumpWidget(
      MaterialApp(
        home: LocalBleDeviceAddScreen(
          setup: setup,
          inputMethod: 'camera',
          journeyId: 'journey-admission-busy',
          pendingPairingStore: store,
          pairingRequest: ({
            required hubType,
            required params,
            required receiveTimeout,
            required sessionId,
          }) async {
            requestedSessionIds.add(sessionId);
            return {
              'hub_type': 'local_ble',
              'status': 'failed',
              'error': 'Another Bluetooth pairing is already in progress.',
            };
          },
          pairingAcknowledgementRequest: (sessionId) async {
            expect(sessionId, 'journey-admission-busy');
            expect(store.pending?.terminalResult?.status,
                PendingLocalBleTerminalResult.failed);
            ordering.add('server-ack');
            return true;
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Find Device'));
    await tester.pumpAndSettle();

    expect(store.saved, hasLength(2));
    expect(store.saved.last.terminalResult?.status,
        PendingLocalBleTerminalResult.failed);
    expect(store.cleared, isEmpty);
    expect(
      find.text('Another Bluetooth pairing is already in progress.'),
      findsOneWidget,
    );
    expect(
      find.byKey(
        const ValueKey('acknowledge-local-ble-pairing-failure'),
      ),
      findsOneWidget,
    );

    await tester.tap(
      find.byKey(
        const ValueKey('acknowledge-local-ble-pairing-failure'),
      ),
    );
    await tester.pumpAndSettle();

    expect(ordering, ['server-ack']);
    expect(store.cleared, ['journey-admission-busy']);
    expect(store.pending, isNull);

    await tester.tap(find.text('Find Device'));
    await tester.pumpAndSettle();

    expect(
      requestedSessionIds,
      ['journey-admission-busy', 'journey-admission-busy-attempt-2'],
    );
    expect(store.pending?.sessionId, 'journey-admission-busy-attempt-2');
  });

  testWidgets('local clear failure keeps successful device acknowledgement',
      (tester) async {
    final store = _FakePendingPairingStore(clearSucceeds: false);
    RhythmPairedDevice? result;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await Navigator.of(context).push(
                MaterialPageRoute<RhythmPairedDevice>(
                  builder: (_) => LocalBleDeviceAddScreen(
                    setup: setup,
                    inputMethod: 'camera',
                    journeyId: 'journey-clear-failure',
                    pendingPairingStore: store,
                    pairingRequest: ({
                      required hubType,
                      required params,
                      required receiveTimeout,
                      required sessionId,
                    }) async =>
                        {
                      'hub_type': 'local_ble',
                      'status': 'complete',
                      'device': {
                        'device_id': 'clear-failure-device',
                        'name': 'Button',
                        'device_type': 'button',
                      },
                    },
                  ),
                ),
              );
            },
            child: const Text('Open'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Find Device'));
    await tester.pumpAndSettle();

    expect(result, isNull);
    expect(find.text('Device added'), findsOneWidget);
    expect(find.textContaining('could not save your acknowledgement'),
        findsOneWidget);
    expect(
      find.byKey(const ValueKey('continue-local-ble-pairing')),
      findsOneWidget,
    );
  });

  testWidgets('keeps terminal warnings visible until the user continues',
      (tester) async {
    RhythmPairedDevice? result;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await Navigator.of(context).push(
                MaterialPageRoute<RhythmPairedDevice>(
                  builder: (_) => LocalBleDeviceAddScreen(
                    setup: setup,
                    inputMethod: 'manual_code',
                    journeyId: 'journey-warning',
                    pairingRequest: ({
                      required hubType,
                      required params,
                      required receiveTimeout,
                      required sessionId,
                    }) async =>
                        {
                      'hub_type': 'local_ble',
                      'status': 'complete',
                      'device': {
                        'device_id': 'local-ble-warning',
                        'name': 'Button',
                        'device_type': 'button',
                      },
                      'warnings': ['Storage acknowledgement is degraded'],
                    },
                  ),
                ),
              );
            },
            child: const Text('Open'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Find Device'));
    await tester.pumpAndSettle();

    expect(result, isNull);
    expect(
      find.byKey(const ValueKey('local-ble-pairing-warnings')),
      findsOneWidget,
    );
    expect(
      find.textContaining('Storage acknowledgement is degraded'),
      findsOneWidget,
    );
    await tester.tap(find.text('Continue'));
    await tester.pumpAndSettle();
    expect(result?.deviceId, 'local-ble-warning');
  });
}
