import 'dart:convert';
import 'dart:io';

import 'package:rhythm_admin_api/src/models.dart';
import 'package:test/test.dart';

void main() {
  test('the staff proxy applies guards matching every live operation tier', () {
    final contract = jsonDecode(
        File(Platform.environment['RHYTHM_ASSISTANT_CONTRACT']!)
            .readAsStringSync());
    for (final op in contract['operations'] as List) {
      final route = {
        'method': 'POST',
        'path': (op['invocation_path'] as String).substring(1)
      };
      final request = DeviceAdminProxyRequestDto.fromJson(route);
      expect(request.isAssistant, true);
      expect(request.isReadOnly, op['tier'] == 'read', reason: op['id']);
      if (request.isMutation) {
        expect(
            request.validateMutationGuards, throwsA(isA<AdminApiException>()),
            reason: op['id']);
        DeviceAdminProxyRequestDto.fromJson({
          ...route,
          'requestId': 'parity-request',
          'expectedServerInstanceId': 'parity-hub'
        }).validateMutationGuards();
      } else {
        request.validateMutationGuards();
      }
    }
    expect(
        DeviceAdminProxyRequestDto.fromJson({
          'method': 'POST',
          'path': 'api/assistant/operations/new_unknown_operation'
        }).isReadOnly,
        false);
  });
}
