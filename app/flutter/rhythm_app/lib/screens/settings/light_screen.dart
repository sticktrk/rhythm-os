import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:uuid/uuid.dart';

import '../../providers/server_sync_provider.dart';
import '../../services/analytics_service.dart';
import '../../utils/app_color_temperature.dart';
import '../../widgets/header_close_button.dart';
import '../../widgets/solar_orbit.dart' show CelestialColors;
import '../../widgets/time_simulator.dart';
import 'light_profile_screen.dart';

enum LightOverrideScope { room, bulb }

/// The "Lighting" destination — the *look* of each mode, presented as a stack of
/// collapsible **profile layers**.
///
/// Each profile (Day, Sleep, …) is one "layer" card: collapsed it shows a live
/// gradient preview + a one-line summary; tapping expands its full editor
/// inline while the others stay tucked away. New profiles slot in by adding a
/// single entry to [_kProfileLayers] — the screen scales without redesign.
class LightScreen extends StatefulWidget {
  const LightScreen({
    super.key,
    this.showBackButton = false,
    this.onClose,
    this.roomId,
    this.roomName,
    this.overrideScope = LightOverrideScope.room,
  });

  /// When pushed as its own route the header shows a back affordance; as an
  /// embedded tab body it doesn't.
  final bool showBackButton;

  /// Returns an embedded top-level Lighting destination to Home.
  final VoidCallback? onClose;

  final String? roomId;
  final String? roomName;
  final LightOverrideScope overrideScope;

  bool get isRoomScoped => roomId != null;

  /// Pushes the Light editor as a full route with a back button.
  static Future<void> show(BuildContext context) {
    AnalyticsService().logScreenView('light');
    return Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => const LightScreen(showBackButton: true),
      ),
    );
  }

  /// Pushes the familiar Light editor scoped to one room's overrides.
  static Future<void> showForRoom(
    BuildContext context, {
    required String roomId,
    required String roomName,
  }) {
    final overrides = context
            .read<ServerSyncProvider>()
            .nodeById(roomId)
            ?.profileSettings
            ?.profileOverrides ??
        const <String, RhythmLightProfileNodeOverride>{};
    AnalyticsService().logRoomLightSettingsOpened(
      hasOverrides: overrides.values.any((override) => !override.isEmpty),
      overrideProfileCount:
          overrides.values.where((override) => !override.isEmpty).length,
    );
    return Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => LightScreen(
          showBackButton: true,
          roomId: roomId,
          roomName: roomName,
        ),
      ),
    );
  }

  /// Pushes the Light editor scoped to one addressable bulb's overrides.
  static Future<void> showForBulb(
    BuildContext context, {
    required String nodeId,
    required String bulbName,
  }) {
    final overrides = context
            .read<ServerSyncProvider>()
            .nodeById(nodeId)
            ?.profileSettings
            ?.profileOverrides ??
        const <String, RhythmLightProfileNodeOverride>{};
    AnalyticsService().logRoomLightSettingsOpened(
      hasOverrides: overrides.values.any((override) => !override.isEmpty),
      overrideProfileCount:
          overrides.values.where((override) => !override.isEmpty).length,
      scope: 'bulb',
    );
    return Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => LightScreen(
          showBackButton: true,
          roomId: nodeId,
          roomName: bulbName,
          overrideScope: LightOverrideScope.bulb,
        ),
      ),
    );
  }

  @override
  State<LightScreen> createState() => _LightScreenState();
}

/// Static description of a profile layer. The ordered list here is the single
/// place new profiles are introduced on the Lighting destination.
class _ProfileLayer {
  final String id;
  final String name;
  final IconData icon;
  final Color accent;

  const _ProfileLayer({
    required this.id,
    required this.name,
    required this.icon,
    required this.accent,
  });
}

