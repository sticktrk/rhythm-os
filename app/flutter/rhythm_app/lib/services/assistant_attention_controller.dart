import 'dart:async';

import 'package:dio/dio.dart';
import 'package:flutter/foundation.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

sealed class AssistantAttentionRequest {
  String get id;
  DateTime? get expiresAt;
}

class ConnectionAttention extends AssistantAttentionRequest {
  ConnectionAttention(this.connection);
  final RhythmAssistantConnection connection;
  @override
  String get id => connection.id;
  @override
  DateTime get expiresAt => connection.expiresAt;
}

class PlanAttention extends AssistantAttentionRequest {
  PlanAttention(this.plan);
  final RhythmAssistantApproval plan;
  @override
  String get id => plan.planId;
  @override
  DateTime? get expiresAt => plan.expiresAt;
}

/// SSE and push are hints. Only the selected hub's authenticated queue can
/// supply a review or decide a request.
class AssistantAttentionController extends ChangeNotifier {
  RhythmAuthApi? _api;
  bool Function()? _current;
  StreamSubscription<dynamic>? _events;
  Object _generation = Object();
  int _revision = 0;
  bool _loading = false;
  List<AssistantAttentionRequest> _pending = [];
  final _dismissed = <String>{};
  String? _hubId;

  RhythmAuthApi? get api => isCurrent ? _api : null;
  bool get hasTarget => _api != null;
  bool get isCurrent => _current?.call() == true;
  String? get hubId => isCurrent ? _hubId : null;

  AssistantAttentionRequest? get next {
    if (!isCurrent) return null;
    return _pending
        .where((request) =>
            !_dismissed.contains(request.id) &&
            request.expiresAt?.isAfter(DateTime.now()) != false)
        .firstOrNull;
  }

  bool contains(String id) =>
      isCurrent && _pending.any((request) => request.id == id);

  void select({
    required String hubId,
    required RhythmAuthApi api,
    required bool Function() isCurrentTarget,
    required Stream<dynamic> attentionEvents,
  }) {
    clear();
    _hubId = hubId;
    _api = api;
    _current = isCurrentTarget;
    _events = attentionEvents.listen((_) => refresh());
    unawaited(refresh());
  }

  void clear() {
    _events?.cancel();
    _events = null;
    _generation = Object();
    _loading = false;
    _hubId = null;
    _api = null;
    _current = null;
    _pending = [];
    _dismissed.clear();
    notifyListeners();
  }

  void push(String hubId) {
    if (hubId == this.hubId) unawaited(refresh());
  }

  Future<void> refresh() async {
    final api = this.api;
    if (api == null) return;
    _revision++;
    if (_loading) return;
    final generation = _generation;
    _loading = true;
    try {
      while (identical(generation, _generation) && isCurrent) {
        final revision = _revision;
        List<AssistantAttentionRequest> pending = [];
        try {
          // A 404 is an older hub. It keeps its existing manual review UI.
          final connections = await api.listAssistantConnections();
          if (generation != _generation || !isCurrent) return;
          final plans = await api.listAssistantApprovals();
          pending = [
            ...connections.map(ConnectionAttention.new),
            ...plans.map(PlanAttention.new),
          ];
        } on DioException {
          // Permission changes, older hubs, and unreachable hubs all clear
          // the prompt. A subsequent SSE, resume, or tap can try again.
        } on FormatException {
          // Unreadable responses cannot authorize showing a review.
        } on TypeError {
          // Some older servers serve their app shell for unknown routes.
        }
        if (generation != _generation || !isCurrent) return;
        if (_revision != revision) continue;
        _pending = pending;
        _dismissed.removeWhere((id) => !contains(id));
        notifyListeners();
        return;
      }
    } finally {
      if (generation == _generation) _loading = false;
    }
  }

  void dismiss(String id) {
    if (contains(id)) _dismissed.add(id);
    notifyListeners();
  }

  void reviewAgain(String id) {
    _dismissed.remove(id);
    final index = _pending.indexWhere((item) => item.id == id);
    if (index >= 0) {
      _pending.insert(0, _pending.removeAt(index));
      notifyListeners();
    } else {
      unawaited(refresh());
    }
  }

  @override
  void dispose() {
    _events?.cancel();
    _generation = Object();
    _current = null;
    super.dispose();
  }
}
