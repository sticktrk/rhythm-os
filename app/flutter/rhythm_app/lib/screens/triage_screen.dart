import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../providers/server_sync_provider.dart';
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
  List<Map<String, dynamic>> _entries = [];
  bool _loading = true;
  bool _busy = false;
  bool _connectionError = false;
  _TriageFilter _filter = _TriageFilter.all;

  @override
  void initState() {
    super.initState();
    _loadEntries();
  }

  int get _deviceCount =>
      _entries.where((e) => (e['kind'] ?? 'device_merge') == 'device_merge').length;

  int get _roomCount =>
      _entries.where((e) => e['kind'] == 'room_binding').length;

  List<Map<String, dynamic>> get _filteredEntries {
    if (_filter == _TriageFilter.all) return _entries;
    final targetKind =
        _filter == _TriageFilter.devices ? 'device_merge' : 'room_binding';
    return _entries
        .where((e) => (e['kind'] ?? 'device_merge') == targetKind)
        .toList();
  }

  Future<void> _loadEntries({bool sync = false}) async {
    debugPrint('TriageScreen: _loadEntries called (busy=$_busy, loading=$_loading, sync=$sync)');
    try {
      final http = context.read<ServerSyncProvider>().httpClient;
      debugPrint('TriageScreen: httpClient connected=${http.connected}');
      if (sync) {
        debugPrint('TriageScreen: triggering sync...');
        await http.triggerSync();
        debugPrint('TriageScreen: sync complete');
      }
      final entries = await http.getTriageEntries();
      debugPrint('TriageScreen: got ${entries?.length ?? 'null'} entries');
      if (entries != null && entries.isNotEmpty) {
        debugPrint('TriageScreen: first entry keys=${entries.first.keys.toList()}, id=${entries.first['id']} (${entries.first['id'].runtimeType})');
      }
      if (mounted) {
        setState(() {
          _connectionError = entries == null;
          _entries = entries ?? [];
          _loading = false;
        });
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
    debugPrint('TriageScreen: build (loading=$_loading, busy=$_busy, connErr=$_connectionError, entries=${_entries.length})');
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
              : _entries.isEmpty
                  ? _buildEmptyState()
                  : _buildEntryList(),
    );
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
                border: Border.all(color: CelestialColors.sunWarm.withValues(alpha: 0.3)),
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
          onTap: () => setState(() => _filter = value),
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

  Widget _buildEntryList() {
    final filtered = _filteredEntries;
    final hasBothKinds = _deviceCount > 0 && _roomCount > 0;
    final headerCount = hasBothKinds ? 2 : 1; // filter bar + summary, or just summary

    final noun = switch (_filter) {
      _TriageFilter.all => 'item',
      _TriageFilter.devices => 'device',
      _TriageFilter.rooms => 'room',
    };

    return ListView.builder(
      padding: const EdgeInsets.all(20),
      itemCount: filtered.length + headerCount,
      itemBuilder: (context, index) {
        // Filter bar (only when both kinds exist)
        if (hasBothKinds && index == 0) {
          return _buildFilterBar();
        }
        // Summary text
        final summaryIndex = hasBothKinds ? 1 : 0;
        if (index == summaryIndex) {
          return Padding(
            padding: const EdgeInsets.only(bottom: 20),
            child: Text(
              '${filtered.length} $noun${filtered.length != 1 ? 's' : ''} need your attention',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                fontSize: 14,
              ),
            ),
          );
        }
        final entry = filtered[index - headerCount];
        final kind = entry['kind'] as String? ?? 'device_merge';
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

  Future<void> _resolveMerge(
    Map<String, dynamic> entry,
    String canonicalId,
  ) async {
    debugPrint('TriageScreen: _resolveMerge called (busy=$_busy, entryId=${entry['id']}, canonicalId=$canonicalId)');
    if (_busy) return;
    setState(() => _busy = true);
    try {
      final entryId = entry['id']?.toString() ?? '';
      final http = context.read<ServerSyncProvider>().httpClient;
      await http.resolveTriageMerge(entryId, canonicalId);
      if (mounted) await _loadEntries();
    } catch (e) {
      debugPrint('TriageScreen: merge failed: $e');
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Merge failed: $e'), backgroundColor: Colors.red.shade800),
        );
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _resolveNew(Map<String, dynamic> entry) async {
    debugPrint('TriageScreen: _resolveNew called (busy=$_busy, entryId=${entry['id']})');
    if (_busy) return;
    setState(() => _busy = true);
    try {
      final entryId = entry['id']?.toString() ?? '';
      final http = context.read<ServerSyncProvider>().httpClient;
      await http.resolveTriageNew(entryId);
      if (mounted) await _loadEntries();
    } catch (e) {
      debugPrint('TriageScreen: keep-separate failed: $e');
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Keep separate failed: $e'), backgroundColor: Colors.red.shade800),
        );
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _resolveBind(Map<String, dynamic> entry, {String? targetRoomId}) async {
    debugPrint('TriageScreen: _resolveBind called (busy=$_busy, entryId=${entry['id']}, targetRoomId=$targetRoomId)');
    if (_busy) return;
    setState(() => _busy = true);
    try {
      final entryId = entry['id']?.toString() ?? '';
      final http = context.read<ServerSyncProvider>().httpClient;
      await http.resolveTriageBind(entryId, targetRoomId: targetRoomId);
      if (mounted) await _loadEntries();
    } catch (e) {
      debugPrint('TriageScreen: bind failed: $e');
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Room merge failed: $e'), backgroundColor: Colors.red.shade800),
        );
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _resolveDismiss(Map<String, dynamic> entry) async {
    debugPrint('TriageScreen: _resolveDismiss called (busy=$_busy, entryId=${entry['id']})');
    if (_busy) return;
    setState(() => _busy = true);
    try {
      final entryId = entry['id']?.toString() ?? '';
      final http = context.read<ServerSyncProvider>().httpClient;
      await http.resolveTriageDismiss(entryId);
      if (mounted) await _loadEntries();
    } catch (e) {
      debugPrint('TriageScreen: dismiss failed: $e');
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Dismiss failed: $e'), backgroundColor: Colors.red.shade800),
        );
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }
}

// =============================================================================
// Device merge card (existing)
// =============================================================================

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
    final discovered =
        (entry['discovered'] as Map<String, dynamic>?) ?? {};
    final name = discovered['name'] as String? ?? 'Unknown Device';
    final deviceType = discovered['device_type'] as String? ?? 'light';
    final manufacturer = discovered['manufacturer'] as String?;
    final model = discovered['model'] as String?;
    final roomName = discovered['room_name'] as String? ?? '';
    final candidates =
        (entry['candidate_matches'] as List<dynamic>?) ?? [];

    final typeLabel = switch (deviceType) {
      'light' => 'Light',
      'button' => 'Button',
      'motion' => 'Motion Sensor',
      _ => deviceType,
    };

    final productInfo = [manufacturer, model]
        .whereType<String>()
        .join(' \u00B7 ');

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
                        color: CelestialColors.textSecondary.withValues(alpha: 0.7),
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
            for (final candidate in candidates)
              _buildCandidateRow(candidate),
          ],
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
                        debugPrint('_TriageCard: Keep Separate tapped (id=${entry['id']})');
                        onKeepSeparate();
                      },
                    ),
                  ),
                  const SizedBox(width: 8),
                  _ActionButton(
                    label: 'Dismiss',
                    color: CelestialColors.textSecondary,
                    onTap: () {
                      debugPrint('_TriageCard: Dismiss tapped (id=${entry['id']})');
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
                        color: CelestialColors.textSecondary.withValues(alpha: 0.6),
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
              onTap: busy ? null : () {
                debugPrint('_TriageCard: Merge tapped (id=${entry['id']}, cid=$cid)');
                onMerge(cid);
              },
              child: Container(
                padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
                decoration: BoxDecoration(
                  color: CelestialColors.sunWarm.withValues(alpha: 0.12),
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(color: CelestialColors.sunWarm.withValues(alpha: 0.3)),
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
    final hubInfo = [hubLabel, hubAddr]
        .where((s) => s.isNotEmpty)
        .join(' \u00B7 ');

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
                          color: CelestialColors.textSecondary.withValues(alpha: 0.7),
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
                        onTap: () => onBind(targetRoomId: targetId.isNotEmpty ? targetId : null),
                      ),
                    ),
                  if (!hasMultipleCandidates)
                    const SizedBox(width: 8),
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
                padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
                decoration: BoxDecoration(
                  color: CelestialColors.sunWarm.withValues(alpha: 0.12),
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(color: CelestialColors.sunWarm.withValues(alpha: 0.3)),
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
