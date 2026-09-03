import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/room_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_app/screens/hubs/bulb_audition_screen.dart';
import 'package:rhythm_app/screens/hubs/matter_bulb_tester_screen.dart'
    show MatterBulbTesterScreen, preferredMatterColorCommand;
import 'package:rhythm_app/services/bulb_audition_service.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _FakeHomeProvider extends HomeProvider {
  @override
  List<Hub> get currentHomeHubs => const [];

  @override
  Hub? getFirstHubOfType(HubType type) => null;

  @override
  Future<void> onUserSignIn() async {}
}

class _FakeRhythmServerApi extends RhythmServerApi {
  _FakeRhythmServerApi() : super(Dio());

  final runCalls = <Map<String, dynamic>>[];
  Map<String, dynamic>? savedReport;
  bool? savedApplyLocal;

  @override
  Future<Map<String, dynamic>?> runBulbAudition({
    required String deviceId,
    required String scenario,
    String? journeyId,
    String? baseScenario,
    Map<String, dynamic>? profileOverride,
    String? legacyTest,
  }) async {
    runCalls.add({
      'scenario': scenario,
      if (baseScenario != null) 'base_scenario': baseScenario,
      if (profileOverride != null) 'profile_override': profileOverride,
    });
    final profile = <String, dynamic>{
      'schema_version': 1,
      'color_route': 'hue_saturation',
      'turn_on': 'stage_color_then_level_with_on_off',
      'level_command': 'move_to_level_with_on_off',
      'command_spacing_ms': {
        'value_ms': 0,
        'basis': 'assumed',
        'source': 'safe_default',
      },
      'execute_if_off_honoured': true,
      'on_restores_previous': false,
      'power_on_behavior': 'unknown',
      'supports_transition': true,
      'source': <String, dynamic>{
        'color_route': 'safe_default',
        'turn_on': 'safe_default',
        'level_command': 'safe_default',
        'supports_transition': 'safe_default',
      },
    };
    for (final entry in (profileOverride ?? const {}).entries) {
      if (entry.key == 'command_spacing_ms') {
        profile[entry.key] = {
          'value_ms': entry.value,
          'basis': 'assumed',
          'source': 'try_with',
        };
      } else {
        profile[entry.key] = entry.value;
        (profile['source'] as Map<String, dynamic>)[entry.key] = 'try_with';
      }
    }
    return {
      'status': 'ok',
      'needs_audition': false,
      'profile_used': profile,
      if (scenario == 'command_spacing')
        'command_spacing_measurement': {
          'value_ms': 50,
          'basis': 'measured',
          'source': 'audition',
        },
      'reported': {
        'after_1500ms': {
          'onoff': {'ok': true, 'value': true},
          'current_level': {'ok': true, 'value': 76},
          'color_temperature_mireds': {'ok': true, 'value': 250},
        },
      },
    };
  }

  @override
  Future<Map<String, dynamic>?> saveBulbAuditionReport(
    Map<String, dynamic> report, {
    bool applyLocal = true,
  }) async {
    savedReport = report;
    savedApplyLocal = applyLocal;
    return {'status': 'saved', 'applied_local': applyLocal};
  }
}

class _TestRhythmConnection extends RhythmConnection {
  _TestRhythmConnection() : api = _FakeRhythmServerApi();

  @override
  final _FakeRhythmServerApi api;
}

class _FakeBulbAuditionReportService implements BulbAuditionReportService {
  final savedReports = <Map<String, dynamic>>[];

  @override
  Future<void> saveLocalReport(Map<String, dynamic> report) async {
    savedReports.add(report);
  }

  @override
  Future<BulbAuditionCloudResult> submitCloudReport({
    required Map<String, dynamic> report,
    String? serverVersion,
    String? serverPlatformContext,
  }) async {
    return const BulbAuditionCloudResult(uploaded: false);
  }
}

class _BulbAuditionHarness {
  _BulbAuditionHarness()
      : connection = _TestRhythmConnection(),
        roomProvider = RoomProvider(),
        reportService = _FakeBulbAuditionReportService() {
    serverSync = ServerSyncProvider(
      connection: connection,
      roomProvider: roomProvider,
      homeProvider: _FakeHomeProvider(),
    );
  }

