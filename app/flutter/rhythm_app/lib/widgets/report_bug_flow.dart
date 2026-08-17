import 'dart:async';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show
        RhythmApiException,
        RhythmCapabilities,
        RhythmDebugBundle,
        RhythmFeature;
import 'package:uuid/uuid.dart';

import '../providers/server_sync_provider.dart';
import '../services/analytics_service.dart';
import '../services/debug_bundle_submission_service.dart';
import '../services/server_endpoint_resolver.dart';
import 'solar_orbit.dart';

const Color _teal = Color(0xFF26C6DA);
const Duration _serverDebugBundleReceiveTimeout = Duration(minutes: 5);
const Uuid _supportReportUuid = Uuid();

class SupportReportRequest {
  const SupportReportRequest({required this.kind, required this.summary});

  final SupportReportKind kind;
  final String summary;
}

/// Public dialog that prompts the user for a classified support report.
///
/// Returns the kind and entered text via [Navigator.pop], or null if cancelled.
class ReportPromptDialog extends StatefulWidget {
  const ReportPromptDialog({super.key});

  @override
  State<ReportPromptDialog> createState() => _ReportPromptDialogState();
}

class _ReportPromptDialogState extends State<ReportPromptDialog> {
  final TextEditingController _controller = TextEditingController();
  SupportReportKind _kind = SupportReportKind.bug;

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      backgroundColor: CelestialColors.backgroundCard,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(20),
      ),
      title: const Text(
        'Report an issue or idea',
        style: TextStyle(
          color: CelestialColors.textPrimary,
          fontSize: 17,
          fontWeight: FontWeight.w600,
        ),
      ),
      content: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text(
              'Choose what you want to share. A private debug bundle is included either way, with recent app logs and redacted server state when available.',
              style: TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 14,
              ),
            ),
            const SizedBox(height: 14),
            SizedBox(
              width: double.infinity,
              child: SegmentedButton<SupportReportKind>(
                key: const Key('support-report-kind-selector'),
                showSelectedIcon: false,
                segments: const [
                  ButtonSegment(
                    value: SupportReportKind.bug,
                    icon: Icon(Icons.bug_report_rounded),
                    label: Text('Bug'),
                  ),
                  ButtonSegment(
                    value: SupportReportKind.feature,
                    icon: Icon(Icons.lightbulb_rounded),
                    label: Text('Feature'),
                  ),
                ],
                selected: {_kind},
                onSelectionChanged: (selection) {
                  setState(() => _kind = selection.single);
                },
              ),
            ),
            const SizedBox(height: 14),
            TextField(
              key: const Key('support-report-summary'),
              controller: _controller,
              autofocus: true,
              maxLines: 4,
              minLines: 3,
              maxLength: 500,
              style: const TextStyle(
                color: CelestialColors.textPrimary,
              ),
              decoration: InputDecoration(
                hintText: _kind == SupportReportKind.bug
                    ? 'What went wrong? (optional)'
                    : 'What would you like Rhythm to do? (optional)',
                hintStyle: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.45),
                ),
                filled: true,
                fillColor: CelestialColors.backgroundDark,
                enabledBorder: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(12),
                  borderSide: BorderSide(
                    color: CelestialColors.orbitRing.withValues(alpha: 0.35),
                  ),
                ),
                focusedBorder: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(12),
                  borderSide: BorderSide(
                    color: _teal.withValues(alpha: 0.7),
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: Text(
            'Cancel',
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.8),
            ),
          ),
        ),
        ElevatedButton(
          onPressed: () => Navigator.of(context).pop(
            SupportReportRequest(kind: _kind, summary: _controller.text),
          ),
          style: ElevatedButton.styleFrom(
            backgroundColor: _teal,
            foregroundColor: const Color(0xFF0A0F14),
          ),
          child: Text(
            _kind == SupportReportKind.bug ? 'Report bug' : 'Request feature',
          ),
        ),
      ],
    );
  }
}

