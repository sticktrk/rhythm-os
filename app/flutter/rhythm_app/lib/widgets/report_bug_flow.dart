import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show RhythmApiException, RhythmDebugBundle;

import '../providers/server_sync_provider.dart';
import '../services/debug_bundle_submission_service.dart';
import '../services/server_endpoint_resolver.dart';
import 'solar_orbit.dart';

const Color _teal = Color(0xFF26C6DA);
const Duration _serverDebugBundleReceiveTimeout = Duration(minutes: 5);

/// Public dialog that prompts the user for a bug summary.
///
/// Returns the entered text via [Navigator.pop], or `null` if cancelled.
class ReportBugPromptDialog extends StatefulWidget {
  const ReportBugPromptDialog({super.key});

  @override
  State<ReportBugPromptDialog> createState() => _ReportBugPromptDialogState();
}

class _ReportBugPromptDialogState extends State<ReportBugPromptDialog> {
  final TextEditingController _controller = TextEditingController();

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
        'Report Bug',
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
              'This sends a snapshot of recent logs and redacted state to Rhythm support.',
              style: TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 14,
              ),
            ),
            const SizedBox(height: 14),
            TextField(
              controller: _controller,
              autofocus: true,
              maxLines: 4,
              minLines: 3,
              maxLength: 500,
              style: const TextStyle(
                color: CelestialColors.textPrimary,
              ),
              decoration: InputDecoration(
                hintText: 'What went wrong? (optional)',
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
          onPressed: () => Navigator.of(context).pop(_controller.text),
          style: ElevatedButton.styleFrom(
            backgroundColor: _teal,
            foregroundColor: const Color(0xFF0A0F14),
          ),
          child: const Text('Report'),
        ),
      ],
    );
  }
}

/// Runs the full bug-report flow: prompt → progress → submit → success/error.
///
/// When [serverHub] is provided, downloads a server debug bundle and submits
/// the full payload. When null, submits a text-only report.
Future<void> showReportBugFlow(
  BuildContext context, {
  Hub? serverHub,
  bool localServerOnly = false,
  String? serverVersionOverride,
  String? serverPlatformContextOverride,
}) async {
  final summary = await showDialog<String>(
    context: context,
    builder: (ctx) => const ReportBugPromptDialog(),
  );
  if (summary == null || !context.mounted) return;

  _showReportBugProgress(context);

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

      // Preferred: the device uploads the bundle straight to storage, so a
      // large bundle never has to fit through the app's download window.
      DebugBundleSubmission? directSubmission;
      RhythmDebugBundle? bundle;
      String? bundleFailure;
      try {
        final direct =
            await DebugBundleSubmissionService.instance.submitViaDeviceUpload(
          serverHub: serverHub,
          deviceClient: client,
          serverVersion: serverVersion,
          serverPlatformContext: serverPlatformContext,
          summary: summary,
        );
        directSubmission = direct.submission;
        // Older firmware ignores the upload request and returns the bundle
        // bytes — reuse them below instead of downloading twice.
        bundle = direct.legacyBundle;
      } catch (error) {
        debugPrint(
          'ReportBugFlow: device-direct upload unavailable from '
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
            'ReportBugFlow: server debug bundle download failed from '
            '${resolved.baseUrl}: $bundleFailure',
          );
        }
      }

      if (directSubmission != null) {
        submission = directSubmission;
      } else if (bundle != null) {
        submission = await DebugBundleSubmissionService.instance.submit(
          serverHub: serverHub,
          bundle: bundle,
          serverVersion: serverVersion,
          serverPlatformContext: serverPlatformContext,
          summary: summary,
        );
      } else {
        submission = await DebugBundleSubmissionService.instance.submitTextOnly(
          summary: _summaryWithDebugBundleFailure(
            summary: summary,
            endpoint: resolved.baseUrl,
            detail: bundleFailure,
          ),
          serverHub: serverHub,
          serverVersion: serverVersion,
          serverPlatformContext: serverPlatformContext,
        );
      }
    } else {
      submission = await DebugBundleSubmissionService.instance.submitTextOnly(
        summary: summary,
      );
    }

    if (!context.mounted) return;
    Navigator.of(context, rootNavigator: true).pop();
    await _showReportBugSubmitted(context, submission.referenceCode);
  } catch (error) {
    if (!context.mounted) return;
    Navigator.of(context, rootNavigator: true).pop();
    final message = switch (error) {
      RhythmApiException apiError => apiError.serverMessage ?? apiError.message,
      DebugBundleSubmissionException submitError => submitError.message,
      _ => 'Failed to report the bug.',
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

void _showReportBugProgress(BuildContext context) {
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
            child: const Text(
              'Creating bug report...',
              style: TextStyle(color: CelestialColors.textPrimary),
            ),
          ),
        ],
      ),
    ),
  );
}

Future<void> _showReportBugSubmitted(
  BuildContext context,
  String referenceCode,
) {
  return showDialog<void>(
    context: context,
    builder: (ctx) => AlertDialog(
      backgroundColor: CelestialColors.backgroundCard,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(20),
      ),
      title: const Text(
        'Bug report submitted',
        style: TextStyle(
          color: CelestialColors.textPrimary,
          fontSize: 17,
          fontWeight: FontWeight.w600,
        ),
      ),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text(
            'Support can now review this report.',
            style: TextStyle(
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
