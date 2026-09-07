import 'package:flutter/material.dart';

import '../services/analytics_service.dart';
import '../services/nearby_ble_discovery_service.dart';
import 'solar_orbit.dart';

/// Scans nearby Bluetooth for every family the appliance can onboard without a
/// QR code and lets the person pick one. Returns the chosen family, or null.
Future<NearbyBleFamily?> showNearbyDeviceSheet(
  BuildContext context, {
  required Set<NearbyBleFamily> families,
  required String source,
  NearbyBleDiscoveryRequest? discoveryRequest,
}) {
  return showModalBottomSheet<NearbyBleFamily>(
    context: context,
    isScrollControlled: true,
    backgroundColor: CelestialColors.backgroundCard,
    shape: const RoundedRectangleBorder(
      borderRadius: BorderRadius.vertical(top: Radius.circular(24)),
    ),
    builder: (_) => NearbyDeviceSheet(
      families: families,
      source: source,
      discoveryRequest: discoveryRequest,
    ),
  );
}

class NearbyDeviceSheet extends StatefulWidget {
  const NearbyDeviceSheet({
    super.key,
    required this.families,
    required this.source,
    this.discoveryRequest,
  });

  final Set<NearbyBleFamily> families;
  final String source;
  final NearbyBleDiscoveryRequest? discoveryRequest;

  @override
  State<NearbyDeviceSheet> createState() => _NearbyDeviceSheetState();
}

class _NearbyDeviceSheetState extends State<NearbyDeviceSheet> {
  static const _teal = Color(0xFF26C6DA);

  NearbyBleDiscoveryResult? _result;
  bool _scanning = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _scan());
  }

  Future<void> _scan() async {
    if (_scanning) return;
    setState(() {
      _scanning = true;
      _result = null;
    });
    final discover =
        widget.discoveryRequest ?? NearbyBleDiscoveryService.instance.discover;
    final result = await discover(
      source: widget.source,
      families: widget.families,
    );
    if (!mounted) return;
    setState(() {
      _scanning = false;
      _result = result;
    });
  }

  void _choose(NearbyBleFamily family) {
    AnalyticsService().logNearbyDeviceFamilySelected(
      source: widget.source,
      family: family.id,
      deviceCount: _result?.countFor(family) ?? 0,
    );
    Navigator.of(context).pop(family);
  }

  String _emptyMessage(NearbyBleDiscoveryOutcome outcome) => switch (outcome) {
        NearbyBleDiscoveryOutcome.permissionDenied =>
          'Rhythm needs Bluetooth permission to look for nearby devices.',
        NearbyBleDiscoveryOutcome.bluetoothUnavailable =>
          'Turn on Bluetooth to look for nearby devices.',
        NearbyBleDiscoveryOutcome.unsupported =>
          'This device cannot scan for nearby Bluetooth devices.',
        NearbyBleDiscoveryOutcome.failed =>
          'The Bluetooth scan did not finish. Try again.',
        _ => 'No supported device is in setup mode nearby. Put the bulb or '
            'strip in pairing mode, keep your phone close, and scan again.',
      };

  @override
  Widget build(BuildContext context) {
    final result = _result;
    final found = result?.families ?? const <NearbyBleFamily>[];
    return SafeArea(
      top: false,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(22, 18, 22, 22),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                const Icon(Icons.bluetooth_searching_rounded, color: _teal),
                const SizedBox(width: 10),
                const Expanded(
                  child: Text(
                    'Nearby devices',
                    style: TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 20,
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                ),
                IconButton(
                  key: const ValueKey('nearby-device-sheet-close'),
                  tooltip: 'Close',
                  onPressed: () => Navigator.of(context).pop(),
                  icon: const Icon(Icons.close_rounded,
                      color: CelestialColors.textSecondary),
                ),
              ],
            ),
            const SizedBox(height: 6),
            Text(
              'Rhythm looks for devices that can be added without a QR code. '
              'Only what your Rhythm Box supports is listed.',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.85),
                fontSize: 13,
                height: 1.4,
              ),
            ),
            const SizedBox(height: 18),
            if (_scanning || result == null)
              const Padding(
                key: ValueKey('nearby-device-sheet-scanning'),
                padding: EdgeInsets.symmetric(vertical: 24),
                child: Center(
                  child: Column(
                    children: [
                      SizedBox(
                        width: 28,
                        height: 28,
                        child: CircularProgressIndicator(strokeWidth: 3),
                      ),
                      SizedBox(height: 12),
                      Text(
                        'Looking nearby…',
                        style: TextStyle(color: CelestialColors.textSecondary),
                      ),
                    ],
                  ),
                ),
              )
            else if (found.isEmpty) ...[
              Text(
                _emptyMessage(result.outcome),
                key: const ValueKey('nearby-device-sheet-empty'),
                style: const TextStyle(
                  color: CelestialColors.textSecondary,
                  height: 1.4,
                ),
              ),
              const SizedBox(height: 16),
              SizedBox(
                width: double.infinity,
                child: FilledButton.tonalIcon(
                  key: const ValueKey('nearby-device-sheet-rescan'),
                  onPressed: _scan,
                  icon: const Icon(Icons.refresh_rounded),
                  label: const Text('Scan Again'),
                ),
              ),
            ] else ...[
              for (final family in found)
                Padding(
                  padding: const EdgeInsets.only(bottom: 10),
                  child: Material(
                    color: CelestialColors.backgroundDark,
                    borderRadius: BorderRadius.circular(16),
                    child: InkWell(
                      key: ValueKey('nearby-device-family-${family.id}'),
                      borderRadius: BorderRadius.circular(16),
                      onTap: () => _choose(family),
                      child: Padding(
                        padding: const EdgeInsets.all(16),
                        child: Row(
                          children: [
                            Icon(
                              family == NearbyBleFamily.hueBle
                                  ? Icons.lightbulb_rounded
                                  : Icons.light_mode_rounded,
                              color: _teal,
                            ),
                            const SizedBox(width: 14),
                            Expanded(
                              child: Column(
                                crossAxisAlignment: CrossAxisAlignment.start,
                                children: [
                                  Text(
                                    family.countLabel(result.countFor(family)),
                                    style: const TextStyle(
                                      color: CelestialColors.textPrimary,
                                      fontWeight: FontWeight.w600,
                                      fontSize: 15,
                                    ),
                                  ),
                                  const SizedBox(height: 3),
                                  Text(
                                    family.hint,
                                    style: TextStyle(
                                      color: CelestialColors.textSecondary
                                          .withValues(alpha: 0.85),
                                      fontSize: 12.5,
                                      height: 1.35,
                                    ),
                                  ),
                                ],
                              ),
                            ),
                            const Icon(
                              Icons.chevron_right_rounded,
                              color: CelestialColors.textSecondary,
                            ),
                          ],
                        ),
                      ),
                    ),
                  ),
                ),
              TextButton.icon(
                key: const ValueKey('nearby-device-sheet-rescan'),
                onPressed: _scan,
                icon: const Icon(Icons.refresh_rounded, size: 18),
                label: const Text('Scan Again'),
              ),
            ],
          ],
        ),
      ),
    );
  }
}
