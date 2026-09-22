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
  // Saved-network check: the Box is unreachable (null) before it answers.
  List<RhythmWifiCheck?> checkAnswers = [];
  final checked = <String>[];
  @override
  Future<RhythmWifiCheck> startWifiCheck({
    required String operationId,
    required String ssid,
    required String password,
  }) async {
    checked.add(ssid);
    return checkAnswers.isEmpty
        ? RhythmWifiCheck.unavailable
        : const RhythmWifiCheck(RhythmWifiCheckState.running);
  }

  @override
  Future<RhythmWifiCheck?> getWifiCheck(String operationId) async =>
      checkAnswers.removeAt(0);
  String startError = 'unavailable';
  @override
  Future<RhythmWifiProfiles> getWifiProfiles() async => RhythmWifiProfiles(
        revision: 1,
        defaultId: defaultId,
        boxProfileId: 'main',
        profiles: const [
          RhythmWifiProfile(id: 'main', ssid: 'Main fixture'),
          RhythmWifiProfile(id: 'ext', ssid: 'Extender fixture'),
          RhythmWifiProfile(id: 'spare', ssid: 'Spare fixture'),
        ],
      );
  String defaultId = 'main';
  @override
  Future<RhythmWifiProfiles> updateWifiProfile({
    required int revision,
    required String action,
    required String correlationId,
    String? id,
    String? ssid,
    String? password,
  }) async {
    changes.add('$action:$id');
    if (action == 'default') defaultId = id!;
    return getWifiProfiles();
  }

  @override
  Future<RhythmWifiChangeReceipt?> getLatestMatterWifiChange(
    String deviceId,
  ) async {
    if (failStatus) throw StateError('offline');
    return receipt;
  }

  @override
  Future<RhythmWifiChangeReceipt> getMatterWifiChange(
    String operationId,
  ) async {
    if (failStatus) throw StateError('offline');
    return receipt!;
  }

  @override
  Future<RhythmWifiChangeReceipt> startMatterWifiChange({
    required String operationId,
    required String deviceId,
    required String profileId,
  }) async {
    starts++;
    throw RhythmWifiException(startError);
  }
}

