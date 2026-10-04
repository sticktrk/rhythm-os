import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:dio/dio.dart';
import 'package:rhythm_app/screens/hubs/ha_matter_management_screen.dart';
import 'package:rhythm_app/services/ha_matter_pairing_journal.dart';
import 'package:rhythm_app/services/phone_matter_commissioner.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _Api extends RhythmHaMatterApi {
  _Api() : super(Dio());
  String receiptStatus = 'unknown';
  bool needsConfirmation = false;
  bool canClose = false;
  final devices = <Map<String, Object?>>[];
  HaMatterDevice? confirmedDevice;
  int pairRequests = 0;
  final receiptQueries = <String>[];
  @override
  String get baseUrl => 'http://addon.invalid';
  @override
  String? get handoffAuthToken => 'owner-token';
  @override
  Future<HaMatterCatalog> getCatalog() async => HaMatterCatalog.fromJson({
        'schema_version': 1,
        'available': true,
        'capabilities': {
          'pair_on_network': true,
          'phone_commissioning': true,
          'original_setup_code': true
        },
        'devices': devices,
      });
  @override
  Future<HaMatterPairingReceipt> getPairing(String id) async {
    receiptQueries.add(id);
    return HaMatterPairingReceipt.fromJson({
      'session_id': id,
      'status': receiptStatus,
      'needs_device_confirmation': needsConfirmation,
      'can_close_attempt': canClose
    });
  }

  @override
  Future<HaMatterPairingReceipt> confirmDevice(
      String id, HaMatterDevice device) async {
    confirmedDevice = device;
    return HaMatterPairingReceipt.fromJson({
      'session_id': id,
      'status': 'completed',
      'original_code_saved': true,
      'needs_device_confirmation': false
    });
  }

  @override
  Future<String?> getOriginalCode(HaMatterDevice device) async =>
      'MT:ORIGINAL-LABEL';
  @override
  Future<HaMatterPairingReceipt> pair(
      {required String sessionId,
      required String setupCode,
      required HaMatterCodeSource codeSource}) async {
    pairRequests++;
    throw const HaMatterRequestException(null);
  }
}

class _Journal extends HaMatterPairingJournal {
  _Journal([this.session]) : super('home:addon');
  String? session;
  final writes = <String>[];
  @override
  String? read() => session;
  @override
  Future<void> save(String value) async {
    session = value;
    writes.add(value);
  }

  @override
  Future<void> clear(String value) async {
    if (session == value) session = null;
  }
}

class _Phone extends PhoneMatterCommissioner {
  _Phone({this.supported = false});
  final bool supported;
  PhoneMatterBackend? backendUsed;
  @override
  Future<bool> isSupported() async => supported;
  @override
  Future<RhythmMatterPairingResponse> commission(
      {required String baseUrl,
      required String originalSetupPayload,
      required String sessionId,
      String? authToken,
      PhoneMatterBackend backend = PhoneMatterBackend.rhythm,
      HaMatterCodeSource codeSource = HaMatterCodeSource.originalLabel,
      bool Function()? isCurrentTarget}) async {
    backendUsed = backend;
    throw const PhoneMatterCommissioningException(
        stage: 'handoff', message: 'Lost native callback');
  }
}

