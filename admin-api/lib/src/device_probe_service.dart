import 'dart:async';
import 'dart:convert';

import 'package:cryptography/cryptography.dart';
import 'package:http/http.dart' as http;
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show RhythmDeviceType, RhythmDiagnosticsApi, RhythmHello;

import 'models.dart';
import 'supabase_rest_client.dart';
import 'support_access_service.dart';

class DeviceProbeService {
  DeviceProbeService({
    required SupabaseRestClient supabase,
    SupportAccessService? supportAccess,
    http.Client? httpClient,
  })  : _supabase = supabase,
        _supportAccess = supportAccess,
        _http = httpClient ?? http.Client();

  final SupabaseRestClient _supabase;
  final SupportAccessService? _supportAccess;
  final http.Client _http;

  Future<DeviceProbeResultDto> probeHub({
    required AdminSession session,
    required String hubId,
  }) async {
    final hub = await _loadHub(session: session, hubId: hubId);
    final distinctCandidates = _endpointCandidates(hub);

    var sawAuthRequired = false;
    for (final candidate in distinctCandidates) {
      final result = await _probeEndpoint(session, hub, candidate);
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

  Future<DeviceDebugBundleDto> downloadDebugBundle({
    required AdminSession session,
    required String hubId,
  }) async {
    final hub = await _loadHub(session: session, hubId: hubId);
    final candidates = _endpointCandidates(hub);
    if (candidates.isEmpty) {
      throw const AdminApiException(
        400,
        'No endpoint is configured for this Light Box.',
      );
    }

    final failures = <String>[];
    var sawAuthRequired = false;
    for (final candidate in candidates) {
      final baseUrl = candidate.endpoint.baseUrl;
      final token = await _authTokenForEndpoint(session, hub, baseUrl);
      final result = await _downloadDebugBundleFromEndpoint(
        hub: hub,
        candidate: candidate,
        authToken: token,
      );
      if (result.bundle != null) return result.bundle!;
      if (result.authRequired) sawAuthRequired = true;
      if (result.message != null) {
        failures.add('${candidate.route} $baseUrl: ${result.message}');
      }
    }

    if (sawAuthRequired) {
      throw AdminApiException(
        403,
        _debugBundleAuthRequiredMessage(hub),
      );
    }

    throw AdminApiException(
      502,
      failures.isEmpty
          ? 'No configured endpoint returned a debug bundle.'
          : 'No configured endpoint returned a debug bundle. '
              '${failures.join(' ')}',
    );
  }

  Future<DeviceDebugBundleSubmissionDto> submitDebugBundle({
    required AdminSession session,
    required String hubId,
    required String uploadUrl,
  }) async {
    final hub = await _loadHub(session: session, hubId: hubId);
    final candidates = _endpointCandidates(hub);
    if (candidates.isEmpty) {
      throw const AdminApiException(
        400,
        'No endpoint is configured for this Light Box.',
      );
    }

    final failures = <String>[];
    var sawAuthRequired = false;
    for (final candidate in candidates) {
      final baseUrl = candidate.endpoint.baseUrl;
      final token = await _authTokenForEndpoint(session, hub, baseUrl);
      final result = await _submitDebugBundleToEndpoint(
        hub: hub,
        candidate: candidate,
        authToken: token,
        uploadUrl: uploadUrl,
      );
      if (result.submission != null) return result.submission!;
      if (result.authRequired) sawAuthRequired = true;
      if (result.message != null) {
        failures.add('${candidate.route} $baseUrl: ${result.message}');
      }
    }

    if (sawAuthRequired) {
      throw AdminApiException(403, _debugBundleAuthRequiredMessage(hub));
    }
    throw AdminApiException(
      502,
      failures.isEmpty
          ? 'No configured endpoint submitted a debug bundle.'
          : 'No configured endpoint submitted a debug bundle. '
              '${failures.join(' ')}',
    );
  }

  Future<DeviceStatusDto> loadStatus({
    required AdminSession session,
    required String hubId,
  }) async {
    final hub = await _loadHub(session: session, hubId: hubId);
    final candidates = _endpointCandidates(hub);
    if (candidates.isEmpty) {
      throw const AdminApiException(
        400,
        'No endpoint is configured for this Light Box.',
      );
    }

    final failures = <String>[];
    for (final candidate in candidates) {
      final baseUrl = candidate.endpoint.baseUrl;
      final token = await _authTokenForEndpoint(session, hub, baseUrl);
      final health = await _fetchJsonFromEndpoint(
        hub: hub,
        candidate: candidate,
        path: 'health',
        authToken: null,
        operation: 'health',
      );
      if (health.success == null) {
        if (health.message != null) {
          failures.add('${candidate.route} $baseUrl: ${health.message}');
        }
        continue;
      }

      final errors = <String, String>{};
      Future<Map<String, dynamic>?> fetchSection(
        String key,
        String path,
      ) async {
        final result = await _fetchJsonFromEndpoint(
          hub: hub,
          candidate: candidate,
          path: path,
          authToken: token,
          operation: key,
        );
        if (result.success != null) return result.success!.body;
        errors[key] = result.authRequired
            ? _operationAuthRequiredMessage(hub, path)
            : result.message ?? '$key request failed.';
        return null;
      }

      final stateJson = await fetchSection('state', 'api/state');
      final remoteAccess =
          await fetchSection('remoteAccess', 'api/remote-access/status');
      final auth = await fetchSection('auth', 'api/auth/status');
      final ota = await fetchSection('ota', 'api/ota/status');

      return DeviceStatusDto(
        hubId: hub.id,
        route: candidate.route,
        baseUrl: baseUrl,
        checkedAt: DateTime.now().toUtc(),
        tokenAvailable: token != null,
        hasEncryptedToken: hub.hasEncryptedToken,
        health: health.success!.body,
        state: stateJson == null ? null : _stateSummaryFromJson(stateJson),
        remoteAccess: remoteAccess,
        auth: auth,
        ota: ota,
        errors: errors,
      );
    }

    throw AdminApiException(
      502,
      failures.isEmpty
          ? 'No configured endpoint returned health status.'
          : 'No configured endpoint returned health status. '
              '${failures.join(' ')}',
    );
  }

  Future<DeviceOtaActionDto> checkForUpdate({
    required AdminSession session,
    required String hubId,
  }) async {
    final result = await _fetchFirstJson(
      session: session,
      hubId: hubId,
      path: 'api/ota/check',
      operation: 'OTA update check',
    );
    return DeviceOtaActionDto(
      hubId: result.hubId,
      route: result.route,
      baseUrl: result.baseUrl,
      action: 'check',
      completedAt: DateTime.now().toUtc(),
      tokenAvailable: result.tokenAvailable,
      hasEncryptedToken: result.hasEncryptedToken,
      result: result.body,
    );
  }

  Future<DeviceOtaActionDto> applyUpdate({
    required AdminSession session,
    required String hubId,
  }) async {
    final result = await _fetchFirstJson(
      session: session,
      hubId: hubId,
      path: 'api/ota/update',
      operation: 'OTA update',
      method: 'POST',
      timeout: const Duration(minutes: 2),
    );
    return DeviceOtaActionDto(
      hubId: result.hubId,
      route: result.route,
      baseUrl: result.baseUrl,
      action: 'update',
      completedAt: DateTime.now().toUtc(),
      tokenAvailable: result.tokenAvailable,
      hasEncryptedToken: result.hasEncryptedToken,
      result: result.body,
    );
  }

  Future<DeviceLogSourcesDto> listLogs({
    required AdminSession session,
    required String hubId,
  }) async {
    final result = await _fetchFirstJson(
      session: session,
      hubId: hubId,
      path: 'api/diag/logs',
      operation: 'log sources',
    );
    final sourcesValue = result.body['sources'];
    final sources = sourcesValue is List
        ? sourcesValue
            .whereType<Map>()
            .map(
              (source) => DeviceLogSourceDto.fromJson(
                source.map((key, value) => MapEntry(key.toString(), value)),
              ),
            )
            .where((source) => source.id.trim().isNotEmpty)
            .toList(growable: false)
        : const <DeviceLogSourceDto>[];
    return DeviceLogSourcesDto(
      hubId: result.hubId,
      route: result.route,
      baseUrl: result.baseUrl,
      fetchedAt: DateTime.now().toUtc(),
      sources: sources,
    );
  }

  Future<DeviceLogTailDto> tailLog({
    required AdminSession session,
    required String hubId,
    required String sourceId,
    int lines = 200,
  }) async {
    final cleanSourceId = sourceId.trim();
    if (cleanSourceId.isEmpty ||
        cleanSourceId.contains('/') ||
        cleanSourceId.contains('\\')) {
      throw const AdminApiException(400, 'Invalid log source id.');
    }
    final clampedLines = lines.clamp(1, 2000).toInt();
    final result = await _fetchFirstJson(
      session: session,
      hubId: hubId,
      path: 'api/diag/logs/$cleanSourceId/tail',
      queryParameters: {'lines': '$clampedLines'},
      operation: 'log tail',
    );
    final tail = asStringMap(result.body['tail']);
    if (tail == null) {
      throw const AdminApiException(
        502,
        'Device returned an invalid log tail response.',
      );
    }

    final source = asStringMap(tail['source']) ?? const <String, dynamic>{};
    final linesValue = tail['lines'];
    final parsedLines = linesValue is List
        ? linesValue
            .whereType<Map>()
            .map(
              (line) => DeviceLogTailLineDto.fromJson(
                line.map((key, value) => MapEntry(key.toString(), value)),
              ),
            )
            .toList(growable: false)
        : const <DeviceLogTailLineDto>[];
    return DeviceLogTailDto(
      hubId: result.hubId,
      route: result.route,
      baseUrl: result.baseUrl,
      fetchedAt: DateTime.now().toUtc(),
      source: DeviceLogSourceDto.fromJson(source),
      lines: parsedLines,
      requestedLines: (tail['requestedLines'] as num?)?.toInt() ??
          (tail['requested_lines'] as num?)?.toInt() ??
          clampedLines,
      returnedLines: (tail['returnedLines'] as num?)?.toInt() ??
          (tail['returned_lines'] as num?)?.toInt() ??
          parsedLines.length,
    );
  }

  Future<DeviceAdminProxyResultDto> proxyJson({
    required AdminSession session,
    required String hubId,
    required DeviceAdminProxyRequestDto request,
  }) async {
    final hub = await _loadHub(session: session, hubId: hubId);
    final candidates = _endpointCandidates(hub);
    if (candidates.isEmpty) {
      throw const AdminApiException(
        400,
        'No endpoint is configured for this Light Box.',
      );
    }

    final failures = <String>[];
    var sawAuthRequired = false;
    var sawPreconditionFailure = false;
    for (final candidate in candidates) {
      final baseUrl = candidate.endpoint.baseUrl;
      final token = await _authTokenForEndpoint(session, hub, baseUrl);
      var result = await _proxyJsonFromEndpoint(
        hub: hub,
        candidate: candidate,
        request: request,
        authToken: token,
      );
      if (result.authRequired && hub.authToken == null && token != null) {
        // The cached support-session token was rejected (revoked or the
        // device restarted); mint a fresh one and retry this endpoint once.
        _supportAccess?.invalidateSessionToken(
          hubId: hub.id,
          baseUrl: baseUrl,
        );
        final freshToken =
            await _supportAccessSessionToken(session, hub, baseUrl);
        if (freshToken != null && freshToken != token) {
          result = await _proxyJsonFromEndpoint(
            hub: hub,
            candidate: candidate,
            request: request,
            authToken: freshToken,
          );
        }
      }
      if (result.success != null) return result.success!;
      if (result.authRequired) sawAuthRequired = true;
      if (result.preconditionFailed) sawPreconditionFailure = true;
      if (result.message != null) {
        failures.add('${candidate.route} $baseUrl: ${result.message}');
      }
      if (request.isMutation && result.mutationDispatched) {
        // Once a mutation has reached an endpoint, a transport or response
        // failure is ambiguous: the device may already have applied it. Do
        // not replay the same mutation through another route.
        break;
      }
    }

    if (sawAuthRequired) {
      throw AdminApiException(
        403,
        _operationAuthRequiredMessage(hub, request.path),
      );
    }

    if (sawPreconditionFailure) {
      throw AdminApiException(
        409,
        failures.isEmpty
            ? 'Device mutation preconditions no longer match.'
            : 'Device mutation preconditions no longer match. '
                '${failures.join(' ')}',
      );
    }

    throw AdminApiException(
      502,
      failures.isEmpty
          ? 'No configured endpoint completed ${request.method} /${request.path}.'
          : 'No configured endpoint completed ${request.method} /${request.path}. '
              '${failures.join(' ')}',
    );
  }

  Future<DeviceProbeResultDto> _probeEndpoint(
    AdminSession session,
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

    final supportSessionToken = hub.authToken == null
        ? await _supportAccessSessionToken(session, hub, baseUrl)
        : null;
    final stateAuthToken = hub.authToken ?? supportSessionToken;
    final state = await _fetchState(baseUrl, stateAuthToken);
    if (state.authRequired) {
      if (supportSessionToken != null) {
        // Don't keep serving a token the device just rejected.
        _supportAccess?.invalidateSessionToken(hubId: hub.id, baseUrl: baseUrl);
      }
      return DeviceProbeResultDto(
        hubId: hub.id,
        status: 'auth_required',
        route: candidate.route,
        baseUrl: baseUrl,
        checkedAt: checkedAt,
        tokenAvailable: stateAuthToken != null,
        hasEncryptedToken: hub.hasEncryptedToken,
        message: _authRequiredMessage(hub, supportSessionToken),
      );
    }

    final hello = state.hello;
    return DeviceProbeResultDto(
      hubId: hub.id,
      status: 'online',
      route: candidate.route,
      baseUrl: baseUrl,
      checkedAt: checkedAt,
      tokenAvailable: stateAuthToken != null,
      hasEncryptedToken: hub.hasEncryptedToken,
      inventory: hello == null ? null : _inventoryFromState(hello),
      serverVersion: hello?.version,
      serverInstanceId: hello?.serverInstanceId ?? hub.serverInstanceId,
      message: state.error,
    );
  }

  Future<_ProxyJsonEndpointResult> _proxyJsonFromEndpoint({
    required _ProbeHub hub,
    required ({String route, HubEndpointDto endpoint}) candidate,
    required DeviceAdminProxyRequestDto request,
    required String? authToken,
  }) async {
    final baseUrl = candidate.endpoint.baseUrl;
    var mutationDispatched = false;
    try {
      String? verifiedServerInstanceId;
      String? preconditionBodySha256;
      if (request.isMutation) {
        final state = await _fetchState(baseUrl, authToken);
        if (state.authRequired) {
          return const _ProxyJsonEndpointResult.authRequired();
        }
        if (state.error != null) {
          return _ProxyJsonEndpointResult.error(
            'mutation identity preflight failed: ${state.error}',
          );
        }
        verifiedServerInstanceId = cleanString(state.hello?.serverInstanceId);
        if (verifiedServerInstanceId == null ||
            verifiedServerInstanceId != request.expectedServerInstanceId) {
          return _ProxyJsonEndpointResult.preconditionFailed(
            'live server identity did not match expectedServerInstanceId.',
          );
        }

        final resource = request.resourcePrecondition;
        if (resource != null) {
          final current = await _fetchJsonFromEndpoint(
            hub: hub,
            candidate: candidate,
            path: resource.path,
            authToken: authToken,
            operation: 'mutation resource preflight',
            queryParameters: resource.queryParameters,
          );
          if (current.authRequired) {
            return const _ProxyJsonEndpointResult.authRequired();
          }
          if (current.success == null) {
            return _ProxyJsonEndpointResult.error(
              current.message ?? 'mutation resource preflight failed.',
            );
          }
          preconditionBodySha256 =
              await _canonicalJsonSha256(current.success!.body);
          if (preconditionBodySha256 != resource.bodySha256) {
            return _ProxyJsonEndpointResult.preconditionFailed(
              'live resource hash did not match the reviewed proposal.',
            );
          }
        }
      }

      final uri = _uriWithAppendedPath(
        baseUrl,
        request.path,
        queryParameters:
            request.queryParameters.isEmpty ? null : request.queryParameters,
      );
      final outbound = http.Request(request.method, uri)
        ..headers.addAll({
          'Accept': 'application/json',
          if (authToken != null) 'Authorization': 'Bearer $authToken',
          if (request.requestId != null) 'X-Request-Id': request.requestId!,
          if (verifiedServerInstanceId != null)
            'X-Expected-Server-Instance-Id': verifiedServerInstanceId,
          if (preconditionBodySha256 != null)
            'X-Expected-Resource-Sha256': preconditionBodySha256,
        });
      if (request.method != 'GET' && request.body != null) {
        outbound.headers['Content-Type'] = 'application/json';
        outbound.body = jsonEncode(request.body);
      }

      mutationDispatched = request.isMutation;
      final streamed = await _http.send(outbound).timeout(request.timeout);
      final response = await http.Response.fromStream(streamed);
      if (response.statusCode == 401 || response.statusCode == 403) {
        return const _ProxyJsonEndpointResult.authRequired();
      }
      if (request.resourcePrecondition != null && response.statusCode == 409) {
        return _ProxyJsonEndpointResult.preconditionFailed(
          '${request.method} /${request.path} was rejected by the device because its live preconditions changed'
          '${_responseErrorSuffix(response)}.',
          mutationDispatched: true,
        );
      }
      if (response.statusCode < 200 || response.statusCode >= 300) {
        return _ProxyJsonEndpointResult.error(
          '${request.method} /${request.path} returned HTTP ${response.statusCode}'
          '${_responseErrorSuffix(response)}.',
          mutationDispatched: mutationDispatched,
        );
      }

      final trimmedBody = response.body.trim();
      Object? decoded;
      if (trimmedBody.isNotEmpty) {
        try {
          decoded = jsonDecode(response.body);
        } catch (_) {
          return _ProxyJsonEndpointResult.error(
            '${request.method} /${request.path} returned invalid JSON.',
            mutationDispatched: mutationDispatched,
          );
        }
      }

      return _ProxyJsonEndpointResult.success(
        DeviceAdminProxyResultDto(
          hubId: hub.id,
          route: candidate.route,
          baseUrl: baseUrl,
          method: request.method,
          path: request.path,
          queryParameters: request.queryParameters,
          statusCode: response.statusCode,
          completedAt: DateTime.now().toUtc(),
          tokenAvailable: authToken != null,
          hasEncryptedToken: hub.hasEncryptedToken,
          body: decoded,
          requestId: request.requestId,
          verifiedServerInstanceId: verifiedServerInstanceId,
          bodySha256:
              decoded == null ? null : await _canonicalJsonSha256(decoded),
          preconditionBodySha256: preconditionBodySha256,
        ),
      );
    } on TimeoutException {
      return _ProxyJsonEndpointResult.error(
        '${request.method} /${request.path} request timed out.',
        mutationDispatched: mutationDispatched,
      );
    } catch (error) {
      return _ProxyJsonEndpointResult.error(
        '${request.method} /${request.path} request failed: $error',
        mutationDispatched: mutationDispatched,
      );
    }
  }

  Future<_DebugBundleEndpointResult> _downloadDebugBundleFromEndpoint({
    required _ProbeHub hub,
    required ({String route, HubEndpointDto endpoint}) candidate,
    required String? authToken,
  }) async {
    final baseUrl = candidate.endpoint.baseUrl;
    try {
      final response = await _http.post(
        _uriWithAppendedPath(baseUrl, 'api/diag/debug-bundle'),
        headers: {
          'Accept': 'application/gzip,application/octet-stream,*/*',
          if (authToken != null) 'Authorization': 'Bearer $authToken',
        },
      ).timeout(const Duration(minutes: 2));
      if (response.statusCode == 401 || response.statusCode == 403) {
        return const _DebugBundleEndpointResult.authRequired();
      }
      if (response.statusCode < 200 || response.statusCode >= 300) {
        return _DebugBundleEndpointResult.error(
          'debug bundle returned HTTP ${response.statusCode}'
          '${_responseErrorSuffix(response)}.',
        );
      }
      if (response.bodyBytes.isEmpty) {
        return const _DebugBundleEndpointResult.error(
          'debug bundle response was empty.',
        );
      }

      return _DebugBundleEndpointResult.bundle(
        DeviceDebugBundleDto(
          hubId: hub.id,
          route: candidate.route,
          baseUrl: baseUrl,
          fileName: _attachmentFileName(
                response.headers['content-disposition'],
              ) ??
              _fallbackDebugBundleFileName(hub.id),
          contentType: response.headers['content-type'] ?? 'application/gzip',
          bytes: response.bodyBytes,
        ),
      );
    } on TimeoutException {
      return const _DebugBundleEndpointResult.error(
        'debug bundle request timed out.',
      );
    } catch (error) {
      return _DebugBundleEndpointResult.error(
        'debug bundle request failed: $error',
      );
    }
  }

  Future<_DebugBundleSubmissionEndpointResult> _submitDebugBundleToEndpoint({
    required _ProbeHub hub,
    required ({String route, HubEndpointDto endpoint}) candidate,
    required String? authToken,
    required String uploadUrl,
  }) async {
    final baseUrl = candidate.endpoint.baseUrl;
    try {
      final response = await _http
          .post(
            _uriWithAppendedPath(baseUrl, 'api/diag/debug-bundle'),
            headers: {
              'Accept': 'application/json,application/gzip,*/*',
              'Content-Type': 'application/json',
              if (authToken != null) 'Authorization': 'Bearer $authToken',
            },
            body: jsonEncode({'upload_url': uploadUrl}),
          )
          .timeout(const Duration(minutes: 3));
      if (response.statusCode == 401 || response.statusCode == 403) {
        return const _DebugBundleSubmissionEndpointResult.authRequired();
      }
      if (response.statusCode < 200 || response.statusCode >= 300) {
        return _DebugBundleSubmissionEndpointResult.error(
          'debug bundle submission returned HTTP ${response.statusCode}'
          '${_responseErrorSuffix(response)}.',
        );
      }

      final contentType = response.headers['content-type'] ?? '';
      final isLegacyBundle = contentType.contains('application/gzip') ||
          (response.bodyBytes.length >= 2 &&
              response.bodyBytes[0] == 0x1f &&
              response.bodyBytes[1] == 0x8b);
      if (isLegacyBundle) {
        return _DebugBundleSubmissionEndpointResult.submission(
          DeviceDebugBundleSubmissionDto(
            hubId: hub.id,
            route: candidate.route,
            baseUrl: baseUrl,
            contentType: contentType.isEmpty ? 'application/gzip' : contentType,
            uploadedByDevice: false,
            fileName: _attachmentFileName(
                  response.headers['content-disposition'],
                ) ??
                _fallbackDebugBundleFileName(hub.id),
            sizeBytes: response.bodyBytes.length,
            legacyBytes: response.bodyBytes,
          ),
        );
      }

      final decoded = jsonDecode(response.body);
      if (decoded is! Map || decoded['uploaded'] != true) {
        return const _DebugBundleSubmissionEndpointResult.error(
          'device did not confirm the debug bundle upload.',
        );
      }
      return _DebugBundleSubmissionEndpointResult.submission(
        DeviceDebugBundleSubmissionDto(
          hubId: hub.id,
          route: candidate.route,
          baseUrl: baseUrl,
          contentType: 'application/gzip',
          uploadedByDevice: true,
          fileName: cleanString(decoded['file_name']),
          sizeBytes: (decoded['size_bytes'] as num?)?.toInt(),
        ),
      );
    } on TimeoutException {
      return const _DebugBundleSubmissionEndpointResult.error(
        'debug bundle submission timed out.',
      );
    } on FormatException {
      return const _DebugBundleSubmissionEndpointResult.error(
        'device returned an invalid debug bundle submission response.',
      );
    } catch (error) {
      return _DebugBundleSubmissionEndpointResult.error(
        'debug bundle submission failed: $error',
      );
    }
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

  Future<_JsonEndpointSuccess> _fetchFirstJson({
    required AdminSession session,
    required String hubId,
    required String path,
    required String operation,
    String method = 'GET',
    Duration timeout = const Duration(seconds: 10),
    Map<String, String>? queryParameters,
  }) async {
    final hub = await _loadHub(session: session, hubId: hubId);
    final candidates = _endpointCandidates(hub);
    if (candidates.isEmpty) {
      throw const AdminApiException(
        400,
        'No endpoint is configured for this Light Box.',
      );
    }

    final failures = <String>[];
    var sawAuthRequired = false;
    for (final candidate in candidates) {
      final baseUrl = candidate.endpoint.baseUrl;
      final token = await _authTokenForEndpoint(session, hub, baseUrl);
      final result = await _fetchJsonFromEndpoint(
        hub: hub,
        candidate: candidate,
        path: path,
        authToken: token,
        operation: operation,
        method: method,
        timeout: timeout,
        queryParameters: queryParameters,
      );
      if (result.success != null) return result.success!;
      if (result.authRequired) sawAuthRequired = true;
      if (result.message != null) {
        failures.add('${candidate.route} $baseUrl: ${result.message}');
      }
    }

    if (sawAuthRequired) {
      throw AdminApiException(
        403,
        _operationAuthRequiredMessage(hub, path),
      );
    }

    throw AdminApiException(
      502,
      failures.isEmpty
          ? 'No configured endpoint returned $operation.'
          : 'No configured endpoint returned $operation. '
              '${failures.join(' ')}',
    );
  }

  Future<_JsonEndpointResult> _fetchJsonFromEndpoint({
    required _ProbeHub hub,
    required ({String route, HubEndpointDto endpoint}) candidate,
    required String path,
    required String? authToken,
    required String operation,
    String method = 'GET',
    Duration timeout = const Duration(seconds: 10),
    Map<String, String>? queryParameters,
  }) async {
    final baseUrl = candidate.endpoint.baseUrl;
    try {
      final uri = _uriWithAppendedPath(
        baseUrl,
        path,
        queryParameters: queryParameters,
      );
      final headers = {
        'Accept': 'application/json',
        if (authToken != null) 'Authorization': 'Bearer $authToken',
      };
      final response = method == 'POST'
          ? await _http.post(uri, headers: headers).timeout(timeout)
          : await _http.get(uri, headers: headers).timeout(timeout);
      if (response.statusCode == 401 || response.statusCode == 403) {
        return const _JsonEndpointResult.authRequired();
      }
      if (response.statusCode < 200 || response.statusCode >= 300) {
        return _JsonEndpointResult.error(
          '$operation returned HTTP ${response.statusCode}'
          '${_responseErrorSuffix(response)}.',
        );
      }

      final decoded = jsonDecode(response.body);
      if (decoded is! Map) {
        return _JsonEndpointResult.error(
          '$operation returned invalid JSON.',
        );
      }
      return _JsonEndpointResult.success(
        _JsonEndpointSuccess(
          hubId: hub.id,
          route: candidate.route,
          baseUrl: baseUrl,
          tokenAvailable: authToken != null,
          hasEncryptedToken: hub.hasEncryptedToken,
          body: decoded.map((key, value) => MapEntry(key.toString(), value)),
        ),
      );
    } on TimeoutException {
      return _JsonEndpointResult.error('$operation request timed out.');
    } catch (error) {
      return _JsonEndpointResult.error('$operation request failed: $error');
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

  List<({String route, HubEndpointDto endpoint})> _endpointCandidates(
    _ProbeHub hub,
  ) {
    final candidates = <({String route, HubEndpointDto endpoint})>[
      if (hub.remoteEndpoint != null)
        (route: 'remote', endpoint: hub.remoteEndpoint!),
      (route: 'local', endpoint: hub.endpoint),
    ];
    final seen = <String>{};
    return [
      for (final candidate in candidates)
        if (seen.add(candidate.endpoint.baseUrl.toLowerCase())) candidate,
    ];
  }

  Future<String?> _authTokenForEndpoint(
    AdminSession session,
    _ProbeHub hub,
    String baseUrl,
  ) async {
    if (hub.authToken != null) return hub.authToken;
    return _supportAccessSessionToken(session, hub, baseUrl);
  }

  Future<String?> _supportAccessSessionToken(
    AdminSession session,
    _ProbeHub hub,
    String baseUrl,
  ) {
    return _supportAccess?.createSessionToken(
          session: session,
          hubId: hub.id,
          baseUrl: baseUrl,
        ) ??
        Future<String?>.value();
  }

  Uri _uriWithAppendedPath(
    String baseUrl,
    String pathToAppend, {
    Map<String, String>? queryParameters,
  }) {
    final uri = Uri.parse(baseUrl.trim());
    final basePath = uri.path.endsWith('/') ? uri.path : '${uri.path}/';
    return uri.replace(
      path: '$basePath$pathToAppend',
      queryParameters: queryParameters,
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
        case RhythmDeviceType.contact:
          otherDevices++;
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

  DeviceStateSummaryDto _stateSummaryFromJson(Map<String, dynamic> json) {
    final hello = RhythmHello.fromJson(json);
    final mode = asStringMap(json['mode']);
    final settings = asStringMap(json['settings']);
    return DeviceStateSummaryDto(
      serverVersion: hello.version,
      serverInstanceId: hello.serverInstanceId,
      platformType: hello.platformType,
      platformContext: hello.platformContext,
      listenPort: hello.listenPort,
      nodeCount: hello.nodes.length,
      hubCount: hello.hubs.length,
      lastTickEpochMs: hello.lastTickEpochMs,
      inventory: _inventoryFromState(hello),
      activeMode: cleanString(mode?['id']) ??
          cleanString(mode?['mode']) ??
          cleanString(json['active_mode']),
      lightRuntime: cleanString(json['light_runtime']) ??
          cleanString(json['runtime_id']) ??
          cleanString(settings?['light_runtime']),
    );
  }

  String _authRequiredMessage(_ProbeHub hub, String? supportSessionToken) {
    if (supportSessionToken != null) {
      return 'Device is reachable, but /api/state rejected the admin support session token.';
    }
    if (hub.authToken == null && hub.hasEncryptedToken) {
      return 'Device is reachable, but its token is client-side encrypted and no admin support grant is available to admin-api.';
    }
    if (hub.authToken == null) {
      return 'Device is reachable, but admin-api has no usable token for /api/state.';
    }
    return 'Device is reachable, but /api/state rejected the token.';
  }

  String _debugBundleAuthRequiredMessage(_ProbeHub hub) {
    return _operationAuthRequiredMessage(hub, 'api/diag/debug-bundle');
  }

  String _operationAuthRequiredMessage(_ProbeHub hub, String path) {
    if (hub.authToken == null && hub.hasEncryptedToken) {
      return 'Device is reachable, but its token is client-side encrypted and no admin support grant is available to admin-api.';
    }
    if (hub.authToken == null) {
      return 'Device is reachable, but admin-api has no usable token for /$path.';
    }
    return 'Device is reachable, but /$path rejected the token.';
  }

  String _responseErrorSuffix(http.Response response) {
    final message = _responseErrorMessage(response);
    return message == null ? '' : ': $message';
  }

  String? _responseErrorMessage(http.Response response) {
    final body = response.body.trim();
    if (body.isEmpty) return null;
    try {
      final decoded = jsonDecode(body);
      if (decoded is Map) {
        final message = decoded['message'] ?? decoded['error'];
        if (message != null) return _truncate(message.toString(), 180);
      }
    } catch (_) {
      // Fall through to raw response text.
    }
    return _truncate(body, 180);
  }

  String _truncate(String value, int maxLength) {
    if (value.length <= maxLength) return value;
    return '${value.substring(0, maxLength)}...';
  }

  String? _attachmentFileName(String? contentDisposition) {
    if (contentDisposition == null) return null;
    final starMatch = RegExp(
      r'''filename\*=UTF-8''([^;]+)''',
      caseSensitive: false,
    ).firstMatch(contentDisposition);
    if (starMatch != null) {
      return _safeFileName(Uri.decodeComponent(starMatch.group(1)!));
    }

    final quotedMatch = RegExp(
      r'''filename="([^"]+)"''',
      caseSensitive: false,
    ).firstMatch(contentDisposition);
    if (quotedMatch != null) return _safeFileName(quotedMatch.group(1)!);

    final bareMatch = RegExp(
      r'''filename=([^;]+)''',
      caseSensitive: false,
    ).firstMatch(contentDisposition);
    if (bareMatch != null) return _safeFileName(bareMatch.group(1)!);
    return null;
  }

  String? _safeFileName(String raw) {
    final sanitized = raw
        .trim()
        .split(RegExp(r'''[/\\]'''))
        .last
        .replaceAll(RegExp(r'''[\x00-\x1f\x7f]'''), '')
        .trim();
    return sanitized.isEmpty ? null : sanitized;
  }

  String _fallbackDebugBundleFileName(String hubId) {
    final safeId = hubId.replaceAll(RegExp(r'[^A-Za-z0-9_.-]+'), '-');
    final suffix = safeId.isEmpty ? 'hub' : safeId;
    return 'rhythm-debug-bundle-$suffix.tar.gz';
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

class _DebugBundleEndpointResult {
  const _DebugBundleEndpointResult._({
    this.bundle,
    this.message,
    this.authRequired = false,
  });

  const _DebugBundleEndpointResult.bundle(DeviceDebugBundleDto bundle)
      : this._(bundle: bundle);

  const _DebugBundleEndpointResult.error(String message)
      : this._(message: message);

  const _DebugBundleEndpointResult.authRequired()
      : this._(authRequired: true, message: 'authentication required');

  final DeviceDebugBundleDto? bundle;
  final String? message;
  final bool authRequired;
}

class _DebugBundleSubmissionEndpointResult {
  const _DebugBundleSubmissionEndpointResult._({
    this.submission,
    this.message,
    this.authRequired = false,
  });

  const _DebugBundleSubmissionEndpointResult.submission(
    DeviceDebugBundleSubmissionDto submission,
  ) : this._(submission: submission);

  const _DebugBundleSubmissionEndpointResult.error(String message)
      : this._(message: message);

  const _DebugBundleSubmissionEndpointResult.authRequired()
      : this._(authRequired: true, message: 'authentication required');

  final DeviceDebugBundleSubmissionDto? submission;
  final String? message;
  final bool authRequired;
}

class _ProxyJsonEndpointResult {
  const _ProxyJsonEndpointResult._({
    this.success,
    this.message,
    this.authRequired = false,
    this.preconditionFailed = false,
    this.mutationDispatched = false,
  });

  const _ProxyJsonEndpointResult.success(DeviceAdminProxyResultDto success)
      : this._(success: success);

  const _ProxyJsonEndpointResult.error(
    String message, {
    bool mutationDispatched = false,
  }) : this._(
          message: message,
          mutationDispatched: mutationDispatched,
        );

  const _ProxyJsonEndpointResult.authRequired()
      : this._(authRequired: true, message: 'authentication required');

  const _ProxyJsonEndpointResult.preconditionFailed(
    String message, {
    bool mutationDispatched = false,
  }) : this._(
          preconditionFailed: true,
          message: message,
          mutationDispatched: mutationDispatched,
        );

  final DeviceAdminProxyResultDto? success;
  final String? message;
  final bool authRequired;
  final bool preconditionFailed;
  final bool mutationDispatched;
}

Future<String> _canonicalJsonSha256(Object? value) async {
  final canonical = _canonicalJsonValue(value);
  final digest = await Sha256().hash(utf8.encode(jsonEncode(canonical)));
  return digest.bytes
      .map((byte) => byte.toRadixString(16).padLeft(2, '0'))
      .join();
}

Object? _canonicalJsonValue(Object? value) {
  if (value is Map) {
    final keys = value.keys.map((key) => key.toString()).toList()..sort();
    return <String, Object?>{
      for (final key in keys) key: _canonicalJsonValue(value[key]),
    };
  }
  if (value is List) {
    return value.map(_canonicalJsonValue).toList(growable: false);
  }
  return value;
}

class _JsonEndpointSuccess {
  const _JsonEndpointSuccess({
    required this.hubId,
    required this.route,
    required this.baseUrl,
    required this.tokenAvailable,
    required this.hasEncryptedToken,
    required this.body,
  });

  final String hubId;
  final String route;
  final String baseUrl;
  final bool tokenAvailable;
  final bool hasEncryptedToken;
  final Map<String, dynamic> body;
}

class _JsonEndpointResult {
  const _JsonEndpointResult._({
    this.success,
    this.message,
    this.authRequired = false,
  });

  const _JsonEndpointResult.success(_JsonEndpointSuccess success)
      : this._(success: success);

  const _JsonEndpointResult.error(String message) : this._(message: message);

  const _JsonEndpointResult.authRequired()
      : this._(authRequired: true, message: 'authentication required');

  final _JsonEndpointSuccess? success;
  final String? message;
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
