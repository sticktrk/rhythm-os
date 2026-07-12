class AdminApiException implements Exception {
  const AdminApiException(this.statusCode, this.message);

  final int statusCode;
  final String message;

  @override
  String toString() => message;
}

class AdminUser {
  const AdminUser({
    required this.id,
    required this.email,
    this.createdAt,
  });

  final String id;
  final String? email;
  final DateTime? createdAt;

  factory AdminUser.fromJson(Map<String, dynamic> json) {
    return AdminUser(
      id: json['id'] as String? ?? '',
      email: json['email'] as String?,
      createdAt: parseDateTime(json['created_at']),
    );
  }

  Map<String, dynamic> toJson() => {
        'id': id,
        if (email != null) 'email': email,
        if (createdAt != null) 'createdAt': createdAt!.toIso8601String(),
      };
}

class StaffStatus {
  const StaffStatus({
    required this.role,
    required this.enabled,
  });

  const StaffStatus.none()
      : role = null,
        enabled = false;

  final String? role;
  final bool enabled;

  bool get isActive => enabled && role != null && role!.trim().isNotEmpty;
  bool get isAdmin => isActive && role == 'admin';

  Map<String, dynamic> toJson() => {
        'role': role,
        'enabled': enabled,
        'isActive': isActive,
        'isAdmin': isAdmin,
      };
}

class AdminSession {
  const AdminSession({
    required this.accessToken,
    required this.user,
    required this.staff,
  });

  final String accessToken;
  final AdminUser user;
  final StaffStatus staff;
}

class HubEndpointDto {
  const HubEndpointDto({
    required this.host,
    required this.port,
    required this.useSsl,
  });

  final String host;
  final int port;
  final bool useSsl;

  String get baseUrl => '${useSsl ? 'https' : 'http'}://$host:$port';

  factory HubEndpointDto.fromJson(Map<String, dynamic> json) {
    return HubEndpointDto(
      host: json['host'] as String? ?? '',
      port: (json['port'] as num?)?.toInt() ?? 80,
      useSsl: json['useSsl'] as bool? ?? json['use_ssl'] as bool? ?? false,
    );
  }

  Map<String, dynamic> toJson() => {
        'host': host,
        'port': port,
        'useSsl': useSsl,
        'baseUrl': baseUrl,
      };
}

class SupportHomeDto {
  const SupportHomeDto({
    required this.id,
    required this.name,
    required this.ownerId,
    required this.memberIds,
    this.locationCity,
    this.timezone,
    this.createdAt,
    this.updatedAt,
  });

  final String id;
  final String name;
  final String ownerId;
  final List<String> memberIds;
  final String? locationCity;
  final String? timezone;
  final DateTime? createdAt;
  final DateTime? updatedAt;

  factory SupportHomeDto.fromSupabase(Map<String, dynamic> row) {
    final location = asStringMap(row['location']);
    return SupportHomeDto(
      id: row['id'] as String? ?? '',
      name: row['name'] as String? ?? 'Unnamed home',
      ownerId: row['owner_id'] as String? ?? '',
      memberIds: (row['member_ids'] as List<dynamic>? ?? const [])
          .map((value) => value.toString())
          .toList(growable: false),
      locationCity: location?['cityName'] as String?,
      timezone: row['timezone'] as String?,
      createdAt: parseDateTime(row['created_at']),
      updatedAt: parseDateTime(row['updated_at']),
    );
  }

  Map<String, dynamic> toJson() => {
        'id': id,
        'name': name,
        'ownerId': ownerId,
        'memberIds': memberIds,
        if (locationCity != null) 'locationCity': locationCity,
        if (timezone != null) 'timezone': timezone,
        if (createdAt != null) 'createdAt': createdAt!.toIso8601String(),
        if (updatedAt != null) 'updatedAt': updatedAt!.toIso8601String(),
      };
}

class SupportHubDto {
  const SupportHubDto({
    required this.id,
    required this.homeId,
    required this.type,
    required this.name,
    required this.endpoint,
    required this.enabled,
    required this.hasLegacyToken,
    required this.hasEncryptedToken,
    this.remoteEndpoint,
    this.serverInstanceId,
    this.lastConnected,
    this.createdAt,
    this.updatedAt,
  });

