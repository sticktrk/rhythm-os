/// Sequencing for removing a Matter device from a Rhythm server.
///
/// Removal must be graceful-first: a forced removal only cleans up local
/// state, leaving the bulb convinced it is still commissioned — a stale
/// fabric NOC that later breaks re-pairing (see issue #120). Force is
/// therefore an explicit, user-confirmed fallback for unreachable devices,
/// never the first attempt.
library;

enum MatterRemovalOutcome { removed, cancelled }

/// Error text used when the server returns no result or no error message
/// (e.g. the request timed out while the server was still trying to reach
/// the device).
const String matterRemovalNoResponseError =
    'The device did not respond. It may be offline or unplugged.';

/// Runs the graceful-first Matter removal sequence.
///
/// Calls [unpair] with `force: false` first. On failure, asks
/// [confirmForceRemove] (typically a dialog) whether to retry with
/// `force: true`; declining cancels the flow. A failed force attempt
/// re-prompts, so the flow only ends in [MatterRemovalOutcome.removed] or a
/// user-chosen [MatterRemovalOutcome.cancelled].
Future<MatterRemovalOutcome> runMatterRemovalFlow({
  required Future<Map<String, dynamic>?> Function({required bool force})
      unpair,
  required Future<bool> Function(String error) confirmForceRemove,
}) async {
  var force = false;
  while (true) {
    final result = await unpair(force: force);
    if ((result?['status'] as String?) == 'complete') {
      return MatterRemovalOutcome.removed;
    }

    final error = result?['error'] as String? ?? matterRemovalNoResponseError;
    if (!await confirmForceRemove(error)) {
      return MatterRemovalOutcome.cancelled;
    }
    force = true;
  }
}
