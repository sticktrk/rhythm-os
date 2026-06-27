import 'dart:async';
import 'dart:convert';

import 'package:http/http.dart' as http;
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show RhythmDeviceType, RhythmDiagnosticsApi, RhythmHello;

import 'models.dart';
import 'supabase_rest_client.dart';

class DeviceProbeService {
  DeviceProbeService({
    required SupabaseRestClient supabase,
    http.Client? httpClient,
  })  : _supabase = supabase,
        _http = httpClient ?? http.Client();

  final SupabaseRestClient _supabase;
  final http.Client _http;

  Future<DeviceProbeResultDto> probeHub({
    required AdminSession session,
    required String hubId,
  }) async {
    final hub = await _loadHub(session: session, hubId: hubId);
    final candidates = <({String route, HubEndpointDto endpoint})>[
      if (hub.remoteEndpoint != null)
        (route: 'remote', endpoint: hub.remoteEndpoint!),
      (route: 'local', endpoint: hub.endpoint),
    ];
    final seen = <String>{};
    final distinctCandidates = [
      for (final candidate in candidates)
        if (seen.add(candidate.endpoint.baseUrl.toLowerCase())) candidate,
    ];

    var sawAuthRequired = false;
    for (final candidate in distinctCandidates) {
      final result = await _probeEndpoint(hub, candidate);
      if (result.status == 'online') return result;
      if (result.status == 'auth_required') {
        sawAuthRequired = true;
        return result;
      }
    }

    return DeviceProbeResultDto(
      hubId: hub.id,
      status: sawAuthRequired ? 'auth_required' : 'offline',
      checkedAt: DateTime.now().toUtc(),
      tokenAvailable: hub.authToken != null,
      hasEncryptedToken: hub.hasEncryptedToken,
      message: distinctCandidates.isEmpty
          ? 'No endpoint is configured for this Light Box.'
          : 'No configured endpoint responded.',
    );
  }

  Future<DeviceProbeResultDto> _probeEndpoint(
    _ProbeHub hub,
    ({String route, HubEndpointDto endpoint}) candidate,
  ) async {
    final baseUrl = candidate.endpoint.baseUrl;
    final checkedAt = DateTime.now().toUtc();
    final diagnostics = RhythmDiagnosticsApi.fromBaseUrl(
      baseUrl: baseUrl,
      authToken: hub.authToken,
      connectTimeout: const Duration(seconds: 4),
      receiveTimeout: const Duration(seconds: 4),
    );
    final healthy = await diagnostics
        .healthCheck()
        .timeout(const Duration(seconds: 5), onTimeout: () => false);
    if (!healthy) {
      return DeviceProbeResultDto(
        hubId: hub.id,
        status: 'offline',
        route: candidate.route,
        baseUrl: baseUrl,
        checkedAt: checkedAt,
        tokenAvailable: hub.authToken != null,
        hasEncryptedToken: hub.hasEncryptedToken,
        message: 'Health check failed.',
      );
    }

    final state = await _fetchState(baseUrl, hub.authToken);
    if (state.authRequired) {
      return DeviceProbeResultDto(
        hubId: hub.id,
        status: 'auth_required',
        route: candidate.route,
        baseUrl: baseUrl,
        checkedAt: checkedAt,
        tokenAvailable: hub.authToken != null,
        hasEncryptedToken: hub.hasEncryptedToken,
        message: hub.authToken == null && hub.hasEncryptedToken
            ? 'Device is reachable, but its token is client-side encrypted and unavailable to admin-api.'
            : 'Device is reachable, but /api/state rejected the token.',
      );
    }

    final hello = state.hello;
    return DeviceProbeResultDto(
      hubId: hub.id,
      status: 'online',
      route: candidate.route,
      baseUrl: baseUrl,
      checkedAt: checkedAt,
      tokenAvailable: hub.authToken != null,
      hasEncryptedToken: hub.hasEncryptedToken,
      inventory: hello == null ? null : _inventoryFromState(hello),
      serverVersion: hello?.version,
      serverInstanceId: hello?.serverInstanceId ?? hub.serverInstanceId,
      message: state.error,
    );
  }

  Future<_StateFetchResult> _fetchState(String baseUrl, String? token) async {
    try {
      final response = await _http.get(
        _uriWithAppendedPath(baseUrl, 'api/state'),
        headers: {
          'Accept': 'application/json',
          if (token != null) 'Authorization': 'Bearer $token',
        },
      ).timeout(const Duration(seconds: 5));
      if (response.statusCode == 401 || response.statusCode == 403) {
        return const _StateFetchResult.authRequired();
      }
      if (response.statusCode < 200 || response.statusCode >= 300) {
        return _StateFetchResult.error(
          'Health check passed, but /api/state returned HTTP ${response.statusCode}.',
        );
      }
      final decoded = jsonDecode(response.body);
      if (decoded is! Map) {
        return const _StateFetchResult.error(
          'Health check passed, but /api/state returned invalid JSON.',
        );
      }
      return _StateFetchResult.hello(
        RhythmHello.fromJson(
          decoded.map((key, value) => MapEntry(key.toString(), value)),
        ),
      );
    } catch (error) {
      return _StateFetchResult.error(
        'Health check passed, but /api/state failed: $error',
      );
    }
  }

