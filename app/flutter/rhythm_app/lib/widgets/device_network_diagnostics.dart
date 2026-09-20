import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../services/analytics_service.dart';
import 'solar_orbit.dart';

/// Read-only, allowlisted projection of the canonical device registry. Discovery
/// presence and cached hub connectivity deliberately do not imply reachability.
class DeviceNetworkDiagnostics extends StatefulWidget {
  final Map<String, dynamic>? device;
  final List<Map<String, dynamic>> hubs;
  final DateTime? fetchedAt;
  final bool refreshing;
  final bool failed;
  final VoidCallback? onRefresh;

  const DeviceNetworkDiagnostics({
    super.key,
    required this.device,
    required this.hubs,
    required this.fetchedAt,
    required this.refreshing,
    required this.failed,
    required this.onRefresh,
  });

  @override
  State<DeviceNetworkDiagnostics> createState() =>
      _DeviceNetworkDiagnosticsState();
}

class _DeviceNetworkDiagnosticsState extends State<DeviceNetworkDiagnostics> {
  bool _copying = false;

  Future<void> _copy(String report) async {
    if (_copying) return;
    setState(() => _copying = true);
    var succeeded = false;
    try {
      await Clipboard.setData(ClipboardData(text: report));
      succeeded = true;
    } catch (_) {
      // Clipboard permissions/availability must not hide the diagnostics.
    }
    unawaited(AnalyticsService().logDeviceNetworkCopyCompleted(
      outcome: succeeded ? 'succeeded' : 'failed',
    ));
    if (!mounted) return;
    setState(() => _copying = false);
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(
      content: Text(succeeded
          ? 'Network diagnostics copied'
          : 'Could not copy network diagnostics'),
    ));
  }

  @override
  Widget build(BuildContext context) {
    final device = widget.device;
    final endpoints = _maps(device?['endpoints']);
    final identifiers = <(String, String)>[
      ('Rhythm ID', _value(device?['id'])),
      for (final id in _maps(device?['hardware_ids']))
        if (_hardwareLabel(id['type']) case final String label)
          (label, _value(id['value'])),
    ];
    final connections = [
      for (final endpoint in endpoints)
        _ConnectionDetails(endpoint, widget.hubs),
    ];
    final fetched = widget.fetchedAt == null
        ? 'Not loaded'
        : _formatTime(widget.fetchedAt!);
    const evidenceHint = 'Discovery records are not a live reachability test. '
        'Hub status is from the latest server update.';
    final report = [
      'Device network diagnostics',
      'Details fetched: $fetched',
      if (widget.failed) 'Refresh failed; showing the last loaded snapshot.',
      evidenceHint,
      for (final (label, value) in identifiers) '$label: $value',
      if (connections.isEmpty) 'No connections reported.',
      for (final connection in connections) ...[
        '',
        connection.title,
        for (final (label, value) in connection.rows) '$label: $value',
      ],
    ].join('\n');

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Wrap(
          alignment: WrapAlignment.spaceBetween,
          children: [
            TextButton.icon(
              key: const ValueKey('device-network-refresh'),
              onPressed: widget.refreshing ? null : widget.onRefresh,
              icon: widget.refreshing
                  ? const SizedBox(
                      width: 18,
                      height: 18,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    )
                  : const Icon(Icons.refresh, size: 18),
              label: Text(widget.refreshing ? 'Refreshing…' : 'Refresh'),
            ),
            TextButton.icon(
              key: const ValueKey('device-network-copy'),
              onPressed: device == null || widget.refreshing || _copying
                  ? null
                  : () => _copy(report),
              icon: const Icon(Icons.copy_outlined, size: 18),
              label: const Text('Copy diagnostics'),
            ),
          ],
        ),
        if (widget.failed)
          Padding(
            padding: const EdgeInsets.only(bottom: 8),
            child: Text(
              device == null
                  ? 'Could not load connection details. Tap Refresh to retry.'
                  : 'Could not refresh. Showing the last loaded snapshot.',
              key: const ValueKey('device-network-error'),
              style: const TextStyle(color: CelestialColors.warning),
            ),
          ),
        if (device != null) ...[
          Text('Details fetched: $fetched',
              style: const TextStyle(
                  color: CelestialColors.textSecondary, fontSize: 11)),
          const SizedBox(height: 8),
          if (connections.isEmpty)
            const Padding(
              padding: EdgeInsets.symmetric(vertical: 12),
              child: Text('No connections reported.'),
            ),
          for (final (index, connection) in connections.indexed)
            _Disclosure(
              storageKey: 'connection-$index-${connection.identity}',
              title: connection.title,
              subtitle: connection.summary,
              rows: connection.rows,
            ),
          _Disclosure(
            storageKey: 'device-network-identifiers',
            title: 'Device identifiers',
            rows: identifiers,
          ),
          const Padding(
            padding: EdgeInsets.only(top: 8),
            child: Text(evidenceHint,
                style: TextStyle(
                    color: CelestialColors.textSecondary, fontSize: 12)),
          ),
        ],
      ],
    );
  }
}

