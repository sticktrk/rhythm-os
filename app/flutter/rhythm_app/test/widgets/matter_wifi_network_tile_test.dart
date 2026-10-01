import 'dart:async';
import 'dart:io';
import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:rhythm_app/services/cloud_backed_server_api.dart';
import 'package:rhythm_app/widgets/matter_wifi_network_tile.dart';

class NetworkApi extends CloudBackedServerApi {
  NetworkApi() : super(delegate: RhythmServerApi(Dio()));
  Future<RhythmWifiNetwork> Function() read = () async =>
      const RhythmWifiNetwork('connected', ssid: 'Observed fixture');
  int calls = 0;
  @override
  Future<RhythmWifiNetwork> getMatterWifiNetwork(String id) {
    calls++;
    return read();
  }
}

void main() {
  Future<void> show(WidgetTester tester, NetworkApi api,
      {int refresh = 0, bool changing = false}) async {
    await tester.pumpWidget(MaterialApp(
        home: Scaffold(
            body: MatterWifiNetworkTile(
                api: api,
                deviceId: 'matter-1',
                refreshToken: refresh,
                changing: changing))));
    await tester.pump();
  }

  testWidgets('shows device observation and clears it on a failed refresh',
      (tester) async {
    final api = NetworkApi();
    await show(tester, api);
    expect(find.text('Current Wi-Fi'), findsOneWidget);
    expect(find.text('Observed fixture'), findsOneWidget);
    api.read = () async => throw const RhythmWifiException('unavailable');
    await tester.tap(find.byTooltip('Refresh current Wi-Fi'));
    await tester.pumpAndSettle();
    expect(find.text('Observed fixture'), findsNothing);
    expect(find.text('Could not read the bulb’s Wi-Fi.'), findsOneWidget);
  });
  testWidgets('late reads cannot overwrite a changed network or pending move',
      (tester) async {
    final api = NetworkApi();
    final stale = Completer<RhythmWifiNetwork>();
    api.read = () => stale.future;
    await show(tester, api);
    api.read =
        () async => const RhythmWifiNetwork('connected', ssid: 'New fixture');
    await show(tester, api, refresh: 1);
    stale.complete(const RhythmWifiNetwork('connected', ssid: 'Stale fixture'));
    await tester.pumpAndSettle();
    expect(find.text('New fixture'), findsOneWidget);
    expect(find.text('Stale fixture'), findsNothing);
    await show(tester, api, refresh: 1, changing: true);
    expect(find.text('New fixture'), findsNothing);
    expect(find.textContaining('Changing network'), findsOneWidget);
    expect(api.calls, 2);
  });
  testWidgets(
      'old firmware, offline and unsupported states never guess a saved SSID',
      (tester) async {
    final api = NetworkApi();
    for (final status in [
      'offline',
      'unsupported',
      'unsupported_server',
      'unavailable',
      'busy'
    ]) {
      api.read = () async => RhythmWifiNetwork(status);
      await show(tester, api, refresh: status.hashCode);
      expect(find.text('Observed fixture'), findsNothing);
      expect(find.byKey(const ValueKey('bulb-current-wifi-value')),
          findsOneWidget);
      expect(tester.takeException(), isNull);
    }
  });
  testWidgets(
      'long SSID wraps on a narrow screen and change remains accessible',
      (tester) async {
    tester.view.physicalSize = const Size(320, 640);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final api = NetworkApi()
      ..read = () async => const RhythmWifiNetwork('connected',
          ssid: 'Thirty-two-byte-network-name-test');
    var opened = false;
    await tester.pumpWidget(MaterialApp(
        debugShowCheckedModeBanner: false,
        theme: ThemeData.dark(),
        home: Scaffold(
            body: MatterWifiNetworkTile(
                api: api,
                deviceId: 'matter-1',
                onTap: () async {
                  opened = true;
                }))));
    await tester.pumpAndSettle();
    expect(
        tester.getSize(find.byKey(const ValueKey('bulb-current-wifi'))).height,
        lessThan(200));
    final screenshotPath =
        Platform.environment['RHYTHM_MATTER_WIFI_NETWORK_SCREENSHOT'];
    if (screenshotPath != null && screenshotPath.isNotEmpty) {
      await expectLater(
          find.byType(MaterialApp), matchesGoldenFile(screenshotPath));
    }
    await tester.tap(find.text('Change network'));
    await tester.pumpAndSettle();
    expect(opened, isTrue);
    expect(tester.takeException(), isNull);
  });
}