  Future<_ProbeHub> _loadHub({
    required AdminSession session,
    required String hubId,
  }) async {
    final table = _supabase.canUseServiceRole ? 'hubs' : 'rhythm_support_hubs';
    final rows = await _supabase.select(
      table: table,
      select: _supabase.canUseServiceRole
          ? 'id,home_id,type,name,endpoint,enabled,token,encrypted_token,remote_endpoint,server_instance_id,last_connected,created_at,updated_at'
          : 'id,home_id,type,name,endpoint,enabled,remote_endpoint,server_instance_id,last_connected,created_at,updated_at,has_legacy_token,has_encrypted_token',
      accessToken: session.accessToken,
      serviceRole: _supabase.canUseServiceRole,
      filters: {
        'id': 'eq.$hubId',
      },
      limit: 1,
    );
    if (rows.isEmpty) {
      throw const AdminApiException(404, 'Light Box hub was not found.');
    }
    final hub = _ProbeHub.fromSupabase(rows.first);
    if (hub.type != 'server') {
      throw const AdminApiException(
          400, 'Only Rhythm server hubs can be probed.');
    }
    return hub;
  }

  Uri _uriWithAppendedPath(String baseUrl, String pathToAppend) {
    final uri = Uri.parse(baseUrl.trim());
    final basePath = uri.path.endsWith('/') ? uri.path : '${uri.path}/';
    return uri.replace(
      path: '$basePath$pathToAppend',
      query: null,
      fragment: null,
    );
  }

  LightBoxInventoryDto _inventoryFromState(RhythmHello state) {
    final seen = <String>{};
    var lights = 0;
    var buttons = 0;
    var motionSensors = 0;
    var otherDevices = 0;

    void addDevice(String id, RhythmDeviceType? type) {
      if (id.isEmpty || !seen.add(id)) return;
      switch (type) {
        case RhythmDeviceType.light:
          lights++;
        case RhythmDeviceType.button:
          buttons++;
        case RhythmDeviceType.motion:
          motionSensors++;
        case null:
          otherDevices++;
      }
    }

    for (final node in state.nodes) {
      if (node.kind.isDevice) {
        addDevice(node.id, RhythmDeviceType.fromNodeKind(node.kind));
      }
      for (final device in node.devices) {
        addDevice(device.id, device.type);
      }
    }

    return LightBoxInventoryDto(
      lights: lights,
      buttons: buttons,
      motionSensors: motionSensors,
      otherDevices: otherDevices,
    );
  }
}

class _ProbeHub {
  const _ProbeHub({
    required this.id,
    required this.type,
    required this.endpoint,
    required this.hasEncryptedToken,
    this.remoteEndpoint,
    this.authToken,
    this.serverInstanceId,
  });

  final String id;
  final String type;
  final HubEndpointDto endpoint;
  final HubEndpointDto? remoteEndpoint;
  final String? authToken;
  final bool hasEncryptedToken;
  final String? serverInstanceId;

  factory _ProbeHub.fromSupabase(Map<String, dynamic> row) {
    final endpoint = asStringMap(row['endpoint']) ?? const <String, dynamic>{};
    final remoteEndpoint = asStringMap(row['remote_endpoint']);
    final legacyToken = _usableLegacyToken(row['token']);
    final encryptedToken = row['encrypted_token'];
    return _ProbeHub(
      id: row['id'] as String? ?? '',
      type: row['type'] as String? ?? '',
      endpoint: HubEndpointDto.fromJson(endpoint),
      remoteEndpoint: remoteEndpoint == null
          ? null
          : HubEndpointDto.fromJson(remoteEndpoint),
      authToken: legacyToken,
      hasEncryptedToken: row['has_encrypted_token'] as bool? ??
          encryptedToken != null || _looksLikeEncryptedEnvelope(row['token']),
      serverInstanceId: cleanString(row['server_instance_id']),
    );
  }
}

class _StateFetchResult {
  const _StateFetchResult._({
    this.hello,
    this.error,
    this.authRequired = false,
  });

  const _StateFetchResult.hello(RhythmHello hello) : this._(hello: hello);

  const _StateFetchResult.error(String error) : this._(error: error);

  const _StateFetchResult.authRequired() : this._(authRequired: true);

  final RhythmHello? hello;
  final String? error;
  final bool authRequired;
}

String? _usableLegacyToken(Object? value) {
  if (value is! String) return null;
  final trimmed = value.trim();
  if (trimmed.isEmpty) return null;
  if (_looksLikeEncryptedEnvelope(trimmed)) return null;
  return trimmed;
}

bool _looksLikeEncryptedEnvelope(Object? value) {
  if (value is! String || value.trim().isEmpty) return false;
  try {
    final decoded = jsonDecode(value);
    return decoded is Map && decoded['version'] == 'account_secret_v1';
  } catch (_) {
    return false;
  }
}
