import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/customer_assistant_service.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/server_sync_provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _Homes extends Fake implements HomeProvider {
  _Homes(this.activeServerHub);
  @override
  final Hub activeServerHub;
}

class _Connection extends Fake implements RhythmConnection {
  @override
  bool get connected => true;
}

class _Sync extends Fake implements ServerSyncProvider {
  _Sync(this.connectedServerHub);
  @override
  final Hub connectedServerHub;
  @override
  String get connectedServerInstanceId => 'box';
  @override
  final RhythmConnection connection = _Connection();
}

void main() {
  test(
      'physical identity alone cannot grant access through the wrong Home or hub',
      () async {
    final hub = Hub.server(
        id: 'device',
        homeId: 'house',
        name: 'Box',
        host: 'example.test',
        token: 'owner');
    final device = ResidentAssistantDevice(_Homes(hub), _Sync(hub), () => true);
    const intended = {
      'change': 'submit_report',
      'home_id': 'house',
      'hub_id': 'device',
      'server_instance_id': 'box'
    };
    await device.check(intended);
    for (final mismatch in ['home_id', 'hub_id', 'server_instance_id']) {
      await expectLater(
          device.check({...intended, mismatch: 'different'}), throwsStateError);
    }
  });

  for (final change in ['remove_member', 'revoke_support']) {
    test('resident completes $change even when the target Box is offline',
        () async {
      final other = Hub.server(
          id: 'other',
          name: 'Other',
          homeId: 'other-home',
          host: 'fixture.invalid');
      final device =
          ResidentAssistantDevice(_Homes(other), _Sync(other), () => true);
      final review = {
        'status': 'review',
        'change': change,
        'plan_id': 'p',
        'home_id': 'home'
      };
      final calls = <CustomerAction>[];
      final service = CustomerAssistantService((body) async {
        calls.add(body);
        return {
          ...review,
          'status': body.containsKey('operation')
              ? 'awaiting_device'
              : body.containsKey('device_claim')
                  ? 'claimed'
                  : 'applied'
        };
      }, () => true);
      final result = await service.confirm(review,
          checkDevice: device.check, executeDevice: device.execute);
      expect(result['status'], 'applied');
      expect(calls, [
        {
          'operation': 'apply_customer_plan',
          'arguments': {'plan_id': 'p', 'confirmed': true}
        },
        {'device_claim': 'p'},
        {'device_complete': 'p', 'outcome': 'acknowledged'},
      ]);
    });
  }

  const review = {
    'status': 'review',
    'plan_id': 'p',
    'change': 'submit_report',
    'home_id': 'home',
    'hub_id': 'hub',
    'server_instance_id': 'box',
    'report_summary': 'lights stayed dark',
    'include_bundle': false,
  };
  for (final lostAccount in [false, true]) {
    test(
        'lost completion or account change leaves the claim uncertain (account change $lostAccount)',
        () async {
      var sameAccount = true;
      var executions = 0;
      final service = CustomerAssistantService((body) async {
        if (body.containsKey('device_claim')) {
          return {...review, 'status': 'claimed'};
        }
        throw StateError('Completion receipt lost');
      }, () => sameAccount);
      final result = await service.confirm(
          {...review, 'status': 'awaiting_device'}, checkDevice: (_) async {},
          executeDevice: (_) async {
        executions++;
        sameAccount = !lostAccount;
        return true;
      });
      expect(result['status'], 'uncertain');
      expect(executions, 1);
    });
  }

  test(
      'text-only review can proceed offline while bundled reports require the reviewed hub',
      () async {
    final other = Hub.server(
        id: 'other', name: 'Other', homeId: 'other-home', host: 'example.test');
    var sameAccount = true;
    final device =
        ResidentAssistantDevice(_Homes(other), _Sync(other), () => sameAccount);
    await device.check(review);
    await expectLater(
        device.check({...review, 'include_bundle': true}), throwsStateError);
    sameAccount = false;
    await expectLater(device.check(review), throwsStateError);
  });

  for (final mismatch in [
    null,
    'home_id',
    'hub_id',
    'server_instance_id',
    'report_summary',
    'include_bundle',
    'plan_id',
    'change'
  ]) {
    test(
        'device claim binds reviewed fields including ${mismatch ?? 'all matching fields'}',
        () async {
      final calls = <CustomerAction>[];
      var dispatched = 0;
      final service = CustomerAssistantService((body) async {
        calls.add(body);
        if (body.containsKey('operation')) {
          expect(body, {
            'operation': 'apply_customer_plan',
            'arguments': {'plan_id': 'p', 'confirmed': true}
          });
          return {...review, 'status': 'awaiting_device'};
        }
        if (body.containsKey('device_claim')) {
          return {
            ...review,
            'status': 'claimed',
            if (mismatch != null) mismatch: true
          };
        }
        return {'status': body['outcome'], 'plan_id': 'p'};
      }, () => true);
      final result = await service.confirm(review, checkDevice: (_) async {},
          executeDevice: (claim) async {
        dispatched++;
        expect(claim['include_bundle'], false);
        return true;
      });
      expect(dispatched, mismatch == null ? 1 : 0);
      expect(result['status'], mismatch == null ? 'acknowledged' : 'uncertain');
      expect(calls.last, {'device_complete': 'p', 'outcome': result['status']});
    });
  }

  test('changed account, switched Box and already claimed plan cannot dispatch',
      () async {
    var calls = 0;
    var sameAccount = false;
    final service = CustomerAssistantService((_) async {
      calls++;
      return {};
    }, () => sameAccount);
    Future<void> execute(CustomerAction r) async {
      await service.confirm(r,
          checkDevice: (_) async {
            throw StateError('Switched Box');
          },
          executeDevice: (_) async => fail('must not dispatch'));
    }

    await expectLater(execute(review), throwsStateError);
    sameAccount = true;
    await expectLater(
        execute({...review, 'status': 'awaiting_device'}), throwsStateError);
    await expectLater(
        execute({...review, 'status': 'claimed'}), throwsStateError);
    expect(calls, 0);
  });

  test(
      'interrupted side effect records an uncertain outcome and never retries it',
      () async {
    final calls = <CustomerAction>[];
    final service = CustomerAssistantService((body) async {
      calls.add(body);
      return {
        ...review,
        'status': body.containsKey('device_claim') ? 'claimed' : 'uncertain'
      };
    }, () => true);
    final result = await service.confirm(
        {...review, 'status': 'awaiting_device'},
        checkDevice: (_) async {},
        executeDevice: (_) async => throw StateError('lost reply'));
    expect(result['status'], 'uncertain');
    expect(calls, [
      {'device_claim': 'p'},
      {'device_complete': 'p', 'outcome': 'uncertain'}
    ]);
  });
}
