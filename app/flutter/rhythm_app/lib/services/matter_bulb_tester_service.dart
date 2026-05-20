import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:package_info_plus/package_info_plus.dart';
import 'package:supabase_flutter/supabase_flutter.dart';

import '../backend/backend.dart';
import '../data/local_data_source.dart';
import 'auth_service.dart';

class MatterBulbTesterCloudResult {
  const MatterBulbTesterCloudResult({
    required this.uploaded,
    this.message,
    this.reportId,
  });

  final bool uploaded;
  final String? message;
  final String? reportId;
}

class MatterBulbTesterService {
  MatterBulbTesterService._();

  static MatterBulbTesterService? _instance;
  static MatterBulbTesterService get instance =>
      _instance ??= MatterBulbTesterService._();

  static const _reportsKey = 'matter_bulb_tester_reports_v1';
  static const _cloudFunctionName = 'report-matter-bulb';

  Future<List<Map<String, dynamic>>> loadLocalReports() async {
    final source = await _dataSource();
    final raw = source.getSettingsValue(_reportsKey);
    if (raw is! String || raw.isEmpty) return const [];

    try {
      final decoded = jsonDecode(raw);
      if (decoded is! List) return const [];
      return decoded
          .whereType<Map>()
          .map((report) => Map<String, dynamic>.from(report))
          .toList(growable: false);
    } catch (error) {
      debugPrint(
          'MatterBulbTesterService: failed to read local reports: $error');
      return const [];
    }
  }

  Future<void> saveLocalReport(Map<String, dynamic> report) async {
    final source = await _dataSource();
    final reports = await loadLocalReports();
    final reportId = report['report_id'] as String?;
    final updated = <Map<String, dynamic>>[
      report,
      for (final existing in reports)
        if (reportId == null || existing['report_id'] != reportId) existing,
    ];
    await source.saveSettingsValue(
        _reportsKey, jsonEncode(updated.take(25).toList()));
  }

  Future<MatterBulbTesterCloudResult> submitCloudReport({
    required Map<String, dynamic> report,
    String? serverVersion,
    String? serverPlatformContext,
  }) async {
    final client = _client;
    final auth = AuthService();
    final user = auth.currentUser;
    final userId = auth.currentUserId;

    if (client == null || user == null || userId == null) {
      return const MatterBulbTesterCloudResult(
        uploaded: false,
        message: 'Cloud reporting is unavailable.',
      );
    }

    try {
      final packageInfo = await PackageInfo.fromPlatform();
      final response = await client.functions.invoke(
        _cloudFunctionName,
        body: {
          'report': report,
          'app_version': packageInfo.version,
          'app_build': packageInfo.buildNumber,
          'app_platform': defaultTargetPlatform.name,
          if (serverVersion != null) 'server_version': serverVersion,
          if (serverPlatformContext != null)
            'server_platform_context': serverPlatformContext,
        },
      );

      final data = response.data;
      if (response.status < 200 || response.status >= 300) {
        return MatterBulbTesterCloudResult(
          uploaded: false,
          message: _extractMessage(data) ?? 'Cloud report failed.',
        );
      }

      return MatterBulbTesterCloudResult(
        uploaded: true,
        reportId: data is Map ? data['id'] as String? : null,
      );
    } catch (error) {
      return MatterBulbTesterCloudResult(
        uploaded: false,
        message: error.toString(),
      );
    }
  }

  SupabaseClient? get _client {
    if (!BackendProvider.isInitialized) return null;
    final backend = BackendProvider.instance.auth;
    if (backend is SupabaseAuthBackend) {
      return backend.client;
    }
    return null;
  }

  Future<LocalDataSource> _dataSource() async {
    final source = LocalDataSource();
    if (!source.isInitialized) {
      await source.initialize();
    }
    return source;
  }

  String? _extractMessage(Object? data) {
    if (data is Map) {
      final error = data['error'] ?? data['message'];
      if (error != null) return error.toString();
    }
    return null;
  }
}
