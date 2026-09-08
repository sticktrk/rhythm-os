import 'dart:io';

import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_app/screens/hubs/rhythmserver_settings_screen.dart';
import 'package:rhythm_app/widgets/solar_orbit.dart';
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
  bool hueAuthoritySupportedForTest = false;
  RhythmHueAuthority? hueAuthorityForTest;

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
  bool get hueRoomAuthorityConsentSupported => hueAuthoritySupportedForTest;

  @override
  Future<RhythmHueAuthority?> fetchHueAuthority() async => hueAuthorityForTest;

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

const _secondHueHub = <String, dynamic>{
  'type': 'hue',
  'address': '192.0.2.26',
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
        debugShowCheckedModeBanner: false,
        home: Scaffold(
          backgroundColor: CelestialColors.backgroundDark,
          body: Padding(
            padding: const EdgeInsets.all(24),
            child: RhythmServerHubManagementSection(
              showConfigured: showConfigured,
              showAddOptions: false,
            ),
          ),
        ),
      ),
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

  testWidgets(
      'third party hubs open one combined device list without vendor menus',
      (tester) async {
    const monster = {'type': 'monster', 'address': 'local', 'connected': true};
    const future = {
      'type': 'future_vendor',
      'address': 'local',
      'connected': false
    };
    syncProvider.replaceHubs(const [_matterHub, monster, future, _hueHub]);
    api.canonicalDevices = const [
      {
        'id': 'strip',
        'name': 'Neon strip',
        'device_type': 'light',
        'endpoints': [
          {
            'hub_key': {'hub_type': 'monster', 'address': 'local'}
          },
          {
            'hub_key': {'hub_type': 'future_vendor', 'address': 'local'}
          },
        ],
      },
      {
        'id': 'button',
        'name': 'Desk button',
        'device_type': 'button',
        'endpoints': [
          {
            'hub_key': {'hub_type': 'future_vendor', 'address': 'local'}
          },
        ],
      },
      {
        'id': 'hue',
        'name': 'Hue-only bulb',
        'device_type': 'light',
        'endpoints': [
          {
            'hub_key': {'hub_type': 'hue', 'address': '192.0.2.25'}
          },
        ],
      },
      {
        'id': 'outside',
        'name': 'Other instance',
        'device_type': 'light',
        'endpoints': [
          {
            'hub_key': {'hub_type': 'future_vendor', 'address': 'other'}
          },
        ],
      },
    ];
    final screenshotPrefix =
        Platform.environment['RHYTHM_THIRD_PARTY_SCREENSHOT'];
    if (screenshotPrefix != null) {
      await tester.binding.setSurfaceSize(const Size(430, 900));
      addTearDown(() => tester.binding.setSurfaceSize(null));
    }
    await tester.pumpWidget(buildSection(showConfigured: true));
    await tester.pumpAndSettle();
    expect(find.text('Third party Hubs'), findsOneWidget);
    expect(find.text('Monster'), findsNothing);
    expect(find.text('Future Vendor'), findsNothing);
    expect(find.text('1 light, 1 button'), findsOneWidget);
    expect(find.text('Matter'), findsOneWidget);
    expect(find.text('Philips Hue'), findsOneWidget);
    expect(api.canonicalDeviceCalls, 1);
    if (screenshotPrefix != null) {
      await expectLater(find.byType(Overlay),
          matchesGoldenFile('$screenshotPrefix-hubs.png'));
    }
    await tester.tap(find.byKey(const ValueKey('third-party-hubs')));
    await tester.pumpAndSettle();
    expect(find.text('Third party Hubs').hitTestable(), findsOneWidget);
    expect(find.text('Neon strip'), findsOneWidget);
    expect(find.text('Desk button'), findsOneWidget);
    expect(find.text('Hue-only bulb'), findsNothing);
    expect(find.text('Other instance'), findsNothing);
    expect(find.text('Monster'), findsNothing);
    expect(find.text('Future Vendor'), findsNothing);
    expect(find.text('Disconnect'), findsNothing);
    expect(find.text('Reconnect'), findsNothing);
    expect(api.canonicalDeviceCalls, 2);
    if (screenshotPrefix != null) {
      await expectLater(find.byType(Overlay),
          matchesGoldenFile('$screenshotPrefix-devices.png'));
    }
  });

  testWidgets(
      'third party group handles legacy metadata and successful empty catalogs',
      (tester) async {
    syncProvider.replaceHubs(const [
      {'type': 'monster', 'address': 'local', 'connected': true},
    ]);
    await tester.pumpWidget(buildSection(showConfigured: true));
    await tester.pumpAndSettle();
    expect(find.text('Third party Hubs'), findsOneWidget);
    expect(find.text('No devices'), findsOneWidget);
    await tester.tap(find.text('Third party Hubs'));
    await tester.pumpAndSettle();
    expect(find.text('No devices discovered'), findsOneWidget);
    expect(find.text('Monster'), findsNothing);
  });

  testWidgets(
      'third party catalog failure can retry to a successful empty list',
      (tester) async {
    syncProvider.replaceHubs(const [
      {'type': 'monster', 'address': 'local', 'connected': true},
    ]);
    api.canonicalDevices = null;
    await tester.pumpWidget(buildSection(showConfigured: true));
    await tester.pumpAndSettle();
    expect(find.text('Devices unavailable'), findsOneWidget);
    await tester.tap(find.text('Third party Hubs'));
    await tester.pumpAndSettle();
    expect(find.text('No devices discovered'), findsNothing);
    expect(find.text('Retry loading devices'), findsOneWidget);

    api.canonicalDevices = const [];
    await tester.tap(find.text('Retry loading devices'));
    await tester.pumpAndSettle();
    expect(find.text('Retry loading devices'), findsNothing);
    expect(find.text('No devices discovered'), findsOneWidget);
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

  testWidgets('each configured hub opens a dedicated address-scoped page', (
    tester,
  ) async {
    syncProvider.replaceHubs(const [_hueHub, _secondHueHub]);
    api.canonicalDevices = const [
      {
        'id': 'hue-light-one',
        'name': 'Bridge One Bulb',
        'device_type': 'light',
        'endpoints': [
          {
            'hub_key': {
              'hub_type': 'hue',
              'address': '192.0.2.25',
            },
          },
        ],
      },
      {
        'id': 'hue-light-two',
        'name': 'Bridge Two Bulb',
        'device_type': 'light',
        'endpoints': [
          {
            'hub_key': {
              'hub_type': 'hue',
              'address': '192.0.2.26',
            },
          },
        ],
      },
    ];

    await tester.pumpWidget(buildSection(showConfigured: true));
    await tester.pumpAndSettle();

    expect(find.text('HUBS'), findsOneWidget);
    expect(find.text('Bridge One Bulb'), findsNothing);
    expect(find.text('Bridge Two Bulb'), findsNothing);

    await tester.tap(find.text('Philips Hue').first);
    await tester.pumpAndSettle();

    expect(find.text('DEVICES'), findsOneWidget);
    expect(find.text('Bridge One Bulb'), findsOneWidget);
    expect(find.text('Bridge Two Bulb'), findsNothing);
  });

  testWidgets('Hue hub page permanently exposes room automation review', (
    tester,
  ) async {
    syncProvider.replaceHubs(const [_hueHub]);
    syncProvider.hueAuthoritySupportedForTest = true;
    syncProvider.hueAuthorityForTest = const RhythmHueAuthority(
      schemaVersion: 1,
      bridges: [
        RhythmHueBridgeAuthority(
          address: '192.0.2.25:443',
          revision: '0123456789abcdef',
          takeoverScope: 'bridge',
          bridgeTakeoverRequested: true,
          rooms: [
            RhythmHueRoomAuthority(
              roomId: 'office',
              name: 'Office',
              owner: RhythmHueRoomAuthorityOwner.rhythm,
              rhythmAutomationEnabled: true,
            ),
          ],
        ),
      ],
    );

    await tester.pumpWidget(buildSection(showConfigured: true));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Philips Hue'));
    await tester.pumpAndSettle();

    expect(find.text('Hue room automation'), findsOneWidget);
    await tester.tap(find.text('Hue room automation'));
    await tester.pumpAndSettle();

    expect(find.text('Who should automate each room?'), findsOneWidget);
    expect(find.text('Office'), findsOneWidget);
  });
}
