import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:supabase_flutter/supabase_flutter.dart';

import 'apple_health_sync_controller.dart';

typedef AppleHealthHistoryPage = Future<List<Map<String, dynamic>>> Function(
    AppleHealthScope scope, int offset, int cutoffEpochMs);

/// Reads existing owner-scoped retained activity; never reads Health data.
class AppleHealthHistorySource {
  final AppleHealthHistoryPage? cloudPage;
  final Future<RhythmHistory> Function(AppleHealthScope scope)? localHistory;

  const AppleHealthHistorySource({this.cloudPage, this.localHistory});

  static AppleHealthHistoryPage supabasePage(SupabaseClient client) =>
      (scope, offset, cutoff) async {
        // Explicit account filter also constrains staff sessions with broader RLS.
        final rows = await client
            .from('server_light_activity_events')
            .select('event_id,node_id,action_id,source_kind,epoch_ms,payload')
            .eq('user_id', scope.accountId)
            .eq('home_id', scope.homeId)
            .eq('server_instance_id', scope.serverId)
            .eq('node_id', RhythmSleepInterval.historyTarget(scope.nodeId))
            .eq('source_kind', 'switch')
            .inFilter('action_id',
                ['sleep_on', 'sleep_off', 'set_mode', 'trigger_transition'])
            .lte('epoch_ms', cutoff)
            .order('epoch_ms', ascending: true)
            .order('event_id', ascending: true)
            .range(offset, offset + pageSize - 1);
        return rows;
      };

  static const pageSize = 500;
  static const maxPages = 100;

  Future<AppleHealthHistorySnapshot> load(
      AppleHealthScope scope, int cutoff) async {
    final events = <RhythmActivityEvent>[];
    var succeeded = false;
    var partial = false;
    if (cloudPage != null && scope.accountId != 'local') {
      final retained = <RhythmActivityEvent>[];
      try {
        for (var page = 0; page < maxPages; page++) {
          final rows = await cloudPage!(scope, page * pageSize, cutoff)
              .timeout(const Duration(seconds: 15));
          for (final row in rows) {
            retained.add(RhythmActivityEvent.fromJson({
              ...row,
              'id': row['event_id'],
              'source': {'kind': row['source_kind']},
            }));
          }
          if (rows.length < pageSize) {
            events.addAll(retained);
            succeeded = true;
            break;
          }
          if (page == maxPages - 1) {
            throw StateError('Retained history exceeds the bounded read');
          }
        }
      } catch (_) {
        // Do not project partially loaded cloud pages across missing boundaries.
        partial = true;
      }
    }
    if (localHistory != null) {
      try {
        events.addAll(
            (await localHistory!(scope).timeout(const Duration(seconds: 15)))
                .activities);
        succeeded = true;
      } catch (_) {
        partial = true;
      }
    }
    if (!succeeded) throw StateError('Activity history is unavailable');
    return AppleHealthHistorySnapshot(events: events, partial: partial);
  }
}