  final String id;
  final String homeId;
  final String type;
  final String name;
  final HubEndpointDto endpoint;
  final HubEndpointDto? remoteEndpoint;
  final bool enabled;
  final bool hasLegacyToken;
  final bool hasEncryptedToken;
  final String? serverInstanceId;
  final DateTime? lastConnected;
  final DateTime? createdAt;
  final DateTime? updatedAt;

  factory SupportHubDto.fromSupabase(Map<String, dynamic> row) {
    final endpoint = asStringMap(row['endpoint']) ?? const <String, dynamic>{};
    final remoteEndpoint = asStringMap(row['remote_endpoint']);
    return SupportHubDto(
      id: row['id'] as String? ?? '',
      homeId: row['home_id'] as String? ?? '',
      type: row['type'] as String? ?? '',
      name: row['name'] as String? ?? 'Unnamed Light Box',
      endpoint: HubEndpointDto.fromJson(endpoint),
      remoteEndpoint: remoteEndpoint == null
          ? null
          : HubEndpointDto.fromJson(remoteEndpoint),
      enabled: row['enabled'] as bool? ?? true,
      hasLegacyToken:
          row['has_legacy_token'] as bool? ?? _hasUsableString(row['token']),
      hasEncryptedToken:
          row['has_encrypted_token'] as bool? ?? row['encrypted_token'] != null,
      serverInstanceId: cleanString(row['server_instance_id']),
      lastConnected: parseDateTime(row['last_connected']),
      createdAt: parseDateTime(row['created_at']),
      updatedAt: parseDateTime(row['updated_at']),
    );
  }

  Map<String, dynamic> toJson() => {
        'id': id,
        'homeId': homeId,
        'type': type,
        'name': name,
        'endpoint': endpoint.toJson(),
        if (remoteEndpoint != null) 'remoteEndpoint': remoteEndpoint!.toJson(),
        'enabled': enabled,
        'hasLegacyToken': hasLegacyToken,
        'hasEncryptedToken': hasEncryptedToken,
        if (serverInstanceId != null) 'serverInstanceId': serverInstanceId,
        if (lastConnected != null)
          'lastConnected': lastConnected!.toIso8601String(),
        if (createdAt != null) 'createdAt': createdAt!.toIso8601String(),
        if (updatedAt != null) 'updatedAt': updatedAt!.toIso8601String(),
      };
}

class SupportCustomerIdentity {
  const SupportCustomerIdentity({
    required this.userId,
    this.email,
    this.name,
  });

  final String userId;
  final String? email;
  final String? name;

  factory SupportCustomerIdentity.fromSupabase(Map<String, dynamic> row) {
    return SupportCustomerIdentity(
      userId: row['user_id'] as String? ?? '',
      email: cleanString(row['email']),
      name: cleanString(row['name']),
    );
  }
}

class SupportCustomerHomeDto {
  const SupportCustomerHomeDto({
    required this.home,
    required this.hubs,
  });

  final SupportHomeDto home;
  final List<SupportHubDto> hubs;

  Map<String, dynamic> toJson() => {
        'home': home.toJson(),
        'hubs': hubs.map((hub) => hub.toJson()).toList(growable: false),
      };
}

class SupportCustomerDto {
  const SupportCustomerDto({
    required this.ownerId,
    required this.customerLabel,
    required this.homes,
    this.customerEmail,
    this.customerName,
  });

  final String ownerId;
  final String customerLabel;
  final String? customerEmail;
  final String? customerName;
  final List<SupportCustomerHomeDto> homes;

  String? get secondaryLabel {
    final email = customerEmail?.trim();
    final name = customerName?.trim();
    if (email == null || email.isEmpty) return null;
    if (name == null || name.isEmpty) return null;
    if (email.toLowerCase() == name.toLowerCase()) return null;
    return email;
  }

  Map<String, dynamic> toJson() => {
        'ownerId': ownerId,
        'customerLabel': customerLabel,
        if (customerEmail != null) 'customerEmail': customerEmail,
        if (customerName != null) 'customerName': customerName,
        if (secondaryLabel != null) 'secondaryLabel': secondaryLabel,
        'homes': homes.map((home) => home.toJson()).toList(growable: false),
      };
}

