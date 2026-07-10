import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/hubs/ota_update_overlay.dart';
import 'package:rhythm_app/services/ota_service.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('coarse OTA states render the full staged update journey',
      (tester) async {
    final service = _FakeOtaService(
      state: OtaState.checking,
      latestVersion: '1.2.3',
    );
    addTearDown(service.dispose);

    await _pumpOverlay(tester, service);

    expect(find.text('Updating Server'), findsOneWidget);
    expect(find.text('Installing v1.2.3'), findsOneWidget);
    expect(find.text('Checking for updates'), findsNWidgets(2));
    expect(find.text('Do not close the app'), findsOneWidget);

    service.setState(OtaState.downloading);
    await tester.pump();
    expect(find.text('Downloading update to v1.2.3...'), findsOneWidget);

    service.setState(OtaState.uploading);
    await tester.pump();
    expect(find.text('Installing update to v1.2.3...'), findsOneWidget);

    service.setState(OtaState.flashing);
    await tester.pump();
    expect(find.text('Installing update to v1.2.3...'), findsOneWidget);

    service.setState(OtaState.rebooting);
    await tester.pump();
    expect(find.text('Restarting Server'), findsOneWidget);
    expect(find.text('Restarting into v1.2.3...'), findsOneWidget);
    expect(find.byIcon(Icons.restart_alt_rounded), findsWidgets);
  });

  testWidgets('repair copy remains stable throughout the update',
      (tester) async {
    final service = _FakeOtaService(
      state: OtaState.downloading,
      latestVersion: 'v2.0.0',
      bundleRepair: true,
    );
    addTearDown(service.dispose);

    await _pumpOverlay(tester, service);

    expect(find.text('Repairing v2.0.0'), findsOneWidget);
    expect(find.text('Downloading repair for v2.0.0...'), findsOneWidget);

    service
      ..bundleRepair = false
      ..setState(OtaState.flashing);
    await tester.pump();
    expect(find.text('Repairing v2.0.0'), findsOneWidget);
    expect(find.text('Installing repair for v2.0.0...'), findsOneWidget);

    service.setState(OtaState.rebooting);
    await tester.pump();
    expect(find.text('Restarting after repair on v2.0.0...'), findsOneWidget);
  });

  testWidgets('SSE download progress shows message percent and byte totals',
      (tester) async {
    final service = _FakeOtaService(
      state: OtaState.checking,
      latestVersion: '3.1.4',
    );
    final connection = _FakeConnection();
    addTearDown(service.dispose);
    addTearDown(connection.dispose);

    await _pumpOverlay(tester, service, connection: connection);
    connection.emitProgress(
      const RhythmOtaUpdateProgress(
        stage: RhythmOtaUpdateStage.downloading,
        message: 'Fetching signed update bundle',
        downloadedBytes: 45 * 1024 * 1024,
        totalBytes: 64 * 1024 * 1024,
        percent: 70,
      ),
    );
    await tester.pump();

    expect(find.text('Fetching signed update bundle'), findsOneWidget);
    expect(find.text('45.0 / 64.0 MB'), findsOneWidget);
    expect(find.text('70%'), findsOneWidget);
    expect(find.byIcon(Icons.cloud_download_outlined), findsWidgets);
  });

  testWidgets('SSE download bytes handle unknown totals and small payloads',
      (tester) async {
    final service = _FakeOtaService(state: OtaState.checking);
    final connection = _FakeConnection();
    addTearDown(service.dispose);
    addTearDown(connection.dispose);

    await _pumpOverlay(tester, service, connection: connection);

    connection.emitProgress(
      const RhythmOtaUpdateProgress(
        stage: RhythmOtaUpdateStage.downloading,
        message: '',
        downloadedBytes: 2048,
        percent: 1,
      ),
    );
    await tester.pump();
    expect(find.text('2 KB'), findsOneWidget);
    expect(find.text('Downloading update...'), findsOneWidget);

    connection.emitProgress(
      const RhythmOtaUpdateProgress(
        stage: RhythmOtaUpdateStage.downloading,
        message: '',
        downloadedBytes: 512,
        totalBytes: 900,
        percent: 56,
      ),
    );
    await tester.pump();
    expect(find.text('512 / 900 B'), findsOneWidget);
    expect(find.text('56%'), findsOneWidget);
  });

  testWidgets('SSE stages use sensible fallback messages', (tester) async {
    final service = _FakeOtaService(
      state: OtaState.checking,
      latestVersion: '4.0.0',
    );
    final connection = _FakeConnection();
    addTearDown(service.dispose);
    addTearDown(connection.dispose);

    await _pumpOverlay(tester, service, connection: connection);

    const cases = <(RhythmOtaUpdateStage, String)>[
      (RhythmOtaUpdateStage.verifying, 'Verifying download integrity'),
      (RhythmOtaUpdateStage.staging, 'Installing update to v4.0.0...'),
      (RhythmOtaUpdateStage.installing, 'Installing update to v4.0.0...'),
      (RhythmOtaUpdateStage.finalizing, 'Installing update to v4.0.0...'),
      (RhythmOtaUpdateStage.restarting, 'Restarting into v4.0.0...'),
    ];
    for (final (stage, message) in cases) {
      connection.emitProgress(
        RhythmOtaUpdateProgress(stage: stage, message: ''),
      );
      await tester.pump();
      expect(find.text(message), findsOneWidget, reason: '$stage fallback');
    }
  });

  testWidgets('failed SSE event stays attached to the last observed stage',
      (tester) async {
    final service = _FakeOtaService(state: OtaState.checking);
    final connection = _FakeConnection();
    addTearDown(service.dispose);
    addTearDown(connection.dispose);

    await _pumpOverlay(tester, service, connection: connection);
    connection.emitProgress(
      const RhythmOtaUpdateProgress(
        stage: RhythmOtaUpdateStage.verifying,
        message: 'Verifying payload',
      ),
    );
    await tester.pump();
    connection.emitProgress(
      const RhythmOtaUpdateProgress(
        stage: RhythmOtaUpdateStage.failed,
        message: 'Checksum mismatch',
      ),
    );
    await tester.pump();

    expect(find.text('Checksum mismatch'), findsNothing);
    expect(find.byIcon(Icons.close_rounded), findsOneWidget);
  });

  testWidgets('completion without a connection uses server status copy',
      (tester) async {
    final service = _FakeOtaService(
      state: OtaState.complete,
      latestVersion: '5.0.0',
      statusMessage: 'Updated and verified on the device',
    );
    addTearDown(service.dispose);

    await _pumpOverlay(tester, service);

    expect(find.text('Update Complete'), findsOneWidget);
    expect(find.text('Updated and verified on the device'), findsOneWidget);
    expect(find.text('Done'), findsOneWidget);
  });

  testWidgets('completion copy falls back for update and repair outcomes',
      (tester) async {
    final updateService = _FakeOtaService(
      state: OtaState.complete,
      latestVersion: '6.0.0',
    );
    addTearDown(updateService.dispose);
    await _pumpOverlay(tester, updateService);
    expect(find.text('Updated to v6.0.0.'), findsOneWidget);

    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pump();

    final repairService = _FakeOtaService(
      state: OtaState.complete,
      latestVersion: '6.0.0',
      bundleRepair: true,
    );
    addTearDown(repairService.dispose);
    await _pumpOverlay(tester, repairService);
    expect(find.text('Repair completed on v6.0.0.'), findsOneWidget);
  });

  testWidgets('completion waits for a fresh post-update connection',
      (tester) async {
    final service = _FakeOtaService(state: OtaState.rebooting);
    final connection = _FakeConnection(
      connectionState: RhythmConnectionState.connected,
    );
    addTearDown(service.dispose);
    addTearDown(connection.dispose);

    await _pumpOverlay(tester, service, connection: connection);
    service.setState(OtaState.complete);
    await tester.pump();

    expect(find.text('Reconnecting Server'), findsOneWidget);
    expect(
      find.text('Waiting for the server connection to finish restoring'),
      findsOneWidget,
    );
    expect(find.text('Update Complete'), findsNothing);

    connection.emitConnectionState(RhythmConnectionState.disconnected);
    await tester.pump();
    expect(find.text('Reconnecting Server'), findsOneWidget);

    connection.emitConnectionState(RhythmConnectionState.connected);
    await tester.pump();
    expect(find.text('Update Complete'), findsOneWidget);
  });

  testWidgets('error state is dismissible from the pushed overlay route',
      (tester) async {
    final service = _FakeOtaService(state: OtaState.error);
    addTearDown(service.dispose);

    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () => OtaUpdateOverlay.show(
              context,
              otaService: service,
            ),
            child: const Text('Open updater'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open updater'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));

    expect(find.text('Update Failed'), findsOneWidget);
    expect(
      find.text('The update did not finish. Please try again.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Close'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.text('Open updater'), findsOneWidget);
    expect(find.text('Update Failed'), findsNothing);
  });
}

Future<void> _pumpOverlay(
  WidgetTester tester,
  OtaService service, {
  RhythmConnection? connection,
}) async {
  await tester.pumpWidget(
    MaterialApp(
      home: OtaUpdateOverlay(
        otaService: service,
        connection: connection,
      ),
    ),
  );
  await tester.pump();
}

class _FakeOtaService extends OtaService {
  _FakeOtaService({
    required OtaState state,
    this.latestVersion,
    this.statusMessage,
    this.bundleRepair = false,
  }) : _state = state;

  OtaState _state;

  @override
  final String? latestVersion;

  @override
  final String? statusMessage;

  bool bundleRepair;

  @override
  OtaState get state => _state;

  @override
  bool get isBundleRepair => bundleRepair;

  void setState(OtaState value) {
    _state = value;
    notifyListeners();
  }
}

class _FakeConnection extends RhythmConnection {
  _FakeConnection({
    RhythmConnectionState connectionState = RhythmConnectionState.disconnected,
  }) : _connectionState = connectionState;

  final _progress = StreamController<RhythmOtaUpdateProgress>.broadcast();
  final _states = StreamController<RhythmConnectionState>.broadcast();
  RhythmConnectionState _connectionState;

  @override
  Stream<RhythmOtaUpdateProgress> get otaUpdateProgressEvents =>
      _progress.stream;

  @override
  Stream<RhythmConnectionState> get connectionStateStream => _states.stream;

  @override
  RhythmConnectionState get connectionState => _connectionState;

  void emitProgress(RhythmOtaUpdateProgress progress) {
    _progress.add(progress);
  }

  void emitConnectionState(RhythmConnectionState state) {
    _connectionState = state;
    _states.add(state);
  }

  @override
  void dispose() {
    unawaited(_progress.close());
    unawaited(_states.close());
    super.dispose();
  }
}
