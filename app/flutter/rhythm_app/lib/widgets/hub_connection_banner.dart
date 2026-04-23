import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show RhythmConnectionState, RhythmHubInfo, RhythmHubStartupRetryStatus;
import '../providers/home_provider.dart';
import '../providers/server_sync_provider.dart';
import '../services/analytics_service.dart';
import '../screens/hubs/rhythmserver_settings_screen.dart';

/// Compact banner shown when the server is connected but one or more
/// configured hubs failed to connect.  Tapping navigates to server settings;
/// the close button dismisses until the hub set changes or all hubs reconnect.
class HubConnectionBanner extends StatefulWidget {
  const HubConnectionBanner({super.key});

  @override
  State<HubConnectionBanner> createState() => _HubConnectionBannerState();
}

class _HubConnectionBannerState extends State<HubConnectionBanner>
    with SingleTickerProviderStateMixin {
  /// Cached hub snapshot — only updated when we get a non-empty list,
  /// so transient clears during reconnect don't cause flicker.
  List<Map<String, dynamic>> _stableHubs = [];

  late AnimationController _pulseController;
  late Animation<double> _pulse;

  @override
  void initState() {
    super.initState();
    _pulseController = AnimationController(
      duration: const Duration(milliseconds: 1800),
      vsync: this,
    )..repeat(reverse: true);
    _pulse = Tween<double>(begin: 0.35, end: 1.0).animate(
      CurvedAnimation(parent: _pulseController, curve: Curves.easeInOut),
    );
  }

  @override
  void dispose() {
    _pulseController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    // Only rebuild when the hub info list actually changes — not on every
    // ServerSyncProvider notification (triage, power save, room tick, etc.).
    final hubs = context.select<ServerSyncProvider, List<Map<String, dynamic>>>(
      (p) => p.serverHubInfos,
    );

    final configured =
        hubs.where((h) => h['type'] != null && h['type'] != 'none').toList();
    final retrying = context.select<ServerSyncProvider, bool>(
      (p) =>
          p.connectionState == RhythmConnectionState.connecting ||
          p.connectionState == RhythmConnectionState.reconnecting,
    );

    // Only accept non-empty snapshots — the provider clears hub infos
    // transiently during reconnect cycles (see _onConnectionStateChanged).
    if (configured.isNotEmpty) {
      _stableHubs = configured;
    }

    if (_stableHubs.isEmpty) return const SizedBox.shrink();

    final disconnected =
        _stableHubs.where((h) => h['connected'] != true).toList();
    final visible = disconnected.isNotEmpty;

    return AnimatedOpacity(
      opacity: visible ? 1.0 : 0.0,
      duration: const Duration(milliseconds: 250),
      curve: Curves.easeOut,
      child: IgnorePointer(
        ignoring: !visible,
        child: AnimatedSize(
          duration: const Duration(milliseconds: 300),
          curve: Curves.easeOutCubic,
          alignment: Alignment.topCenter,
          child: visible
              ? _buildBanner(disconnected, retrying)
              : const SizedBox(width: double.infinity, height: 0),
        ),
      ),
    );
  }

  Widget _buildBanner(
    List<Map<String, dynamic>> disconnected,
    bool busy,
  ) {
    final count = disconnected.length;
    final manualRetryRequired =
        disconnected.where(_hubNeedsManualRetry).toList(growable: false);
    final retryingAutomatically =
        disconnected.where(_hubIsAutoRetrying).toList(growable: false);
    final names = disconnected.map((h) => _hubLabel(h['type'] as String));
    final headline = switch ((
      manualRetryRequired.isNotEmpty,
      retryingAutomatically.length == count && count > 0,
    )) {
      (true, _) =>
        count == 1 ? '${names.first} needs retry' : '$count hubs need retry',
      (false, true) =>
        count == 1 ? '${names.first} reconnecting' : '$count hubs reconnecting',
      _ =>
        count == 1 ? '${names.first} unreachable' : '$count hubs unreachable',
    };
    final retryButtonLabel = manualRetryRequired.isNotEmpty
        ? (busy ? 'Retrying' : 'Retry')
        : retryingAutomatically.isNotEmpty
            ? 'Retrying'
            : (busy ? 'Retrying' : 'Retry');
    final retryButtonEnabled = !busy && retryingAutomatically.isEmpty;

    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
      child: Container(
        decoration: BoxDecoration(
          color: const Color(0xFF1A1708),
          borderRadius: BorderRadius.circular(14),
          border: Border.all(
            color: const Color(0xFFE8A54B).withValues(alpha: 0.25),
          ),
          boxShadow: [
            BoxShadow(
              color: const Color(0xFFE8A54B).withValues(alpha: 0.06),
              blurRadius: 12,
              spreadRadius: 0,
            ),
          ],
        ),
        child: Padding(
          padding: const EdgeInsets.fromLTRB(14, 10, 10, 10),
          child: Row(
            children: [
              Expanded(
                child: GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  onTap: () => _openServerSettings(count),
                  child: Row(
                    children: [
                      // Pulsing amber dot
                      AnimatedBuilder(
                        animation: _pulse,
                        builder: (context, _) {
                          return Container(
                            width: 8,
                            height: 8,
                            decoration: BoxDecoration(
                              shape: BoxShape.circle,
                              color: const Color(0xFFE8A54B)
                                  .withValues(alpha: _pulse.value),
                              boxShadow: [
                                BoxShadow(
                                  color: const Color(0xFFE8A54B)
                                      .withValues(alpha: 0.4 * _pulse.value),
                                  blurRadius: 6,
                                  spreadRadius: 1,
                                ),
                              ],
                            ),
                          );
                        },
                      ),
                      const SizedBox(width: 10),
                      // Headline + hub pills
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            Text(
                              headline,
                              style: const TextStyle(
                                color: Color(0xFFE8A54B),
                                fontSize: 13,
                                fontWeight: FontWeight.w600,
                                letterSpacing: 0.1,
                              ),
                            ),
                            const SizedBox(height: 6),
                            Wrap(
                              spacing: 6,
                              runSpacing: 4,
                              children: [
                                for (final hub in disconnected)
                                  _HubChip(
                                    type: hub['type'] as String,
                                    pulse: _pulse,
                                  ),
                              ],
                            ),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
              ),
              const SizedBox(width: 10),
              _RetryButton(
                busy: busy,
                label: retryButtonLabel,
                onPressed: retryButtonEnabled
                    ? () => _retryHubs(
                          manualRetryRequired: manualRetryRequired,
                          disconnectedCount: count,
                        )
                    : null,
              ),
            ],
          ),
        ),
      ),
    );
  }

  void _openServerSettings(int disconnectedCount) {
    HapticFeedback.lightImpact();
    final homeProvider = context.read<HomeProvider>();
    final serverHub = homeProvider.getFirstHubOfType(HubType.server);
    if (serverHub != null) {
      RhythmServerSettingsScreen.show(context, hub: serverHub);
      AnalyticsService().logHubRecoveryAction(
        action: 'open_settings',
        disconnectedCount: disconnectedCount,
      );
    }
  }

  Future<void> _retryHubs({
    required List<Map<String, dynamic>> manualRetryRequired,
    required int disconnectedCount,
  }) async {
    HapticFeedback.selectionClick();
    final syncProvider = context.read<ServerSyncProvider>();
    if (manualRetryRequired.isNotEmpty) {
      await syncProvider.retryHubs(manualRetryRequired);
    } else {
      await syncProvider.fullRefresh();
    }
    AnalyticsService().logHubRecoveryAction(
      action: 'retry',
      disconnectedCount: disconnectedCount,
    );
  }

  static RhythmHubInfo _parsedHubInfo(Map<String, dynamic> hubInfo) {
    return RhythmHubInfo.fromJson(hubInfo);
  }

  static bool _hubNeedsManualRetry(Map<String, dynamic> hubInfo) {
    return (hubInfo['connected'] as bool? ?? false) != true &&
        _parsedHubInfo(hubInfo).startupRetry?.status ==
            RhythmHubStartupRetryStatus.manualRetryRequired;
  }

  static bool _hubIsAutoRetrying(Map<String, dynamic> hubInfo) {
    return (hubInfo['connected'] as bool? ?? false) != true &&
        _parsedHubInfo(hubInfo).startupRetry?.status ==
            RhythmHubStartupRetryStatus.scheduled;
  }

  static String _hubLabel(String type) => switch (type) {
        'hue' => 'Hue',
        'homeassistant' || 'home_assistant' => 'Home Assistant',
        'matter' => 'Matter',
        _ => type,
      };
}

class _RetryButton extends StatelessWidget {
  final bool busy;
  final String label;
  final Future<void> Function()? onPressed;

  const _RetryButton({
    required this.busy,
    required this.label,
    required this.onPressed,
  });

  @override
  Widget build(BuildContext context) {
    final accent = const Color(0xFFE8A54B);
    final enabled = onPressed != null;
    return Material(
      type: MaterialType.transparency,
      child: InkWell(
        onTap: onPressed,
        borderRadius: BorderRadius.circular(10),
        child: Ink(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
          decoration: BoxDecoration(
            color: accent.withValues(alpha: enabled ? 0.12 : 0.08),
            borderRadius: BorderRadius.circular(10),
            border: Border.all(
              color: accent.withValues(alpha: enabled ? 0.24 : 0.18),
            ),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (busy)
                SizedBox(
                  width: 14,
                  height: 14,
                  child: CircularProgressIndicator(
                    strokeWidth: 2,
                    valueColor: AlwaysStoppedAnimation<Color>(
                      accent.withValues(alpha: enabled ? 0.9 : 0.7),
                    ),
                  ),
                )
              else
                Icon(
                  Icons.refresh_rounded,
                  size: 15,
                  color: accent.withValues(alpha: enabled ? 0.9 : 0.7),
                ),
              const SizedBox(width: 6),
              Text(
                label,
                style: TextStyle(
                  color: accent.withValues(alpha: enabled ? 0.92 : 0.72),
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 0.1,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Compact chip showing a disconnected hub.
class _HubChip extends StatelessWidget {
  final String type;
  final Animation<double> pulse;

  static const _red = Color(0xFFEF4444);

  const _HubChip({
    required this.type,
    required this.pulse,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
      decoration: BoxDecoration(
        color: _red.withValues(alpha: 0.1),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(
          color: _red.withValues(alpha: 0.2),
        ),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(
            _hubIcon(type),
            size: 12,
            color: _red.withValues(alpha: 0.9),
          ),
          const SizedBox(width: 5),
          Text(
            _hubLabel(type),
            style: TextStyle(
              fontSize: 11,
              fontWeight: FontWeight.w500,
              color: _red.withValues(alpha: 0.9),
              letterSpacing: 0.2,
            ),
          ),
          const SizedBox(width: 5),
          AnimatedBuilder(
            animation: pulse,
            builder: (context, _) {
              return Container(
                width: 5,
                height: 5,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: _red.withValues(alpha: pulse.value * 0.9),
                ),
              );
            },
          ),
        ],
      ),
    );
  }

  static String _hubLabel(String type) => switch (type) {
        'hue' => 'Hue',
        'homeassistant' || 'home_assistant' => 'HA',
        'matter' => 'Matter',
        _ => type,
      };

  static IconData _hubIcon(String type) => switch (type) {
        'hue' => Icons.lightbulb_outline,
        'homeassistant' || 'home_assistant' => Icons.home_outlined,
        'matter' => Icons.memory_outlined,
        _ => Icons.hub_outlined,
      };
}
