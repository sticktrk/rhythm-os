import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/widgets/device_network_diagnostics.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  String? copiedText;
  setUp(() {
    copiedText = null;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, (call) async {
      if (call.method == 'Clipboard.setData') {
        copiedText = (call.arguments as Map)['text'] as String;
      }
      return null;
    });
  });
  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, null);
  });
  const address = '2001:db8:1234:5678:9012:3456:7890:abcd';
  const nativeId = 'synthetic-native-device-id-that-must-never-be-truncated';
  final device = <String, dynamic>{
    'id': 'synthetic-canonical-device-id',
    'hardware_ids': [
      {'type': 'mac', 'value': '00:17:88:01:09:ab:cd:ef'},
      {'type': 'serial', 'value': 'synthetic-serial'},
      {'type': 'matter_id', 'value': '1234'},
      {'type': 'future_secret', 'value': 'do-not-export-hardware'},
    ],
    'endpoints': [
      {
        'hub_key': {'hub_type': 'hue', 'address': address},
        'native_id': nativeId,
        'preferred': true,
        'last_seen': 1700000000,
        'capabilities': {'password': 'do-not-export-capabilities'},
      },
      {
        'hub_key': {'hub_type': 'matter', 'address': 'local'},
        'native_id': '42',
        'preferred': false,
        'active': false,
        'last_seen': 0,
      },
    ],
    'setup_code': 'do-not-export-setup',
    'token': 'do-not-export-token',
  };
  const hubs = <Map<String, dynamic>>[
    {'type': 'hue', 'address': 'another-bridge', 'connected': false},
    {'type': 'hue', 'address': address, 'connected': true},
    {'type': 'matter', 'address': 'another-controller', 'connected': true},
  ];

  Future<void> show(
    WidgetTester tester, {
    Map<String, dynamic>? data,
    List<Map<String, dynamic>> hubData = hubs,
    bool failed = false,
    bool refreshing = false,
  }) async {
    await tester.pumpWidget(MaterialApp(
      home: Scaffold(
        body: SingleChildScrollView(
          child: DeviceNetworkDiagnostics(
            device: data,
            hubs: hubData,
            fetchedAt: data == null ? null : DateTime.utc(2026, 9, 20, 12),
            refreshing: refreshing,
            failed: failed,
            onRefresh: () {},
          ),
        ),
      ),
    ));
    await tester.pump();
  }

  Future<String> copy(WidgetTester tester) async {
    final target = find.byKey(const ValueKey('device-network-copy'));
    await tester.ensureVisible(target);
    await tester.tap(target);
    await tester.pumpAndSettle();
    return copiedText!;
  }

  testWidgets('exports only diagnostic fields and exact connection evidence',
      (tester) async {
    await show(tester, data: device);
    final report = await copy(tester);
    expect(report, contains('Native device ID: $nativeId'));
    expect(report, contains('Hub address: $address'));
    expect(report, contains('MAC / IEEE address: 00:17:88:01:09:ab:cd:ef'));
    expect(report, contains('Serial number: synthetic-serial'));
    expect(report, contains('Matter ID: 1234'));
    expect(
        report, contains('Last seen by integration: 2023-11-14 22:13:20 UTC'));
    expect(report, contains('Discovery: Present in last discovery'));
    expect(report, contains('Discovery: Missing from last discovery'));
    expect(report, contains('Hub status (last reported): Connected'));
    expect(report, contains('Hub status (last reported): Not reported'));
    expect(report, isNot(contains('Disconnected')));
    expect(report, isNot(contains('do-not-export')));
    expect(report, contains('not a live reachability test'));
  });

  testWidgets('long network values wrap and expand on a narrow phone',
      (tester) async {
    await tester.binding.setSurfaceSize(const Size(320, 700));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await show(tester, data: device);
    await tester.tap(find.text('Hue Bridge'));
    await tester.pumpAndSettle();
    expect(find.text(nativeId), findsOneWidget);
    expect(find.text(address), findsOneWidget);
    final native = tester.getRect(find.text(nativeId));
    expect(native.left, greaterThanOrEqualTo(0));
    expect(native.right, lessThanOrEqualTo(320));
    expect(native.height, greaterThan(20));
    expect(tester.takeException(), isNull);
    final identifiers = find.text('Device identifiers');
    await tester.ensureVisible(identifiers);
    await tester.tap(identifiers);
    await tester.pumpAndSettle();
    expect(find.text('00:17:88:01:09:ab:cd:ef'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('older and malformed optional metadata stays explicit',
      (tester) async {
    await show(tester, data: {
      'id': 'legacy-device',
      'endpoints': [
        {
          'hub_key': {'hub_type': 'hue', 'address': 'legacy-bridge'},
          'native_id': 'old-native-id',
          'last_seen': double.infinity,
        },
        {
          'hub_key': {'hub_type': 'future', 'address': 'future-bridge'},
          'active': 'invalid',
          'last_seen': 999999999999999,
        },
      ],
    });
    final report = await copy(tester);
    expect(report, contains('Discovery: Present in last discovery'));
    expect(report, contains('Discovery: Not reported'));
    expect(report, contains('Last seen by integration: Not reported'));
    expect(report, isNot(contains('Connected')));
    expect(report, isNot(contains('Online')));
    expect(tester.takeException(), isNull);
  });

  testWidgets('failed and empty snapshots have honest retry and copy states',
      (tester) async {
    await show(tester, failed: true);
    expect(find.textContaining('Tap Refresh to retry'), findsOneWidget);
    expect(
        tester
            .widget<TextButton>(
                find.byKey(const ValueKey('device-network-copy')))
            .onPressed,
        isNull);
    await show(tester, data: device, failed: true);
    expect(find.textContaining('Showing the last loaded snapshot'),
        findsOneWidget);
    expect(await copy(tester), contains('Refresh failed'));
    await show(tester, data: {'id': 'empty', 'endpoints': []});
    expect(find.text('No connections reported.'), findsOneWidget);
    expect(await copy(tester), isNot(contains(nativeId)));
  });

  testWidgets(
      'pending refresh disables repeat actions and clipboard failure recovers',
      (tester) async {
    await show(tester, data: device, refreshing: true);
    for (final key in ['device-network-refresh', 'device-network-copy']) {
      expect(tester.widget<TextButton>(find.byKey(ValueKey(key))).onPressed,
          isNull);
    }
    await show(tester, data: device);
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) async {
        if (call.method == 'Clipboard.setData') {
          throw PlatformException(code: 'unavailable');
        }
        return null;
      },
    );
    addTearDown(() => tester.binding.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, null));
    await tester.tap(find.byKey(const ValueKey('device-network-copy')));
    await tester.pumpAndSettle();
    expect(find.text('Could not copy network diagnostics'), findsOneWidget);
    expect(
        tester
            .widget<TextButton>(
                find.byKey(const ValueKey('device-network-copy')))
            .onPressed,
        isNotNull);
    expect(tester.takeException(), isNull);
  });
}