class SupportSnapshotDto {
  const SupportSnapshotDto({
    required this.staffStatus,
    required this.customers,
  });

  final StaffStatus staffStatus;
  final List<SupportCustomerDto> customers;

  Map<String, dynamic> toJson() {
    final homes = customers.fold<int>(
      0,
      (total, customer) => total + customer.homes.length,
    );
    final hubs = customers.fold<int>(
      0,
      (total, customer) =>
          total +
          customer.homes.fold<int>(
            0,
            (homeTotal, home) => homeTotal + home.hubs.length,
          ),
    );
    return {
      'staffStatus': staffStatus.toJson(),
      'totals': {
        'customers': customers.length,
        'homes': homes,
        'hubs': hubs,
      },
      'customers': customers
          .map((customer) => customer.toJson())
          .toList(growable: false),
    };
  }
}

class LightBoxInventoryDto {
  const LightBoxInventoryDto({
    required this.lights,
    required this.buttons,
    required this.motionSensors,
    required this.otherDevices,
  });

  final int lights;
  final int buttons;
  final int motionSensors;
  final int otherDevices;

  int get total => lights + buttons + motionSensors + otherDevices;

  Map<String, dynamic> toJson() => {
        'lights': lights,
        'buttons': buttons,
        'motionSensors': motionSensors,
        'otherDevices': otherDevices,
        'total': total,
      };
}

class DeviceProbeResultDto {
  const DeviceProbeResultDto({
    required this.hubId,
    required this.status,
    required this.checkedAt,
    required this.tokenAvailable,
    required this.hasEncryptedToken,
    this.route,
    this.baseUrl,
    this.inventory,
    this.serverVersion,
    this.serverInstanceId,
    this.message,
  });

  final String hubId;
  final String status;
  final String? route;
  final String? baseUrl;
  final DateTime checkedAt;
  final LightBoxInventoryDto? inventory;
  final String? serverVersion;
  final String? serverInstanceId;
  final bool tokenAvailable;
  final bool hasEncryptedToken;
  final String? message;

  Map<String, dynamic> toJson() => {
        'hubId': hubId,
        'status': status,
        if (route != null) 'route': route,
        if (baseUrl != null) 'baseUrl': baseUrl,
        'checkedAt': checkedAt.toIso8601String(),
        'tokenAvailable': tokenAvailable,
        'hasEncryptedToken': hasEncryptedToken,
        if (inventory != null) 'inventory': inventory!.toJson(),
        if (serverVersion != null) 'serverVersion': serverVersion,
        if (serverInstanceId != null) 'serverInstanceId': serverInstanceId,
        if (message != null) 'message': message,
      };
}

class DeviceDebugBundleDto {
  const DeviceDebugBundleDto({
    required this.hubId,
    required this.route,
    required this.baseUrl,
    required this.fileName,
    required this.contentType,
    required this.bytes,
  });

  final String hubId;
  final String route;
  final String baseUrl;
  final String fileName;
  final String contentType;
  final List<int> bytes;
}

class DeviceDebugBundleSubmissionDto {
  const DeviceDebugBundleSubmissionDto({
    required this.hubId,
    required this.route,
    required this.baseUrl,
    required this.contentType,
    required this.uploadedByDevice,
    this.fileName,
    this.sizeBytes,
    this.legacyBytes,
  });

  final String hubId;
  final String route;
  final String baseUrl;
  final String contentType;
  final bool uploadedByDevice;
  final String? fileName;
  final int? sizeBytes;
  final List<int>? legacyBytes;
}

class DeviceLogSourceDto {
  const DeviceLogSourceDto({
    required this.id,
    required this.fileName,
    required this.bytes,
    this.modifiedAt,
  });

  final String id;
  final String fileName;
  final int bytes;
  final DateTime? modifiedAt;

