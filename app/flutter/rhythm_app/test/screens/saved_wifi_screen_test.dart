import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:rhythm_app/services/cloud_backed_server_api.dart';
import 'package:rhythm_app/screens/network/saved_wifi_screen.dart';
import 'package:rhythm_app/screens/network/matter_wifi_change_screen.dart';

class WifiApi extends CloudBackedServerApi {
  WifiApi() : super(delegate: RhythmServerApi(Dio()));
  final changes = <String>[];
  RhythmWifiChangeReceipt? receipt;
  bool failStatus = false;
  int starts = 0;
  String startError = 'unavailable';
  @override
  Future<RhythmWifiProfiles> getWifiProfiles() async =>
      const RhythmWifiProfiles(revision: 1, defaultId: 'main', profiles: [
        RhythmWifiProfile(id: 'main', ssid: 'Main fixture'),
        RhythmWifiProfile(id: 'ext', ssid: 'Extender fixture')
      ]);
  @override
  Future<RhythmWifiProfiles> updateWifiProfile(
      {required int revision,
      required String action,
      required String correlationId,
      String? id,
      String? ssid,
      String? password}) async {
    changes.add(action);
    return getWifiProfiles();
  }

  @override
  Future<RhythmWifiChangeReceipt?> getLatestMatterWifiChange(
      String deviceId) async {
    if (failStatus) throw StateError('offline');
    return receipt;
  }

  @override
  Future<RhythmWifiChangeReceipt> getMatterWifiChange(
      String operationId) async {
    if (failStatus) throw StateError('offline');
    return receipt!;
  }

  @override
  Future<RhythmWifiChangeReceipt> startMatterWifiChange(
      {required String operationId,
      required String deviceId,
      required String profileId}) async {
    starts++;
    throw RhythmWifiException(startError);
  }
}

void main() {
  testWidgets('alternate selection does not change the default',
      (tester) async {
    final api = WifiApi();
    String? chosen;
    await tester.pumpWidget(MaterialApp(
        home: Builder(
            builder: (context) => Scaffold(
                body: TextButton(
                    onPressed: () async {
                      chosen = await SavedWifiScreen.select(context, api);
                    },
                    child: const Text('Open'))))));
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    expect(find.text('Provisioning default'), findsOneWidget);
    await tester.tap(find.text('Extender fixture'));
    await tester.tap(find.byKey(const ValueKey('wifi-use')));
    await tester.pumpAndSettle();
    expect(chosen, 'ext');
    expect(api.changes, isEmpty);
  });
  testWidgets('pending receipt resumes without repeating a change',
      (tester) async {
    final api = WifiApi()
      ..receipt =
          const RhythmWifiChangeReceipt(operationId: 'old', status: 'pending');
    await tester.pumpWidget(MaterialApp(
        home: MatterWifiChangeScreen(api: api, deviceId: 'matter-1')));
    await tester.pump();
    await tester.pump();
    expect(
        tester
            .widget<FilledButton>(
                find.byKey(const ValueKey('wifi-change-start')))
            .onPressed,
        isNull);
    api.failStatus = true;
    await tester.pump(const Duration(seconds: 4));
    await tester.pump();
    expect(find.textContaining('could not be checked'), findsOneWidget);
    expect(api.starts, 0);
    await tester.pumpWidget(const SizedBox());
  });
  testWidgets('uncertain POST keeps status-only reconciliation',
      (tester) async {
    final api = WifiApi();
    await tester.pumpWidget(MaterialApp(
        home: MatterWifiChangeScreen(api: api, deviceId: 'matter-1')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('wifi-change-start')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('wifi-use')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Change network'));
    await tester.pumpAndSettle();
    expect(api.starts, 1);
    expect(find.textContaining('uncertain'), findsOneWidget);
    expect(
        tester
            .widget<FilledButton>(
                find.byKey(const ValueKey('wifi-change-start')))
            .onPressed,
        isNull);
  });
  testWidgets('definitive rejection refreshes status without resending',
      (tester) async {
    final api = WifiApi()..startError = 'conflict';
    await tester.pumpWidget(MaterialApp(
        home: MatterWifiChangeScreen(api: api, deviceId: 'matter-1')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('wifi-change-start')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('wifi-use')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Change network'));
    await tester.pumpAndSettle();
    expect(find.textContaining('not accepted'), findsOneWidget);
    await tester.tap(find.text('Check status'));
    await tester.pumpAndSettle();
    expect(api.starts, 1);
    expect(
        tester
            .widget<FilledButton>(
                find.byKey(const ValueKey('wifi-change-start')))
            .onPressed,
        isNotNull);
  });
}