void main() {
  testWidgets('settings choose the default without touching the Box network', (
    tester,
  ) async {
    final api = WifiApi();
    await tester.pumpWidget(MaterialApp(home: SavedWifiScreen(api: api)));
    await tester.pumpAndSettle();
    expect(
      find.text('Default for new accessories · Rhythm Box connection'),
      findsOneWidget,
    );
    // The Box connection follows the Box; it has no edit or remove menu.
    expect(find.byKey(const ValueKey('wifi-profile-menu-main')), findsNothing);

    await tester.tap(find.text('Extender fixture'));
    await tester.pumpAndSettle();
    expect(api.changes, ['default:ext']);
    expect(find.text('Rhythm Box connection'), findsOneWidget);
    expect(find.text('Default for new accessories'), findsOneWidget);
    // Tapping the current default is not another write.
    await tester.tap(find.text('Extender fixture'));
    await tester.pumpAndSettle();
    expect(api.changes, ['default:ext']);

    // Removing the default asks for its successor first, so new accessories
    // are never left without a network and the owner makes the choice.
    await tester.tap(find.byKey(const ValueKey('wifi-profile-menu-ext')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Remove saved network'));
    await tester.pumpAndSettle();
    expect(api.changes, ['default:ext']);
    await tester.tap(find.byKey(const ValueKey('wifi-successor-spare')));
    await tester.pumpAndSettle();
    expect(api.changes, ['default:ext', 'default:spare', 'remove:ext']);
    // Any other saved network is removed directly.
    await tester.tap(find.byKey(const ValueKey('wifi-profile-menu-ext')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Remove saved network'));
    await tester.pumpAndSettle();
    expect(api.changes.last, 'remove:ext');
    expect(api.changes.length, 4);
  });
  for (final (answers, dialog, saves) in [
    // Offline mid-check, then proven.
    (
      <RhythmWifiCheck?>[
        null,
        const RhythmWifiCheck(RhythmWifiCheckState.passed)
      ],
      null,
      true
    ),
    (
      <RhythmWifiCheck?>[
        const RhythmWifiCheck(RhythmWifiCheckState.failed,
            reason: 'join_failed')
      ],
      'password is probably wrong',
      false
    ),
    (
      <RhythmWifiCheck?>[
        const RhythmWifiCheck(RhythmWifiCheckState.failed, reason: 'not_found')
      ],
      'could not see this network',
      true
    ),
    // A Box that cannot check never blocks the save.
    (<RhythmWifiCheck?>[], null, true),
  ]) {
    testWidgets('a new network is proven before it is saved: $dialog $saves', (
      tester,
    ) async {
      final api = WifiApi()..checkAnswers = answers;
      await tester.pumpWidget(MaterialApp(
          home: SavedWifiScreen(
              api: api, checkPollInterval: const Duration(milliseconds: 10))));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('wifi-add')));
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(TextField).first, 'Garage fixture');
      await tester.enterText(find.byType(TextField).last, 'fixture-password');
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      // Nothing happens to the Box until the owner accepts the downtime.
      expect(api.checked, isEmpty);
      await tester.tap(find.text('Check network'));
      await tester.pump();
      if (answers.isNotEmpty) {
        expect(find.byKey(const ValueKey('wifi-checking')), findsOneWidget);
      }
      await tester.pumpAndSettle();
      expect(api.checked, ['Garage fixture']);
      if (dialog != null) {
        expect(find.textContaining(dialog), findsOneWidget);
        await tester.tap(find.text(saves ? 'Save anyway' : 'Cancel'));
        await tester.pumpAndSettle();
      }
      expect(api.changes, saves ? ['save:null'] : isEmpty);
      expect(find.byKey(const ValueKey('wifi-checking')), findsNothing);
    });
  }
  testWidgets('alternate selection does not change the default', (
    tester,
  ) async {
    final api = WifiApi();
    String? chosen;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: TextButton(
              onPressed: () async {
                chosen = await SavedWifiScreen.select(context, api);
              },
              child: const Text('Open'),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    expect(find.textContaining('Default for new accessories'), findsOneWidget);
    await tester.tap(find.text('Extender fixture'));
    await tester.tap(find.byKey(const ValueKey('wifi-use')));
    await tester.pumpAndSettle();
    expect(chosen, 'ext');
    expect(api.changes, isEmpty);
  });
  testWidgets('pending receipt resumes without repeating a change', (
    tester,
  ) async {
    final api = WifiApi()
      ..receipt = const RhythmWifiChangeReceipt(
        operationId: 'old',
        status: 'pending',
      );
    await tester.pumpWidget(
      MaterialApp(
        home: MatterWifiChangeScreen(api: api, deviceId: 'matter-1'),
      ),
    );
    await tester.pump();
    await tester.pump();
    expect(
      tester
          .widget<FilledButton>(find.byKey(const ValueKey('wifi-change-start')))
          .onPressed,
      isNull,
    );
    api.failStatus = true;
    await tester.pump(const Duration(seconds: 4));
    await tester.pump();
    expect(find.textContaining('could not be checked'), findsOneWidget);
    expect(api.starts, 0);
    await tester.pumpWidget(const SizedBox());
  });
  testWidgets('uncertain POST keeps status-only reconciliation', (
    tester,
  ) async {
    final api = WifiApi();
    await tester.pumpWidget(
      MaterialApp(
        home: MatterWifiChangeScreen(api: api, deviceId: 'matter-1'),
      ),
    );
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
          .widget<FilledButton>(find.byKey(const ValueKey('wifi-change-start')))
          .onPressed,
      isNull,
    );
  });
  testWidgets('definitive rejection refreshes status without resending', (
    tester,
  ) async {
    final api = WifiApi()..startError = 'conflict';
    await tester.pumpWidget(
      MaterialApp(
        home: MatterWifiChangeScreen(api: api, deviceId: 'matter-1'),
      ),
    );
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
          .widget<FilledButton>(find.byKey(const ValueKey('wifi-change-start')))
          .onPressed,
      isNotNull,
    );
  });
}
