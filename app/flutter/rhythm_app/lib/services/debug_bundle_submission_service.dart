import 'package:flutter/foundation.dart';
import 'package:package_info_plus/package_info_plus.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:supabase_flutter/supabase_flutter.dart';
import 'package:uuid/uuid.dart';

import '../backend/backend.dart';
import 'auth_service.dart';

class DebugBundleSubmission {
  const DebugBundleSubmission({
    required this.id,
    required this.referenceCode,
    required this.status,
    this.createdAt,
    this.githubIssueUrl,
    this.githubIssueNumber,
    this.githubIssueError,
  });

  final String id;
  final String referenceCode;
  final String status;
  final DateTime? createdAt;
  final String? githubIssueUrl;
  final int? githubIssueNumber;
  final String? githubIssueError;

  factory DebugBundleSubmission.fromRow(Map<String, dynamic> row) {
    return DebugBundleSubmission(
      id: row['id'] as String? ?? '',
      referenceCode: row['reference_code'] as String? ?? '',
      status: row['status'] as String? ?? 'received',
      createdAt: _tryParseDateTime(row['created_at']),
      githubIssueUrl: row['github_issue_url'] as String?,
      githubIssueNumber: _tryParseInt(row['github_issue_number']),
      githubIssueError: row['github_issue_error'] as String?,
    );
  }

  DebugBundleSubmission copyWith({
    String? status,
    String? githubIssueUrl,
    int? githubIssueNumber,
    String? githubIssueError,
  }) {
    return DebugBundleSubmission(
      id: id,
      referenceCode: referenceCode,
      status: status ?? this.status,
      createdAt: createdAt,
      githubIssueUrl: githubIssueUrl ?? this.githubIssueUrl,
      githubIssueNumber: githubIssueNumber ?? this.githubIssueNumber,
      githubIssueError: githubIssueError ?? this.githubIssueError,
    );
  }
}

class DebugBundleSubmissionException implements Exception {
  const DebugBundleSubmissionException(this.message, {this.cause});

  final String message;
  final Object? cause;

  @override
  String toString() => message;
}

class DebugBundleSubmissionService {
  DebugBundleSubmissionService._();

  static const String bucketName = 'support-debug-bundles';
  static const String tableName = 'support_debug_bundle_submissions';
  static const String reportBugFunctionName = 'report-bug';

  static DebugBundleSubmissionService? _instance;
  static DebugBundleSubmissionService get instance =>
      _instance ??= DebugBundleSubmissionService._();

  static const _uuid = Uuid();

  Future<DebugBundleSubmission> submit({
    required Hub serverHub,
    required RhythmDebugBundle bundle,
    required String serverVersion,
    required String serverPlatformContext,
    String? summary,
  }) async {
    final auth = AuthService();
    final userId = auth.currentUserId;
    final client = _client;

    if (client == null || userId == null || auth.currentUser == null) {
      throw const DebugBundleSubmissionException(
        'Debug bundle submissions are unavailable right now.',
      );
    }

    final packageInfo = await _loadPackageInfo();
    final submissionId = _uuid.v4();
    final sanitizedFileName = _sanitizeFileName(bundle.fileName);
    final storagePath = _buildStoragePath(
      userId: userId,
      submissionId: submissionId,
      fileName: sanitizedFileName,
    );

    try {
      await client.storage.from(bucketName).uploadBinary(
            storagePath,
            bundle.bytes,
            fileOptions: FileOptions(
              contentType: bundle.contentType,
              upsert: false,
            ),
          );
    } catch (error) {
      throw DebugBundleSubmissionException(
        _formatUploadError(error),
        cause: error,
      );
    }

    late final DebugBundleSubmission submission;
    try {
      final row = await client
          .from(tableName)
          .insert({
            'user_id': userId,
            'user_email': auth.currentUser?.email,
            'is_anonymous': auth.isAnonymous,
            'summary': _normalizeSummary(summary),
            'app_version': packageInfo.version,
            'app_build': packageInfo.buildNumber,
            'app_platform': _platformLabel(),
            'server_hub_id': serverHub.id,
            'server_name': serverHub.name,
            'server_host': serverHub.endpoint.host,
            'server_port': serverHub.endpoint.port,
            'server_version': serverVersion,
            'server_platform_context': serverPlatformContext,
            'bundle_storage_path': storagePath,
            'bundle_file_name': sanitizedFileName,
            'bundle_content_type': bundle.contentType,
            'bundle_size_bytes': bundle.bytes.length,
          })
          .select()
          .single();

      submission =
          DebugBundleSubmission.fromRow(Map<String, dynamic>.from(row));
    } catch (error) {
      await _deleteUploadedBundle(client, storagePath);
      throw DebugBundleSubmissionException(
        _formatInsertError(error),
        cause: error,
      );
    }

    final issueReport = await _createGitHubIssue(client, submission.id);
    return submission.copyWith(
      status: issueReport.status,
      githubIssueUrl: issueReport.url,
      githubIssueNumber: issueReport.number,
      githubIssueError: issueReport.error,
    );
  }