const List<_ProfileLayer> _kProfileLayers = [
  _ProfileLayer(
    id: 'rhythm',
    name: 'Day',
    icon: Icons.wb_sunny_rounded,
    accent: Color(0xFFFFB74D),
  ),
  _ProfileLayer(
    id: 'sleep',
    name: 'Sleep',
    icon: Icons.bedtime_rounded,
    accent: Color(0xFF7C83FF),
  ),
];

class _LightScreenState extends State<LightScreen> {
  static const _uuid = Uuid();

  // Accordion: a single layer is expanded at a time. Defaults to the first.
  String? _expandedId = _kProfileLayers.first.id;
  bool _resettingRoom = false;

  bool get _isBulbScoped => widget.overrideScope == LightOverrideScope.bulb;
  String get _scopeName =>
      widget.roomName ?? (_isBulbScoped ? 'This bulb' : 'This room');
  String get _scopeNoun => _isBulbScoped ? 'Bulb' : 'Room';
  String get _analyticsScope => _isBulbScoped ? 'bulb' : 'room';

  void _toggle(String id) {
    HapticFeedback.selectionClick();
    setState(() => _expandedId = _expandedId == id ? null : id);
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.fromLTRB(16, 4, 16, 28),
                child: Consumer<ServerSyncProvider>(
                  builder: (context, sync, _) {
                    final roomOverrides = widget.roomId == null
                        ? const <String, RhythmLightProfileNodeOverride>{}
                        : sync
                                .nodeById(widget.roomId!)
                                ?.profileSettings
                                ?.profileOverrides ??
                            const <String, RhythmLightProfileNodeOverride>{};
                    final hasRoomOverrides = roomOverrides.values
                        .any((profileOverride) => !profileOverride.isEmpty);
                    return Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        if (widget.isRoomScoped) ...[
                          _buildRoomScopeBanner(
                            sync,
                            hasOverrides: hasRoomOverrides,
                          ),
                          const SizedBox(height: 12),
                        ],
                        for (final layer in _kProfileLayers)
                          _ProfileLayerCard(
                            layer: layer,
                            preview: _previewFor(
                              _effectiveConfigFor(
                                sync,
                                layer.id,
                                roomOverrides,
                              ),
                            ),
                            customized:
                                !(roomOverrides[layer.id]?.isEmpty ?? true),
                            expanded: _expandedId == layer.id,
                            onToggle: () => _toggle(layer.id),
                            child: LightProfileScreen(
                              initialProfile: layer.id,
                              embedded: true,
                              roomId: widget.roomId,
                              roomName: widget.roomName,
                              overrideScope: _analyticsScope,
                            ),
                          ),
                        // The Time Simulator scrubs the Day curve, so it only
                        // belongs here when Day is the active mode.
                        if (!widget.isRoomScoped &&
                            sync.hasBeenSynced &&
                            sync.activeMode == RhythmMode.day) ...[
                          const SizedBox(height: 4),
                          const TimeSimulator(),
                        ],
                      ],
                    );
                  },
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader() {
    final title = widget.isRoomScoped
        ? Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(
                widget.roomName ?? 'Room',
                textAlign: TextAlign.center,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 18,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 0.2,
                ),
              ),
              const SizedBox(height: 2),
              Text(
                'Light settings · $_scopeNoun override',
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.78),
                  fontSize: 11.5,
                  fontWeight: FontWeight.w500,
                  letterSpacing: 0.15,
                ),
              ),
            ],
          )
        : const Text(
            'Lighting',
            textAlign: TextAlign.center,
            style: TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 18,
              fontWeight: FontWeight.w600,
              letterSpacing: 0.3,
            ),
          );

    if (!widget.showBackButton) {
      if (widget.onClose != null) {
        return Container(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 16),
          child: Row(
            children: [
              HeaderCloseButton(onTap: widget.onClose!),
              Expanded(child: title),
              const SizedBox(width: 40),
            ],
          ),
        );
      }
      return Container(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 16),
        alignment: Alignment.center,
        child: title,
      );
    }

    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: Row(
        children: [
          GestureDetector(
            onTap: () => Navigator.of(context).pop(),
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: CelestialColors.accentBlue.withValues(alpha: 0.2),
              ),
              child: const Icon(
                Icons.chevron_left,
                color: CelestialColors.accentBlue,
                size: 24,
              ),
            ),
          ),
          Expanded(child: title),
          const SizedBox(width: 40),
        ],
      ),
    );
  }

  RhythmCurveConfig? _configFor(ServerSyncProvider sync, String id) {
    for (final profile in sync.profiles) {
      if (profile.id == id) return profile;
    }
    return null;
  }

  RhythmCurveConfig? _effectiveConfigFor(
    ServerSyncProvider sync,
    String id,
    Map<String, RhythmLightProfileNodeOverride> overrides,
  ) {
    final global = _configFor(sync, id);
    if (global == null) return null;
    return overrides[id]?.applyTo(global) ?? global;
  }

  Widget _buildRoomScopeBanner(
    ServerSyncProvider sync, {
    required bool hasOverrides,
  }) {
    final supported =
        sync.lightProfileOverridesSupportedForNode(widget.roomId!);
    final accent =
        hasOverrides ? const Color(0xFFF9A825) : CelestialColors.accentBlue;
    return Container(
      padding: const EdgeInsets.fromLTRB(14, 12, 12, 12),
      decoration: BoxDecoration(
        color: accent.withValues(alpha: 0.08),
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: accent.withValues(alpha: 0.22)),
      ),
      child: Row(
        children: [
          Container(
            width: 32,
            height: 32,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: accent.withValues(alpha: 0.14),
            ),
            child: Icon(
              supported
                  ? hasOverrides
                      ? Icons.tune_rounded
                      : Icons.home_rounded
                  : Icons.system_update_rounded,
              size: 17,
              color: accent,
            ),
          ),
          const SizedBox(width: 11),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  supported
                      ? hasOverrides
                          ? 'Custom light settings'
                          : 'Automatic lighting'
                      : 'Appliance update required',
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 13.5,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                const SizedBox(height: 2),
                Text(
                  supported
                      ? hasOverrides
                          ? 'Only changed values differ from automatic Day and Sleep lighting.'
                          : 'Automatic Day and Sleep settings flow into this ${_isBulbScoped ? 'bulb' : 'room'}.'
                      : 'Light overrides are not supported by this appliance.',
                  style: TextStyle(
                    color:
                        CelestialColors.textSecondary.withValues(alpha: 0.78),
                    fontSize: 11.5,
                    height: 1.25,
                  ),
                ),
              ],
            ),
          ),
          if (supported && hasOverrides) ...[
            const SizedBox(width: 10),
            TextButton(
              key: ValueKey(
                'room-light-settings-reset-all-${widget.roomId}',
              ),
              onPressed: _resettingRoom ? null : _confirmResetRoom,
              style: TextButton.styleFrom(
                foregroundColor: accent,
                padding:
                    const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
                visualDensity: VisualDensity.compact,
              ),
              child: _resettingRoom
                  ? const SizedBox(
                      width: 16,
                      height: 16,
                      child: CircularProgressIndicator(
                        strokeWidth: 2,
                        color: Color(0xFFF9A825),
                      ),
                    )
                  : const Text(
                      'Use auto',
                      style: TextStyle(fontWeight: FontWeight.w600),
                    ),
            ),
          ],
        ],
      ),
    );
  }

  Future<void> _confirmResetRoom() async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: const Text(
          'Use automatic light settings?',
          style: TextStyle(color: CelestialColors.textPrimary),
        ),
        content: Text(
          '$_scopeName will follow automatic Day and Sleep lighting again. '
          'Other settings and power state stay unchanged.',
          style: const TextStyle(color: CelestialColors.textSecondary),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(false),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(true),
            child: const Text('Use automatic settings'),
          ),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;

    final journeyId = '$_analyticsScope-light-settings-${_uuid.v4()}';
    setState(() => _resettingRoom = true);
    final succeeded =
        await context.read<ServerSyncProvider>().resetNodeLightProfileOverrides(
              widget.roomId!,
              correlationId: journeyId,
            );
    if (!mounted) return;
    setState(() => _resettingRoom = false);
    AnalyticsService().logRoomLightSettingsResetCompleted(
      journeyId: journeyId,
      profile: 'all',
      outcome: succeeded ? 'succeeded' : 'failed',
      failureStage: succeeded ? null : 'request',
      scope: _analyticsScope,
    );
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          succeeded
              ? '$_scopeName now follows automatic light settings.'
              : '$_scopeNoun light settings could not be reset.',
        ),
      ),
    );
  }
}

