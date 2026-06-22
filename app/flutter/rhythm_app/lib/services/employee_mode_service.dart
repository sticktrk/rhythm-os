import 'dart:async';

import 'package:flutter/foundation.dart';

class EmployeeModeSession {
  const EmployeeModeSession({
    required this.grantId,
    this.hostname,
    this.directToken,
    this.directTokenId,
    this.hubId,
    this.homeName,
    this.hubName,
    this.expiresAt,
  });

  final String grantId;
  final String? hostname;
  final String? directToken;
  final String? directTokenId;
  final String? hubId;
  final String? homeName;
  final String? hubName;
  final DateTime? expiresAt;

  String get displayName {
    final explicit = hubName?.trim();
    if (explicit != null && explicit.isNotEmpty) return explicit;
    final host = hostname?.trim();
    if (host != null && host.isNotEmpty) return host;
    return 'Support Session';
  }

  @override
  bool operator ==(Object other) {
    return other is EmployeeModeSession &&
        other.grantId == grantId &&
        other.hostname == hostname &&
        other.directToken == directToken &&
        other.directTokenId == directTokenId &&
        other.hubId == hubId &&
        other.homeName == homeName &&
        other.hubName == hubName &&
        other.expiresAt == expiresAt;
  }

  @override
  int get hashCode => Object.hash(
        grantId,
        hostname,
        directToken,
        directTokenId,
        hubId,
        homeName,
        hubName,
        expiresAt,
      );
}

class EmployeeModeService extends ChangeNotifier {
  EmployeeModeService._();

  static final EmployeeModeService instance = EmployeeModeService._();

  static const _grantIdDefine = String.fromEnvironment(
    'RHYTHM_EMPLOYEE_GRANT_ID',
  );
  static const _hostnameDefine = String.fromEnvironment(
    'RHYTHM_EMPLOYEE_HOSTNAME',
  );
  static const _hubIdDefine = String.fromEnvironment('RHYTHM_EMPLOYEE_HUB_ID');
  static const _homeNameDefine =
      String.fromEnvironment('RHYTHM_EMPLOYEE_HOME_NAME');
  static const _hubNameDefine =
      String.fromEnvironment('RHYTHM_EMPLOYEE_HUB_NAME');
  static const _expiresAtDefine =
      String.fromEnvironment('RHYTHM_EMPLOYEE_EXPIRES_AT');
  static const _directTokenDefine =
      String.fromEnvironment('RHYTHM_EMPLOYEE_DIRECT_TOKEN');
  static const _directTokenIdDefine =
      String.fromEnvironment('RHYTHM_EMPLOYEE_DIRECT_TOKEN_ID');

  final List<Future<void> Function()> _onEnabled = [];
  final List<Future<void> Function()> _onDisabled = [];

  Timer? _expiryTimer;
  EmployeeModeSession? _session;

  bool get isActive => _session != null;
  EmployeeModeSession? get session => _session;
  String? get grantId => _session?.grantId;
  bool get isExpired {
    final expiresAt = _session?.expiresAt;
    if (expiresAt == null) return false;
    return !DateTime.now().isBefore(expiresAt);
  }

  void Function() onEmployeeModeEnabled(Future<void> Function() callback) {
    _onEnabled.add(callback);
    return () => _onEnabled.remove(callback);
  }

  void Function() onEmployeeModeDisabled(Future<void> Function() callback) {
    _onDisabled.add(callback);
    return () => _onDisabled.remove(callback);
  }

  bool activateFromLaunch(Uri uri) {
    final params = _launchParameters(uri);
    final hasExplicitSupportGrant = _hasNonEmpty(params, 'employee_grant_id') ||
        _hasNonEmpty(params, 'support_grant_id');
    final hasEmployeeDefine = _grantIdDefine.trim().isNotEmpty;
    final allowGenericParams = hasExplicitSupportGrant ||
        hasEmployeeDefine ||
        _isSupportLaunch(uri, params);
    final grantId = _firstNonEmpty([
      params['employee_grant_id'],
      params['support_grant_id'],
      if (allowGenericParams) params['grant_id'],
      _grantIdDefine,
    ]);
    if (grantId == null) return false;

    return activate(
      EmployeeModeSession(
        grantId: grantId,
        hostname: _firstNonEmpty([
          params['employee_hostname'],
          params['support_hostname'],
          if (allowGenericParams) params['hostname'],
          _hostnameDefine,
        ]),
        directToken: _firstNonEmpty([
          params['employee_direct_token'],
          params['support_direct_token'],
          if (allowGenericParams) params['direct_token'],
          _directTokenDefine,
        ]),
        directTokenId: _firstNonEmpty([
          params['employee_direct_token_id'],
          params['support_direct_token_id'],
          if (allowGenericParams) params['direct_token_id'],
          _directTokenIdDefine,
        ]),
        hubId: _firstNonEmpty([
          params['employee_hub_id'],
          params['support_hub_id'],
          if (allowGenericParams) params['hub_id'],
          _hubIdDefine,
        ]),
        homeName: _firstNonEmpty([
          params['employee_home_name'],
          params['support_home_name'],
          if (allowGenericParams) params['home_name'],
          _homeNameDefine,
        ]),
        hubName: _firstNonEmpty([
          params['employee_hub_name'],
          params['support_hub_name'],
          if (allowGenericParams) params['hub_name'],
          _hubNameDefine,
        ]),
        expiresAt: _parseDateTime(_firstNonEmpty([
          params['employee_expires_at'],
          params['support_expires_at'],
          if (allowGenericParams) params['expires_at'],
          _expiresAtDefine,
        ])),
      ),
    );
  }