  factory DeviceLogSourceDto.fromJson(Map<String, dynamic> json) {
    return DeviceLogSourceDto(
      id: json['id'] as String? ?? '',
      fileName: json['fileName'] as String? ??
          json['file_name'] as String? ??
          json['id'] as String? ??
          '',
      bytes: (json['bytes'] as num?)?.toInt() ?? 0,
      modifiedAt: parseDateTime(json['modifiedAt'] ?? json['modified_at']),
    );
  }

  Map<String, dynamic> toJson() => {
        'id': id,
        'fileName': fileName,
        'bytes': bytes,
        if (modifiedAt != null) 'modifiedAt': modifiedAt!.toIso8601String(),
      };
}

class DeviceLogSourcesDto {
  const DeviceLogSourcesDto({
    required this.hubId,
    required this.route,
    required this.baseUrl,
    required this.fetchedAt,
    required this.sources,
  });

  final String hubId;
  final String route;
  final String baseUrl;
  final DateTime fetchedAt;
  final List<DeviceLogSourceDto> sources;

  Map<String, dynamic> toJson() => {
        'hubId': hubId,
        'route': route,
        'baseUrl': baseUrl,
        'fetchedAt': fetchedAt.toIso8601String(),
        'sources': sources.map((source) => source.toJson()).toList(),
      };
}

class DeviceLogTailLineDto {
  const DeviceLogTailLineDto({
    required this.source,
    required this.lineNumber,
    required this.text,
  });

  final String source;
  final int lineNumber;
  final String text;

  factory DeviceLogTailLineDto.fromJson(Map<String, dynamic> json) {
    return DeviceLogTailLineDto(
      source: json['source'] as String? ?? '',
      lineNumber: (json['lineNumber'] as num?)?.toInt() ??
          (json['line_number'] as num?)?.toInt() ??
          0,
      text: json['text'] as String? ?? '',
    );
  }

  Map<String, dynamic> toJson() => {
        'source': source,
        'lineNumber': lineNumber,
        'text': text,
      };
}

class DeviceLogTailDto {
  const DeviceLogTailDto({
    required this.hubId,
    required this.route,
    required this.baseUrl,
    required this.fetchedAt,
    required this.source,
    required this.lines,
    required this.requestedLines,
    required this.returnedLines,
  });

  final String hubId;
  final String route;
  final String baseUrl;
  final DateTime fetchedAt;
  final DeviceLogSourceDto source;
  final List<DeviceLogTailLineDto> lines;
  final int requestedLines;
  final int returnedLines;

  Map<String, dynamic> toJson() => {
        'hubId': hubId,
        'route': route,
        'baseUrl': baseUrl,
        'fetchedAt': fetchedAt.toIso8601String(),
        'source': source.toJson(),
        'lines': lines.map((line) => line.toJson()).toList(),
        'requestedLines': requestedLines,
        'returnedLines': returnedLines,
      };
}

class DeviceStateSummaryDto {
  const DeviceStateSummaryDto({
    required this.serverVersion,
    required this.platformType,
    required this.platformContext,
    required this.nodeCount,
    required this.hubCount,
    this.serverInstanceId,
    this.listenPort,
    this.lastTickEpochMs,
    this.inventory,
    this.activeMode,
    this.lightRuntime,
  });

  final String serverVersion;
  final String? serverInstanceId;
  final String platformType;
  final String platformContext;
  final int? listenPort;
  final int nodeCount;
  final int hubCount;
  final int? lastTickEpochMs;
  final LightBoxInventoryDto? inventory;
  final String? activeMode;
  final String? lightRuntime;

  Map<String, dynamic> toJson() => {
        'serverVersion': serverVersion,
        if (serverInstanceId != null) 'serverInstanceId': serverInstanceId,
        'platformType': platformType,
        'platformContext': platformContext,
        if (listenPort != null) 'listenPort': listenPort,
        'nodeCount': nodeCount,
        'hubCount': hubCount,
        if (lastTickEpochMs != null) 'lastTickEpochMs': lastTickEpochMs,
        if (inventory != null) 'inventory': inventory!.toJson(),
        if (activeMode != null) 'activeMode': activeMode,
        if (lightRuntime != null) 'lightRuntime': lightRuntime,
      };
}

