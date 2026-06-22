import 'dart:convert';

import 'package:archive/archive.dart';
import 'package:flutter/foundation.dart';
import 'package:package_info_plus/package_info_plus.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:supabase_flutter/supabase_flutter.dart';
import 'package:uuid/uuid.dart';

import '../backend/backend.dart';
import 'app_log_service.dart';
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
    final uploadBundle = await _bundleWithAppLog(bundle);
    late final _UploadedDebugBundle uploadedBundle;

    try {
      uploadedBundle = await _uploadBundle(
        client: client,
        userId: userId,
        submissionId: submissionId,
        bundle: uploadBundle,
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
            'bundle_storage_path': uploadedBundle.storagePath,
            'bundle_file_name': uploadedBundle.fileName,
            'bundle_content_type': uploadedBundle.contentType,
            'bundle_size_bytes': uploadedBundle.sizeBytes,
          })
          .select()
          .single();

      submission =
          DebugBundleSubmission.fromRow(Map<String, dynamic>.from(row));
    } catch (error) {
      await _deleteUploadedBundle(client, uploadedBundle.storagePath);
      throw DebugBundleSubmissionException(
        _formatInsertError(error),
        cause: error,
      );
    }

    final issueReport = await _createGitHubIssue(client, submission.id);
    return _applyGitHubIssueReport(submission, issueReport);
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
    final submissionId = _uuid.v4();
    _UploadedDebugBundle? uploadedBundle;
    try {
      uploadedBundle = await _uploadBundle(
        client: client,
        userId: userId,
        submissionId: submissionId,
        bundle: await _appOnlyBundle(),
      );
    } catch (error) {
      debugPrint(
          'DebugBundleSubmissionService: app log upload skipped: $error');
    }

    late final DebugBundleSubmission submission;
    try {
      final row = await client
          .from(tableName)
          .insert({
            'id': submissionId,
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
            if (uploadedBundle != null)
              'bundle_storage_path': uploadedBundle.storagePath,
            if (uploadedBundle != null)
              'bundle_file_name': uploadedBundle.fileName,
            if (uploadedBundle != null)
              'bundle_content_type': uploadedBundle.contentType,
            if (uploadedBundle != null)
              'bundle_size_bytes': uploadedBundle.sizeBytes,
          })
          .select()
          .single();

      submission =
          DebugBundleSubmission.fromRow(Map<String, dynamic>.from(row));
    } catch (error) {
      if (uploadedBundle != null) {
        await _deleteUploadedBundle(client, uploadedBundle.storagePath);
      }
      throw DebugBundleSubmissionException(
        _formatInsertError(error),
        cause: error,
      );
    }

    final issueReport = await _createGitHubIssue(client, submission.id);
    return _applyGitHubIssueReport(submission, issueReport);
  }

  Future<_UploadedDebugBundle> _uploadBundle({
    required SupabaseClient client,
    required String userId,
    required String submissionId,
    required RhythmDebugBundle bundle,
  }) async {
    final sanitizedFileName = _sanitizeFileName(bundle.fileName);
    final storagePath = _buildStoragePath(
      userId: userId,
      submissionId: submissionId,
      fileName: sanitizedFileName,
    );

    await client.storage.from(bucketName).uploadBinary(
          storagePath,
          bundle.bytes,
          fileOptions: FileOptions(
            contentType: bundle.contentType,
            upsert: false,
          ),
        );

    return _UploadedDebugBundle(
      storagePath: storagePath,
      fileName: sanitizedFileName,
      contentType: bundle.contentType,
      sizeBytes: bundle.bytes.length,
    );
  }

  Future<RhythmDebugBundle> _bundleWithAppLog(RhythmDebugBundle bundle) async {
    return _appendAppLogToBundle(
      bundle: bundle,
      appLogText: await AppLogService.instance.snapshotText(),
    );
  }

  Future<RhythmDebugBundle> _appOnlyBundle() async {
    final archive = Archive();
    _addAppLogFiles(
      archive,
      await AppLogService.instance.snapshotText(),
    );
    return _encodeArchive(
      archive: archive,
      fileName: _appOnlyBundleFileName(),
    );
  }

  @visibleForTesting
  static RhythmDebugBundle appendAppLogToBundleForTesting({
    required RhythmDebugBundle bundle,
    required String appLogText,
  }) {
    return _appendAppLogToBundle(bundle: bundle, appLogText: appLogText);
  }

  @visibleForTesting
  static DebugBundleSubmission applyGitHubIssueResponseForTesting(
    DebugBundleSubmission submission, {
    required int status,
    required Object? data,
  }) {
    return _applyGitHubIssueReport(
      submission,
      _parseGitHubIssueFunctionResponse(status: status, data: data),
    );
  }

  static RhythmDebugBundle _appendAppLogToBundle({
    required RhythmDebugBundle bundle,
    required String appLogText,
  }) {
    final archive = _decodeTarGz(bundle.bytes) ??
        (Archive()
          ..addFile(ArchiveFile.bytes(
            'server/${_safeArchiveFileName(bundle.fileName)}',
            bundle.bytes,
          ))
          ..addFile(ArchiveFile.string(
            'server/README.txt',
            'The original server bundle could not be decoded by the app. '
                'It is preserved as server/${_safeArchiveFileName(bundle.fileName)}.\n',
          )));
    _addAppLogFiles(archive, appLogText);
    return _encodeArchive(
      archive: archive,
      fileName: _ensureTarGzFileName(bundle.fileName),
    );
  }

  static Archive? _decodeTarGz(Uint8List bytes) {
    try {
      final tarBytes = GZipDecoder().decodeBytes(bytes);
      final decoded = TarDecoder().decodeBytes(tarBytes, storeData: true);
      return _cloneArchive(decoded);
    } catch (_) {
      return null;
    }
  }

  static Archive _cloneArchive(Archive source) {
    final archive = Archive()..comment = source.comment;
    for (final file in source) {
      final copy = file.isDirectory
          ? ArchiveFile.directory(file.name)
          : file.isSymbolicLink
              ? ArchiveFile.symlink(file.name, file.symbolicLink!)
              : ArchiveFile.bytes(file.name, file.readBytes() ?? Uint8List(0));
      copy
        ..mode = file.mode
        ..ownerId = file.ownerId
        ..groupId = file.groupId
        ..creationTime = file.creationTime
        ..lastModTime = file.lastModTime
        ..crc32 = file.crc32
        ..comment = file.comment
        ..compression = file.compression
        ..compressionLevel = file.compressionLevel;
      archive.addFile(copy);
    }
    return archive;
  }

  static void _addAppLogFiles(Archive archive, String appLogText) {
    archive.addFile(ArchiveFile.string('app/app.log', appLogText));
    archive.addFile(ArchiveFile.string(
      'app/metadata.json',
      jsonEncode({
        'kind': 'rhythm_app_log',
        'generated_at': DateTime.now().toUtc().toIso8601String(),
        'path': 'app/app.log',
      }),
    ));
  }

  static RhythmDebugBundle _encodeArchive({
    required Archive archive,
    required String fileName,
  }) {
    final tarBytes = TarEncoder().encodeBytes(archive);
    final gzipBytes = GZipEncoder().encodeBytes(tarBytes);
    return RhythmDebugBundle(
      fileName: fileName,
      bytes: Uint8List.fromList(gzipBytes),
      contentType: 'application/gzip',
    );
  }

  static String _appOnlyBundleFileName() {
    final timestamp = DateTime.now()
        .toUtc()
        .toIso8601String()
        .replaceAll(RegExp(r'[^0-9A-Za-z]'), '');
    return 'rhythm-app-debug-bundle-$timestamp.tar.gz';
  }

  static String _ensureTarGzFileName(String fileName) {
    final safeName = _safeArchiveFileName(fileName);
    if (safeName.endsWith('.tar.gz')) return safeName;
    if (safeName.endsWith('.gz')) return safeName;
    return '$safeName.tar.gz';
  }

  static String _safeArchiveFileName(String fileName) {
    final leaf = fileName.split('/').last.split('\\').last.trim();
    final sanitized = leaf.replaceAll(RegExp(r'[^A-Za-z0-9._-]'), '_');
    return sanitized.isEmpty ? 'rhythm-debug-bundle.tar.gz' : sanitized;
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

      return _parseGitHubIssueFunctionResponse(
        status: response.status,
        data: response.data,
      );
    } catch (error) {
      return _GitHubIssueReport(
        status: 'received',
        error: _formatGitHubIssueError(error.toString()),
      );
    }
  }

  static DebugBundleSubmission _applyGitHubIssueReport(
    DebugBundleSubmission submission,
    _GitHubIssueReport issueReport,
  ) {
    return submission.copyWith(
      status: issueReport.status,
      githubIssueUrl: issueReport.url,
      githubIssueNumber: issueReport.number,
      githubIssueError: issueReport.error,
    );
  }

  static _GitHubIssueReport _parseGitHubIssueFunctionResponse({
    required int status,
    required Object? data,
  }) {
    if (status < 200 || status >= 300) {
      return _GitHubIssueReport(
        status: 'received',
        error: _formatGitHubIssueError(
          _extractResponseMessage(data) ?? 'HTTP $status',
        ),
      );
    }

    if (data is! Map) {
      return const _GitHubIssueReport(
        status: 'received',
        error:
            'Debug bundle uploaded, but GitHub did not return issue details.',
      );
    }

    final issueCreated = data['issue_created'] == true;
    final alreadyExists = data['already_exists'] == true;
    final issueReport = _GitHubIssueReport.fromMap(data);
    if (!issueCreated && !alreadyExists) {
      return _GitHubIssueReport(
        status: issueReport.status,
        url: issueReport.url,
        number: issueReport.number,
        error: _formatGitHubIssueError(
          issueReport.error ?? 'No details returned.',
        ),
      );
    }

    return issueReport;
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

  static String _formatGitHubIssueError(String detail) {
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

class _UploadedDebugBundle {
  const _UploadedDebugBundle({
    required this.storagePath,
    required this.fileName,
    required this.contentType,
    required this.sizeBytes,
  });

  final String storagePath;
  final String fileName;
  final String contentType;
  final int sizeBytes;
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
