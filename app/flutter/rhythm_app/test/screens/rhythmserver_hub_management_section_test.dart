import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_app/screens/hubs/rhythmserver_settings_screen.dart';
import 'package:rhythm_app/screens/settings/sections/lights_devices_section.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _SummaryApi extends RhythmServerApi {
  _SummaryApi() : super(Dio());

  int canonicalDeviceCalls = 0;
  List<Map<String, dynamic>>? canonicalDevices = const [];
  bool throwOnCanonicalDevices = false;

  @override
  Future<List<Map<String, dynamic>>?> getCanonicalDevices() async {
    canonicalDeviceCalls += 1;
    if (throwOnCanonicalDevices) throw StateError('summary unavailable');
    return canonicalDevices;
  }
}

class _SummaryConnection extends RhythmConnection {
  _SummaryConnection(this.summaryApi);

  final _SummaryApi summaryApi;

  @override
  RhythmServerApi get api => summaryApi;

  @override
  RhythmConnectionState get connectionState => RhythmConnectionState.connected;

  @override
  bool get connected => true;
}

class _MutableHubProvider extends ServerSyncProvider {
  _MutableHubProvider({
    required super.connection,
    required super.roomProvider,
    required super.homeProvider,
    required List<Map<String, dynamic>> hubs,
  }) : _hubs = hubs;

  List<Map<String, dynamic>> _hubs;
  RhythmHubCapabilities? hueCapabilitiesForTest;
  bool hueAuthorityConsentSupportedForTest = false;
  RhythmHueAuthority? hueAuthorityForTest;
  int hueAuthorityFetches = 0;

  @override
  List<Map<String, dynamic>> get serverHubInfos => _hubs;

  @override
  RhythmConnectionState get connectionState => RhythmConnectionState.connected;

  @override
  Set<String> get connectedHubTypes => _hubs
      .where((hub) => hub['connected'] == true && hub['type'] != 'none')
      .map((hub) => hub['type'] as String)
      .toSet();

  @override
  RhythmHubCapabilities? hubCapabilities(String hubType) {
    if (hubType == 'hue') return hueCapabilitiesForTest;
    return super.hubCapabilities(hubType);
  }

  @override
  bool get hueRoomAuthorityConsentSupported =>
      hueAuthorityConsentSupportedForTest;

  @override
  Future<RhythmHueAuthority?> fetchHueAuthority() async {
    hueAuthorityFetches += 1;
    return hueAuthorityForTest;
  }

  void replaceHubs(List<Map<String, dynamic>> hubs) {
    _hubs = hubs;
    notifyListeners();
  }

  void pulse() => notifyListeners();
}

const _localBleHub = <String, dynamic>{
  'type': 'local_ble',
  'address': 'default',
  'connected': true,
};

const _matterHub = <String, dynamic>{
  'type': 'matter',
  'address': 'local',
  'connected': true,
};