class DeviceStatusDto {
  const DeviceStatusDto({
    required this.hubId,
    required this.route,
    required this.baseUrl,
    required this.checkedAt,
    required this.tokenAvailable,
    required this.hasEncryptedToken,
    required this.errors,
    this.health,
    this.state,
    this.remoteAccess,
    this.auth,
    this.ota,
  });

  final String hubId;
  final String route;
  final String baseUrl;
  final DateTime checkedAt;
  final bool tokenAvailable;
  final bool hasEncryptedToken;
  final Map<String, dynamic>? health;
  final DeviceStateSummaryDto? state;
  final Map<String, dynamic>? remoteAccess;
  final Map<String, dynamic>? auth;
  final Map<String, dynamic>? ota;
  final Map<String, String> errors;

  Map<String, dynamic> toJson() => {
        'hubId': hubId,
        'route': route,
        'baseUrl': baseUrl,
        'checkedAt': checkedAt.toIso8601String(),
        'tokenAvailable': tokenAvailable,
        'hasEncryptedToken': hasEncryptedToken,
        if (health != null) 'health': health,
        if (state != null) 'state': state!.toJson(),
        if (remoteAccess != null) 'remoteAccess': remoteAccess,
        if (auth != null) 'auth': auth,
        if (ota != null) 'ota': ota,
        'errors': errors,
      };
}

class DeviceOtaActionDto {
  const DeviceOtaActionDto({
    required this.hubId,
    required this.route,
    required this.baseUrl,
    required this.action,
    required this.completedAt,
    required this.tokenAvailable,
    required this.hasEncryptedToken,
    required this.result,
  });

  final String hubId;
  final String route;
  final String baseUrl;
  final String action;
  final DateTime completedAt;
  final bool tokenAvailable;
  final bool hasEncryptedToken;
  final Map<String, dynamic> result;

  Map<String, dynamic> toJson() => {
        'hubId': hubId,
        'route': route,
        'baseUrl': baseUrl,
        'action': action,
        'completedAt': completedAt.toIso8601String(),
        'tokenAvailable': tokenAvailable,
        'hasEncryptedToken': hasEncryptedToken,
        'result': result,
      };
}

class FleetFindingReportRequestDto {
  const FleetFindingReportRequestDto({
    required this.title,
    required this.severity,
    required this.scanRunId,
    required this.detectedAt,
    required this.occurrences,
    required this.affectedHubCount,
    required this.samples,
    required this.findingIds,
    this.journeyStage,
    this.serverVersion,
    this.platformContext,
  });

  final String title;
  final String severity;
  final String scanRunId;
  final DateTime detectedAt;
  final int occurrences;
  final int affectedHubCount;
  final List<String> samples;
  final List<String> findingIds;
  final String? journeyStage;
  final String? serverVersion;
  final String? platformContext;

