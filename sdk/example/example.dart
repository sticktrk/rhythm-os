import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() async {
  // Enable SDK debug logging (optional).
  RhythmSdk.enableLogging(); // or: RhythmSdk.enableLogging(level: Level.WARNING)

  // --- Config API (stateless) ---
  final configApi = RhythmConfigApi(baseUrl: 'http://192.168.1.100/');

  final healthy = await configApi.healthCheck();
  print('Server healthy: $healthy');

  final configState = await configApi.getConfigState();
  print('Brightness range: ${configState.config.minBrightness}-${configState.config.maxBrightness}');

  final curveData = await configApi.getCurveData();
  print('Curve points: ${curveData.hours.length}');

  // --- Real-time connection ---
  final connection = RhythmConnection();

  // Listen for room state changes.
  connection.rhythmStateEvents.listen((state) {
    print('Room ${state.roomId}: brightness=${state.brightness}, kelvin=${state.kelvin}');
  });

  // Connect and start polling + SSE.
  await connection.connect('192.168.1.100');

  // Use the server API through the connection.
  await connection.api.roomAction(roomId: 'abc123', action: 'toggle');

  // --- Diagnostics (standalone) ---
  final diag = RhythmDiagnosticsApi(host: '192.168.1.100');
  final vitals = await diag.getDiagVitals();
  print('Vitals: $vitals');

  // --- OTA ---
  final ota = RhythmOtaApi();
  final release = await ota.checkForUpdate('1.0.0');
  if (release != null) {
    print('Update available: ${release.version}');
    await for (final progress in ota.startUpdate(release, deviceHost: '192.168.1.100')) {
      print('OTA: ${progress.state} ${progress.progressPercent}%');
    }
  }

  // Clean up.
  connection.dispose();
}
