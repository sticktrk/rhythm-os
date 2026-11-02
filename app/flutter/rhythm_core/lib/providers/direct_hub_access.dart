import 'package:flutter/foundation.dart';
import 'package:http/http.dart' as http;

/// Device transports owned by the selected app backend. This does not govern
/// Rhythm server discovery, enrollment, or the server's authenticated API.
class DirectHubAccess {
  DirectHubAccess._();

  static final _DirectHubAccessChanges _changes = _DirectHubAccessChanges();
  static Listenable get changes => _changes;
  static String? _scope;
  static bool _allowed = true;
  static int _generation = 0;

  static bool get allowed => _allowed;

  static void requireAllowed() => capture().check();

  static void select({required String? scope, required bool allowed}) {
    if (_scope == scope && _allowed == allowed) return;
    _scope = scope;
    _allowed = allowed;
    _generation++;
    _changes.changed();
  }

  /// Bind a transport or multi-step operation to this exact selection. An old
  /// operation stays invalid even after switching back to its original home.
  static DirectHubAccessLease capture() => DirectHubAccessLease._(_generation);
}

class DirectHubAccessLease {
  DirectHubAccessLease._(this._generation);
  final int _generation;

  /// Selection identity is also useful for HA-owned handoffs, which are
  /// intentionally allowed while direct device access itself is denied.
  bool get isSameSelection => _generation == DirectHubAccess._generation;

  bool get isCurrent => DirectHubAccess.allowed && isSameSelection;

  void check() {
    if (!isCurrent) {
      throw StateError(
        'Direct device access is unavailable for this connection.',
      );
    }
  }

  /// Stop active discovery/sockets as soon as backend ownership changes.
  VoidCallback cancelOnChange(VoidCallback cancel) {
    void listener() {
      if (!isCurrent) cancel();
    }

    DirectHubAccess.changes.addListener(listener);
    return () => DirectHubAccess.changes.removeListener(listener);
  }
}

/// A guard at the send boundary also protects already-created REST providers.
class DirectHubHttpClient extends http.BaseClient {
  DirectHubHttpClient(this._inner) {
    _removeListener = _lease.cancelOnChange(close);
  }

  final http.Client _inner;
  final DirectHubAccessLease _lease = DirectHubAccess.capture();
  late final VoidCallback _removeListener;
  bool _closed = false;

  @override
  Future<http.StreamedResponse> send(http.BaseRequest request) async {
    _lease.check();
    if (_closed) throw StateError('Direct device connection has closed.');
    final response = await _inner.send(request);
    _lease.check();
    return response;
  }

  @override
  void close() {
    if (_closed) return;
    _closed = true;
    _removeListener();
    _inner.close();
  }
}

class _DirectHubAccessChanges extends ChangeNotifier {
  void changed() => notifyListeners();
}
