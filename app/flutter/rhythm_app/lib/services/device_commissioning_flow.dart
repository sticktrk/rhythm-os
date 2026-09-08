import 'package:uuid/uuid.dart';

/// Protocol adapters translate their authoritative Box receipt to this shape.
/// Phone setup alone is never a successful commissioning result.
enum CommissioningReceiptState { complete, failed, notFound, pending }

class CommissioningReceipt<T> {
  const CommissioningReceipt(this.state, {this.result});

  final CommissioningReceiptState state;
  final T? result;
}

enum CommissioningStartState {
  ready,
  recovered,
  pending,
  unavailable,
  inactive
}

class CommissioningStart<T> {
  const CommissioningStart(this.state, {this.result});

  final CommissioningStartState state;
  final T? result;
}

/// Shared phone/Box lifecycle. Adapters own discovery, credentials and wire
/// protocols; this owner fences retries and allocates one ID per attempt.
///
/// Mark the final handoff before dispatch. Only an authoritative terminal
/// response clears it. A missing receipt must come from the Box's causal
/// not-found fence; a transport failure cannot authorize another attempt.
class DeviceCommissioningFlow<T> {
  DeviceCommissioningFlow({required this.readReceipt});

  final Future<CommissioningReceipt<T>?> Function(String) readReceipt;
  String sessionId = 'pair-${const Uuid().v4()}';
  int attemptNumber = 0;
  bool isRunning = false;
  bool _disposed = false;
  String? _pendingSessionId;

  Future<CommissioningStart<T>> begin() async {
    if (isRunning || _disposed) {
      return const CommissioningStart(CommissioningStartState.inactive);
    }
    isRunning = true;
    final pending = _pendingSessionId;
    if (pending != null) {
      CommissioningReceipt<T>? receipt;
      try {
        receipt = await readReceipt(pending);
      } catch (_) {
        // Reconciliation failures retain the previous attempt for another read.
      }
      if (_disposed) {
        isRunning = false;
        return const CommissioningStart(CommissioningStartState.inactive);
      }
      switch (receipt?.state) {
        case CommissioningReceiptState.complete:
          isRunning = false;
          if (receipt?.result == null) {
            return const CommissioningStart(
                CommissioningStartState.unavailable);
          }
          _pendingSessionId = null;
          return CommissioningStart(CommissioningStartState.recovered,
              result: receipt!.result);
        case CommissioningReceiptState.failed:
        case CommissioningReceiptState.notFound:
          _pendingSessionId = null;
        case CommissioningReceiptState.pending:
          isRunning = false;
          return const CommissioningStart(CommissioningStartState.pending);
        case null:
          isRunning = false;
          return const CommissioningStart(CommissioningStartState.unavailable);
      }
    }
    attemptNumber += 1;
    sessionId = 'pair-${const Uuid().v4()}';
    return const CommissioningStart(CommissioningStartState.ready);
  }

  String stageSessionId(String stage) => '$sessionId-$stage';

  void expectReceipt([String? requestSessionId]) {
    if (!_disposed) _pendingSessionId = requestSessionId ?? sessionId;
  }

  void acceptTerminalResponse() => _pendingSessionId = null;

  void end() => isRunning = false;

  void dispose() {
    _disposed = true;
    isRunning = false;
    _pendingSessionId = null;
  }
}
