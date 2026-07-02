import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../backend/backend.dart';
import 'account_cloud_sync_service.dart';
import 'auth_service.dart';

class ServerActivityCloudProvisioningService {
  ServerActivityCloudProvisioningService._();

  static const String bootstrapFunctionName = 'server-activity-bootstrap';

  static final ServerActivityCloudProvisioningService instance =
      ServerActivityCloudProvisioningService._();

  final Map<String, Future<void>> _inFlight = <String, Future<void>>{};

  bool get canProvision {
    if (!BackendProvider.isInitialized) return false;
    final auth = AuthService();
    return BackendProvider.instance.auth is SupabaseAuthBackend &&
        auth.isSignedIn &&
        !auth.isAnonymous &&
        auth.currentUserId != null;
  }

  Future<void> ensureConfigured({
    required Hub serverHub,
    required RhythmRuntimeApi runtimeApi,
    Home? home,
    String? serverInstanceId,
  }) async {
    if (!canProvision || serverHub.type != HubType.server) return;
    final key = '${serverHub.homeId}:${serverHub.id}';
    final inFlight = _inFlight[key];
    if (inFlight != null) return inFlight;

    final future = _ensureConfiguredInternal(
      serverHub: serverHub,
      runtimeApi: runtimeApi,
      home: home,
      serverInstanceId: serverInstanceId,
    );
    _inFlight[key] = future;
    try {
      await future;
    } finally {
      _inFlight.remove(key);
    }
  }

  Future<void> _ensureConfiguredInternal({
    required Hub serverHub,
    required RhythmRuntimeApi runtimeApi,
    required Home? home,
    required String? serverInstanceId,
  }) async {
    final status = await runtimeApi.getActivityCloudConfig();
    if (_statusMatchesHub(
      status,
      serverHub,
      expectedServerInstanceId: serverInstanceId ?? serverHub.serverInstanceId,
    )) {
      return;
    }

    final client =
        (BackendProvider.instance.auth as SupabaseAuthBackend).client;
    final response = await client.functions.invoke(
      bootstrapFunctionName,
      body: _bootstrapBody(
        serverHub: serverHub,
        home: home,
        serverInstanceId: serverInstanceId ?? serverHub.serverInstanceId,
      ),
    );
    final data = Map<String, dynamic>.from(response.data as Map);
    final config = <String, dynamic>{
      'enabled': true,
      'ingest_url': data['ingest_url']?.toString(),
      'upload_token': data['upload_token']?.toString(),
      'home_id': data['home_id']?.toString() ?? home?.id ?? serverHub.homeId,
      'hub_id': data['hub_id']?.toString() ?? serverHub.id,
      if (data['token_id'] != null) 'token_id': data['token_id']?.toString(),
      if (data['server_instance_id'] != null)
        'server_instance_id': data['server_instance_id']?.toString(),
    };

    final updatedStatus = await runtimeApi.putActivityCloudConfig(config);
    if (updatedStatus == null || updatedStatus['configured'] != true) {
      throw StateError('Server did not accept activity cloud config.');
    }
    debugPrint(
      'ServerActivityCloudProvisioningService: configured device uploads '
      'for hub=${serverHub.id}',
    );
  }

  bool _statusMatchesHub(
    Map<String, dynamic>? status,
    Hub serverHub, {
    String? expectedServerInstanceId,
  }) {
    if (status == null || status['configured'] != true) return false;
    if (status['needs_reprovision'] == true ||
        status['upload_status']?.toString() == 'auth_failed') {
      return false;
    }
    if (status['hub_id']?.toString() != serverHub.id ||
        status['home_id']?.toString() != serverHub.homeId) {
      return false;
    }
    final normalizedServerInstanceId =
        (expectedServerInstanceId ?? serverHub.serverInstanceId)?.trim();
    if (normalizedServerInstanceId != null &&
        normalizedServerInstanceId.isNotEmpty &&
        status['server_instance_id']?.toString() !=
            normalizedServerInstanceId) {
      return false;
    }
    return true;
  }

  Map<String, dynamic> _bootstrapBody({
    required Hub serverHub,
    required Home? home,
    required String? serverInstanceId,
  }) {
    final normalizedServerInstanceId = serverInstanceId?.trim();
    final snapshotHub = normalizedServerInstanceId != null &&
            normalizedServerInstanceId.isNotEmpty
        ? serverHub.copyWith(serverInstanceId: normalizedServerInstanceId)
        : serverHub;
    return {
      'hub_id': serverHub.id,
      if (normalizedServerInstanceId != null &&
          normalizedServerInstanceId.isNotEmpty)
        'server_instance_id': normalizedServerInstanceId,
      if (home != null) ...{
        'home': AccountCloudSyncService.homeSnapshotPayload(home),
        'server_hub': AccountCloudSyncService.serverHubSnapshotPayload(
          snapshotHub,
        ),
      },
    };
  }

  @visibleForTesting
  void resetForTest() {
    _inFlight.clear();
  }
}