const _hueHub = <String, dynamic>{
  'type': 'hue',
  'address': '192.0.2.25',
  'connected': true,
};

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late _SummaryApi api;
  late _SummaryConnection connection;
  late RoomProvider roomProvider;
  late HomeProvider homeProvider;
  late _MutableHubProvider syncProvider;

  setUp(() {
    api = _SummaryApi();
    connection = _SummaryConnection(api);
    roomProvider = RoomProvider();
    homeProvider = HomeProvider();
    syncProvider = _MutableHubProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: homeProvider,
      hubs: const [_localBleHub],
    );
  });

  tearDown(() {
    syncProvider.dispose();
    connection.dispose();
    roomProvider.dispose();
    homeProvider.dispose();
  });

  Widget buildSection({required bool showConfigured}) {
    return MultiProvider(
      providers: [
        Provider<RhythmConnection>.value(value: connection),
        ChangeNotifierProvider<ServerSyncProvider>.value(value: syncProvider),
      ],
      child: MaterialApp(
        home: Scaffold(
          body: RhythmServerHubManagementSection(
            showConfigured: showConfigured,
            showAddOptions: false,
          ),
        ),
      ),
    );
  }

  Widget buildDevicesList() {
    return MultiProvider(
      providers: [
        Provider<RhythmConnection>.value(value: connection),
        ChangeNotifierProvider<ServerSyncProvider>.value(value: syncProvider),
      ],
      child: const MaterialApp(home: DevicesListScreen()),
    );
  }

  testWidgets(
      'hidden configured section performs no canonical summary work across rebuilds',
      (tester) async {
    await tester.pumpWidget(buildSection(showConfigured: false));

    for (var index = 0; index < 30; index += 1) {
      syncProvider.pulse();
      await tester.pump(const Duration(milliseconds: 16));
    }
    syncProvider.replaceHubs(const [_localBleHub, _matterHub]);
    await tester.pumpAndSettle();

    expect(api.canonicalDeviceCalls, 0);
  });

  testWidgets(
      'configured empty local BLE hub loads no devices with one bounded fetch',
      (tester) async {
    await tester.pumpWidget(buildSection(showConfigured: true));
    await tester.pumpAndSettle();

    expect(api.canonicalDeviceCalls, 1);
    expect(find.text('No devices'), findsOneWidget);

    for (var index = 0; index < 30; index += 1) {
      syncProvider.pulse();
      await tester.pump(const Duration(milliseconds: 16));
    }

    expect(api.canonicalDeviceCalls, 1);
    expect(find.text('No devices'), findsOneWidget);
  });

  testWidgets('new configured empty hub triggers one new summary fetch',
      (tester) async {
    await tester.pumpWidget(buildSection(showConfigured: true));
    await tester.pumpAndSettle();
    expect(api.canonicalDeviceCalls, 1);

    syncProvider.replaceHubs(const [_localBleHub, _matterHub]);
    await tester.pumpAndSettle();

    expect(api.canonicalDeviceCalls, 2);
    expect(find.text('No devices'), findsNWidgets(2));

    syncProvider.pulse();
    await tester.pumpAndSettle();
    expect(api.canonicalDeviceCalls, 2);
  });

  testWidgets('failed summary fetch does not retry on every frame',
      (tester) async {
    api.throwOnCanonicalDevices = true;
    await tester.pumpWidget(buildSection(showConfigured: true));
    await tester.pumpAndSettle();

    expect(api.canonicalDeviceCalls, 1);
    expect(find.text('Devices unavailable'), findsOneWidget);

    for (var index = 0; index < 30; index += 1) {
      syncProvider.pulse();
      await tester.pump(const Duration(milliseconds: 16));
    }
    expect(api.canonicalDeviceCalls, 1);
  });

  testWidgets('Hue detail offers Bridge button search only when advertised', (
    tester,
  ) async {
    syncProvider.replaceHubs(const [_hueHub]);
    syncProvider.hueCapabilitiesForTest = const RhythmHubCapabilities(
      type: 'hue',
      configurable: true,
      deviceOnboardingMethods: [
        RhythmDeviceOnboardingMethod.hueBridgeButtonSearch,
      ],
      supportsUnpairing: true,
      unpairableDeviceTypes: ['light', 'button'],
      supportsRoomlessDevices: true,
    );

    await tester.pumpWidget(buildSection(showConfigured: true));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Philips Hue'));
    await tester.pumpAndSettle();

    expect(find.text('Pair button or switch'), findsOneWidget);
  });

  testWidgets('legacy Hue detail hides Bridge button search', (tester) async {
    syncProvider.replaceHubs(const [_hueHub]);

    await tester.pumpWidget(buildSection(showConfigured: true));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Philips Hue'));
    await tester.pumpAndSettle();

    expect(find.text('Pair button or switch'), findsNothing);
  });

  testWidgets('Settings Devices opens automation review for existing Hue hub',
      (tester) async {
    syncProvider.replaceHubs(const [_localBleHub, _hueHub]);
    syncProvider.hueAuthorityConsentSupportedForTest = true;
    syncProvider.hueAuthorityForTest = const RhythmHueAuthority(
      schemaVersion: 1,
      bridges: [
        RhythmHueBridgeAuthority(
          address: '192.0.2.25:443',
          revision: '0123456789abcdef',
          takeoverScope: 'bridge',
          bridgeTakeoverRequested: false,
          rooms: [
            RhythmHueRoomAuthority(
              roomId: 'office',
              name: 'Office',
              owner: RhythmHueRoomAuthorityOwner.unreviewed,
              rhythmAutomationEnabled: false,
            ),
          ],
        ),
      ],
    );

    await tester.pumpWidget(buildDevicesList());
    await tester.pumpAndSettle();
    await tester.tap(find.text('Philips Hue'));
    await tester.pumpAndSettle();

    expect(find.text('Review room automation'), findsOneWidget);
    await tester.tap(find.text('Review room automation'));
    await tester.pumpAndSettle();

    expect(syncProvider.hueAuthorityFetches, 1);
    expect(find.text('Hue room automation'), findsOneWidget);
    expect(find.text('Office'), findsOneWidget);
    expect(find.text('Save room choices'), findsOneWidget);
  });
}
