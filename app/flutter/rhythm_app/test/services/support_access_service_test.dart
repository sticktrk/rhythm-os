import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/support_access_service.dart';
import 'package:rhythm_core/rhythm_core.dart';

class _GrantCheck {
  final calls = <Map<String, dynamic>>[];
  _GrantCheck get functions => this;
  Future<({Map<String, dynamic> data})> invoke(String name,
      {required Map<String, dynamic> body}) async {
    calls.add(body);
    return (data: {'allowed': false});
  }
}

void main() {
  group('SupportAccessService', () {
    test(
        'suppressed automatic grant checks cloud consent before issuing a local credential',
        () async {
      final check = _GrantCheck();
      var minted = false;
      final service = SupportAccessService.testing(
          supabaseClientFactory: () => check,
          authApiFactory: ({required baseUrl, String? authToken}) {
            minted = true;
            throw StateError('Should not issue a credential');
          });
      await expectLater(service.grantForHub(_serverHub()), throwsStateError);
      expect(minted, isFalse);
      expect(check.calls.single['action'], 'check');
    });

    test('grant body sends beta admin support token metadata', () {
      final body = SupportAccessService.buildGrantBody(
        serverHub: _serverHub(),
        tokenId: 'support-token-id',
        token: 'rhythm_support_raw',
        label: ' admin support ',
      );

      expect(body, {
        'action': 'grant',
        'hub_id': 'hub-1',
        'home_id': 'home-1',
        'scope': 'beta_admin',
        'token_id': 'support-token-id',
        'token': 'rhythm_support_raw',
        'label': 'admin support',
      });
    });

    test(
        'explicit support grant carries its claimed review to the grant service',
        () {
      final body = SupportAccessService.buildGrantBody(
          serverHub: _serverHub(),
          tokenId: 'id',
          token: 'rhythm_support_test',
          assistantPlanId: 'review');
      expect(body['assistant_plan_id'], 'review');
    });

    test('revoke body targets beta admin grant', () {
      final body = SupportAccessService.buildRevokeBody(
        serverHub: _serverHub(),
      );

      expect(body, {
        'action': 'revoke',
        'hub_id': 'hub-1',
        'home_id': 'home-1',
        'scope': 'beta_admin',
      });
    });
  });
}

Hub _serverHub() {
  return Hub.server(
    id: 'hub-1',
    homeId: 'home-1',
    name: 'Kitchen Server',
    host: '192.168.5.123',
    token: 'owner-token',
  );
}
