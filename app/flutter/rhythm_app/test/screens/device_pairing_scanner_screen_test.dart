import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/screens/hubs/device_pairing_code_entry_screen.dart';
import 'package:rhythm_app/screens/hubs/device_pairing_scanner_screen.dart';
import 'package:rhythm_app/services/analytics_service.dart';

import '../helpers/capturing_analytics_backend.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late ValueChanged<Iterable<String>> detect;
  late CapturingAnalyticsBackend analyticsBackend;

  Widget buildScanner() {
    return MaterialApp(
      home: DevicePairingScannerScreen(
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
      expect(find.text('Pair this device in the Hue app'), findsOneWidget);

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
    });
    expect(
      event.properties.values,
      isNot(contains('MT:Y.K908OC16750648G00')),
    );
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
}