void main() {
  const previous = '12345678-1234-4234-8234-123456789012';
  late _Api api;
  late _Journal journal;
  late bool active;
  setUp(() {
    api = _Api();
    journal = _Journal();
    active = true;
  });
  Future<void> open(WidgetTester tester, {_Phone? phone}) async {
    await tester.pumpWidget(MaterialApp(
        home: HaMatterManagementScreen(
            api: api,
            journal: journal,
            isCurrentTarget: () => active,
            phoneCommissioner: phone ?? _Phone())));
    await tester.pumpAndSettle();
  }

  testWidgets('restores unresolved pointer and never sends another pair',
      (tester) async {
    journal.session = previous;
    api.receiptStatus = 'unknown';
    await open(tester);
    expect(find.textContaining('outcome is not known'), findsOneWidget);
    final add = tester.widget<FilledButton>(
        find.widgetWithText(FilledButton, 'Add device already on the network'));
    expect(add.onPressed, isNull);
    await tester.tap(find.text('Check pairing result'));
    await tester.pumpAndSettle();
    expect(api.receiptQueries, [previous, previous]);
    expect(journal.session, previous);
    expect(api.pairRequests, 0);
  });
  testWidgets('pair timeout retains only session pointer and fences retry',
      (tester) async {
    await open(tester);
    await tester.enterText(find.byType(TextField), '34970112332');
    await tester.pump();
    await tester.tap(find.text('Add device already on the network'));
    await tester.pumpAndSettle();
    expect(journal.session, matches(RegExp(r'^[0-9a-f-]{36}$')));
    expect(journal.writes.join(), isNot(contains('34970112332')));
    expect(find.textContaining('could not be confirmed'), findsOneWidget);
    final add = tester.widget<FilledButton>(
        find.widgetWithText(FilledButton, 'Add device already on the network'));
    expect(add.onPressed, isNull);
  });
  testWidgets(
      'lost phone callback reconciles addon receipt and retains original confirmation',
      (tester) async {
    final phone = _Phone(supported: true);
    api.receiptStatus = 'completed';
    api.needsConfirmation = true;
    await open(tester, phone: phone);
    await tester.enterText(find.byType(TextField), '34970112332');
    await tester.pump();
    await tester.ensureVisible(find.text('Set up new device with this phone'));
    await tester.tap(find.text('Set up new device with this phone'));
    await tester.pumpAndSettle();
    expect(phone.backendUsed, PhoneMatterBackend.haAddon);
    expect(find.textContaining('Review the correct device'), findsOneWidget);
    expect(journal.session, isNotNull);
    expect(api.pairRequests, 0);
  });
  testWidgets(
      'fenced missing phone handoff releases pointer for an explicit retry',
      (tester) async {
    final phone = _Phone(supported: true);
    api.receiptStatus = 'failed';
    await open(tester, phone: phone);
    await tester.enterText(find.byType(TextField), '34970112332');
    await tester.pump();
    await tester.ensureVisible(find.text('Set up new device with this phone'));
    await tester.tap(find.text('Set up new device with this phone'));
    await tester.pumpAndSettle();
    expect(journal.session, isNull);
    expect(find.textContaining('could not finish pairing'), findsOneWidget);
    expect(api.pairRequests, 0);
  });

  testWidgets(
      'explicit device confirmation binds code then reveals only on owner request',
      (tester) async {
    journal.session = previous;
    api.receiptStatus = 'completed';
    api.needsConfirmation = true;
    api.devices.add({
      'device_id': 'device-42',
      'identity': 'proof-42',
      'name': 'Reviewed lamp',
      'node_id': 42
    });
    await open(tester);
    final menu = find.byType(PopupMenuButton<String>);
    await tester.scrollUntilVisible(menu, 180,
        scrollable: find
            .descendant(
                of: find.byType(ListView), matching: find.byType(Scrollable))
            .first);
    await tester.tap(menu);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Save pairing code for this device'));
    await tester.pumpAndSettle();
    expect(find.text('Save pairing code for Reviewed lamp?'), findsOneWidget);
    await tester.tap(find.text('Confirm'));
    await tester.pumpAndSettle();
    expect(api.confirmedDevice?.deviceId, 'device-42');
    expect(api.confirmedDevice?.identity, 'proof-42');
    expect(journal.session, isNull);
    await tester.scrollUntilVisible(menu, 180,
        scrollable: find
            .descendant(
                of: find.byType(ListView), matching: find.byType(Scrollable))
            .first);
    await tester.tap(menu);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Show original label code'));
    // The operation stays busy while its modal is open, so the background
    // progress indicator intentionally continues animating.
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.text('Original Matter label code'), findsOneWidget);
    expect(find.text('MT:ORIGINAL-LABEL'), findsNothing);
    expect(find.text('•••• •••• ••••'), findsOneWidget);
    await tester.tap(find.text('REVEAL'));
    await tester.pump();
    expect(find.text('MT:ORIGINAL-LABEL'), findsOneWidget);
    await tester.tap(find.text('DONE'));
    await tester.pumpAndSettle();
  });

  testWidgets(
      'terminal unknown closes only after explicit review without a new POST',
      (tester) async {
    journal.session = previous;
    api.canClose = true;
    await open(tester);
    await tester
        .tap(find.text('I reviewed Home Assistant; close this attempt'));
    await tester.pumpAndSettle();
    expect(journal.session, previous);
    await tester.tap(find.text('Confirm'));
    await tester.pumpAndSettle();
    expect(journal.session, isNull);
    expect(api.pairRequests, 0);
    expect(find.text('Reviewed attempt closed. No new pairing was started.'),
        findsOneWidget);
  });

  testWidgets('fits a narrow phone with enlarged text', (tester) async {
    tester.view.physicalSize = const Size(320, 640);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(MaterialApp(
        builder: (context, child) => MediaQuery(
            data: MediaQuery.of(context)
                .copyWith(textScaler: const TextScaler.linear(1.5)),
            child: child!),
        home: HaMatterManagementScreen(
            api: api,
            journal: journal,
            isCurrentTarget: () => active,
            phoneCommissioner: _Phone())));
    await tester.pumpAndSettle();
    await tester.scrollUntilVisible(
        find.text('Add device already on the network'), 150,
        scrollable: find
            .descendant(
                of: find.byType(ListView), matching: find.byType(Scrollable))
            .first);
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });

  testWidgets('changed target cannot dispatch through captured addon client',
      (tester) async {
    await open(tester);
    await tester.enterText(find.byType(TextField), '34970112332');
    await tester.pump();
    active = false;
    await tester.tap(find.text('Add device already on the network'));
    await tester.pumpAndSettle();
    expect(journal.writes, isEmpty);
    expect(api.pairRequests, 0);
  });
}