  static Map<String, String> _launchParameters(Uri uri) {
    final params = <String, String>{...uri.queryParameters};
    final fragmentQuery = _fragmentQuery(uri.fragment);
    if (fragmentQuery == null) return params;

    try {
      final fragmentParams = Uri.splitQueryString(fragmentQuery);
      for (final entry in fragmentParams.entries) {
        params.putIfAbsent(entry.key, () => entry.value);
      }
    } catch (_) {
      // Ignore malformed fragment query data; regular query params still work.
    }
    return params;
  }

  static String? _fragmentQuery(String fragment) {
    final clean = fragment.trim();
    if (clean.isEmpty) return null;

    final queryStart = clean.indexOf('?');
    if (queryStart >= 0 && queryStart < clean.length - 1) {
      return clean.substring(queryStart + 1);
    }
    if (clean.contains('=')) {
      return clean.startsWith('?') ? clean.substring(1) : clean;
    }
    return null;
  }

  static bool _hasNonEmpty(Map<String, String> params, String key) {
    final value = params[key]?.trim();
    return value != null && value.isNotEmpty;
  }

  static bool _isSupportLaunch(Uri uri, Map<String, String> params) {
    if (_hasTruthyFlag(params, 'employee_mode') ||
        _hasTruthyFlag(params, 'support_mode')) {
      return true;
    }

    final mode = params['mode']?.trim().toLowerCase();
    if (mode == 'employee' || mode == 'support') return true;

    final path = uri.path.toLowerCase();
    final fragment = uri.fragment.toLowerCase();
    return path.contains('employee') ||
        path.contains('support') ||
        fragment.contains('employee') ||
        fragment.contains('support');
  }

  static bool _hasTruthyFlag(Map<String, String> params, String key) {
    final raw = params[key]?.trim().toLowerCase();
    return raw == '1' || raw == 'true' || raw == 'yes';
  }

  bool activate(EmployeeModeSession session) {
    if (_sessionIsExpired(session)) {
      return false;
    }

    _expiryTimer?.cancel();
    _expiryTimer = null;

    final changed = _session != session;
    _session = session;
    _scheduleExpiry(session);
    if (changed) {
      for (final callback in List<Future<void> Function()>.of(_onEnabled)) {
        unawaited(callback());
      }
    }
    notifyListeners();
    return true;
  }

  void attachDirectAccess({
    required String hostname,
    required String token,
    String? tokenId,
    DateTime? expiresAt,
  }) {
    final current = _session;
    if (current == null) return;

    final next = EmployeeModeSession(
      grantId: current.grantId,
      hostname: hostname.trim().isEmpty ? current.hostname : hostname.trim(),
      directToken: token.trim().isEmpty ? current.directToken : token.trim(),
      directTokenId: tokenId == null || tokenId.trim().isEmpty
          ? current.directTokenId
          : tokenId.trim(),
      hubId: current.hubId,
      homeName: current.homeName,
      hubName: current.hubName,
      expiresAt: expiresAt ?? current.expiresAt,
    );
    if (_sessionIsExpired(next)) {
      exit();
      return;
    }

    _expiryTimer?.cancel();
    _session = next;
    _scheduleExpiry(next);
    notifyListeners();
  }

  void exit() {
    if (_session == null) return;
    _expiryTimer?.cancel();
    _expiryTimer = null;
    _session = null;
    for (final callback in List<Future<void> Function()>.of(_onDisabled)) {
      unawaited(callback());
    }
    notifyListeners();
  }

  void _scheduleExpiry(EmployeeModeSession session) {
    final expiresAt = session.expiresAt;
    if (expiresAt == null) return;

    final remaining = expiresAt.difference(DateTime.now());
    if (remaining <= Duration.zero) {
      _expireIfCurrent(session);
      return;
    }

    _expiryTimer = Timer(remaining, () => _expireIfCurrent(session));
  }

  void _expireIfCurrent(EmployeeModeSession session) {
    if (_session != session) return;
    exit();
  }

  static bool _sessionIsExpired(EmployeeModeSession session) {
    final expiresAt = session.expiresAt;
    if (expiresAt == null) return false;
    return !DateTime.now().isBefore(expiresAt);
  }

  static String? _firstNonEmpty(Iterable<String?> values) {
    for (final value in values) {
      final trimmed = value?.trim();
      if (trimmed != null && trimmed.isNotEmpty) return trimmed;
    }
    return null;
  }

  static DateTime? _parseDateTime(String? value) {
    if (value == null) return null;
    return DateTime.tryParse(value);
  }
}
