import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
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

    expect(find.text('Add Bulb'), findsOneWidget);
    expect(find.byTooltip('Back to Add & Review'), findsOneWidget);
    expect(find.text('Enter a Code'), findsOneWidget);
  });

  testWidgets('fits the camera actions on a short phone', (tester) async {
    await tester.binding.setSurfaceSize(const Size(320, 568));
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.pumpWidget(buildScanner());

    expect(find.text('Scan the code on your bulb'), findsOneWidget);
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
      expect(find.text('Pair this bulb in the Hue app'), findsOneWidget);

      await tester.tap(find.text('Scan Another Code'));
      await tester.pump();
      detect(const ['https://example.com/not-a-pairing-code']);
      await tester.pump();
      expect(find.text('Unknown QR code'), findsOneWidget);
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
}