/// A compact, live preview of a profile's look: a gradient swatch plus a short
/// value summary, derived from the profile's curve config.
class _LayerPreview {
  final List<Color> gradient;
  final String summary;

  const _LayerPreview(this.gradient, this.summary);
}

Color? _directColor(RhythmCurveConfig config) {
  final curve = config.curve;
  final direct = switch (curve) {
    RhythmSuperGaussianCurve(:final directColor) => directColor,
    RhythmConstantCurve(:final directColor) => directColor,
    _ => null,
  };
  if (direct == null) return null;
  return Color.fromARGB(255, direct.rgb.r, direct.rgb.g, direct.rgb.b);
}

_LayerPreview _previewFor(RhythmCurveConfig? config) {
  if (config == null) {
    final grey = CelestialColors.orbitRing.withValues(alpha: 0.6);
    return _LayerPreview([grey, grey], '—');
  }

  // Fixed-color profiles (e.g. a Sleep "warm glow") preview as that color, with
  // brightness as the headline value.
  final fixed = _directColor(config);
  if (fixed != null) {
    return _LayerPreview(
      [Color.lerp(fixed, Colors.black, 0.4)!, fixed],
      '${config.maxBrightness}%',
    );
  }

  // Curve profiles preview as their color-temperature ramp.
  final minK = config.minColorTemp;
  final maxK = config.maxColorTemp;
  final gradient = [
    AppColorTemperature.toColor(minK),
    AppColorTemperature.toColor((minK + maxK) ~/ 2),
    AppColorTemperature.toColor(maxK),
  ];
  final summary = minK == maxK ? '$minK K' : '$minK–$maxK K';
  return _LayerPreview(gradient, summary);
}

