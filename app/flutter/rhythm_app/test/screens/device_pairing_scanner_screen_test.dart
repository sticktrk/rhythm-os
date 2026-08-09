import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/screens/hubs/device_pairing_code_entry_screen.dart';
import 'package:rhythm_app/screens/hubs/device_pairing_scanner_screen.dart';
import 'package:rhythm_app/services/analytics_service.dart';
import 'package:rhythm_app/services/hue_ble_auto_discovery_service.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../helpers/capturing_analytics_backend.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late ValueChanged<Iterable<String>> detect;
  late CapturingAnalyticsBackend analyticsBackend;

  Widget buildScanner({
    bool hueBridgeSerialSearchAvailable = false,
    bool hueBridgeOnly = false,
    bool autoDiscoverHueBle = false,
    HueBleDiscoveryRequest? hueBleDiscoveryRequest,
  }) {
    return MaterialApp(
      home: DevicePairingScannerScreen(
        hueBridgeSerialSearchAvailable: hueBridgeSerialSearchAvailable,
        hueBridgeOnly: hueBridgeOnly,
        autoDiscoverHueBle: autoDiscoverHueBle,
        analyticsSource: 'device_camera',
        hueBleDiscoveryRequest: hueBleDiscoveryRequest,
        cameraBuilder: (context, onDetect) {
          detect = onDetect;
          return const ColoredBox(color: Colors.black);
        },
      ),
    );
  }

  setUp(() async {
    detect = (_) {};
    analyticsBackend = CapturingAnalyticsBackend();
    await analyticsBackend.initialize();
    BackendProvider.setInstanceForTesting(
      auth: OfflineAuthBackend(),
      analytics: analyticsBackend,
    );
    await AnalyticsService().initialize();
  });

  tearDown(BackendProvider.resetForTesting);

  testWidgets('offers back and manual-code actions from the camera', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(430, 900));
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.pumpWidget(buildScanner());

    expect(find.text('Add Device'), findsOneWidget);
    expect(find.byTooltip('Back to Add & Review'), findsOneWidget);
    expect(find.text('Enter a Code'), findsOneWidget);
  });

  testWidgets('offers a nearby Hue bulb without blocking camera intake', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(430, 900));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    var discoveryCalls = 0;

    await tester.pumpWidget(
      buildScanner(
        autoDiscoverHueBle: true,
        hueBleDiscoveryRequest: ({required source}) async {
          discoveryCalls += 1;
          expect(source, 'device_camera');
          return const HueBleDiscoveryResult(
            HueBleDiscoveryOutcome.found,
            deviceCount: 1,
          );
        },
      ),
    );
    await tester.pumpAndSettle();

    expect(discoveryCalls, 1);
    expect(
      find.byKey(const ValueKey('nearby-hue-ble-prompt')),
      findsOneWidget,
    );
    expect(find.text('Nearby bulb found'), findsOneWidget);
    expect(
      find.text(
        'Rhythm found a nearby Hue Bluetooth bulb. Would you like to add it?',
      ),
      findsOneWidget,
    );

    await tester.tap(find.byKey(const ValueKey('nearby-hue-ble-dismiss')));
    await tester.pumpAndSettle();

    expect(find.text('Scan any device QR code'), findsOneWidget);
    expect(find.text('Enter a Code'), findsOneWidget);
    final promptEvent = analyticsBackend.events.singleWhere(
      (event) => event.name == 'hue_ble_nearby_prompt_answered',
    );
    expect(promptEvent.properties, {
      'source': 'device_camera',
      'outcome': 'dismissed',
    });
  });

  testWidgets('returns an accepted nearby Hue bulb to the add flow', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(430, 900));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    DevicePairingScannerResult? result;

    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: TextButton(
              onPressed: () async {
                result = await Navigator.of(context)
                    .push<DevicePairingScannerResult>(
                  MaterialPageRoute(
                    builder: (_) => DevicePairingScannerScreen(
                      autoDiscoverHueBle: true,
                      analyticsSource: 'device_camera',
                      journeyId: 'device-pair-nearby',
                      hueBleDiscoveryRequest: ({required source}) async {
                        return const HueBleDiscoveryResult(
                          HueBleDiscoveryOutcome.found,
                          deviceCount: 1,
                        );
                      },
                      cameraBuilder: (context, onDetect) {
                        detect = onDetect;
                        return const ColoredBox(color: Colors.black);
                      },
                    ),
                  ),
                );
              },
              child: const Text('Open scanner'),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Open scanner'));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('nearby-hue-ble-accept')));
    await tester.pumpAndSettle();

    expect(result?.action, DevicePairingScannerAction.hueBle);
    expect(result?.inputMethod, 'auto_discovery');
    expect(result?.journeyId, 'device-pair-nearby');
  });

  testWidgets('fits the camera actions on a short phone', (tester) async {
    await tester.binding.setSurfaceSize(const Size(320, 568));
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.pumpWidget(buildScanner());

    expect(find.text('Scan any device QR code'), findsOneWidget);
    expect(find.text('Enter a Code'), findsOneWidget);
    expect(tester.takeException(), isNull);

    detect(const ['X-HM://0023ISYWY8H2B']);
    await tester.pump();
    expect(find.text('HomeKit isn’t supported'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'explains HomeKit, Hue, and unknown scans without leaving camera',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(430, 900));
      addTearDown(() => tester.binding.setSurfaceSize(null));

      await tester.pumpWidget(buildScanner());

      detect(const ['X-HM://0023ISYWY8H2B']);
      await tester.pump();
      expect(find.text('HomeKit isn’t supported'), findsOneWidget);

      await tester.tap(find.text('Scan Another Code'));
      await tester.pump();
      detect(const ['https://www.philips-hue.com/connectproduct']);
      await tester.pump();
      expect(find.text('Use Add Device'), findsOneWidget);

      await tester.tap(find.text('Scan Another Code'));
      await tester.pump();
      detect(const ['https://example.com/not-a-pairing-code']);
      await tester.pump();
      expect(find.text('Code not recognized'), findsOneWidget);
    },
  );

  testWidgets('returns a Matter scan to its caller', (tester) async {
    await tester.binding.setSurfaceSize(const Size(430, 900));
    addTearDown(() => tester.binding.setSurfaceSize(null));

    DevicePairingScannerResult? result;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: TextButton(
              onPressed: () async {
                result = await Navigator.of(context)
                    .push<DevicePairingScannerResult>(
                  MaterialPageRoute(
                    builder: (_) => DevicePairingScannerScreen(
                      cameraBuilder: (context, onDetect) {
                        detect = onDetect;
                        return const ColoredBox(color: Colors.black);
                      },
                    ),
                  ),
                );
              },
              child: const Text('Open scanner'),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Open scanner'));
    await tester.pumpAndSettle();
    detect(const ['MT:Y.K908OC16750648G00']);
    await tester.pumpAndSettle();

    expect(result?.action, DevicePairingScannerAction.matter);
    expect(result?.payload, 'MT:Y.K908OC16750648G00');
    expect(result?.inputMethod, 'camera');
    final event = analyticsBackend.events.singleWhere(
      (event) => event.name == 'device_pairing_code_detected',
    );
    expect(event.properties, {
      'code_kind': 'matter',
      'outcome': 'continued_to_pairing',
      'input_method': 'camera',
    });
    expect(
      event.properties.values,
      isNot(contains('MT:Y.K908OC16750648G00')),
    );
  });

  testWidgets('returns a supported local-BLE profile without logging QR data',
      (tester) async {
    const qr = 'B:0A0B0C0D0E0F%G\$S:SYNTHETIC000001\$M:TESTMODEL001';
    DevicePairingScannerResult? result;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await Navigator.of(context).push(
                MaterialPageRoute<DevicePairingScannerResult>(
                  builder: (_) => DevicePairingScannerScreen(
                    supportedLocalBleProfileIds: const {
                      RhythmDeviceProfileId.oreinOc02001Button,
                    },
                    journeyId: 'journey-local-1',
                    cameraBuilder: (context, onDetect) {
                      detect = onDetect;
                      return const ColoredBox(color: Colors.black);
                    },
                  ),
                ),
              );
            },
            child: const Text('Open scanner'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open scanner'));
    await tester.pumpAndSettle();
    detect(const [qr]);
    await tester.pumpAndSettle();

    expect(result?.action, DevicePairingScannerAction.localBle);
    expect(
      result?.localBleSetup?.profileId,
      RhythmDeviceProfileId.oreinOc02001Button,
    );
    expect(result?.inputMethod, 'camera');
    expect(result?.journeyId, 'journey-local-1');
    final event = analyticsBackend.events.singleWhere(
      (event) => event.name == 'device_pairing_code_detected',
    );
    expect(event.properties, {
      'code_kind': 'local_ble',
      'outcome': 'continued_to_pairing',
      'journey_id': 'journey-local-1',
      'input_method': 'camera',
      'profile_id': RhythmDeviceProfileId.oreinOc02001Button,
    });
    expect(event.properties.values, isNot(contains(qr)));
  });

  testWidgets('asks when Matter and a supported local-BLE code share a frame',
      (tester) async {
    const qr = 'B:0A0B0C0D0E0F%G\$S:SYNTHETIC000001\$M:TESTMODEL001';
    DevicePairingScannerResult? result;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await Navigator.of(context).push(
                MaterialPageRoute<DevicePairingScannerResult>(
                  builder: (_) => DevicePairingScannerScreen(
                    supportedLocalBleProfileIds: const {
                      RhythmDeviceProfileId.oreinOc02001Button,
                    },
                    journeyId: 'journey-multiple-1',
                    cameraBuilder: (context, onDetect) {
                      detect = onDetect;
                      return const ColoredBox(color: Colors.black);
                    },
                  ),
                ),
              );
            },
            child: const Text('Open scanner'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open scanner'));
    await tester.pumpAndSettle();
    detect(const [qr, 'MT:Y.K908OC16750648G00']);
    await tester.pump();

    expect(find.byKey(const ValueKey('choose-matter')), findsOneWidget);
    expect(find.byKey(const ValueKey('choose-local-ble')), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('choose-local-ble')));
    await tester.pumpAndSettle();

    expect(result?.action, DevicePairingScannerAction.localBle);
    expect(result?.journeyId, 'journey-multiple-1');
    final detection = analyticsBackend.events.firstWhere(
      (event) => event.name == 'device_pairing_code_detected',
    );
    expect(detection.properties['code_kind'], 'multiple');
    expect(
      detection.properties['profile_id'],
      RhythmDeviceProfileId.oreinOc02001Button,
    );
    expect(detection.properties.values, isNot(contains(qr)));
  });

  testWidgets('unsupported local-BLE data does not detour a valid Matter scan',
      (tester) async {
    const qr = 'B:0A0B0C0D0E0F%G\$S:SYNTHETIC000001\$M:TESTMODEL001';
    DevicePairingScannerResult? result;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              result = await Navigator.of(context).push(
                MaterialPageRoute<DevicePairingScannerResult>(
                  builder: (_) => DevicePairingScannerScreen(
                    cameraBuilder: (context, onDetect) {
                      detect = onDetect;
                      return const ColoredBox(color: Colors.black);
                    },
                  ),
                ),
              );
            },
            child: const Text('Open scanner'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open scanner'));
    await tester.pumpAndSettle();
    detect(const [qr, 'MT:Y.K908OC16750648G00']);
    await tester.pumpAndSettle();

    expect(result?.action, DevicePairingScannerAction.matter);
    expect(result?.payload, 'MT:Y.K908OC16750648G00');
  });

  testWidgets('returns a Hue serial only for connected bridge search', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(430, 900));
    addTearDown(() => tester.binding.setSurfaceSize(null));

    DevicePairingScannerResult? result;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: TextButton(
              onPressed: () async {
                result = await Navigator.of(context)
                    .push<DevicePairingScannerResult>(
                  MaterialPageRoute(
                    builder: (_) => DevicePairingScannerScreen(
                      hueBridgeSerialSearchAvailable: true,
                      cameraBuilder: (context, onDetect) {
                        detect = onDetect;
                        return const ColoredBox(color: Colors.black);
                      },
                    ),
                  ),
                );
              },
              child: const Text('Open scanner'),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Open scanner'));
    await tester.pumpAndSettle();
    detect(const [
      'HUE:Z:0123456789ABCDEF0123456789ABCDEF0123 '
          'M:0017880109E277DA D:L3B A:1184',
    ]);
    await tester.pumpAndSettle();

    expect(result?.action, DevicePairingScannerAction.hueBridge);
    expect(result?.payload, 'E277DA');
    expect(result?.inputMethod, 'camera');
  });

  testWidgets('bridge scanner is Hue-specific and never routes Matter', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(430, 900));
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.pumpWidget(
      buildScanner(
        hueBridgeSerialSearchAvailable: true,
        hueBridgeOnly: true,
      ),
    );

    expect(find.text('Add Hue Bulb'), findsOneWidget);
    expect(find.text('Scan Hue bulb QR'), findsOneWidget);
    expect(find.text('Enter Bulb Serial'), findsOneWidget);

    detect(const ['MT:Y.K908OC16750648G00']);
    await tester.pump();

    expect(find.text('Hue bulb serial not found'), findsOneWidget);
    expect(find.text('Pair with Matter'), findsNothing);
  });

  testWidgets(
    'universal scanner asks when Matter and Hue are in the same frame',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(430, 900));
      addTearDown(() => tester.binding.setSurfaceSize(null));

      DevicePairingScannerResult? result;
      await tester.pumpWidget(
        MaterialApp(
          home: Builder(
            builder: (context) => Scaffold(
              body: TextButton(
                onPressed: () async {
                  result = await Navigator.of(context)
                      .push<DevicePairingScannerResult>(
                    MaterialPageRoute(
                      builder: (_) => DevicePairingScannerScreen(
                        hueBridgeSerialSearchAvailable: true,
                        cameraBuilder: (context, onDetect) {
                          detect = onDetect;
                          return const ColoredBox(color: Colors.black);
                        },
                      ),
                    ),
                  );
                },
                child: const Text('Open scanner'),
              ),
            ),
          ),
        ),
      );

      await tester.tap(find.text('Open scanner'));
      await tester.pumpAndSettle();
      detect(const ['MT:Y.K908OC16750648G00', 'E277DA']);
      await tester.pump();

      expect(result, isNull);
      expect(find.text('Choose how to add this device'), findsOneWidget);
      expect(find.text('Search with Hue Bridge'), findsOneWidget);
      expect(find.text('Pair with Matter'), findsOneWidget);

      await tester.tap(
        find.byKey(const ValueKey('choose-hue-bridge')),
      );
      await tester.pumpAndSettle();

      expect(result?.action, DevicePairingScannerAction.hueBridge);
      expect(result?.payload, 'E277DA');
    },
  );

  testWidgets('Hue serial guides to nearby scan without a connected bridge', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(430, 900));
    addTearDown(() => tester.binding.setSurfaceSize(null));

    DevicePairingScannerResult? result;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: TextButton(
              onPressed: () async {
                result = await Navigator.of(context)
                    .push<DevicePairingScannerResult>(
                  MaterialPageRoute(
                    builder: (_) => DevicePairingScannerScreen(
                      cameraBuilder: (context, onDetect) {
                        detect = onDetect;
                        return const ColoredBox(color: Colors.black);
                      },
                    ),
                  ),
                );
              },
              child: const Text('Open scanner'),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Open scanner'));
    await tester.pumpAndSettle();

    detect(const ['E277DA']);
    await tester.pump();

    expect(result, isNull);
    expect(find.text('Use Add Device'), findsOneWidget);
    expect(find.textContaining('directly'), findsOneWidget);
  });

  testWidgets('manual entry identifies codes through the shared processor',
      (tester) async {
    await tester.binding.setSurfaceSize(const Size(430, 900));
    addTearDown(() => tester.binding.setSurfaceSize(null));

    DevicePairingScannerResult? result;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: TextButton(
              onPressed: () async {
                result = await Navigator.of(context)
                    .push<DevicePairingScannerResult>(
                  MaterialPageRoute(
                    builder: (_) => const DevicePairingCodeEntryScreen(),
                  ),
                );
              },
              child: const Text('Enter manually'),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Enter manually'));
    await tester.pumpAndSettle();

    expect(find.text('Enter a Code'), findsOneWidget);
    expect(find.text('Enter any setup code'), findsOneWidget);
    expect(find.text('Identify & Continue'), findsOneWidget);

    await tester.enterText(
      find.byKey(const ValueKey('device-pairing-code-input')),
      'X-HM://0023ISYWY8H2B',
    );
    tester.testTextInput.hide();
    await tester.ensureVisible(find.text('Identify & Continue'));
    await tester.pump();
    await tester.tap(find.text('Identify & Continue'));
    await tester.pump();
    expect(find.text('HomeKit isn’t supported'), findsOneWidget);

    await tester.enterText(
      find.byKey(const ValueKey('device-pairing-code-input')),
      '3497-011-2332',
    );
    await tester.ensureVisible(find.text('Identify & Continue'));
    await tester.pump();
    await tester.tap(find.text('Identify & Continue'));
    await tester.pumpAndSettle();

    expect(result?.action, DevicePairingScannerAction.matter);
    expect(result?.payload, '3497-011-2332');
    expect(result?.inputMethod, 'manual_code');
  });

  testWidgets('manual local-BLE entry preserves journey and input method',
      (tester) async {
    const qr = 'B:0A0B0C0D0E0F%G\$S:SYNTHETIC000001\$M:TESTMODEL001';
    DevicePairingScannerResult? result;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: TextButton(
              onPressed: () async {
                result = await Navigator.of(context).push(
                  MaterialPageRoute<DevicePairingScannerResult>(
                    builder: (_) => const DevicePairingCodeEntryScreen(
                      supportedLocalBleProfileIds: {
                        RhythmDeviceProfileId.oreinOc02001Button,
                      },
                      journeyId: 'journey-manual-local-1',
                    ),
                  ),
                );
              },
              child: const Text('Enter manually'),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Enter manually'));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const ValueKey('device-pairing-code-input')),
      qr,
    );
    tester.testTextInput.hide();
    await tester.ensureVisible(find.text('Identify & Continue'));
    await tester.pump();
    await tester.tap(find.text('Identify & Continue'));
    await tester.pumpAndSettle();

    expect(result?.action, DevicePairingScannerAction.localBle);
    expect(result?.inputMethod, 'manual_code');
    expect(result?.journeyId, 'journey-manual-local-1');
    expect(
      result?.localBleSetup?.profileId,
      RhythmDeviceProfileId.oreinOc02001Button,
    );
    final event = analyticsBackend.events.singleWhere(
      (event) => event.name == 'device_pairing_code_detected',
    );
    expect(event.properties['input_method'], 'manual_code');
    expect(event.properties['journey_id'], 'journey-manual-local-1');
    expect(event.properties.values, isNot(contains(qr)));
  });

  testWidgets('manual entry routes a Hue serial through the bridge',
      (tester) async {
    DevicePairingScannerResult? result;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: TextButton(
              onPressed: () async {
                result = await Navigator.of(context)
                    .push<DevicePairingScannerResult>(
                  MaterialPageRoute(
                    builder: (_) => const DevicePairingCodeEntryScreen(
                      hueBridgeSerialSearchAvailable: true,
                    ),
                  ),
                );
              },
              child: const Text('Enter manually'),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Enter manually'));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const ValueKey('device-pairing-code-input')),
      '27f706',
    );
    tester.testTextInput.hide();
    await tester.ensureVisible(find.text('Identify & Continue'));
    await tester.pump();
    await tester.tap(find.text('Identify & Continue'));
    await tester.pumpAndSettle();

    expect(result?.action, DevicePairingScannerAction.hueBridge);
    expect(result?.payload, '27F706');
    expect(result?.inputMethod, 'manual_code');
  });

  testWidgets('bridge manual entry only accepts a Hue bulb serial',
      (tester) async {
    await tester.binding.setSurfaceSize(const Size(430, 900));
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.pumpWidget(
      const MaterialApp(
        home: DevicePairingCodeEntryScreen(
          hueBridgeSerialSearchAvailable: true,
          hueBridgeOnly: true,
        ),
      ),
    );

    expect(find.text('Enter Bulb Serial'), findsOneWidget);
    expect(find.text('Enter Hue bulb serial'), findsOneWidget);
    expect(find.text('Search with Hue Bridge'), findsOneWidget);

    await tester.enterText(
      find.byKey(const ValueKey('device-pairing-code-input')),
      '3497-011-2332',
    );
    tester.testTextInput.hide();
    await tester.ensureVisible(find.text('Search with Hue Bridge'));
    await tester.pump();
    await tester.tap(find.text('Search with Hue Bridge'));
    await tester.pump();

    expect(find.text('Hue bulb serial not found'), findsOneWidget);
  });
}