  factory FleetFindingReportRequestDto.fromJson(Map<String, dynamic> json) {
    final rawTitle = cleanString(json['title']);
    final title = rawTitle == null ? null : _sanitizeFleetText(rawTitle, 180);
    final severity = cleanString(json['severity'])?.toLowerCase() ?? 'error';
    final scanRunId = cleanString(json['scanRunId']);
    final detectedAt = parseDateTime(json['detectedAt']);
    if (title == null || title.length > 180) {
      throw const AdminApiException(
        400,
        'Fleet finding title is required and must be at most 180 characters.',
      );
    }
    if (!const {'critical', 'error', 'warning'}.contains(severity)) {
      throw const AdminApiException(
        400,
        'Fleet finding severity must be critical, error, or warning.',
      );
    }
    if (scanRunId == null ||
        scanRunId.length > 100 ||
        !RegExp(r'^[A-Za-z0-9_.:-]+$').hasMatch(scanRunId)) {
      throw const AdminApiException(400, 'Fleet scan run id is required.');
    }
    if (detectedAt == null) {
      throw const AdminApiException(
        400,
        'Fleet finding detectedAt must be an ISO-8601 timestamp.',
      );
    }
    final rawSamples = json['samples'];
    final samples = rawSamples is List
        ? rawSamples
            .whereType<String>()
            .map((sample) => _sanitizeFleetText(sample, 500))
            .where((sample) => sample.isNotEmpty)
            .take(12)
            .toList(growable: false)
        : const <String>[];
    final requestedJourneyStage = cleanString(json['journeyStage']);
    final rawFindingIds = json['findingIds'];
    final findingIds = rawFindingIds is List
        ? rawFindingIds
            .whereType<String>()
            .map((value) => value.trim())
            .where(
              (value) => RegExp(r'^[A-Za-z0-9_.:-]{1,120}$').hasMatch(value),
            )
            .take(12)
            .toList(growable: false)
        : const <String>[];
    final requestedOccurrences = json['occurrences'];
    final requestedAffectedHubCount = json['affectedHubCount'];
    const journeyStages = {
      'discovery',
      'BLE',
      'Wi-Fi',
      'LAN/auth',
      'cloud identity',
      'remote access',
      'OTA/reboot',
      'integration commissioning',
      'canonicalization',
      'command dispatch',
      'physical confirmation',
      'unknown',
    };
    return FleetFindingReportRequestDto(
      title: title,
      severity: severity,
      scanRunId: scanRunId,
      detectedAt: detectedAt,
      occurrences: ((requestedOccurrences is num
                  ? requestedOccurrences.toInt()
                  : null) ??
              1)
          .clamp(1, 100000)
          .toInt(),
      affectedHubCount: ((requestedAffectedHubCount is num
                  ? requestedAffectedHubCount.toInt()
                  : null) ??
              1)
          .clamp(1, 100000)
          .toInt(),
      samples: samples,
      findingIds: findingIds,
      journeyStage: journeyStages.contains(requestedJourneyStage)
          ? requestedJourneyStage
          : 'unknown',
      serverVersion: _sanitizeFleetMetadata(json['serverVersion']),
      platformContext: _sanitizeFleetMetadata(json['platformContext']),
    );
  }
}

String? _sanitizeFleetMetadata(Object? value) {
  final cleaned = cleanString(value);
  return cleaned == null ? null : _sanitizeFleetText(cleaned, 100);
}

String _sanitizeFleetText(String value, int maxLength) {
  var sanitized = value
      .replaceAll(RegExp(r'\x1b\[[0-9;]*m'), '')
      .replaceAll(
        RegExp(r'\bbearer\s+[A-Za-z0-9._~+/=-]+', caseSensitive: false),
        'Bearer <redacted>',
      )
      .replaceAll(
        RegExp(
          r'\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b',
        ),
        '<jwt>',
      )
      .replaceAll(
        RegExp(
          r'\b(token|secret|password|authorization|api[_-]?key)\s*[:=]\s*[^\s,;]+',
          caseSensitive: false,
        ),
        'credential=<redacted>',
      )
      .replaceAll(
        RegExp(r'\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b'),
        '<email>',
      )
      .replaceAll(RegExp(r'https?://[^\s)]+'), '<url>')
      .replaceAll(
        RegExp(
          r'\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b',
        ),
        '<uuid>',
      )
      .trim();
  if (sanitized.length > maxLength) {
    sanitized = '${sanitized.substring(0, maxLength - 3)}...';
  }
  return sanitized;
}

class FleetFindingReportResultDto {
  const FleetFindingReportResultDto({
    required this.submissionId,
    required this.referenceCode,
    required this.reportResult,
  });

  final String submissionId;
  final String referenceCode;
  final Map<String, dynamic> reportResult;

  Map<String, dynamic> toJson() => {
        'submissionId': submissionId,
        'referenceCode': referenceCode,
        ...reportResult,
      };
}

class DeviceAdminProxyRequestDto {
  const DeviceAdminProxyRequestDto({
    required this.method,
    required this.path,
    required this.queryParameters,
    required this.body,
    required this.timeout,
  });

  final String method;
  final String path;
  final Map<String, String> queryParameters;
  final Object? body;
  final Duration timeout;