  final _TestRhythmConnection connection;
  final RoomProvider roomProvider;
  final _FakeBulbAuditionReportService reportService;
  late final ServerSyncProvider serverSync;

  _FakeRhythmServerApi get api => connection.api;

  Future<void> pump(WidgetTester tester, RhythmDevice device) async {
    await tester.pumpWidget(
      ChangeNotifierProvider<ServerSyncProvider>.value(
        value: serverSync,
        child: MaterialApp(
          home: MatterBulbTesterScreen(
            device: device,
            nativeDeviceId: 'matter-1-1',
            reportService: reportService,
          ),
        ),
      ),
    );
  }

  void dispose() {
    serverSync.dispose();
    roomProvider.dispose();
    connection.dispose();
  }
}

void main() {
  const device = RhythmDevice(
    id: 'matter-light-1',
    type: RhythmDeviceType.light,
    name: 'Test bulb',
    manufacturer: 'Example',
    model: 'Color A19',
  );

  Future<void> pumpTester(WidgetTester tester) async {
    await tester.pumpWidget(
      const MaterialApp(
        home: BulbAuditionScreen(
          device: device,
          nativeDeviceId: 'matter-1-1',
        ),
      ),
    );
  }

  Future<void> selectStep(WidgetTester tester, String id) async {
    final step = find.byKey(ValueKey('matter-bulb-test-step-$id'));
    await tester.ensureVisible(step);
    await tester.pumpAndSettle();
    await tester.tap(step);
    await tester.pump();
    tester
        .state<ScrollableState>(find.byType(Scrollable).first)
        .position
        .jumpTo(0);
    await tester.pumpAndSettle();
  }

  Future<void> runTryWith(WidgetTester tester, String label) async {
    await tester.tap(find.byKey(const ValueKey('bulb-audition-try-with')));
    await tester.pumpAndSettle();
    await tester.tap(find.text(label).last);
    await tester.pumpAndSettle();
  }

  group('preferred Matter color command', () {
    test('keeps adaptive whites on native CT when direct color also works', () {
      expect(
        preferredMatterColorCommand(
          colorTemperatureWorked: true,
          hueSaturationWorked: true,
          xyWorked: false,
        ),
        'color_temperature',
      );
    });

    test('falls back through HS, XY, then dimming only', () {
      expect(
        preferredMatterColorCommand(
          colorTemperatureWorked: false,
          hueSaturationWorked: true,
          xyWorked: true,
        ),
        'hue_saturation',
      );
      expect(
        preferredMatterColorCommand(
          colorTemperatureWorked: false,
          hueSaturationWorked: false,
          xyWorked: true,
        ),
        'xy',
      );
      expect(
        preferredMatterColorCommand(
          colorTemperatureWorked: false,
          hueSaturationWorked: false,
          xyWorked: false,
        ),
        'onoff_or_dimming_only',
      );
    });
  });

  testWidgets('shows the 16-scenario audition and skips answers for preflight',
      (tester) async {
    await pumpTester(tester);

    expect(find.text('1/16'), findsOneWidget);
    expect(find.text('Preflight'), findsAtLeastNWidgets(1));
    expect(find.text('Yes'), findsNothing);
    expect(find.text('No'), findsNothing);

    await tester.drag(find.byType(ListView), const Offset(0, -600));
    await tester.pumpAndSettle();
    final fromOffStep =
        find.byKey(const ValueKey('matter-bulb-test-step-turn_on_from_off'));
    await tester.ensureVisible(fromOffStep);
    await tester.pumpAndSettle();
    await tester.tap(fromOffStep);
    await tester.pump();
    tester
        .state<ScrollableState>(find.byType(Scrollable).first)
        .position
        .jumpTo(0);
    await tester.pumpAndSettle();

    expect(find.text('3/16'), findsOneWidget);
    expect(find.text('Yes'), findsOneWidget);
    expect(find.text('No'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('bulb-audition-try-with')),
      findsOneWidget,
    );
    await tester.tap(find.byKey(const ValueKey('bulb-audition-try-with')));
    await tester.pumpAndSettle();
    expect(find.text('Explicit On first'), findsOneWidget);
    expect(
      find.text('Audition turns the bulb off, then runs the runtime plan.'),
      findsAtLeastNWidgets(1),
    );
  });

  testWidgets('includes subscription and power-cycle scenarios',
      (tester) async {
    await pumpTester(tester);

    await tester.drag(find.byType(ListView), const Offset(0, -600));
    await tester.pumpAndSettle();
    expect(
      find.byKey(
        const ValueKey('matter-bulb-test-step-subscription_establish'),
      ),
      findsOneWidget,
    );
    expect(
      find.byKey(
        const ValueKey('matter-bulb-test-step-power_cycle_then_tick'),
      ),
      findsOneWidget,
    );
  });

  testWidgets('accepted Try-with fields compose and allow a local save',
      (tester) async {
    await tester.binding.setSurfaceSize(const Size(430, 1000));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final harness = _BulbAuditionHarness();
    addTearDown(harness.dispose);
    await harness.pump(tester, device);

    await selectStep(tester, 'turn_on_from_off');
    await tester.tap(find.text('Run real plan'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('No'));
    await tester.pumpAndSettle();
    await selectStep(tester, 'turn_on_from_off');
    await runTryWith(tester, 'Explicit On first');
    await tester.tap(find.text('Yes'));
    await tester.pumpAndSettle();

    await selectStep(tester, 'tick_while_on');
    await runTryWith(tester, 'XY colour');
    await tester.tap(find.text('Yes'));
    await tester.pumpAndSettle();

    final tryCalls = harness.api.runCalls
        .where((call) => call['scenario'] == 'try_with')
        .toList();
    expect(tryCalls, hasLength(2));
    expect(
      tryCalls.last['profile_override'],
      containsPair('turn_on', 'explicit_on_first'),
    );
    expect(
      tryCalls.last['profile_override'],
      containsPair('color_route', 'xy'),
    );

    final save = find.text('Save Results');
    await tester.scrollUntilVisible(
      save,
      500,
      scrollable: find.byType(Scrollable).first,
    );
    await tester.pumpAndSettle();
    await tester.tap(save);
    await tester.pumpAndSettle();

    expect(harness.api.savedApplyLocal, isTrue);
    final profile = Map<String, dynamic>.from(
      harness.api.savedReport!['control_profile'] as Map,
    );
    expect(profile['turn_on'], 'explicit_on_first');
    expect(profile['color_route'], 'xy');
    expect(profile['source'], containsPair('turn_on', 'try_with'));
    expect(profile['source'], containsPair('color_route', 'try_with'));
  });

  testWidgets('Reported renders the authoritative 1500 ms readback',
      (tester) async {
    await tester.binding.setSurfaceSize(const Size(430, 1000));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final harness = _BulbAuditionHarness();
    addTearDown(harness.dispose);
    await harness.pump(tester, device);

    await selectStep(tester, 'turn_on_from_off');
    await tester.tap(find.text('Run real plan'));
    await tester.pumpAndSettle();

    expect(find.text('Reported'), findsOneWidget);
    expect(find.text('On · 30 % · 4000 K'), findsOneWidget);
  });

  testWidgets('measured command spacing survives later scenario results',
      (tester) async {
    await tester.binding.setSurfaceSize(const Size(430, 1000));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final harness = _BulbAuditionHarness();
    addTearDown(harness.dispose);
    await harness.pump(tester, device);

    await selectStep(tester, 'command_spacing');
    await tester.tap(find.text('Measure command spacing'));
    await tester.pumpAndSettle();
    await selectStep(tester, 'preflight');
    await tester.tap(find.text('Read attributes'));
    await tester.pumpAndSettle();

    final save = find.text('Save Results');
    await tester.scrollUntilVisible(
      save,
      500,
      scrollable: find.byType(Scrollable).first,
    );
    await tester.tap(save);
    await tester.pumpAndSettle();

    expect(
      harness.api.savedReport!['control_profile']['command_spacing_ms'],
      {
        'value_ms': 50,
        'basis': 'measured',
        'source': 'audition',
      },
    );
  });
}