  /// Submit a text-only bug report — no debug bundle, no required server hub.
  Future<DebugBundleSubmission> submitTextOnly({
    String? summary,
    Hub? serverHub,
    String? serverVersion,
    String? serverPlatformContext,
  }) async {
    final auth = AuthService();
    final userId = auth.currentUserId;
    final client = _client;

    if (client == null || userId == null || auth.currentUser == null) {
      throw const DebugBundleSubmissionException(
        'Bug report submissions are unavailable right now.',
      );
    }

    final packageInfo = await _loadPackageInfo();

    late final DebugBundleSubmission submission;
    try {
      final row = await client
          .from(tableName)
          .insert({
            'user_id': userId,
            'user_email': auth.currentUser?.email,
            'is_anonymous': auth.isAnonymous,
            'summary': _normalizeSummary(summary),
            'app_version': packageInfo.version,
            'app_build': packageInfo.buildNumber,
            'app_platform': _platformLabel(),
            if (serverHub != null) 'server_hub_id': serverHub.id,
            if (serverHub != null) 'server_name': serverHub.name,
            if (serverHub != null) 'server_host': serverHub.endpoint.host,
            if (serverHub != null) 'server_port': serverHub.endpoint.port,
            if (serverVersion != null) 'server_version': serverVersion,
            if (serverPlatformContext != null)
              'server_platform_context': serverPlatformContext,
          })
          .select()
          .single();

      submission =
          DebugBundleSubmission.fromRow(Map<String, dynamic>.from(row));
    } catch (error) {
      throw DebugBundleSubmissionException(
        _formatInsertError(error),
        cause: error,
      );
    }

    final issueReport = await _createGitHubIssue(client, submission.id);
    return submission.copyWith(
      status: issueReport.status,
      githubIssueUrl: issueReport.url,
      githubIssueNumber: issueReport.number,
      githubIssueError: issueReport.error,
    );
  }

