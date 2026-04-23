import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import '../providers/room_provider.dart';
import '../providers/server_sync_provider.dart';
import '../services/analytics_service.dart';
import '../widgets/room_picker_sheet.dart';
import '../widgets/solar_orbit.dart'; // For CelestialColors

enum _TriageFilter { all, devices, rooms }

/// Triage resolution screen.
///
/// Shows pending triage entries (device merges and room bindings) and allows
/// the user to merge, keep separate, bind rooms, or dismiss.
class TriageScreen extends StatefulWidget {
  const TriageScreen({super.key});

  @override
  State<TriageScreen> createState() => _TriageScreenState();
}

class _TriageScreenState extends State<TriageScreen> {
  static const _deviceKinds = <String>{'device_merge', 'unassigned_device'};
  static const _roomKinds = <String>{'room_binding', 'hub_configured'};

  List<Map<String, dynamic>> _entries = [];
  bool _loading = true;
  bool _busy = false;
  bool _connectionError = false;
  _TriageFilter _filter = _TriageFilter.all;

  @override
  void initState() {
    super.initState();
    AnalyticsService().logScreenView('device_review');
    _loadEntries();
  }

  String _kindForEntry(Map<String, dynamic> entry) =>
      entry['kind'] as String? ?? 'device_merge';

  bool _isDeviceEntry(Map<String, dynamic> entry) =>
      _deviceKinds.contains(_kindForEntry(entry));

  bool _isRoomEntry(Map<String, dynamic> entry) =>
      _roomKinds.contains(_kindForEntry(entry));

  int get _deviceCount => _entries.where(_isDeviceEntry).length;

  int get _roomCount => _entries.where(_isRoomEntry).length;

  List<Map<String, dynamic>> get _filteredEntries {
    if (_filter == _TriageFilter.all) return _entries;
    final matches =
        _filter == _TriageFilter.devices ? _isDeviceEntry : _isRoomEntry;
    return _entries.where(matches).toList();
  }

  Future<void> _loadEntries({bool sync = false}) async {
    debugPrint(
        'TriageScreen: _loadEntries called (busy=$_busy, loading=$_loading, sync=$sync)');
    try {
      final syncProvider = context.read<ServerSyncProvider>();
      final http = syncProvider.api;
      debugPrint('TriageScreen: connected=${syncProvider.synced}');
      if (sync) {
        debugPrint('TriageScreen: triggering sync...');
        await http.triggerSync();
        await syncProvider.fullRefresh();
        debugPrint('TriageScreen: sync complete');
      }
      final entries = await http.getTriageEntries();
      debugPrint('TriageScreen: got ${entries?.length ?? 'null'} entries');
      if (entries != null && entries.isNotEmpty) {
        debugPrint(
            'TriageScreen: first entry keys=${entries.first.keys.toList()}, id=${entries.first['id']} (${entries.first['id'].runtimeType})');
      }
      if (mounted) {
        setState(() {
          _connectionError = entries == null;
          _entries = entries ?? [];
          _loading = false;
        });
      }
      if (entries != null) {
        final deviceCount = entries.where(_isDeviceEntry).length;
        final roomCount = entries.where(_isRoomEntry).length;
        AnalyticsService().logTriageViewed(
          entryCount: entries.length,
          deviceCount: deviceCount,
          roomCount: roomCount,
        );
      }
    } catch (e, st) {
      debugPrint('TriageScreen: _loadEntries failed: $e\n$st');
      if (mounted) {
        setState(() {
          _connectionError = true;
          _loading = false;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final serverSync = context.watch<ServerSyncProvider>();
    debugPrint(
        'TriageScreen: build (loading=$_loading, busy=$_busy, connErr=$_connectionError, entries=${_entries.length})');
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      appBar: AppBar(
        backgroundColor: CelestialColors.backgroundDark,
        foregroundColor: CelestialColors.textPrimary,
        title: const Text('Device Review'),
        elevation: 0,
        actions: [
          if (!_loading)
            IconButton(
              icon: const Icon(Icons.refresh),
              onPressed: _busy ? null : () => _loadEntries(sync: true),
            ),
        ],
      ),
      body: _loading
          ? const Center(
              child: CircularProgressIndicator(color: CelestialColors.sunWarm),
            )
          : _connectionError
              ? _buildConnectionError()
              : _buildBody(serverSync),
    );
  }

  Widget _buildBody(ServerSyncProvider serverSync) {
    if (_entries.isEmpty &&
        !serverSync.hasReviewAttention &&
        serverSync.reviewHistory.isEmpty) {
      return _buildEmptyState();
    }
    return _buildEntryList(serverSync);
  }

  Widget _buildConnectionError() {
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(
            Icons.cloud_off,
            color: CelestialColors.textSecondary.withValues(alpha: 0.4),
            size: 64,
          ),
          const SizedBox(height: 16),
          Text(
            'Not connected to server',
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.6),
              fontSize: 16,
            ),
          ),
          const SizedBox(height: 8),
          Text(
            'Check your connection and try again.',
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.4),
              fontSize: 14,
            ),
          ),
          const SizedBox(height: 24),
          GestureDetector(
            onTap: () {
              setState(() => _loading = true);
              _loadEntries();
            },
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 10),
              decoration: BoxDecoration(
                color: CelestialColors.sunWarm.withValues(alpha: 0.12),
                borderRadius: BorderRadius.circular(10),
                border: Border.all(
                    color: CelestialColors.sunWarm.withValues(alpha: 0.3)),
              ),
              child: const Text(
                'Retry',
                style: TextStyle(
                  color: CelestialColors.sunWarm,
                  fontSize: 14,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildEmptyState() {
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(
            Icons.check_circle_outline,
            color: CelestialColors.textSecondary.withValues(alpha: 0.4),
            size: 64,
          ),
          const SizedBox(height: 16),
          Text(
            'All clear',
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.6),
              fontSize: 16,
            ),
          ),
          const SizedBox(height: 8),
          Text(
            'No items need your attention right now.',
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.4),
              fontSize: 14,
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildFilterBar() {
    Widget pill(String label, _TriageFilter value) {
      final active = _filter == value;
      return Expanded(
        child: GestureDetector(
          onTap: () {
            AnalyticsService().logTriageFilterChanged(value.name);
            setState(() => _filter = value);
          },
          child: Container(
            padding: const EdgeInsets.symmetric(vertical: 8),
            decoration: BoxDecoration(
              color: active
                  ? CelestialColors.sunWarm.withValues(alpha: 0.2)
                  : CelestialColors.backgroundDark.withValues(alpha: 0.5),
              borderRadius: BorderRadius.circular(10),
              border: Border.all(
                color: active
                    ? CelestialColors.sunWarm.withValues(alpha: 0.4)
                    : CelestialColors.textSecondary.withValues(alpha: 0.15),
              ),
            ),
            child: Center(
              child: Text(
                label,
                style: TextStyle(
                  color: active
                      ? CelestialColors.sunWarm
                      : CelestialColors.textSecondary.withValues(alpha: 0.7),
                  fontSize: 13,
                  fontWeight: active ? FontWeight.w600 : FontWeight.w400,
                ),
              ),
            ),
          ),
        ),
      );
    }

    return Padding(
      padding: const EdgeInsets.only(bottom: 16),
      child: Row(
        children: [
          pill('All ${_entries.length}', _TriageFilter.all),
          const SizedBox(width: 8),
          pill('Devices $_deviceCount', _TriageFilter.devices),
          const SizedBox(width: 8),
          pill('Rooms $_roomCount', _TriageFilter.rooms),
        ],
      ),
    );
  }

  Widget _buildEntryList(ServerSyncProvider serverSync) {
    final filtered = _filteredEntries;
    final hasBothKinds = _deviceCount > 0 && _roomCount > 0;
    final headerWidgets = <Widget>[
      if (serverSync.hasReviewAttention)
        Padding(
          padding: const EdgeInsets.only(bottom: 16),
          child: _buildReviewSummaryCard(serverSync),
        ),
      if (serverSync.reviewHistory.isNotEmpty)
        Padding(
          padding: const EdgeInsets.only(bottom: 20),
          child: _buildReviewHistorySection(serverSync),
        ),
      if (hasBothKinds) _buildFilterBar(),
      Padding(
        padding: EdgeInsets.only(
          bottom: filtered.isEmpty ? 0 : 20,
          top: hasBothKinds ? 0 : 4,
        ),
        child: Text(
          _summaryLabel(serverSync, filtered.length),
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.7),
            fontSize: 14,
          ),
        ),
      ),
    ];

