import 'dart:async';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmCurveConfig;
import '../../models/config_model.dart';
import '../../providers/server_sync_provider.dart';

/// Full-screen modal for global device preferences.
///
/// Exposes behavioural settings:
/// - Update Interval (rhythm_interval_secs) — rhythm recalculation period
/// - Power Save — turn off lights when idle
class PreferencesScreen extends StatefulWidget {
  const PreferencesScreen({super.key});

  static Future<void> show(BuildContext context) {
    return Navigator.of(context).push(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return const PreferencesScreen();
        },
        transitionsBuilder: (context, animation, secondaryAnimation, child) {
          final curve = CurvedAnimation(
            parent: animation,
            curve: Curves.easeOutCubic,
            reverseCurve: Curves.easeInCubic,
          );
          return SlideTransition(
            position: Tween<Offset>(
              begin: const Offset(0, 1),
              end: Offset.zero,
            ).animate(curve),
            child: child,
          );
        },
        transitionDuration: const Duration(milliseconds: 350),
        reverseTransitionDuration: const Duration(milliseconds: 300),
      ),
    );
  }

  @override
  State<PreferencesScreen> createState() => _PreferencesScreenState();
}

class _PreferencesScreenState extends State<PreferencesScreen>
    with SingleTickerProviderStateMixin {
  // Current slider values.
  double _intervalSecs = 60;
  bool _powerSave = false;

  bool _loading = true;
  bool _connected = false;

  // Debounce timers per setting.
  Timer? _intervalDebounce;

  // Glow animation for the header icon.
  late AnimationController _glowController;
  late Animation<double> _glowAnimation;

  @override
  void initState() {
    super.initState();

    _glowController = AnimationController(
      duration: const Duration(milliseconds: 2500),
      vsync: this,
    )..repeat(reverse: true);
    _glowAnimation = Tween<double>(begin: 0.3, end: 0.7).animate(
      CurvedAnimation(parent: _glowController, curve: Curves.easeInOut),
    );

    _loadSettings();
  }

  @override
  void dispose() {
    _intervalDebounce?.cancel();
    _glowController.dispose();
    super.dispose();
  }

  Future<void> _loadSettings() async {
    final syncProvider = context.read<ServerSyncProvider>();
    _connected = syncProvider.synced;

    if (!_connected) {
      setState(() => _loading = false);
      return;
    }

    // Try fetching live settings; fall back to hello cache.
    final settings = await syncProvider.api.getSettings();
    if (settings != null) {
      _intervalSecs = settings.rhythmIntervalSecs.toDouble();
      _powerSave = settings.powerSave;
    }

    if (mounted) setState(() => _loading = false);
  }

  void _onIntervalChanged(double value) {
    setState(() => _intervalSecs = value);
    _intervalDebounce?.cancel();
    _intervalDebounce = Timer(const Duration(milliseconds: 500), () {
      _pushSetting(rhythmIntervalSecs: value.round());
    });
  }

  void _onPowerSaveChanged(bool value) {
    setState(() => _powerSave = value);
    _pushSetting(powerSave: value);
  }

  Future<void> _pushSetting({
    int? rhythmIntervalSecs,
    bool? powerSave,
  }) async {
    await context.read<ServerSyncProvider>().api.settingsSet(
      rhythmIntervalSecs: rhythmIntervalSecs,
      powerSave: powerSave,
    );
  }

  // ---------------------------------------------------------------------------
  // Build
  // ---------------------------------------------------------------------------

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: _Palette.bg,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(),
            Expanded(
              child: _loading
                  ? const Center(
                      child: CircularProgressIndicator(
                        strokeWidth: 2,
                        color: _Palette.amber,
                      ),
                    )
                  : _connected
                      ? _buildContent()
                      : _buildDisconnected(),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader() {
    return Padding(
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
                color: _Palette.amber.withValues(alpha: 0.12),
                border: Border.all(
                  color: _Palette.amber.withValues(alpha: 0.25),
                ),
              ),
              child:
                  const Icon(Icons.close, color: _Palette.amber, size: 20),
            ),
          ),
          const Expanded(
            child: Text(
              'Preferences',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: _Palette.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.3,
              ),
            ),
          ),
          const SizedBox(width: 40), // Balance close button
        ],
      ),
    );
  }

  Widget _buildDisconnected() {
    return Center(
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 40),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(
              width: 64,
              height: 64,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _Palette.textSecondary.withValues(alpha: 0.08),
              ),
              child: Icon(
                Icons.link_off_rounded,
                color: _Palette.textSecondary.withValues(alpha: 0.5),
                size: 28,
              ),
            ),
            const SizedBox(height: 20),
            Text(
              'Device Not Connected',
              style: TextStyle(
                color: _Palette.textPrimary.withValues(alpha: 0.8),
                fontSize: 17,
                fontWeight: FontWeight.w600,
              ),
            ),
            const SizedBox(height: 8),
            Text(
              'Connect to a Rhythm Server to configure preferences.',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: _Palette.textSecondary.withValues(alpha: 0.7),
                fontSize: 14,
                height: 1.4,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildContent() {
    return SingleChildScrollView(
      padding: const EdgeInsets.fromLTRB(20, 8, 20, 40),
      child: Column(
        children: [
          _buildHeroIcon(),
          const SizedBox(height: 28),
          _buildSettingCard(
            icon: Icons.update_rounded,
            color: _Palette.blue,
            title: 'Background Light Interval',
            value: _intervalSecs,
            min: 10,
            max: 300,
            divisions: 29,
            formatValue: _formatInterval,
            onChanged: _onIntervalChanged,
          ),
          const SizedBox(height: 14),
          _buildToggleCard(
            icon: Icons.eco_rounded,
            color: _Palette.green,
            title: 'Power Save',
            value: _powerSave,
            onChanged: _onPowerSaveChanged,
          ),
          const SizedBox(height: 28),
          _buildResetButton(),
        ],
      ),
    );
  }

  Widget _buildHeroIcon() {
    return AnimatedBuilder(
      animation: _glowAnimation,
      builder: (context, child) {
        return Container(
          width: 72,
          height: 72,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: _Palette.amber.withValues(alpha: 0.08),
            border: Border.all(
              color: _Palette.amber.withValues(alpha: 0.18),
            ),
            boxShadow: [
              BoxShadow(
                color:
                    _Palette.amber.withValues(alpha: _glowAnimation.value * 0.15),
                blurRadius: 32,
                spreadRadius: 0,
              ),
            ],
          ),
          child: Icon(
            Icons.tune_rounded,
            color: _Palette.amber.withValues(
              alpha: 0.6 + _glowAnimation.value * 0.4,
            ),
            size: 32,
          ),
        );
      },
    );
  }

  Widget _buildSettingCard({
    required IconData icon,
    required Color color,
    required String title,
    required double value,
    required double min,
    required double max,
    required int divisions,
    required String Function(double) formatValue,
    required ValueChanged<double> onChanged,
  }) {
    return Container(
      padding: const EdgeInsets.fromLTRB(18, 18, 18, 12),
      decoration: BoxDecoration(
        color: _Palette.card,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(color: _Palette.border),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // Header row: icon + title + formatted value
          Row(
            children: [
              Container(
                width: 36,
                height: 36,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: color.withValues(alpha: 0.12),
                ),
                child: Icon(icon, color: color, size: 18),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Text(
                  title,
                  style: const TextStyle(
                    color: _Palette.textPrimary,
                    fontSize: 15,
                    fontWeight: FontWeight.w600,
                    letterSpacing: -0.1,
                  ),
                ),
              ),
              // Current value badge
              Container(
                padding:
                    const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
                decoration: BoxDecoration(
                  color: color.withValues(alpha: 0.1),
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(color: color.withValues(alpha: 0.2)),
                ),
                child: Text(
                  formatValue(value),
                  style: TextStyle(
                    color: color,
                    fontSize: 14,
                    fontWeight: FontWeight.w700,
                    fontFeatures: const [FontFeature.tabularFigures()],
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: 14),
          // Slider
          SliderTheme(
            data: SliderThemeData(
              activeTrackColor: color,
              inactiveTrackColor: color.withValues(alpha: 0.12),
              thumbColor: color,
              overlayColor: color.withValues(alpha: 0.12),
              trackHeight: 4,
              thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 8),
              overlayShape: const RoundSliderOverlayShape(overlayRadius: 18),
            ),
            child: Slider(
              value: value.clamp(min, max),
              min: min,
              max: max,
              divisions: divisions,
              onChanged: onChanged,
            ),
          ),
          // Min/max labels
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 6),
            child: Row(
              mainAxisAlignment: MainAxisAlignment.spaceBetween,
              children: [
                Text(
                  formatValue(min),
                  style: TextStyle(
                    color: _Palette.textSecondary.withValues(alpha: 0.4),
                    fontSize: 11,
                  ),
                ),
                Text(
                  formatValue(max),
                  style: TextStyle(
                    color: _Palette.textSecondary.withValues(alpha: 0.4),
                    fontSize: 11,
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildToggleCard({
    required IconData icon,
    required Color color,
    required String title,
    required bool value,
    required ValueChanged<bool> onChanged,
  }) {
    return Container(
      padding: const EdgeInsets.fromLTRB(18, 18, 18, 18),
      decoration: BoxDecoration(
        color: _Palette.card,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(color: _Palette.border),
      ),
      child: Row(
        children: [
          Container(
            width: 36,
            height: 36,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: color.withValues(alpha: 0.12),
            ),
            child: Icon(icon, color: color, size: 18),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Text(
              title,
              style: const TextStyle(
                color: _Palette.textPrimary,
                fontSize: 15,
                fontWeight: FontWeight.w600,
                letterSpacing: -0.1,
              ),
            ),
          ),
          const SizedBox(width: 12),
          SizedBox(
            height: 28,
            child: Switch.adaptive(
              value: value,
              onChanged: onChanged,
              activeTrackColor: color,
              activeThumbColor: _Palette.textPrimary,
            ),
          ),
        ],
      ),
    );
  }

  // ---------------------------------------------------------------------------
  // Reset to Defaults
  // ---------------------------------------------------------------------------

  Widget _buildResetButton() {
    return GestureDetector(
      onTap: _confirmReset,
      child: Container(
        padding: const EdgeInsets.symmetric(vertical: 16),
        decoration: BoxDecoration(
          color: _Palette.card,
          borderRadius: BorderRadius.circular(18),
          border: Border.all(color: _Palette.border),
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(
              Icons.restart_alt_rounded,
              color: Colors.redAccent.shade100,
              size: 20,
            ),
            const SizedBox(width: 8),
            Text(
              'Reset to Defaults',
              style: TextStyle(
                color: Colors.redAccent.shade100,
                fontSize: 15,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Future<void> _confirmReset() async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: _Palette.card,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(18),
          side: const BorderSide(color: _Palette.border),
        ),
        title: const Text(
          'Reset to Defaults?',
          style: TextStyle(color: _Palette.textPrimary, fontSize: 17),
        ),
        content: const Text(
          'This will reset all settings and the light curve to factory defaults.',
          style: TextStyle(color: _Palette.textSecondary, fontSize: 14),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: const Text('Cancel',
                style: TextStyle(color: _Palette.textSecondary)),
          ),
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: Text('Reset',
                style: TextStyle(color: Colors.redAccent.shade100)),
          ),
        ],
      ),
    );

    if (confirmed == true) {
      await _resetToDefaults();
    }
  }

  Future<void> _resetToDefaults() async {
    final api = context.read<ServerSyncProvider>().api;

    await Future.wait([
      api.settingsSet(
        rhythmIntervalSecs: 60,
        powerSave: true,
      ),
      api.configSet(const RhythmCurveConfig()),
    ]);

    setState(() {
      _intervalSecs = 60;
      _powerSave = true;
    });

    // Update ConfigModel so the designer reflects the change.
    if (mounted) {
      context.read<ConfigModel>().resetToDefaults();
    }
  }

  // ---------------------------------------------------------------------------
  // Formatters
  // ---------------------------------------------------------------------------

  String _formatInterval(double secs) {
    if (secs >= 60 && secs % 60 == 0) return '${(secs / 60).round()}m';
    return '${secs.round()}s';
  }

}

// -----------------------------------------------------------------------------
// Palette — warm dark theme consistent with the app
// -----------------------------------------------------------------------------

class _Palette {
  static const bg = Color(0xFF0B0E13);
  static const card = Color(0xFF13171E);
  static const border = Color(0xFF232A35);
  static const textPrimary = Color(0xFFE8EDF4);
  static const textSecondary = Color(0xFF8A919C);
  static const amber = Color(0xFFF9A825);
  static const blue = Color(0xFF58A6FF);
  static const green = Color(0xFF22C55E);
}
