import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/support_access_service.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  group('SupportAccessService', () {
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