    return ListView.builder(
      padding: const EdgeInsets.all(20),
      itemCount: filtered.length + headerWidgets.length,
      itemBuilder: (context, index) {
        if (index < headerWidgets.length) {
          return headerWidgets[index];
        }
        final entry = filtered[index - headerWidgets.length];
        final kind = _kindForEntry(entry);
        if (kind == 'room_binding') {
          return Padding(
            padding: const EdgeInsets.only(bottom: 16),
            child: _RoomBindingCard(
              key: ValueKey(entry['id']),
              entry: entry,
              busy: _busy,
              onBind: ({String? targetRoomId}) =>
                  _resolveBind(entry, targetRoomId: targetRoomId),
              onKeepSeparate: () => _resolveNew(entry),
              onDismiss: () => _resolveDismiss(entry),
            ),
          );
        }
        if (kind == 'unassigned_device') {
          return Padding(
            padding: const EdgeInsets.only(bottom: 16),
            child: _UnassignedDeviceCard(
              key: ValueKey(entry['id']),
              entry: entry,
              busy: _busy,
              onAssignRoom: () => _resolveAssignRoom(entry),
              onDismiss: () => _resolveDismiss(entry),
            ),
          );
        }
        if (kind == 'hub_configured') {
          final reviewEntry = _reviewEntryForId(
            serverSync,
            entry['id']?.toString() ?? '',
          );
          return Padding(
            padding: const EdgeInsets.only(bottom: 16),
            child: _InfoTriageCard(
              key: ValueKey(entry['id']),
              entry: entry,
              busy: _busy,
              summary: reviewEntry?.summary,
              guidance: reviewEntry?.guidance,
              onRecheck: _recheckConflicts,
              onDismiss: () => _resolveDismiss(entry),
            ),
          );
        }
        return Padding(
          padding: const EdgeInsets.only(bottom: 16),
          child: _TriageCard(
            key: ValueKey(entry['id']),
            entry: entry,
            busy: _busy,
            onMerge: (canonicalId) => _resolveMerge(entry, canonicalId),
            onKeepSeparate: () => _resolveNew(entry),
            onDismiss: () => _resolveDismiss(entry),
          ),
        );
      },
    );
  }

  String _summaryLabel(ServerSyncProvider serverSync, int filteredCount) {
    if (_entries.isEmpty) {
      if (serverSync.hasReviewAttention) {
        return 'No pending triage entries. Review the restore and conflict status below.';
      }
      return 'No items need your attention right now.';
    }

    final noun = switch (_filter) {
      _TriageFilter.all => 'item',
      _TriageFilter.devices => 'device',
      _TriageFilter.rooms => 'room',
    };
    return '$filteredCount $noun${filteredCount != 1 ? 's' : ''} need your attention';
  }

  RhythmReviewEntry? _reviewEntryForId(
    ServerSyncProvider serverSync,
    String id,
  ) {
    for (final entry in serverSync.review.triageEntries) {
      if (entry.id == id) return entry;
    }
    return null;
  }

  Widget _buildReviewSummaryCard(ServerSyncProvider serverSync) {
    final review = serverSync.review;
    final detailLines = <String>[
      if (review.pending.total > 0)
        '${review.pending.total} pending review item${review.pending.total == 1 ? '' : 's'}',
      if (review.disconnectedHubs.isNotEmpty)
        'Reconnect ${review.disconnectedHubs.length} hub${review.disconnectedHubs.length == 1 ? '' : 's'}: ${review.disconnectedHubs.take(2).map((hub) => hub.label).join(', ')}${review.disconnectedHubs.length > 2 ? '...' : ''}',
      if (review.preferredEndpoints.isNotEmpty)
        '${review.preferredEndpoints.length} device${review.preferredEndpoints.length == 1 ? '' : 's'} kept an explicit preferred endpoint',
      if (review.hubConfiguredConflicts.isNotEmpty)
        '${review.hubConfiguredConflicts.length} native automation conflict${review.hubConfiguredConflicts.length == 1 ? '' : 's'} still need recheck',
    ];

    return Container(
      decoration: BoxDecoration(
        color: const Color(0xFF1D2C36),
        borderRadius: BorderRadius.circular(16),
        border: Border.all(
          color: const Color(0xFF64B5F6).withValues(alpha: 0.24),
        ),
      ),
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Row(
            children: [
              Icon(
                Icons.fact_check_outlined,
                color: Color(0xFF64B5F6),
                size: 20,
              ),
              SizedBox(width: 10),
              Text(
                'Restore Review',
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 15,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ],
          ),
          const SizedBox(height: 10),
          Text(
            'Use this screen to verify reconnects, previous review decisions, and native-automation conflicts after a restore or major resync.',
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.78),
              fontSize: 13,
              height: 1.35,
            ),
          ),
          for (final line in detailLines) ...[
            const SizedBox(height: 8),
            Text(
              line,
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.72),
                fontSize: 12,
              ),
            ),
          ],
          if (review.preferredEndpoints.isNotEmpty) ...[
            const SizedBox(height: 10),
            Text(
              'Preferred route: ${review.preferredEndpoints.first.name} via ${review.preferredEndpoints.first.hubLabel}',
              style: TextStyle(
                color: const Color(0xFF9FD3FF).withValues(alpha: 0.9),
                fontSize: 12,
              ),
            ),
          ],
        ],
      ),
    );
  }

  Widget _buildReviewHistorySection(ServerSyncProvider serverSync) {
    final history = serverSync.reviewHistory.take(4).toList(growable: false);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          'Recent Decisions',
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.8),
            fontSize: 12,
            fontWeight: FontWeight.w600,
            letterSpacing: 0.8,
          ),
        ),
        const SizedBox(height: 10),
        for (final entry in history)
          Padding(
            padding: const EdgeInsets.only(bottom: 8),
            child: Container(
              decoration: BoxDecoration(
                color: CelestialColors.backgroundCard,
                borderRadius: BorderRadius.circular(14),
                border: Border.all(
                  color: _historyColor(entry).withValues(alpha: 0.22),
                ),
              ),
              padding: const EdgeInsets.all(14),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Container(
                    padding:
                        const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
                    decoration: BoxDecoration(
                      color: _historyColor(entry).withValues(alpha: 0.14),
                      borderRadius: BorderRadius.circular(999),
                    ),
                    child: Text(
                      entry.statusLabel,
                      style: TextStyle(
                        color: _historyColor(entry),
                        fontSize: 11,
                        fontWeight: FontWeight.w700,
                      ),
                    ),
                  ),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          entry.name.isEmpty ? entry.summary : entry.name,
                          style: const TextStyle(
                            color: CelestialColors.textPrimary,
                            fontSize: 13,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                        const SizedBox(height: 4),
                        Text(
                          entry.summary,
                          style: TextStyle(
                            color: CelestialColors.textSecondary
                                .withValues(alpha: 0.72),
                            fontSize: 12,
                            height: 1.3,
                          ),
                        ),
                        const SizedBox(height: 4),
                        Text(
                          _historyMeta(entry),
                          style: TextStyle(
                            color: CelestialColors.textSecondary
                                .withValues(alpha: 0.5),
                            fontSize: 11,
                          ),
                        ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
          ),
      ],
    );
  }

  Color _historyColor(RhythmReviewEntry entry) {
    if (entry.isKeepSeparate) return const Color(0xFF64B5F6);
    if (entry.isDismissed) return const Color(0xFFB0BEC5);
    if (entry.isConfirmed) return const Color(0xFF81C784);
    return CelestialColors.sunWarm;
  }

  String _historyMeta(RhythmReviewEntry entry) {
    final resolvedAt = entry.resolvedAtDateTime;
    final when = resolvedAt == null ? 'Recently' : _relativeTime(resolvedAt);
    final hub = hubTypeLabel(entry.hubType);
    return [hub, when].where((part) => part.isNotEmpty).join(' - ');
  }

  String _relativeTime(DateTime time) {
    final delta = DateTime.now().difference(time);
    if (delta.inMinutes < 1) return 'just now';
    if (delta.inHours < 1) return '${delta.inMinutes}m ago';
    if (delta.inDays < 1) return '${delta.inHours}h ago';
    return '${delta.inDays}d ago';
  }

  Future<void> _refreshReviewState() async {
    try {
      await context.read<ServerSyncProvider>().fullRefresh();
    } catch (e, st) {
      debugPrint('TriageScreen: review refresh failed: $e\n$st');
    }
  }

  Future<void> _resolveMerge(
    Map<String, dynamic> entry,
    String canonicalId,
  ) async {
    debugPrint(
        'TriageScreen: _resolveMerge called (busy=$_busy, entryId=${entry['id']}, canonicalId=$canonicalId)');
    if (_busy) return;
    setState(() => _busy = true);
    try {
      final entryId = entry['id']?.toString() ?? '';
      final http = context.read<ServerSyncProvider>().api;
      final success = await http.resolveTriageMerge(entryId, canonicalId);
      if (!success) {
        throw StateError('The server rejected this merge.');
      }
      await _refreshReviewState();
      AnalyticsService().logTriageResolution(
        kind: 'device_merge',
        action: 'merge',
        hasTarget: true,
      );
      if (mounted) await _loadEntries();
    } catch (e) {
      debugPrint('TriageScreen: merge failed: $e');
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
              content: Text('Merge failed: $e'),
              backgroundColor: Colors.red.shade800),
        );
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _resolveNew(Map<String, dynamic> entry) async {
    debugPrint(
        'TriageScreen: _resolveNew called (busy=$_busy, entryId=${entry['id']})');
    if (_busy) return;
    setState(() => _busy = true);
    try {
      final entryId = entry['id']?.toString() ?? '';
      final http = context.read<ServerSyncProvider>().api;
      final result = await http.resolveTriageNewResult(entryId);
      if (result == null) {
        throw StateError('The server did not accept this resolution.');
      }
      await _refreshReviewState();
      AnalyticsService().logTriageResolution(
        kind: _kindForEntry(entry),
        action: 'keep_separate',
      );
      if (mounted) await _loadEntries();
    } catch (e) {
      debugPrint('TriageScreen: keep-separate failed: $e');
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
              content: Text('Keep separate failed: $e'),
              backgroundColor: Colors.red.shade800),
        );
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _resolveBind(Map<String, dynamic> entry,
      {String? targetRoomId}) async {
    debugPrint(
        'TriageScreen: _resolveBind called (busy=$_busy, entryId=${entry['id']}, targetRoomId=$targetRoomId)');
    if (_busy) return;
    setState(() => _busy = true);
    try {
      final entryId = entry['id']?.toString() ?? '';
      final http = context.read<ServerSyncProvider>().api;
      final success =
          await http.resolveTriageBind(entryId, targetRoomId: targetRoomId);
      if (!success) {
        throw StateError('The server rejected this room merge.');
      }
      await _refreshReviewState();
      AnalyticsService().logTriageResolution(
        kind: 'room_binding',
        action: 'merge',
        hasTarget: targetRoomId != null,
      );
      if (mounted) await _loadEntries();
    } catch (e) {
      debugPrint('TriageScreen: bind failed: $e');
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
              content: Text('Room merge failed: $e'),
              backgroundColor: Colors.red.shade800),
        );
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _resolveAssignRoom(Map<String, dynamic> entry) async {
    debugPrint(
        'TriageScreen: _resolveAssignRoom called (busy=$_busy, entryId=${entry['id']})');
    if (_busy) return;

    final roomId = await _selectRoomId();
    if (roomId == null || !mounted) return;

    setState(() => _busy = true);
    try {
      final entryId = entry['id']?.toString() ?? '';
      final http = context.read<ServerSyncProvider>().api;
      final success = await http.resolveTriageRoom(entryId, roomId);
      if (!success) {
        throw StateError('The server rejected this room assignment.');
      }
      await _refreshReviewState();
      AnalyticsService().logTriageResolution(
        kind: 'unassigned_device',
        action: 'assign_room',
        hasTarget: true,
      );
      if (mounted) await _loadEntries();
    } catch (e) {
      debugPrint('TriageScreen: assign-room failed: $e');
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
              content: Text('Room assignment failed: $e'),
              backgroundColor: Colors.red.shade800),
        );
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<String?> _selectRoomId() async {
    final rooms = context.read<RoomProvider>().rooms;
    return showRoomPickerSheet(
      context,
      title: 'Assign to Room',
      rooms: [
        for (final room in rooms)
          RoomPickerOption(
            id: room.id,
            name: room.name,
            subtitle: context
                .read<ServerSyncProvider>()
                .deviceSummaryForRoom(room.id),
          ),
      ],
      allowCreateRoom: true,
      emptyMessage:
          'No rooms exist yet. Create one now to finish assigning this device.',
      onCreateRoom: _createRoomForAssignment,
    );
  }

  Future<String?> _createRoomForAssignment() async {
    final roomName = await _promptForRoomName();
    final trimmedName = roomName?.trim() ?? '';
    if (trimmedName.isEmpty || !mounted) return null;

    final result =
        await context.read<ServerSyncProvider>().api.createTopologyRoom(
              trimmedName,
            );
    final roomId = result?['id'] as String?;
    if (roomId != null && roomId.isNotEmpty) {
      return roomId;
    }

    if (mounted) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('Room creation failed')),
      );
    }
    return null;
  }

  Future<String?> _promptForRoomName() async {
    final controller = TextEditingController();
    return showDialog<String>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: const Text(
          'Create Room',
          style: TextStyle(color: CelestialColors.textPrimary),
        ),
        content: TextField(
          controller: controller,
          autofocus: true,
          style: const TextStyle(color: CelestialColors.textPrimary),
          decoration: InputDecoration(
            hintText: 'Room name',
            hintStyle: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.6),
            ),
          ),
          onSubmitted: (value) => Navigator.of(dialogContext).pop(value.trim()),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(),
            child: Text(
              'Cancel',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.8),
              ),
            ),
          ),
          TextButton(
            onPressed: () =>
                Navigator.of(dialogContext).pop(controller.text.trim()),
            child: const Text(
              'Create',
              style: TextStyle(color: CelestialColors.sunWarm),
            ),
          ),
        ],
      ),
    );
  }

  Future<void> _resolveDismiss(Map<String, dynamic> entry) async {
    debugPrint(
        'TriageScreen: _resolveDismiss called (busy=$_busy, entryId=${entry['id']})');
    if (_busy) return;
    setState(() => _busy = true);
    try {
      final entryId = entry['id']?.toString() ?? '';
      final http = context.read<ServerSyncProvider>().api;
      final success = await http.resolveTriageDismiss(entryId);
      if (!success) {
        throw StateError('The server rejected this dismissal.');
      }
      await _refreshReviewState();
      AnalyticsService().logTriageResolution(
        kind: _kindForEntry(entry),
        action: 'dismiss',
      );
      if (mounted) await _loadEntries();
    } catch (e) {
      debugPrint('TriageScreen: dismiss failed: $e');
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
              content: Text('Dismiss failed: $e'),
              backgroundColor: Colors.red.shade800),
        );
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _recheckConflicts() async {
    if (_busy) return;
    setState(() => _busy = true);
    try {
      await _loadEntries(sync: true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }
}

// =============================================================================
// Device merge card (existing)
// =============================================================================

Map<String, dynamic> _mapField(Map<String, dynamic> source, String key) {
  final value = source[key];
  if (value is Map<String, dynamic>) return value;
  if (value is Map) return Map<String, dynamic>.from(value);
  return const {};
}

Map<String, dynamic> _triageDeviceData(Map<String, dynamic> entry) {
  for (final key in ['unassigned_device', 'discovered', 'device']) {
    final data = _mapField(entry, key);
    if (data.isNotEmpty) return data;
  }
  return entry;
}

String _deviceTypeLabel(String deviceType) {
  return switch (deviceType) {
    'light' => 'Light',
    'button' => 'Button',
    'motion' => 'Motion Sensor',
    _ => deviceType,
  };
}

class _TriageCard extends StatelessWidget {
  final Map<String, dynamic> entry;
  final bool busy;
  final void Function(String canonicalId) onMerge;
  final VoidCallback onKeepSeparate;
  final VoidCallback onDismiss;

  const _TriageCard({
    super.key,
    required this.entry,
    this.busy = false,
    required this.onMerge,
    required this.onKeepSeparate,
    required this.onDismiss,
  });

  @override
  Widget build(BuildContext context) {
    debugPrint('_TriageCard: build (id=${entry['id']}, busy=$busy)');
    final discovered = _triageDeviceData(entry);
    final name = discovered['name'] as String? ?? 'Unknown Device';
    final deviceType = discovered['device_type'] as String? ?? 'light';
    final manufacturer = discovered['manufacturer'] as String?;
    final model = discovered['model'] as String?;
    final roomName = discovered['room_name'] as String? ?? '';
    final candidates = (entry['candidate_matches'] as List<dynamic>?) ?? [];

    final typeLabel = _deviceTypeLabel(deviceType);

    final productInfo =
        [manufacturer, model].whereType<String>().join(' \u00B7 ');

    return Container(
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(
          color: CelestialColors.sunWarm.withValues(alpha: 0.2),
        ),
      ),
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // Header
          Row(
            children: [
              Container(
                width: 40,
                height: 40,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: CelestialColors.sunWarm.withValues(alpha: 0.15),
                ),
                child: const Icon(
                  Icons.device_unknown_outlined,
                  color: CelestialColors.sunWarm,
                  size: 20,
                ),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      'New device: "$name"',
                      style: const TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 15,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    Text(
                      '$typeLabel${productInfo.isNotEmpty ? ' \u00B7 $productInfo' : ''}',
                      style: TextStyle(
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.7),
                        fontSize: 12,
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ),
          if (roomName.isNotEmpty) ...[
            const SizedBox(height: 8),
            Text(
              'Found in: $roomName',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.6),
                fontSize: 12,
              ),
            ),
          ],
          // Candidates
          if (candidates.isNotEmpty) ...[
            const SizedBox(height: 16),
            Padding(
              padding: const EdgeInsets.only(bottom: 8),
              child: Text(
                candidates.length > 1 ? 'Possible matches:' : 'Possible match:',
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.6),
                  fontSize: 11,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 1.0,
                ),
              ),
            ),
            for (final candidate in candidates) _buildCandidateRow(candidate),
          ],
          const SizedBox(height: 12),
          Text(
            'Keep Separate saves this as a different physical device. Dismiss only closes this suggestion without recording that separate-device decision.',
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.6),
              fontSize: 12,
              height: 1.3,
            ),
          ),
          const SizedBox(height: 16),
          // Actions
          Opacity(
            opacity: busy ? 0.5 : 1.0,
            child: IgnorePointer(
              ignoring: busy,
              child: Row(
                children: [
                  Expanded(
                    child: _ActionButton(
                      label: 'Keep Separate',
                      color: const Color(0xFF64B5F6),
                      onTap: () {
                        debugPrint(
                            '_TriageCard: Keep Separate tapped (id=${entry['id']})');
                        onKeepSeparate();
                      },
                    ),
                  ),
                  const SizedBox(width: 8),
                  _ActionButton(
                    label: 'Dismiss',
                    color: CelestialColors.textSecondary,
                    onTap: () {
                      debugPrint(
                          '_TriageCard: Dismiss tapped (id=${entry['id']})');
                      onDismiss();
                    },
                  ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildCandidateRow(dynamic candidate) {
    final c = candidate as Map<String, dynamic>;
    final cid = c['canonical_id'] as String? ?? '';
    final candidateName = c['name'] as String? ?? '';
    final score = c['score'] as int? ?? 0;
    final reasons = (c['reasons'] as List<dynamic>?)
            ?.map((r) => _formatReason(r.toString()))
            .join(', ') ??
        '';

    // Show name if available, fall back to truncated ID for pre-update entries
    final displayName = candidateName.isNotEmpty
        ? candidateName
        : (cid.length > 16 ? '${cid.substring(0, 16)}...' : cid);

    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
        decoration: BoxDecoration(
          color: CelestialColors.backgroundDark.withValues(alpha: 0.5),
          borderRadius: BorderRadius.circular(8),
        ),
        child: Row(
          children: [
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    displayName,
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 13,
                    ),
                  ),
                  if (reasons.isNotEmpty)
                    Text(
                      reasons,
                      style: TextStyle(
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.6),
                        fontSize: 11,
                      ),
                    ),
                ],
              ),
            ),
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
              decoration: BoxDecoration(
                color: CelestialColors.sunWarm.withValues(alpha: 0.15),
                borderRadius: BorderRadius.circular(6),
              ),
              child: Text(
                'Score: $score',
                style: const TextStyle(
                  color: CelestialColors.sunWarm,
                  fontSize: 10,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
            const SizedBox(width: 8),
            GestureDetector(
              onTap: busy
                  ? null
                  : () {
                      debugPrint(
                          '_TriageCard: Merge tapped (id=${entry['id']}, cid=$cid)');
                      onMerge(cid);
                    },
              child: Container(
                padding:
                    const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
                decoration: BoxDecoration(
                  color: CelestialColors.sunWarm.withValues(alpha: 0.12),
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(
                      color: CelestialColors.sunWarm.withValues(alpha: 0.3)),
                ),
                child: const Text(
                  'Merge',
                  style: TextStyle(
                    color: CelestialColors.sunWarm,
                    fontSize: 12,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  String _formatReason(String reason) {
    return switch (reason) {
      'exact_name' => 'Same name',
      'exact_hardware' => 'Same hardware',
      'partial_name' => 'Similar name',
      'same_manufacturer' => 'Same manufacturer',
      'same_model' => 'Same model',
      'same_device_type' => 'Same type',
      'same_room' => 'Same room',
      _ => reason,
    };
  }
}

// =============================================================================
// Unassigned device card
// =============================================================================

class _UnassignedDeviceCard extends StatelessWidget {
  final Map<String, dynamic> entry;
  final bool busy;
  final VoidCallback onAssignRoom;
  final VoidCallback onDismiss;

  const _UnassignedDeviceCard({
    super.key,
    required this.entry,
    this.busy = false,
    required this.onAssignRoom,
    required this.onDismiss,
  });

  @override
  Widget build(BuildContext context) {
    final device = _triageDeviceData(entry);
    final name = device['name'] as String? ??
        device['display_name'] as String? ??
        'Unknown Device';
    final deviceType = device['device_type'] as String? ?? 'light';
    final manufacturer = device['manufacturer'] as String?;
    final model = device['model'] as String?;
    final hubName = device['hub_name'] as String? ??
        device['hub_room_name'] as String? ??
        '';
    final productInfo =
        [manufacturer, model].whereType<String>().join(' \u00B7 ');

    return Container(
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(
          color: const Color(0xFF81C784).withValues(alpha: 0.25),
        ),
      ),
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Container(
                width: 40,
                height: 40,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: const Color(0xFF81C784).withValues(alpha: 0.15),
                ),
                child: const Icon(
                  Icons.device_hub_outlined,
                  color: Color(0xFF81C784),
                  size: 20,
                ),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      'Assign room: "$name"',
                      style: const TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 15,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    Text(
                      '${_deviceTypeLabel(deviceType)}${productInfo.isNotEmpty ? ' \u00B7 $productInfo' : ''}',
                      style: TextStyle(
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.7),
                        fontSize: 12,
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ),
          const SizedBox(height: 12),
          Text(
            'This device does not have a Rhythm room yet.',
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.7),
              fontSize: 13,
            ),
          ),
          if (hubName.isNotEmpty) ...[
            const SizedBox(height: 4),
            Text(
              'Discovered from: $hubName',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.5),
                fontSize: 12,
              ),
            ),
          ],
          const SizedBox(height: 16),
          Opacity(
            opacity: busy ? 0.5 : 1.0,
            child: IgnorePointer(
              ignoring: busy,
              child: Row(
                children: [
                  Expanded(
                    child: _ActionButton(
                      label: 'Assign Room',
                      color: const Color(0xFF81C784),
                      onTap: onAssignRoom,
                    ),
                  ),
                  const SizedBox(width: 8),
                  _ActionButton(
                    label: 'Dismiss',
                    color: CelestialColors.textSecondary,
                    onTap: onDismiss,
                  ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

// =============================================================================
// Informational triage card
// =============================================================================

class _InfoTriageCard extends StatelessWidget {
  final Map<String, dynamic> entry;
  final bool busy;
  final String? summary;
  final String? guidance;
  final VoidCallback onRecheck;
  final VoidCallback onDismiss;

  const _InfoTriageCard({
    super.key,
    required this.entry,
    this.busy = false,
    this.summary,
    this.guidance,
    required this.onRecheck,
    required this.onDismiss,
  });

  @override
  Widget build(BuildContext context) {
    final info = _mapField(entry, 'hub_configured');
    final hubKey = _mapField(entry, 'hub_key');
    final hubType =
        info['hub_type'] as String? ?? hubKey['hub_type'] as String? ?? 'hub';
    final address = info['address'] as String? ?? hubKey['address'] as String?;
    final title = switch (entry['kind'] as String?) {
      'hub_configured' => 'Hub configured',
      _ => 'Review item',
    };
    final details = [
      hubType.toUpperCase(),
      if (address != null && address.isNotEmpty) address,
    ].join(' \u00B7 ');
    final message = guidance ??
        switch (entry['kind'] as String?) {
          'hub_configured' =>
            'Remove the native automation in the hub app, then tap Recheck to confirm Rhythm cleared the conflict.',
          _ => 'This triage item is not yet specialized in the UI.',
        };

    return Container(
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(
          color: CelestialColors.textSecondary.withValues(alpha: 0.2),
        ),
      ),
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Container(
                width: 40,
                height: 40,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: CelestialColors.textSecondary.withValues(alpha: 0.12),
                ),
                child: const Icon(
                  Icons.info_outline,
                  color: CelestialColors.textSecondary,
                  size: 20,
                ),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      title,
                      style: const TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 15,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    if (details.isNotEmpty)
                      Text(
                        details,
                        style: TextStyle(
                          color: CelestialColors.textSecondary
                              .withValues(alpha: 0.7),
                          fontSize: 12,
                        ),
                      ),
                  ],
                ),
              ),
            ],
          ),
          const SizedBox(height: 12),
          if (summary != null && summary!.isNotEmpty) ...[
            Text(
              summary!,
              style: const TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 13,
                fontWeight: FontWeight.w600,
              ),
            ),
            const SizedBox(height: 8),
          ],
          Text(
            message,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.7),
              fontSize: 13,
            ),
          ),
          const SizedBox(height: 16),
          Opacity(
            opacity: busy ? 0.5 : 1.0,
            child: IgnorePointer(
              ignoring: busy,
              child: Row(
                children: [
                  Expanded(
                    child: _ActionButton(
                      label: 'Recheck',
                      color: const Color(0xFF64B5F6),
                      onTap: onRecheck,
                    ),
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: _ActionButton(
                      label: 'Dismiss',
                      color: CelestialColors.textSecondary,
                      onTap: onDismiss,
                    ),
                  ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

// =============================================================================
// Room binding card (new)
// =============================================================================

class _RoomBindingCard extends StatelessWidget {
  final Map<String, dynamic> entry;
  final bool busy;
  final void Function({String? targetRoomId}) onBind;
  final VoidCallback onKeepSeparate;
  final VoidCallback onDismiss;

  const _RoomBindingCard({
    super.key,
    required this.entry,
    this.busy = false,
    required this.onBind,
    required this.onKeepSeparate,
    required this.onDismiss,
  });

  static const _roomBlue = Color(0xFF64B5F6);

  @override
  Widget build(BuildContext context) {
    final binding = (entry['room_binding'] as Map<String, dynamic>?) ?? {};
    final hubKey = (entry['hub_key'] as Map<String, dynamic>?) ?? {};
    final hubRoomName = binding['hub_room_name'] as String? ?? 'Unknown Room';
    final targetName = binding['target_rhythm_room_name'] as String? ?? '';
    final targetId = binding['target_rhythm_room_id'] as String? ?? '';
    final candidates = (binding['candidate_rooms'] as List<dynamic>?) ?? [];
    final lightIds = (binding['light_device_ids'] as List<dynamic>?) ?? [];
    final hubType = hubKey['hub_type'] as String? ?? '';
    final hubAddr = hubKey['address'] as String? ?? '';

    final hubLabel = switch (hubType) {
      'ha' => 'Home Assistant',
      'hue' => 'Hue',
      _ => hubType,
    };
    final hubInfo =
        [hubLabel, hubAddr].where((s) => s.isNotEmpty).join(' \u00B7 ');

    final hasMultipleCandidates = candidates.length > 1;

    return Container(
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(
          color: _roomBlue.withValues(alpha: 0.2),
        ),
      ),
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // Header
          Row(
            children: [
              Container(
                width: 40,
                height: 40,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: _roomBlue.withValues(alpha: 0.15),
                ),
                child: const Icon(
                  Icons.meeting_room,
                  color: _roomBlue,
                  size: 20,
                ),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      'Room: "$hubRoomName"',
                      style: const TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 15,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    if (hubInfo.isNotEmpty)
                      Text(
                        hubInfo,
                        style: TextStyle(
                          color: CelestialColors.textSecondary
                              .withValues(alpha: 0.7),
                          fontSize: 12,
                        ),
                      ),
                  ],
                ),
              ),
            ],
          ),
          const SizedBox(height: 12),
          // Match info
          if (targetName.isNotEmpty)
            Text(
              'Matches: "$targetName" (existing)',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                fontSize: 13,
              ),
            ),
          if (lightIds.isNotEmpty) ...[
            const SizedBox(height: 4),
            Text(
              '${lightIds.length} light${lightIds.length != 1 ? 's' : ''} in this room',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.5),
                fontSize: 12,
              ),
            ),
          ],
          // Candidate picker (3+ hub case)
          if (hasMultipleCandidates) ...[
            const SizedBox(height: 16),
            Padding(
              padding: const EdgeInsets.only(bottom: 8),
              child: Text(
                'Merge into:',
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.6),
                  fontSize: 11,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 1.0,
                ),
              ),
            ),
            for (final candidate in candidates)
              _buildCandidateRoomRow(candidate),
          ],
          const SizedBox(height: 12),
          Text(
            'Keep Separate saves this as a separate Rhythm room. Dismiss only hides the current suggestion without saving that room-separation choice.',
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.6),
              fontSize: 12,
              height: 1.3,
            ),
          ),
          const SizedBox(height: 16),
          // Actions
          Opacity(
            opacity: busy ? 0.5 : 1.0,
            child: IgnorePointer(
              ignoring: busy,
              child: Row(
                children: [
                  // Single "Merge Rooms" button when only one target
                  if (!hasMultipleCandidates)
                    Expanded(
                      child: _ActionButton(
                        label: 'Merge Rooms',
                        color: CelestialColors.sunWarm,
                        onTap: () => onBind(
                            targetRoomId:
                                targetId.isNotEmpty ? targetId : null),
                      ),
                    ),
                  if (!hasMultipleCandidates) const SizedBox(width: 8),
                  Expanded(
                    child: _ActionButton(
                      label: 'Keep Separate',
                      color: _roomBlue,
                      onTap: onKeepSeparate,
                    ),
                  ),
                  const SizedBox(width: 8),
                  _ActionButton(
                    label: 'Dismiss',
                    color: CelestialColors.textSecondary,
                    onTap: onDismiss,
                  ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildCandidateRoomRow(dynamic candidate) {
    // candidate_rooms entries are [room_id, room_name] tuples (JSON arrays)
    final pair = candidate as List<dynamic>;
    final roomId = pair[0] as String? ?? '';
    final roomName = pair[1] as String? ?? roomId;

    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
        decoration: BoxDecoration(
          color: CelestialColors.backgroundDark.withValues(alpha: 0.5),
          borderRadius: BorderRadius.circular(8),
        ),
        child: Row(
          children: [
            Expanded(
              child: Text(
                roomName,
                style: const TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 13,
                ),
              ),
            ),
            GestureDetector(
              onTap: busy ? null : () => onBind(targetRoomId: roomId),
              child: Container(
                padding:
                    const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
                decoration: BoxDecoration(
                  color: CelestialColors.sunWarm.withValues(alpha: 0.12),
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(
                      color: CelestialColors.sunWarm.withValues(alpha: 0.3)),
                ),
                child: const Text(
                  'Merge',
                  style: TextStyle(
                    color: CelestialColors.sunWarm,
                    fontSize: 12,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// =============================================================================
// Shared action button
// =============================================================================

class _ActionButton extends StatelessWidget {
  final String label;
  final Color color;
  final VoidCallback onTap;

  const _ActionButton({
    required this.label,
    required this.color,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
        decoration: BoxDecoration(
          color: color.withValues(alpha: 0.12),
          borderRadius: BorderRadius.circular(10),
          border: Border.all(color: color.withValues(alpha: 0.3)),
        ),
        child: Center(
          child: Text(
            label,
            style: TextStyle(
              color: color,
              fontSize: 13,
              fontWeight: FontWeight.w600,
            ),
          ),
        ),
      ),
    );
  }
}