/// Runs the full support-report flow: prompt → bundle → issue → result.
///
/// When [serverHub] is provided, downloads a server debug bundle and submits
/// the full payload. When null, submits the app-only debug bundle.
Future<void> showSupportReportFlow(
  BuildContext context, {
  Hub? serverHub,
  bool localServerOnly = false,
  String? serverVersionOverride,
  String? serverPlatformContextOverride,
}) async {
  final request = await showDialog<SupportReportRequest>(
    context: context,
    builder: (ctx) => const ReportPromptDialog(),
  );
  if (request == null || !context.mounted) return;

  final submissionId = _supportReportUuid.v4();
  final journeyId = 'support-report-$submissionId';
  var bundleScope = serverHub == null ? 'app_only' : 'server_and_app';
  var collectingInBackground = false;
  unawaited(
    AnalyticsService().logSupportReportAttempted(
      journeyId: journeyId,
      reportKind: request.kind.name,
      bundleScope: bundleScope,
    ),
  );
  _showSupportReportProgress(context, request.kind);

  try {
    final DebugBundleSubmission submission;
    if (serverHub != null) {
      final syncProvider = context.read<ServerSyncProvider>();
      final rawServerVersion =
          serverVersionOverride ?? syncProvider.firmwareVersion;
      final serverVersion = rawServerVersion == '0.0.0'
          ? 'Unknown'
          : _formatVersion(rawServerVersion);
      final serverPlatformContext =
          serverPlatformContextOverride ?? syncProvider.serverPlatformContext;
      final resolved = localServerOnly
          ? ServerEndpointResolver.local(serverHub)
          : await ServerEndpointResolver.resolve(
              serverHub,
              syncProvider: syncProvider,
            );
      final client = resolved.diagnosticsApi(
        debugBundleReceiveTimeout: _serverDebugBundleReceiveTimeout,
      );

      final supportsServerQueue =
          supportsServerManagedSupportReport(syncProvider.serverCapabilities);
      if (supportsServerQueue) {
        submission =
            await DebugBundleSubmissionService.instance.submitViaServerQueue(
          submissionId: submissionId,
          reportKind: request.kind,
          serverHub: serverHub,
          deviceClient: client,
          serverVersion: serverVersion,
          serverPlatformContext: serverPlatformContext,
          summary: request.summary,
        );
        bundleScope = 'server_async';
        collectingInBackground = true;
      } else {
        // Previous-floor servers keep the synchronous direct upload/download
        // contract. Only an explicitly advertised capability enters the path
        // that tells the user it is safe to close the app.
        DebugBundleSubmission? directSubmission;
        RhythmDebugBundle? bundle;
        String? bundleFailure;
        try {
          final direct =
              await DebugBundleSubmissionService.instance.submitViaDeviceUpload(
            submissionId: submissionId,
            reportKind: request.kind,
            serverHub: serverHub,
            deviceClient: client,
            serverVersion: serverVersion,
            serverPlatformContext: serverPlatformContext,
            summary: request.summary,
          );
          directSubmission = direct.submission;
          bundle = direct.legacyBundle;
        } catch (error) {
          debugPrint(
            'SupportReportFlow: device-direct upload unavailable from '
            '${resolved.baseUrl}, falling back to download: $error',
          );
        }

        if (directSubmission == null && bundle == null) {
          try {
            bundle = await client.downloadDebugBundle();
            bundleFailure = null;
          } catch (error) {
            bundleFailure = _formatDebugBundleFailure(error);
            debugPrint(
              'SupportReportFlow: server debug bundle download failed from '
              '${resolved.baseUrl}: $bundleFailure',
            );
          }
        }

        if (directSubmission != null) {
          submission = directSubmission;
        } else if (bundle != null) {
          submission = await DebugBundleSubmissionService.instance.submit(
            submissionId: submissionId,
            reportKind: request.kind,
            serverHub: serverHub,
            bundle: bundle,
            serverVersion: serverVersion,
            serverPlatformContext: serverPlatformContext,
            summary: request.summary,
          );
        } else {
          bundleScope = 'app_only_after_server_failure';
          submission =
              await DebugBundleSubmissionService.instance.submitTextOnly(
            submissionId: submissionId,
            reportKind: request.kind,
            summary: _summaryWithDebugBundleFailure(
              summary: request.summary,
              endpoint: resolved.baseUrl,
              detail: bundleFailure,
            ),
            serverHub: serverHub,
            serverVersion: serverVersion,
            serverPlatformContext: serverPlatformContext,
          );
        }
      }
    } else {
      submission = await DebugBundleSubmissionService.instance.submitTextOnly(
        submissionId: submissionId,
        reportKind: request.kind,
        summary: request.summary,
      );
    }

    unawaited(
      AnalyticsService().logSupportReportCompleted(
        journeyId: journeyId,
        reportKind: request.kind.name,
        bundleScope: bundleScope,
        outcome: collectingInBackground ? 'accepted' : 'succeeded',
      ),
    );
    if (!context.mounted) return;
    Navigator.of(context, rootNavigator: true).pop();
    await _showSupportReportSubmitted(
      context,
      submission.referenceCode,
      request.kind,
      collectingInBackground: collectingInBackground,
    );
  } catch (error) {
    unawaited(
      AnalyticsService().logSupportReportCompleted(
        journeyId: journeyId,
        reportKind: request.kind.name,
        bundleScope: bundleScope,
        outcome: 'failed',
        failureStage: error is RhythmApiException
            ? 'server_bundle'
            : error is DebugBundleSubmissionException
                ? 'submission'
                : 'unknown',
      ),
    );
    if (!context.mounted) return;
    Navigator.of(context, rootNavigator: true).pop();
    final message = switch (error) {
      RhythmApiException apiError => apiError.serverMessage ?? apiError.message,
      DebugBundleSubmissionException submitError => submitError.message,
      _ => 'Failed to submit the report.',
    };
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(message),
        behavior: SnackBarBehavior.floating,
        backgroundColor: Colors.red.shade400,
      ),
    );
  }
}

