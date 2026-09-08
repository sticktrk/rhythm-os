import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/device_commissioning_flow.dart';

void main() {
  test('lost success is recovered without a second commissioning attempt',
      () async {
    final queries = <String>[];
    final flow = DeviceCommissioningFlow<String>(readReceipt: (id) async {
      queries.add(id);
      return const CommissioningReceipt(CommissioningReceiptState.complete,
          result: 'box-owned-device');
    });
    expect((await flow.begin()).state, CommissioningStartState.ready);
    final handoffId = flow.stageSessionId('adopt');
    flow.expectReceipt(handoffId);
    flow.end();
    final retry = await flow.begin();
    expect(retry.state, CommissioningStartState.recovered);
    expect(retry.result, 'box-owned-device');
    expect(queries, [handoffId]);
    expect(flow.attemptNumber, 1);
  });

  for (final state in [
    CommissioningReceiptState.failed,
    CommissioningReceiptState.notFound
  ]) {
    test('$state permits fallback with a fresh attempt identity', () async {
      final flow = DeviceCommissioningFlow<String>(
        readReceipt: (_) async => CommissioningReceipt(state),
      );
      await flow.begin();
      final previous = flow.sessionId;
      flow.expectReceipt();
      flow.end();
      expect((await flow.begin()).state, CommissioningStartState.ready);
      expect(flow.sessionId, isNot(previous));
      expect(flow.attemptNumber, 2);
    });
  }

  test('pending, unavailable and malformed success never allow replay',
      () async {
    final receipts = <CommissioningReceipt<String>?>[
      const CommissioningReceipt(CommissioningReceiptState.pending),
      null,
      const CommissioningReceipt(CommissioningReceiptState.complete),
    ];
    final queries = <String>[];
    final flow = DeviceCommissioningFlow<String>(readReceipt: (id) async {
      queries.add(id);
      return receipts.removeAt(0);
    });
    await flow.begin();
    final previous = flow.sessionId;
    flow.expectReceipt();
    flow.end();
    expect((await flow.begin()).state, CommissioningStartState.pending);
    expect((await flow.begin()).state, CommissioningStartState.unavailable);
    expect((await flow.begin()).state, CommissioningStartState.unavailable);
    expect(queries, [previous, previous, previous]);
    expect(flow.attemptNumber, 1);
  });

  test('double taps and disposal cannot start work after an awaited receipt',
      () async {
    final receipt = Completer<CommissioningReceipt<String>?>();
    final flow =
        DeviceCommissioningFlow<String>(readReceipt: (_) => receipt.future);
    await flow.begin();
    expect((await flow.begin()).state, CommissioningStartState.inactive);
    flow.expectReceipt();
    flow.end();
    final retry = flow.begin();
    expect((await flow.begin()).state, CommissioningStartState.inactive);
    flow.dispose();
    receipt.complete(
        const CommissioningReceipt(CommissioningReceiptState.notFound));
    expect((await retry).state, CommissioningStartState.inactive);
    expect(flow.attemptNumber, 1);
    expect((await flow.begin()).state, CommissioningStartState.inactive);
  });
}