  Future<_GitHubIssueReport> _createGitHubIssue(
    SupabaseClient client,
    String submissionId,
  ) async {
    try {
      final response = await client.functions.invoke(
        reportBugFunctionName,
        body: {'submission_id': submissionId},
      );

      final data = response.data;
      if (response.status < 200 || response.status >= 300) {
        throw DebugBundleSubmissionException(
          _formatGitHubIssueError(
            _extractResponseMessage(data) ?? 'HTTP ${response.status}',
          ),
        );
      }

      if (data is! Map) {
        throw const DebugBundleSubmissionException(
          'Debug bundle uploaded, but GitHub did not return issue details.',
        );
      }
      final issueCreated = data['issue_created'] == true;
      final alreadyExists = data['already_exists'] == true;
      final issueReport = _GitHubIssueReport.fromMap(data);
      if (!issueCreated && !alreadyExists) {
        throw DebugBundleSubmissionException(
          _formatGitHubIssueError(
            issueReport.error ?? 'No details returned.',
          ),
        );
      }
      return issueReport;
    } catch (error) {
      if (error is DebugBundleSubmissionException) rethrow;
      throw DebugBundleSubmissionException(
        _formatGitHubIssueError(error.toString()),
        cause: error,
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

  String _buildStoragePath({
    required String userId,
    required String submissionId,
    required String fileName,
  }) {
    final now = DateTime.now().toUtc();
    final month = now.month.toString().padLeft(2, '0');
    final day = now.day.toString().padLeft(2, '0');
    return '$userId/${now.year}/$month/$day/$submissionId-$fileName';
  }

  String _sanitizeFileName(String fileName) {
    final trimmed = fileName.trim();
    final sanitized = trimmed.replaceAll(RegExp(r'[^A-Za-z0-9._-]'), '_');
    if (sanitized.isEmpty) {
      return 'rhythm-debug-bundle.tar.gz';
    }
    if (sanitized.endsWith('.tar.gz') || sanitized.endsWith('.gz')) {
      return sanitized;
    }
    return '$sanitized.tar.gz';
  }

  String? _normalizeSummary(String? summary) {
    final trimmed = summary?.trim();
    if (trimmed == null || trimmed.isEmpty) return null;
    return trimmed;
  }

  String _platformLabel() {
    if (kIsWeb) return 'web';
    return switch (defaultTargetPlatform) {
      TargetPlatform.android => 'android',
      TargetPlatform.iOS => 'ios',
      TargetPlatform.macOS => 'macos',
      TargetPlatform.windows => 'windows',
      TargetPlatform.linux => 'linux',
      TargetPlatform.fuchsia => 'fuchsia',
    };
  }

  Future<void> _deleteUploadedBundle(
    SupabaseClient client,
    String storagePath,
  ) async {
    try {
      await client.storage.from(bucketName).remove([storagePath]);
    } catch (_) {
      // Ignore cleanup failures. The DB write error is the actionable one.
    }
  }

  String _formatUploadError(Object error) {
    if (error is StorageException) {
      final detail = error.message.trim();
      if (detail.isNotEmpty) {
        return 'Failed to upload the debug bundle. $detail';
      }
    }
    return 'Failed to upload the debug bundle.';
  }

  String _formatInsertError(Object error) {
    if (error is PostgrestException) {
      final detail = error.message.trim();
      if (detail.isNotEmpty) {
        return 'Failed to submit the bug report. $detail';
      }
    }
    return 'Failed to submit the bug report.';
  }

  String _formatGitHubIssueError(String detail) {
    final trimmed = detail.trim();
    if (trimmed.isEmpty) {
      return 'Debug bundle uploaded, but failed to create the GitHub issue.';
    }
    return 'Debug bundle uploaded, but failed to create the GitHub issue. $trimmed';
  }

  Future<PackageInfo> _loadPackageInfo() async {
    try {
      return await PackageInfo.fromPlatform();
    } catch (_) {
      return PackageInfo(
        appName: 'Rhythm',
        packageName: 'rhythm_app',
        version: 'unknown',
        buildNumber: 'unknown',
      );
    }
  }
}

class _GitHubIssueReport {
  const _GitHubIssueReport({
    required this.status,
    this.url,
    this.number,
    this.error,
  });

  final String status;
  final String? url;
  final int? number;
  final String? error;

  factory _GitHubIssueReport.fromMap(Map<dynamic, dynamic> data) {
    return _GitHubIssueReport(
      status: data['status'] as String? ?? 'received',
      url: data['issue_url'] as String?,
      number: _tryParseInt(data['issue_number']),
      error: _extractResponseMessage(data),
    );
  }
}

DateTime? _tryParseDateTime(dynamic value) {
  if (value is String && value.isNotEmpty) {
    return DateTime.tryParse(value);
  }
  return null;
}

int? _tryParseInt(dynamic value) {
  if (value is int) return value;
  if (value is num) return value.toInt();
  if (value is String && value.isNotEmpty) {
    return int.tryParse(value);
  }
  return null;
}

String? _extractResponseMessage(dynamic data) {
  if (data is String && data.trim().isNotEmpty) {
    return data.trim();
  }
  if (data is Map) {
    final value = data['error'] ?? data['message'] ?? data['reason'];
    if (value is String && value.trim().isNotEmpty) {
      return value.trim();
    }
  }
  return null;
}