void _showSupportReportProgress(
  BuildContext context,
  SupportReportKind kind,
) {
  showDialog<void>(
    context: context,
    barrierDismissible: false,
    builder: (ctx) => AlertDialog(
      backgroundColor: CelestialColors.backgroundCard,
      content: Row(
        children: [
          const SizedBox(
            width: 20,
            height: 20,
            child: CircularProgressIndicator(strokeWidth: 2),
          ),
          const SizedBox(width: 16),
          Expanded(
            child: Text(
              kind == SupportReportKind.bug
                  ? 'Creating bug report...'
                  : 'Creating feature request...',
              style: const TextStyle(color: CelestialColors.textPrimary),
            ),
          ),
        ],
      ),
    ),
  );
}

Future<void> _showSupportReportSubmitted(
  BuildContext context,
  String referenceCode,
  SupportReportKind kind, {
  required bool collectingInBackground,
}) {
  return showDialog<void>(
    context: context,
    builder: (ctx) => AlertDialog(
      backgroundColor: CelestialColors.backgroundCard,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(20),
      ),
      title: Text(
        kind == SupportReportKind.bug
            ? 'Bug report submitted'
            : 'Feature request submitted',
        style: const TextStyle(
          color: CelestialColors.textPrimary,
          fontSize: 17,
          fontWeight: FontWeight.w600,
        ),
      ),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            _submittedMessage(kind, collectingInBackground),
            style: const TextStyle(
              color: CelestialColors.textSecondary,
              fontSize: 14,
            ),
          ),
          const SizedBox(height: 12),
          Text(
            'Reference: $referenceCode',
            style: const TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 15,
              fontWeight: FontWeight.w600,
              fontFamily: 'monospace',
            ),
          ),
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(ctx).pop(),
          child: const Text(
            'Done',
            style: TextStyle(color: _teal),
          ),
        ),
      ],
    ),
  );
}

@visibleForTesting
String supportReportSubmittedMessageForTesting({
  required SupportReportKind kind,
  required bool collectingInBackground,
}) {
  return _submittedMessage(kind, collectingInBackground);
}

String _submittedMessage(
  SupportReportKind kind,
  bool collectingInBackground,
) {
  if (collectingInBackground) {
    return 'RhythmServer is collecting the private debug bundle in the '
        'background. You can close the app.';
  }
  return kind == SupportReportKind.bug
      ? 'Support can now review this report.'
      : 'The product team can now review this request.';
}

@visibleForTesting
bool supportsServerManagedSupportReport(RhythmCapabilities? capabilities) {
  return capabilities?.supportsFeature(RhythmFeature.asyncDebugBundleUpload) ??
      false;
}

String _formatVersion(String version) {
  return version.startsWith('v') || version.startsWith('V')
      ? version
      : 'v$version';
}

String _formatDebugBundleFailure(Object error) {
  if (error is RhythmApiException) {
    final detail = error.serverMessage?.trim();
    final status = error.statusCode;
    if (detail != null && detail.isNotEmpty) {
      return status == null
          ? '${error.message}: $detail'
          : '${error.message} (HTTP $status): $detail';
    }
    return status == null ? error.message : '${error.message} (HTTP $status)';
  }
  return error.toString();
}

@visibleForTesting
String summaryWithDebugBundleFailureForTesting({
  required String summary,
  required String endpoint,
  required String? detail,
}) {
  return _summaryWithDebugBundleFailure(
    summary: summary,
    endpoint: endpoint,
    detail: detail,
  );
}

String _summaryWithDebugBundleFailure({
  required String summary,
  required String endpoint,
  required String? detail,
}) {
  final trimmedSummary = summary.trim();
  final lines = <String>[
    if (trimmedSummary.isNotEmpty) trimmedSummary,
    if (trimmedSummary.isNotEmpty) '',
    'Server debug bundle download failed before upload.',
    'Endpoint: $endpoint',
    if (detail != null && detail.trim().isNotEmpty) 'Error: ${detail.trim()}',
  ];
  return lines.join('\n');
}
