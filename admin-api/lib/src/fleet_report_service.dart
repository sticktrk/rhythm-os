import 'dart:math';

import 'device_probe_service.dart';
import 'models.dart';
import 'supabase_rest_client.dart';
import 'support_service.dart';

class FleetReportService {
  const FleetReportService({
    required SupabaseRestClient supabase,
    required SupportService support,
    required DeviceProbeService probes,
  })  : _supabase = supabase,
        _support = support,
        _probes = probes;

  static const _bucket = 'support-debug-bundles';
  static const _table = 'support_debug_bundle_submissions';

  final SupabaseRestClient _supabase;
  final SupportService _support;
  final DeviceProbeService _probes;

  Future<FleetFindingReportResultDto> report({
    required AdminSession session,
    required String hubId,
    required FleetFindingReportRequestDto finding,
  }) async {
    if (!_supabase.canUseServiceRole) {
      throw const AdminApiException(
        503,
        'Fleet reporting requires SUPABASE_SERVICE_ROLE_KEY.',
      );
    }
    final hubs = await _support.loadFleetHubs(session);
    final hub = hubs.where((candidate) => candidate.id == hubId).firstOrNull;
    if (hub == null) {
      throw const AdminApiException(
        404,
        'Enabled Rhythm server hub was not found.',
      );
    }

    final bundle = await _probes.downloadDebugBundle(
      session: session,
      hubId: hubId,
    );
    final submissionId = _uuidV4();
    final fileName = _safeFileName(bundle.fileName, submissionId);
    final storagePath = '${session.user.id}/fleet/$submissionId/$fileName';

    await _supabase.uploadStorageObject(
      bucket: _bucket,
      path: storagePath,
      bytes: bundle.bytes,
      contentType: 'application/gzip',
    );

    late final Map<String, dynamic> submission;
    try {
      submission = await _supabase.insert(
        table: _table,
        serviceRole: true,
        values: {
          'id': submissionId,
          'user_id': session.user.id,
          'user_email': session.user.email,
          'is_anonymous': false,
          'summary': _summary(finding),
          'app_version': 'admin-api',
          'app_build': finding.scanRunId,
          'app_platform': 'admin-fleet',
          'server_hub_id': hub.id,
          'server_name': hub.name,
          'server_host': hub.remoteEndpoint?.host ?? hub.endpoint.host,
          'server_port': hub.remoteEndpoint?.port ?? hub.endpoint.port,
          'server_version': finding.serverVersion,
          'server_platform_context': finding.platformContext,
          'bundle_storage_path': storagePath,
          'bundle_file_name': fileName,
          'bundle_content_type': 'application/gzip',
          'bundle_size_bytes': bundle.bytes.length,
        },
      );
    } catch (_) {
      try {
        await _supabase.deleteStorageObject(
          bucket: _bucket,
          path: storagePath,
        );
      } catch (_) {
        // Preserve the insert error; orphan cleanup is best effort.
      }
      rethrow;
    }

    final reportResult = await _supabase.invokeFunction(
      name: 'report-bug',
      accessToken: session.accessToken,
      body: {'submission_id': submissionId},
    );
    return FleetFindingReportResultDto(
      submissionId: submissionId,
      referenceCode: submission['reference_code'] as String? ?? '',
      fingerprint: finding.fingerprint,
      reportResult: reportResult,
    );
  }

  String _summary(FleetFindingReportRequestDto finding) {
    return [
      finding.title,
      '',
      'Fleet fingerprint: ${finding.fingerprint}',
      'Fleet scan run: ${finding.scanRunId}',
      'Detected at: ${finding.detectedAt.toUtc().toIso8601String()}',
      'Severity: ${finding.severity}',
      'Occurrences: ${finding.occurrences}',
      'Affected hubs: ${finding.affectedHubCount}',
      if (finding.journeyStage != null)
        'Journey stage: ${finding.journeyStage}',
      if (finding.samples.isNotEmpty) '',
      if (finding.samples.isNotEmpty) 'Sanitized samples:',
      for (final sample in finding.samples) '- $sample',
    ].join('\n');
  }
}

extension<T> on Iterable<T> {
  T? get firstOrNull {
    final iterator = this.iterator;
    return iterator.moveNext() ? iterator.current : null;
  }
}

String _uuidV4() {
  final random = Random.secure();
  final bytes = List<int>.generate(16, (_) => random.nextInt(256));
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  final hex =
      bytes.map((byte) => byte.toRadixString(16).padLeft(2, '0')).join();
  return '${hex.substring(0, 8)}-${hex.substring(8, 12)}-'
      '${hex.substring(12, 16)}-${hex.substring(16, 20)}-'
      '${hex.substring(20)}';
}

String _safeFileName(String value, String fallback) {
  final sanitized = value
      .split(RegExp(r'''[/\\]'''))
      .last
      .replaceAll(RegExp(r'[^A-Za-z0-9_.-]+'), '-')
      .replaceAll(RegExp(r'-+'), '-')
      .trim();
  return sanitized.isEmpty ? 'rhythm-debug-bundle-$fallback.tar.gz' : sanitized;
}