  factory DeviceAdminProxyRequestDto.fromJson(Map<String, dynamic> json) {
    final method = (json['method'] as String? ?? 'GET').trim().toUpperCase();
    final path = _normalizeDeviceAdminPath(json['path']);
    final query =
        _parseStringQueryMap(json['query'] ?? json['queryParameters']);
    final requestedTimeoutSeconds =
        (json['timeoutSeconds'] as num?)?.toInt() ?? 20;
    final timeoutSeconds = requestedTimeoutSeconds < 1
        ? 1
        : requestedTimeoutSeconds > 120
            ? 120
            : requestedTimeoutSeconds;
    if (!const {'GET', 'POST', 'PUT', 'PATCH', 'DELETE'}.contains(method)) {
      throw const AdminApiException(
        400,
        'Device admin method must be GET, POST, PUT, PATCH, or DELETE.',
      );
    }

    return DeviceAdminProxyRequestDto(
      method: method,
      path: path,
      queryParameters: query,
      body: json.containsKey('body') ? json['body'] : null,
      timeout: Duration(seconds: timeoutSeconds),
    );
  }

  static String _normalizeDeviceAdminPath(Object? value) {
    if (value is! String || value.trim().isEmpty) {
      throw const AdminApiException(400, 'Device admin path is required.');
    }
    var path = value.trim();
    if (path.startsWith('/')) path = path.substring(1);
    final lower = path.toLowerCase();
    if (lower.startsWith('http://') ||
        lower.startsWith('https://') ||
        path.contains('?') ||
        path.contains('#') ||
        path.contains('\\') ||
        path.split('/').any((part) => part == '..')) {
      throw const AdminApiException(
        400,
        'Device admin path must be a relative device API path.',
      );
    }
    if (!(path == 'health' || path.startsWith('api/'))) {
      throw const AdminApiException(
        400,
        'Device admin path must target health or /api/*.',
      );
    }
    if (path == 'api/diag/debug-bundle' || path == 'api/ota/upload') {
      throw const AdminApiException(
        400,
        'This endpoint is not available through the JSON device admin proxy.',
      );
    }
    return path;
  }

  static Map<String, String> _parseStringQueryMap(Object? value) {
    if (value == null) return const {};
    if (value is! Map) {
      throw const AdminApiException(
        400,
        'Device admin query parameters must be an object.',
      );
    }
    final result = <String, String>{};
    for (final entry in value.entries) {
      final key = entry.key.toString().trim();
      if (key.isEmpty) continue;
      final queryValue = entry.value;
      if (queryValue == null) continue;
      result[key] = queryValue.toString();
    }
    return result;
  }
}

class DeviceAdminProxyResultDto {
  const DeviceAdminProxyResultDto({
    required this.hubId,
    required this.route,
    required this.baseUrl,
    required this.method,
    required this.path,
    required this.queryParameters,
    required this.statusCode,
    required this.completedAt,
    required this.tokenAvailable,
    required this.hasEncryptedToken,
    required this.body,
  });

  final String hubId;
  final String route;
  final String baseUrl;
  final String method;
  final String path;
  final Map<String, String> queryParameters;
  final int statusCode;
  final DateTime completedAt;
  final bool tokenAvailable;
  final bool hasEncryptedToken;
  final Object? body;

  Map<String, dynamic> toJson() => {
        'hubId': hubId,
        'route': route,
        'baseUrl': baseUrl,
        'method': method,
        'path': path,
        if (queryParameters.isNotEmpty) 'queryParameters': queryParameters,
        'statusCode': statusCode,
        'completedAt': completedAt.toIso8601String(),
        'tokenAvailable': tokenAvailable,
        'hasEncryptedToken': hasEncryptedToken,
        'body': body,
      };
}

Map<String, dynamic>? asStringMap(Object? value) {
  if (value is! Map) return null;
  return value.map((key, value) => MapEntry(key.toString(), value));
}

String? cleanString(Object? value) {
  if (value is! String) return null;
  final trimmed = value.trim();
  return trimmed.isEmpty ? null : trimmed;
}

DateTime? parseDateTime(Object? value) {
  if (value is DateTime) return value;
  if (value is! String || value.trim().isEmpty) return null;
  return DateTime.tryParse(value);
}

bool _hasUsableString(Object? value) =>
    value is String && value.trim().isNotEmpty;