/// One collapsible profile "layer". Collapsed: icon + name + gradient preview +
/// summary. Expanded: the full embedded editor slides open beneath the header.
///
/// The editor [child] is always mounted (its draft + server state survive a
/// collapse); expansion is a clipped height reveal, not a remount.
class _ProfileLayerCard extends StatelessWidget {
  final _ProfileLayer layer;
  final _LayerPreview preview;
  final bool customized;
  final bool expanded;
  final VoidCallback onToggle;
  final Widget child;

  const _ProfileLayerCard({
    required this.layer,
    required this.preview,
    this.customized = false,
    required this.expanded,
    required this.onToggle,
    required this.child,
  });

  @override
  Widget build(BuildContext context) {
    final accent = layer.accent;
    return AnimatedContainer(
      duration: const Duration(milliseconds: 300),
      curve: Curves.easeOutCubic,
      margin: const EdgeInsets.only(bottom: 12),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(20),
        border: Border.all(
          color: expanded
              ? accent.withValues(alpha: 0.32)
              : CelestialColors.orbitRing.withValues(alpha: 0.5),
        ),
        boxShadow: expanded
            ? [
                BoxShadow(
                  color: accent.withValues(alpha: 0.12),
                  blurRadius: 26,
                  spreadRadius: -6,
                ),
              ]
            : const [],
      ),
      clipBehavior: Clip.antiAlias,
      child: Column(
        children: [
          _buildHeader(accent),
          // Keep the editor mounted; reveal it with a clipped height factor so
          // no state is lost when a layer collapses.
          ClipRect(
            child: AnimatedAlign(
              alignment: Alignment.topCenter,
              heightFactor: expanded ? 1 : 0,
              duration: const Duration(milliseconds: 300),
              curve: Curves.easeOutCubic,
              child: child,
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildHeader(Color accent) {
    return GestureDetector(
      onTap: onToggle,
      behavior: HitTestBehavior.opaque,
      child: IntrinsicHeight(
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            // Colored "layer spine" — the per-mode identity edge.
            Container(
              width: 4,
              color: accent.withValues(alpha: expanded ? 0.95 : 0.6),
            ),
            Expanded(
              child: Padding(
                padding: const EdgeInsets.fromLTRB(14, 14, 12, 14),
                child: Row(
                  children: [
                    Container(
                      width: 32,
                      height: 32,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: accent.withValues(alpha: 0.14),
                        border: Border.all(
                          color: accent.withValues(alpha: 0.28),
                        ),
                      ),
                      child: Icon(layer.icon, color: accent, size: 17),
                    ),
                    const SizedBox(width: 12),
                    // Fixed-width so every bar starts at the same x regardless
                    // of name length (Day vs Sleep).
                    SizedBox(
                      width: customized ? 82 : 64,
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          Text(
                            layer.name,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: const TextStyle(
                              color: CelestialColors.textPrimary,
                              fontSize: 16,
                              fontWeight: FontWeight.w600,
                              letterSpacing: -0.1,
                            ),
                          ),
                          if (customized)
                            Text(
                              'CUSTOM',
                              key: ValueKey(
                                'room-light-layer-custom-${layer.id}',
                              ),
                              style: TextStyle(
                                color: accent.withValues(alpha: 0.88),
                                fontSize: 8,
                                fontWeight: FontWeight.w700,
                                letterSpacing: 0.7,
                                height: 1.15,
                              ),
                            ),
                        ],
                      ),
                    ),
                    const SizedBox(width: 14),
                    Expanded(child: _GradientSwatch(colors: preview.gradient)),
                    const SizedBox(width: 12),
                    // Fixed-width, right-aligned so the swatches all end at the
                    // same x and read as a clean aligned column.
                    SizedBox(
                      width: 96,
                      child: Text(
                        preview.summary,
                        textAlign: TextAlign.right,
                        maxLines: 1,
                        softWrap: false,
                        overflow: TextOverflow.visible,
                        style: TextStyle(
                          color: CelestialColors.textSecondary
                              .withValues(alpha: 0.85),
                          fontSize: 12.5,
                          fontWeight: FontWeight.w500,
                          letterSpacing: 0.2,
                          fontFeatures: const [FontFeature.tabularFigures()],
                        ),
                      ),
                    ),
                    const SizedBox(width: 6),
                    AnimatedRotation(
                      turns: expanded ? 0.5 : 0,
                      duration: const Duration(milliseconds: 250),
                      curve: Curves.easeOutCubic,
                      child: Icon(
                        Icons.keyboard_arrow_down_rounded,
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.5),
                        size: 22,
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// A small horizontal gradient bar previewing a profile's light — the visual
/// signature of the layer at a glance.
class _GradientSwatch extends StatelessWidget {
  final List<Color> colors;

  const _GradientSwatch({required this.colors});

  @override
  Widget build(BuildContext context) {
    return Container(
      height: 10,
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(6),
        gradient: LinearGradient(
          colors: colors.length == 1 ? [colors.first, colors.first] : colors,
        ),
        border: Border.all(
          color: Colors.white.withValues(alpha: 0.08),
        ),
        boxShadow: [
          BoxShadow(
            color: colors.last.withValues(alpha: 0.25),
            blurRadius: 8,
            spreadRadius: -3,
          ),
        ],
      ),
    );
  }
}
