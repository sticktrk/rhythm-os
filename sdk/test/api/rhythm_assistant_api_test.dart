import 'package:dio/dio.dart';
import 'package:mocktail/mocktail.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

import '../helpers/mock_dio.dart';

void main() {
  late MockDio dio;
  late RhythmServerApi api;

  setUp(() {
    dio = MockDio();
    api = RhythmServerApi(dio);
  });

  test('discovers and parses the runtime assistant contract', () async {
    when(() => dio.get(any())).thenAnswer(
      (_) async => Response(
        requestOptions: RequestOptions(path: 'api/assistant/contract'),
        statusCode: 200,
        data: {
          'schema_version': 1,
          'contract_sha256': 'a' * 64,
          'server_version': '0.6.573-beta',
          'topology_snapshot_path': '/api/assistant/topology',
          'operations': [
            {
              'id': RhythmAssistantOperationId.identifyDevice,
              'description': 'Identify one canonical light',
              'method': 'POST',
              'path': '/api/devices/canonical/{device_id}/flash',
              'effect': 'temporary_light_output',
              'confirmation': 'session_probe_permission',
              'freshness_precondition': 'canonical_light_device',
              'verification': 'server_acknowledgement_then_user_observation',
              'physical_confirmation_required': true,
              'input_schema': {'type': 'object'},
              'result_schema': {'type': 'object'},
            },
          ],
        },
      ),
    );

    final contract = await api.getAssistantContract();

    expect(contract, isNotNull);
    expect(contract!.schemaVersion, 1);
    expect(
      contract.supportsOperation(RhythmAssistantOperationId.identifyDevice),
      isTrue,
    );
    expect(
      contract
          .operation(RhythmAssistantOperationId.identifyDevice)!
          .physicalConfirmationRequired,
      isTrue,
    );
    verify(() => dio.get('api/assistant/contract')).called(1);
  });

  test('returns null only for a previous appliance 404', () async {
    when(() => dio.get(any())).thenThrow(
      DioException(
        requestOptions: RequestOptions(path: 'api/assistant/contract'),
        response: Response(
          requestOptions: RequestOptions(path: 'api/assistant/contract'),
          statusCode: 404,
        ),
      ),
    );

    expect(await api.getAssistantContract(), isNull);
  });

  test('keeps non-404 contract transport failure visible', () async {
    final failure = DioException(
      requestOptions: RequestOptions(path: 'api/assistant/contract'),
      type: DioExceptionType.connectionError,
    );
    when(() => dio.get(any())).thenThrow(failure);

    await expectLater(api.getAssistantContract(), throwsA(same(failure)));
  });

  test('parses topology snapshot with its exact resource hash', () async {
    when(() => dio.get(any())).thenAnswer(
      (_) async => Response(
        requestOptions: RequestOptions(path: 'api/assistant/topology'),
        statusCode: 200,
        data: {
          'schema_version': 1,
          'contract_sha256': 'a' * 64,
          'server_instance_id': 'srv-1',
          'server_version': '0.6.573-beta',
          'observed_at_epoch_ms': 1234,
          'topology_resource_sha256': 'b' * 64,
          'nodes': [
            {'id': 'foyer', 'name': 'Foyer', 'kind': 'room'},
          ],
        },
      ),
    );

    final snapshot = await api.getAssistantTopologySnapshot();

    expect(snapshot.serverInstanceId, 'srv-1');
    expect(snapshot.topologyResourceSha256, 'b' * 64);
    expect(snapshot.nodes.single.name, 'Foyer');
    verify(() => dio.get('api/assistant/topology')).called(1);
  });

  test('plans and applies the exact reviewed move payload', () async {
    final planJson = {
      'schema_version': 1,
      'plan_id': 'move-plan-1',
      'correlation_id': 'setup-move-1',
      'operation': RhythmAssistantOperationId.applyMoveDeviceRoomPlan,
      'contract_sha256': 'a' * 64,
      'server_instance_id': 'srv-1',
      'topology_resource_sha256': 'b' * 64,
      'device': {'id': 'light-1', 'name': 'Ceiling Light'},
      'from_room': {'id': 'bathroom', 'name': 'Bathroom'},
      'to_room': {'id': 'foyer', 'name': 'Foyer'},
      'resulting_placement': 'user_override',
      'requires_confirmation': true,
      'warnings': ['move_creates_user_override'],
    };
    when(() => dio.post(any(), data: any(named: 'data'))).thenAnswer(
      (invocation) async {
        final path = invocation.positionalArguments.single as String;
        if (path.endsWith('/apply')) {
          return Response(
            requestOptions: RequestOptions(path: path),
            statusCode: 200,
            data: {
              'schema_version': 1,
              'plan_id': 'move-plan-1',
              'correlation_id': 'setup-move-1',
              'operation': RhythmAssistantOperationId.applyMoveDeviceRoomPlan,
              'status': 'applied',
              'contract_sha256': 'a' * 64,
              'server_instance_id': 'srv-1',
              'topology_resource_sha256': 'c' * 64,
              'device_id': 'light-1',
              'previous_parent_id': 'bathroom',
              'current_parent_id': 'foyer',
              'resulting_placement': 'user_override',
              'server_acknowledged': true,
              'canonical_readback_verified': true,
              'physical_verification': 'not_required',
              'completed_at_epoch_ms': 1235,
            },
          );
        }
        return Response(
          requestOptions: RequestOptions(path: path),
          statusCode: 200,
          data: planJson,
        );
      },
    );

    final plan = await api.planAssistantDeviceRoomMove(
      deviceId: 'light-1',
      toRoomId: 'foyer',
      correlationId: 'setup-move-1',
    );
    final receipt = await api.applyAssistantDeviceRoomMove(plan);

    expect(plan.requiresConfirmation, isTrue);
    expect(plan.fromRoom!.id, 'bathroom');
    expect(plan.warnings, ['move_creates_user_override']);
    expect(receipt.status, 'applied');
    expect(receipt.canonicalReadbackVerified, isTrue);

    verify(() => dio.post(
          'api/assistant/plans/device-room-move',
          data: {
            'device_id': 'light-1',
            'to_room_id': 'foyer',
            'correlation_id': 'setup-move-1',
          },
        )).called(1);
    verify(() => dio.post(
          'api/assistant/plans/device-room-move/apply',
          data: {
            'plan_id': 'move-plan-1',
            'correlation_id': 'setup-move-1',
            'operation': RhythmAssistantOperationId.applyMoveDeviceRoomPlan,
            'contract_sha256': 'a' * 64,
            'server_instance_id': 'srv-1',
            'topology_resource_sha256': 'b' * 64,
            'device_id': 'light-1',
            'from_room_id': 'bathroom',
            'to_room_id': 'foyer',
          },
        )).called(1);
  });
}
