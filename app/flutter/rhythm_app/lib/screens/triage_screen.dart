import 'dart:async';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import '../providers/room_provider.dart';
import '../providers/server_sync_provider.dart';
import '../services/analytics_service.dart';
import '../services/hue_ble_auto_discovery_service.dart';
import '../widgets/nearby_hue_ble_prompt.dart';
import '../widgets/room_picker_sheet.dart';
import '../widgets/settings_row.dart';
import '../widgets/solar_orbit.dart'; // For CelestialColors
import 'hubs/device_pairing_flow.dart';
import 'hubs/hue_authority_screen.dart';
import 'hubs/rhythmserver_settings_screen.dart';

enum _TriageFilter { all, devices, rooms }

/// Triage resolution screen.
///
/// Shows pending triage entries (device merges and room bindings) and allows
/// the user to merge, keep separate, bind rooms, or dismiss.
class TriageScreen extends StatefulWidget {
  const TriageScreen({
    super.key,
    @visibleForTesting this.hueBleDiscoveryRequest,
  });

  final HueBleDiscoveryRequest? hueBleDiscoveryRequest;

  /// Combined "Add & Review" screen — pair new devices/rooms and resolve any
  /// pending device-review items in one place.
  static Future<void> show(BuildContext context) {
    AnalyticsService().logScreenView('add_review');
    return Navigator.of(context).push(
      MaterialPageRoute(builder: (_) => const TriageScreen()),
    );
  }

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
  RhythmHueAuthority? _hueAuthority;
  String? _reviewingHueBridgeAddress;
  _TriageFilter _filter = _TriageFilter.all;

