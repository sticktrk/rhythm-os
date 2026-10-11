import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/apple_health_service.dart';
import 'package:rhythm_app/services/apple_health_sync_controller.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../helpers/fake_apple_health_writer.dart';

const scope = AppleHealthScope(
    accountId: 'owner', homeId: 'home', serverId: 'box', nodeId: 'bedroom');
const start = 1770073200000;
final events = [
  RhythmActivityEvent(
      id: 'sleep',
      nodeId: 'bedroom',
      actionId: 'sleep_on',
      epochMs: start,
      source: const RhythmActivitySource(
          rawSource: 'button', kind: 'switch', marksTouched: true)),
  RhythmActivityEvent(
      id: 'wake',
      nodeId: 'bedroom',
      actionId: 'sleep_off',
      epochMs: start + 8 * 3600000,
      source: const RhythmActivitySource(
          rawSource: 'button', kind: 'switch', marksTouched: true)),
];

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late FakeAppleHealthWriter writer;
  late AppleHealthScope? environment;
  Object? preference;
  late AppleHealthSyncController controller;
  late Future<AppleHealthHistorySnapshot> Function() load;

  AppleHealthSyncController create() => AppleHealthSyncController(
      writer: writer,
      currentEnvironment: () => environment,
      readPreference: () => preference,
      writePreference: (value) async {
        preference = value;
      },
      loadHistory: (_, __) => load(),
      now: () => DateTime.fromMillisecondsSinceEpoch(start + 10 * 3600000));

  setUp(() {
    writer = FakeAppleHealthWriter();
    environment = scope;
    preference = null;
    load = () async => AppleHealthHistorySnapshot(events: events);
    controller = create();
  });
  tearDown(() => controller.dispose());

  test(
      'defaults off, consent persists and uncertain/repeated saves remain idempotent after restart',
      () async {
    expect(controller.enabled, isFalse);
    await controller.sync();
    expect(writer.calls, isEmpty);
    writer.failAfterSave = true;
    await controller.enable(scope);
    expect(writer.requests, 1);
    expect(controller.enabled, isTrue);
    expect(controller.status, AppleHealthSyncStatus.failed);
    expect(writer.samples, hasLength(1));
    controller.dispose();
    controller = create();
    writer.failAfterSave = false;
    await controller.sync();
    await controller.sync();
    expect(controller.status, AppleHealthSyncStatus.synced);
    expect(writer.calls, hasLength(3));
    expect(writer.samples, hasLength(1));
    expect(writer.calls.first.single, writer.calls.last.single);
    expect(writer.calls.last.single.keys.toSet(),
        {'sync_id', 'start_epoch_ms', 'end_epoch_ms'});
    expect(writer.calls.last.single['sync_id'], isNot(contains('owner')));
  });

  test('permission denial does not enable; later revocation never writes',
      () async {
    writer.state = AppleHealthAuthorization.denied;
    await controller.enable(scope);
    expect(preference, isNull);
    expect(controller.enabled, isFalse);
    expect(controller.status, AppleHealthSyncStatus.denied);
    writer.state = AppleHealthAuthorization.authorized;
    await controller.enable(scope);
    writer.state = AppleHealthAuthorization.denied;
    await controller.sync();
    expect(writer.calls, hasLength(1));
    expect(controller.status, AppleHealthSyncStatus.denied);
  });

  test('failed history stays retryable and a successful empty read stays empty',
      () async {
    load = () async => throw StateError('offline');
    await controller.enable(scope);
    expect(controller.status, AppleHealthSyncStatus.offline);
    expect(writer.calls, isEmpty);
    load = () async => const AppleHealthHistorySnapshot(events: []);
    await controller.sync();
    expect(controller.status, AppleHealthSyncStatus.empty);
    load =
        () async => AppleHealthHistorySnapshot(events: events, partial: true);
    await controller.sync();
    expect(controller.status, AppleHealthSyncStatus.partial);
    expect(writer.samples, hasLength(1));
  });

  test(
      'disabling while history loads fences native writes and keeps prior Health samples',
      () async {
    await controller.enable(scope);
    final pending = Completer<AppleHealthHistorySnapshot>();
    load = () => pending.future;
    final flight = controller.sync();
    await Future<void>.delayed(Duration.zero);
    await controller.disable();
    pending.complete(AppleHealthHistorySnapshot(events: events));
    await flight;
    expect(writer.calls, hasLength(1));
    expect(writer.samples, hasLength(1));
    expect(controller.status, AppleHealthSyncStatus.disabled);
    expect(preference, isNull);
  });

  test(
      'account/home/server changes and cleared settings cannot resume stale consent',
      () async {
    await controller.enable(scope);
    for (final changed in [
      const AppleHealthScope(
          accountId: 'other',
          homeId: 'home',
          serverId: 'box',
          nodeId: 'bedroom'),
      const AppleHealthScope(
          accountId: 'owner',
          homeId: 'other',
          serverId: 'box',
          nodeId: 'bedroom'),
      const AppleHealthScope(
          accountId: 'owner',
          homeId: 'home',
          serverId: 'other',
          nodeId: 'bedroom'),
    ]) {
      environment = changed;
      await controller.sync();
      expect(controller.status, AppleHealthSyncStatus.offline);
    }
    environment = scope;
    preference = null;
    controller.environmentChanged();
    await controller.sync();
    expect(controller.enabled, isFalse);
    expect(writer.calls, hasLength(1));
  });

  test(
      'home switch during a read cannot write, and duplicate sync calls share one flight',
      () async {
    await controller.enable(scope);
    final pending = Completer<AppleHealthHistorySnapshot>();
    var reads = 0;
    load = () {
      reads++;
      return pending.future;
    };
    final flight = controller.sync();
    await Future<void>.delayed(Duration.zero);
    await controller.sync();
    environment = null;
    controller.environmentChanged();
    pending.complete(AppleHealthHistorySnapshot(events: events));
    await flight;
    expect(reads, 1);
    expect(writer.calls, hasLength(1));
  });

  test('permission sheet inactive lifecycle does not cancel enable', () async {
    controller.didChangeAppLifecycleState(AppLifecycleState.inactive);
    await controller.enable(scope);
    expect(controller.enabled, isTrue);
    expect(writer.samples, hasLength(1));
    controller.didChangeAppLifecycleState(AppLifecycleState.paused);
    await controller.sync();
    expect(writer.calls, hasLength(1));
    controller.didChangeAppLifecycleState(AppLifecycleState.resumed);
    await Future<void>.delayed(Duration.zero);
    expect(writer.calls, hasLength(2));
  });

  test('unknown preference schema and malformed scope default off', () {
    for (final invalid in [
      {'schema_version': 2, 'enabled': true, 'scope': scope.toJson()},
      {'schema_version': 1, 'enabled': true, 'scope': {}},
      'invalid'
    ]) {
      preference = invalid;
      final restored = create();
      expect(restored.enabled, isFalse);
      restored.dispose();
    }
  });

  test(
      'normal server notifications while permission sheet is open preserve enable',
      () async {
    final permission = Completer<AppleHealthAuthorization>();
    writer.requestResult = permission.future;
    final flight = controller.enable(scope);
    controller.environmentChanged();
    controller.environmentChanged();
    permission.complete(AppleHealthAuthorization.authorized);
    await flight;
    expect(controller.enabled, isTrue);
    expect(writer.samples, hasLength(1));
  });

  test(
      'reconnect retries after success while ordinary notifications do not repeat writes',
      () async {
    var connected = true;
    controller.dispose();
    controller = AppleHealthSyncController(
        writer: writer,
        isConnected: () => connected,
        currentEnvironment: () => environment,
        readPreference: () => preference,
        writePreference: (value) async {
          preference = value;
        },
        loadHistory: (_, __) => load(),
        now: () => DateTime.fromMillisecondsSinceEpoch(start + 10 * 3600000));
    controller.environmentChanged();
    await controller.enable(scope);
    controller.environmentChanged();
    await Future<void>.delayed(Duration.zero);
    expect(writer.calls, hasLength(1));
    connected = false;
    controller.environmentChanged();
    connected = true;
    controller.environmentChanged();
    await Future<void>.delayed(Duration.zero);
    expect(writer.calls, hasLength(2));
  });

  test(
      'reset during a pending read fences writes even without a provider notification',
      () async {
    await controller.enable(scope);
    final pending = Completer<AppleHealthHistorySnapshot>();
    load = () => pending.future;
    final flight = controller.sync();
    await Future<void>.delayed(Duration.zero);
    preference = null;
    pending.complete(AppleHealthHistorySnapshot(events: events));
    await flight;
    await controller.sync();
    expect(controller.enabled, isFalse);
    expect(writer.calls, hasLength(1));
  });
}
