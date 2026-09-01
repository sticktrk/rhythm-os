/// Outcome of a server write for callers applying optimistic UI.
///
/// Distinguishes a definitive server refusal from transport uncertainty:
/// after a timeout or connection drop the server may still have committed
/// the write, so only [rejected] justifies rolling back optimistic state.
/// An [indeterminate] outcome should leave the optimistic value under its
/// normal short-lived lock and let authoritative server state reconcile it.
enum RhythmWriteAck {
  /// The server received and applied the write.
  accepted,

  /// The server responded with an error and did not apply the write.
  rejected,

  /// No conclusive response — the write may or may not have been applied.
  indeterminate,
}