  @override
  void initState() {
    super.initState();
    AnalyticsService().logScreenView('device_review');
    _loadEntries();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) unawaited(_discoverNearbyHueBle());
    });
  }

  Future<void> _discoverNearbyHueBle() async {
    final syncProvider = context.read<ServerSyncProvider>();
    if (!syncProvider.canAddHueBleDevice) return;

    final discover = widget.hueBleDiscoveryRequest ??
        HueBleAutoDiscoveryService.instance.discover;
    final discovery = await discover(source: 'add_review');
    if (!mounted ||
        !discovery.found ||
        ModalRoute.of(context)?.isCurrent != true) {
      return;
    }

    final accepted = await showNearbyHueBlePrompt(
      context,
      source: 'add_review',
      discovery: discovery,
    );
    if (!accepted || !mounted) return;

    await startDevicePairingFlow(
      context,
      target: DevicePairingTarget.hueBle,
      analyticsSource: 'add_review',
      hueBleInputMethod: 'auto_discovery',
    );
    if (mounted) await _loadEntries();
  }

  String _kindForEntry(Map<String, dynamic> entry) =>
      entry['kind'] as String? ?? 'device_merge';

  bool _isDeviceEntry(Map<String, dynamic> entry) =>
      _deviceKinds.contains(_kindForEntry(entry));

  bool _isRoomEntry(Map<String, dynamic> entry) =>
      _roomKinds.contains(_kindForEntry(entry));

  int get _deviceCount => _entries.where(_isDeviceEntry).length;

  int get _roomCount => _entries.where(_isRoomEntry).length;

  List<RhythmHueBridgeAuthority> get _hueBridgesNeedingReview =>
      _hueAuthority?.bridges
          .where(
            (bridge) => bridge.rooms.any(
              (room) => room.owner == RhythmHueRoomAuthorityOwner.unreviewed,
            ),
          )
          .toList(growable: false) ??
      const <RhythmHueBridgeAuthority>[];

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
      RhythmHueAuthority? hueAuthority;
      if (syncProvider.hueRoomAuthorityConsentSupported) {
        try {
          hueAuthority = await syncProvider.fetchHueAuthority();
        } catch (error, stackTrace) {
          debugPrint(
            'TriageScreen: Hue authority load failed: $error\n$stackTrace',
          );
        }
      }
      debugPrint('TriageScreen: got ${entries?.length ?? 'null'} entries');
      if (entries != null && entries.isNotEmpty) {
        debugPrint(
            'TriageScreen: first entry keys=${entries.first.keys.toList()}, id=${entries.first['id']} (${entries.first['id'].runtimeType})');
      }
      if (mounted) {
        setState(() {
          _connectionError = entries == null;
          _entries = entries ?? [];
          _hueAuthority = hueAuthority;
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
        title: const Text('Add & Review'),
        elevation: 0,
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
    final hueBridgesNeedingReview = _hueBridgesNeedingReview;

    return ListView(
      padding: const EdgeInsets.all(20),
      children: [
        // Adding new hardware is the primary job of this screen.
        if (serverSync.canScanToAddDevice) ...[
          _buildAddDeviceCard(),
          const SizedBox(height: 30),
        ],
        // Third-party hubs bring in devices that are already paired elsewhere.
        RhythmServerHubManagementSection(
          showConfigured: true,
          showMatterAddOption: true,
          showHueBleAddOption: false,
          addOptionsTitle: 'DEVICE ACTIONS',
          addOptionsSubtitle:
              'Add Matter hardware or refresh devices from every connected hub.',
          resyncLabel: 'Sync All Hubs',
          resyncBusyLabel: 'Syncing…',
          resyncTrailingLabel: 'Refresh paired hardware',
          onResynced: () => _loadEntries(),
        ),
        const SizedBox(height: 30),
        _sectionHeading(
          'Create a Room',
          subtitle: 'Create a Rhythm room for organizing your devices.',
        ),
        const SizedBox(height: 10),
        SettingsGroup(
          children: [
            SettingsRow(
              icon: Icons.add_home_outlined,
              iconColor: const Color(0xFF7C83FF),
              label: 'Add a Room',
              onTap: () => _addRoom(),
            ),
          ],
        ),
        const SizedBox(height: 30),
        // ── Review ──────────────────────────────────────────────────────
        if (hueBridgesNeedingReview.isNotEmpty) ...[
          _sectionHeading(
            'Automation Review',
            subtitle:
                'Choose who controls rooms on newly connected Hue bridges.',
          ),
          const SizedBox(height: 10),
          SettingsGroup(
            children: [
              for (final bridge in hueBridgesNeedingReview)
                SettingsRow(
                  icon: Icons.admin_panel_settings_outlined,
                  iconColor: const Color(0xFFFFB900),
                  label: _reviewingHueBridgeAddress == bridge.address
                      ? 'Loading Hue room automation…'
                      : 'Review Hue room automation',
                  value: _hueReviewSummary(bridge),
                  onTap: _reviewingHueBridgeAddress == null
                      ? () => _reviewHueAutomation(bridge)
                      : null,
                ),
            ],
          ),
          const SizedBox(height: 30),
        ],
        _sectionLabel('Device Review'),
        const SizedBox(height: 12),
        if (hasBothKinds) ...[
          _buildFilterBar(),
          const SizedBox(height: 16),
        ],
        Padding(
          padding: EdgeInsets.only(bottom: filtered.isEmpty ? 0 : 18),
          child: Text(
            _summaryLabel(serverSync, filtered.length),
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.7),
              fontSize: 14,
            ),
          ),
        ),
        for (final entry in filtered) _entryCard(entry, serverSync),
      ],
    );
  }

  String _hueReviewSummary(RhythmHueBridgeAuthority bridge) {
    final roomCount = bridge.rooms
        .where(
          (room) => room.owner == RhythmHueRoomAuthorityOwner.unreviewed,
        )
        .length;
    return '$roomCount ${roomCount == 1 ? 'room needs' : 'rooms need'} review';
  }

  Future<void> _reviewHueAutomation(RhythmHueBridgeAuthority bridge) async {
    if (_reviewingHueBridgeAddress != null) return;
    setState(() => _reviewingHueBridgeAddress = bridge.address);
    final reviewed = await HueAuthorityScreen.show(
      context,
      bridge,
      source: 'add_review',
    );
    if (!mounted) return;
    setState(() => _reviewingHueBridgeAddress = null);
    if (reviewed == true) {
      await _loadEntries();
    }
  }

  Widget _sectionLabel(String text) => Padding(
        padding: const EdgeInsets.only(left: 4, bottom: 2),
        child: Text(
          text.toUpperCase(),
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.6),
            fontSize: 12,
            fontWeight: FontWeight.w600,
            letterSpacing: 1.2,
          ),
        ),
      );

  Widget _sectionHeading(String title, {String? subtitle}) => Padding(
        padding: const EdgeInsets.symmetric(horizontal: 4),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              title.toUpperCase(),
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.6),
                fontSize: 12,
                fontWeight: FontWeight.w600,
                letterSpacing: 1.2,
              ),
            ),
            if (subtitle != null) ...[
              const SizedBox(height: 4),
              Text(
                subtitle,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.58),
                  fontSize: 13,
                  height: 1.35,
                ),
              ),
            ],
          ],
        ),
      );

  Widget _buildAddDeviceCard() {
    const accent = Color(0xFF26A69A);
    const accentBright = Color(0xFF4DD0C8);
    return Semantics(
      key: const ValueKey('add-review-scan-device'),
      button: true,
      excludeSemantics: true,
      label: 'Scan to Add Device',
      hint: 'Scan a setup code or find nearby Hue Bluetooth bulbs',
      onTap: _scanToAddDevice,
      child: Material(
        color: Colors.transparent,
        child: Ink(
          decoration: BoxDecoration(
            gradient: LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: [
                accent.withValues(alpha: 0.30),
                CelestialColors.accentBlue.withValues(alpha: 0.13),
              ],
            ),
            borderRadius: BorderRadius.circular(18),
            border: Border.all(
              color: accentBright.withValues(alpha: 0.72),
              width: 1.4,
            ),
            boxShadow: [
              BoxShadow(
                color: accent.withValues(alpha: 0.16),
                blurRadius: 22,
                spreadRadius: 1,
              ),
            ],
          ),
          child: InkWell(
            borderRadius: BorderRadius.circular(18),
            onTap: _scanToAddDevice,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(18, 17, 16, 17),
              child: Row(
                children: [
                  Container(
                    width: 54,
                    height: 54,
                    decoration: BoxDecoration(
                      color: accentBright,
                      borderRadius: BorderRadius.circular(16),
                      boxShadow: [
                        BoxShadow(
                          color: accent.withValues(alpha: 0.36),
                          blurRadius: 14,
                        ),
                      ],
                    ),
                    child: const Icon(
                      Icons.qr_code_scanner_rounded,
                      color: Color(0xFF092D2A),
                      size: 28,
                    ),
                  ),
                  const SizedBox(width: 16),
                  const Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          'NEW HARDWARE',
                          style: TextStyle(
                            color: accentBright,
                            fontSize: 10,
                            fontWeight: FontWeight.w700,
                            letterSpacing: 1.2,
                          ),
                        ),
                        SizedBox(height: 3),
                        Text(
                          'Add a Device',
                          style: TextStyle(
                            color: CelestialColors.textPrimary,
                            fontSize: 18,
                            fontWeight: FontWeight.w700,
                            letterSpacing: -0.25,
                          ),
                        ),
                        SizedBox(height: 5),
                        Text(
                          'Scan a code or find nearby bulbs',
                          style: TextStyle(
                            color: CelestialColors.textSecondary,
                            fontSize: 13,
                            height: 1.3,
                          ),
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(width: 10),
                  Container(
                    width: 32,
                    height: 32,
                    decoration: BoxDecoration(
                      color: accentBright.withValues(alpha: 0.16),
                      shape: BoxShape.circle,
                    ),
                    child: const Icon(
                      Icons.arrow_forward_rounded,
                      color: accentBright,
                      size: 19,
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }

  Future<void> _scanToAddDevice() async {
    await startDevicePairingFlow(
      context,
      analyticsSource: 'add_review',
    );
    if (mounted) {
      await _loadEntries();
    }
  }

  Future<void> _addRoom() async {
    final room = await createTopologyRoomOptionFromPrompt(context);
    if (room != null && mounted) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Added room "${room.name}"')),
      );
    }
  }

  Widget _entryCard(Map<String, dynamic> entry, ServerSyncProvider serverSync) {
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
          onUseStandalone: () => _resolveNew(entry),
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
  }

  String _summaryLabel(ServerSyncProvider serverSync, int filteredCount) {
    if (_entries.isEmpty) {
      return 'No devices need your attention right now.';
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
    final rooms =
        context.read<RoomProvider>().rooms.where((room) => room.kind.isRoom);
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
      onCreateRoom: () async =>
          (await createTopologyRoomOptionFromPrompt(context))?.id,
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
  final VoidCallback onUseStandalone;
  final VoidCallback onDismiss;

  const _UnassignedDeviceCard({
    super.key,
    required this.entry,
    this.busy = false,
    required this.onAssignRoom,
    required this.onUseStandalone,
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
            'This device does not have a Rhythm room yet. Rhythm will leave it paused until you assign it or explicitly use it standalone.',
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
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Expanded(
                        child: _ActionButton(
                          label: 'Assign Room',
                          color: const Color(0xFF81C784),
                          onTap: onAssignRoom,
                        ),
                      ),
                      const SizedBox(width: 8),
                      Expanded(
                        child: _ActionButton(
                          label: 'Use Standalone',
                          color: CelestialColors.accentBlue,
                          onTap: onUseStandalone,
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 8),
                  _ActionButton(
                    label: 'Ignore Device',
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
      'hue_ble' => 'Hue Bluetooth',
      'local_ble' => 'Local Bluetooth',
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