List<Map> _maps(Object? value) =>
    value is List ? value.whereType<Map>().toList() : const [];

String _value(Object? value) =>
    value is String && value.trim().isNotEmpty ? value.trim() : 'Not reported';

String? _hardwareLabel(Object? type) => switch (type) {
      'mac' => 'MAC / IEEE address',
      'serial' => 'Serial number',
      'matter_id' => 'Matter ID',
      _ => null,
    };

String _formatTime(DateTime date) =>
    '${date.toUtc().toIso8601String().split('.').first.replaceFirst('T', ' ')} UTC';

String _lastSeen(Object? value) {
  // Canonical last_seen is Unix seconds, not milliseconds. Zero is unknown.
  if (value is! num || !value.isFinite || value <= 0 || value > 253402300799) {
    return 'Not reported';
  }
  return _formatTime(
      DateTime.fromMillisecondsSinceEpoch((value * 1000).toInt(), isUtc: true));
}

class _ConnectionDetails {
  final Map endpoint;
  final List<Map<String, dynamic>> hubs;

  _ConnectionDetails(this.endpoint, this.hubs);

  Map get hubKey =>
      endpoint['hub_key'] is Map ? endpoint['hub_key'] as Map : {};
  String get type => _value(hubKey['hub_type']);
  String get address => _value(hubKey['address']);
  String get identity => '$type/$address/${_value(endpoint['native_id'])}';
  String get title => switch (type) {
        'hue' => 'Hue Bridge',
        'hue_ble' => 'Hue Bluetooth',
        'local_ble' => 'Local Bluetooth',
        'ha' || 'homeassistant' || 'home_assistant' => 'Home Assistant',
        'matter' => 'Matter',
        'Not reported' => 'Unknown connection',
        _ => type,
      };

  bool? get active => !endpoint.containsKey('active')
      ? true // The canonical endpoint wire contract omits active=true.
      : endpoint['active'] is bool
          ? endpoint['active'] as bool
          : null;

  String get summary => [
        if (endpoint['preferred'] == true) 'Preferred',
        if (active == false) 'Missing from discovery',
        if (active == null) 'Discovery unknown',
        if (active == true) 'Seen in last discovery',
      ].join(' · ');

  String get hubStatus {
    // Never borrow another bridge's status when the exact owner is absent.
    for (final hub in hubs) {
      if (hub['type'] == hubKey['hub_type'] &&
          hub['address'] == hubKey['address'] &&
          hubKey['address'] is String) {
        return switch (hub['connected']) {
          true => 'Connected',
          false => 'Disconnected',
          _ => 'Not reported',
        };
      }
    }
    return 'Not reported';
  }

  List<(String, String)> get rows => [
        ('Integration', title),
        ('Hub address', address),
        ('Native device ID', _value(endpoint['native_id'])),
        (
          'Preferred connection',
          switch (endpoint['preferred']) {
            true => 'Yes',
            false => 'No',
            _ => 'Not reported',
          }
        ),
        (
          'Discovery',
          switch (active) {
            true => 'Present in last discovery',
            false => 'Missing from last discovery',
            _ => 'Not reported',
          }
        ),
        ('Last seen by integration', _lastSeen(endpoint['last_seen'])),
        ('Hub status (last reported)', hubStatus),
      ];
}

class _Disclosure extends StatelessWidget {
  final String storageKey;
  final String title;
  final String? subtitle;
  final List<(String, String)> rows;

  const _Disclosure({
    required this.storageKey,
    required this.title,
    this.subtitle,
    required this.rows,
  });

  @override
  Widget build(BuildContext context) => Card(
        margin: const EdgeInsets.only(bottom: 6),
        color: CelestialColors.backgroundDark.withValues(alpha: 0.5),
        elevation: 0,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(14),
          side: BorderSide(
            color: CelestialColors.orbitRing.withValues(alpha: 0.3),
          ),
        ),
        child: ExpansionTile(
          shape: const Border(),
          collapsedShape: const Border(),
          key: PageStorageKey(storageKey),
          tilePadding: const EdgeInsets.symmetric(horizontal: 16),
          title: Text(title,
              style: const TextStyle(
                  color: CelestialColors.textPrimary, fontSize: 14)),
          subtitle: subtitle == null
              ? null
              : Text(subtitle!,
                  style: const TextStyle(
                      color: CelestialColors.textSecondary, fontSize: 12)),
          children: [
            for (final (label, value) in rows)
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 4, 16, 12),
                child: SizedBox(
                  width: double.infinity,
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(label,
                          style: const TextStyle(
                              color: CelestialColors.textSecondary,
                              fontSize: 12)),
                      const SizedBox(height: 3),
                      SelectionArea(
                        child: Text(value,
                            style: const TextStyle(
                                color: CelestialColors.textPrimary,
                                fontSize: 14)),
                      ),
                    ],
                  ),
                ),
              ),
          ],
        ),
      );
}
