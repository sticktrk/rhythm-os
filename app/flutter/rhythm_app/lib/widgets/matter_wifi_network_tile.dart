import 'package:flutter/material.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import '../services/cloud_backed_server_api.dart';
import 'solar_orbit.dart';

/// SSIDs stay inside this owner-only view. A failed refresh clears old evidence.
class MatterWifiNetworkTile extends StatefulWidget {
  const MatterWifiNetworkTile(
      {super.key,
      required this.api,
      required this.deviceId,
      this.changing = false,
      this.refreshToken = 0,
      this.onTap});
  final CloudBackedServerApi api;
  final String deviceId;
  final bool changing;
  final int refreshToken;
  final Future<void> Function()? onTap;
  @override
  State<MatterWifiNetworkTile> createState() => _MatterWifiNetworkTileState();
}

class _MatterWifiNetworkTileState extends State<MatterWifiNetworkTile> {
  RhythmWifiNetwork? _network;
  String? _error;
  bool _loading = true;
  int _generation = 0;
  @override
  void initState() {
    super.initState();
    _read();
  }

  @override
  void didUpdateWidget(covariant MatterWifiNetworkTile oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.deviceId != widget.deviceId ||
        oldWidget.api != widget.api ||
        oldWidget.changing != widget.changing ||
        oldWidget.refreshToken != widget.refreshToken) {
      _read();
    }
  }

  Future<void> _read() async {
    final generation = ++_generation;
    setState(() {
      _network = null;
      _error = null;
      _loading = !widget.changing;
    });
    if (widget.changing) return;
    RhythmWifiNetwork? network;
    String? error;
    try {
      network = await widget.api.getMatterWifiNetwork(widget.deviceId);
    } on RhythmWifiException catch (e) {
      error = e.category == 'owner_required'
          ? 'Owner access required to read Wi-Fi.'
          : 'Could not read the bulb’s Wi-Fi.';
    } catch (_) {
      error = 'Could not read the bulb’s Wi-Fi.';
    }
    if (!mounted || generation != _generation) return;
    setState(() {
      _network = network;
      _error = error;
      _loading = false;
    });
  }

  String get _description {
    if (widget.changing) return 'Changing network — waiting for the bulb.';
    if (_loading) return 'Reading from bulb…';
    if (_error != null) return _error!;
    return switch (_network?.status) {
      'connected' => _network!.ssid!,
      'offline' => 'Bulb unreachable — current network unknown.',
      'unsupported' => 'This device does not report a Wi-Fi network.',
      'unsupported_server' =>
        'Update the Rhythm Box to see the current network.',
      'busy' => 'Network setup is in progress. Check again shortly.',
      _ => 'Current network unavailable.',
    };
  }

  @override
  Widget build(BuildContext context) => Material(
      color: CelestialColors.backgroundCard,
      borderRadius: BorderRadius.circular(14),
      clipBehavior: Clip.antiAlias,
      child: ListTile(
        key: const ValueKey('bulb-current-wifi'),
        contentPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
        leading: const Icon(Icons.wifi_rounded, color: CelestialColors.sunWarm),
        title: const Text('Current Wi-Fi',
            style: TextStyle(color: CelestialColors.textPrimary)),
        subtitle: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(_description,
                  key: const ValueKey('bulb-current-wifi-value'),
                  style: const TextStyle(color: CelestialColors.textSecondary)),
              if (_network?.status == 'connected')
                Text(
                    _network?.observedAtMs == null
                        ? 'Reported by bulb'
                        : 'Reported by bulb at ${TimeOfDay.fromDateTime(DateTime.fromMillisecondsSinceEpoch(_network!.observedAtMs!)).format(context)}',
                    style: const TextStyle(
                        color: CelestialColors.textSecondary, fontSize: 12)),
              if (widget.onTap != null)
                const Text('Change network',
                    style: TextStyle(color: CelestialColors.sunWarm)),
            ]),
        onTap: widget.onTap == null
            ? null
            : () async {
                await widget.onTap!();
                if (mounted) await _read();
              },
        trailing: IconButton(
            tooltip: 'Refresh current Wi-Fi',
            onPressed: _loading || widget.changing ? null : _read,
            icon: const Icon(Icons.refresh,
                color: CelestialColors.textSecondary)),
      ));
}
