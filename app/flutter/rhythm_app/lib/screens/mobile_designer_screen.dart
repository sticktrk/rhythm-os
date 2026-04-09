import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import '../api/hybrid_client.dart';
import '../models/config_model.dart';
import '../services/analytics_service.dart';
import '../widgets/solar_orbit.dart';
import '../widgets/polar_day_editor.dart';
import '../widgets/time_scrubber.dart';
import '../widgets/light_output_display.dart';
import '../providers/home_provider.dart';

/// Mobile-first Solar Orbit Designer screen.
///
/// A touch-first interface where the sun is the star. Features:
/// - Central sun orb that you drag around an orbital ring
/// - Sun appearance (size, color, glow) shows lighting output at any time
/// - Time scrubber for precise time selection
/// - Gesture-based curve tuning (long-press sun to enter tune mode)
class MobileDesignerScreen extends StatefulWidget {
  const MobileDesignerScreen({super.key});

  @override
  State<MobileDesignerScreen> createState() => _MobileDesignerScreenState();
}

class _MobileDesignerScreenState extends State<MobileDesignerScreen> {
  CurveData? _curveData;
  bool _isLoading = true;
  String? _error;
  Timer? _nowTimer;
  bool _isTuneMode = false;
  String? _tuneParameter;
  bool _isLightOn = true;
  int? _manualBrightness;
  bool _isFollowingNow = true; // Auto-follow current time until user drags
  final bool _isEditMode = false; // Polar day editor mode

  @override
  void initState() {
    super.initState();
    _loadData();
    // Update to current time every 30 seconds when auto-following
    _nowTimer = Timer.periodic(const Duration(seconds: 30), (_) {
      _updateToCurrentTime();
    });
  }

  @override
  void dispose() {
    _nowTimer?.cancel();
    super.dispose();
  }

  Future<void> _loadData() async {
    setState(() {
      _isLoading = true;
      _error = null;
    });

    try {
      final api = context.read<RhythmApi>();
      final configModel = context.read<ConfigModel>();

      final results = await Future.wait([
        api.getConfigState(),
        api.getCurveData(),
      ]);

      if (mounted) {
        final configState = results[0] as ConfigState;
        final curveData = results[1] as CurveData;

        configModel.updateFromConfigState(configState);

        setState(() {
          _curveData = curveData;
          _isLoading = false;
        });

        // Set initial position to current time
        if (_isFollowingNow) {
          _updateToCurrentTime();
        }
      }
    } catch (e) {
      if (mounted) {
        setState(() {
          _error = e.toString();
          _isLoading = false;
        });
      }
    }
  }

  Future<void> _updateCurveData(CurveConfigDto config) async {
    try {
      final api = context.read<RhythmApi>();
      final existingSolar = _curveData?.solar;

      CurveData? curveData;
      if (api is HybridApiClient) {
        curveData = api.getCurveDataHighRes(
          config: config,
          samplesPerHour: 4,
        );

        if (curveData != null && existingSolar != null) {
          curveData = CurveData(
            hours: curveData.hours,
            brightness: curveData.brightness,
            kelvin: curveData.kelvin,
            solar: existingSolar,
          );
        }
      }

      curveData ??= await api.getCurveData(overrides: config);

      if (mounted) {
        setState(() {
          _curveData = curveData;
        });
      }
    } catch (e) {
      // Ignore errors during live preview
    }
  }

  void _onHourChanged(double hour) {
    // User manually dragged - stop auto-following
    setState(() {
      _isFollowingNow = false;
      _manualBrightness = null; // Reset manual brightness when changing time
    });
    final configModel = context.read<ConfigModel>();
    final solarNoon = _curveData?.solar.solarNoon;
    configModel.setSelectedHour(hour, solarNoon: solarNoon);
  }

  void _updateToCurrentTime() {
    if (!_isFollowingNow || !mounted) return;
    final now = DateTime.now();
    final currentHour = now.hour + now.minute / 60.0;
    final configModel = context.read<ConfigModel>();
    final solarNoon = _curveData?.solar.solarNoon;
    configModel.setSelectedHour(currentHour, solarNoon: solarNoon);
  }

  void _resetToNow() {
    setState(() {
      _isFollowingNow = true;
      _manualBrightness = null;
    });
    _updateToCurrentTime();
    // Apply lighting values for "now" if lights are on
    if (_isLightOn) {
      _controlHueLights(turnOn: true);
    }
  }

  void _enterTuneMode() {
    setState(() {
      _isTuneMode = true;
    });
  }

  void _exitTuneMode() {
    setState(() {
      _isTuneMode = false;
      _tuneParameter = null;
    });
  }

  void _onEditConfigChanged(CurveConfigDto newConfig) {
    final configModel = context.read<ConfigModel>();
    configModel.updateConfig(newConfig);
    _updateCurveData(newConfig);
  }

  Future<void> _toggleLight() async {
    final newState = !_isLightOn;
    setState(() {
      _isLightOn = newState;
    });

    // Track light toggle
    AnalyticsService().logLightToggle(turnedOn: newState);

    // Control Hue lights
    await _controlHueLights(turnOn: newState);
  }

  Future<void> _controlHueLights({required bool turnOn}) async {
    try {
      final model = context.read<ConfigModel>();
      final api = context.read<RhythmApi>();

      // Get Hue config from HomeProvider
      final homeProvider = context.read<HomeProvider>();
      final hueHub = homeProvider.getFirstHubOfType(HubType.hue);
      if (hueHub == null || !hueHub.hasCredentials) {
        debugPrint('[Orbit] No Hue credentials configured');
        return;
      }
      final config = HueConfig(
        bridgeIp: hueHub.endpoint.host,
        username: hueHub.token!,
      );

      final provider = HueProvider(config);
      final devices = await provider.discoverDevices();

      if (devices.isEmpty) {
        debugPrint('[Orbit] No Hue lights found');
        return;
      }

      // Get current lighting values for turn on
      // Always use real current time, not the blue dot position
      final now = DateTime.now();
      final currentHour = now.hour + now.minute / 60.0;

      int brightness = 80;
      int kelvin = 4000;

      if (api is HybridApiClient) {
        final values = api.calculateLighting(
          config: model.config,
          currentHour: currentHour,
        );
        if (values != null) {
          brightness = values.brightness;
          kelvin = values.kelvin;
        }
      }

      debugPrint(
        '[Orbit] ${turnOn ? "Turning ON" : "Turning OFF"} ${devices.length} lights ($brightness%, ${kelvin}K)',
      );
      HapticFeedback.mediumImpact();

      // Control all lights
      for (final device in devices) {
        try {
          if (turnOn) {
            await provider.turnOn(device.id,
                brightness: brightness, kelvin: kelvin);
          } else {
            await provider.turnOff(device.id);
          }
        } catch (e) {
          debugPrint('[Orbit] Failed to control ${device.name}: $e');
        }
      }

      await provider.dispose();
      debugPrint('[Orbit] Done controlling lights');
    } catch (e) {
      debugPrint('[Orbit] Hue control error: $e');
    }
  }

  void _onBrightnessChanged(int brightness) {
    setState(() {
      _manualBrightness = brightness;
    });
  }

  void _onBrightnessChangeEnd() {
    // Keep the manual brightness
  }

  void _onTuneGesture(TuneGesture gesture, double delta) {
    final configModel = context.read<ConfigModel>();
    final config = configModel.config;
    final isMorning = configModel.activeHalf == 'morning';

    // Track the gesture type (only on first call per gesture)
    if (_tuneParameter == null) {
      final gestureNames = {
        TuneGesture.verticalDrag: 'vertical',
        TuneGesture.horizontalDrag: 'horizontal',
        TuneGesture.pinch: 'pinch',
      };
      AnalyticsService().logTuneGesture(gestureNames[gesture] ?? 'unknown');
    }

    CurveConfigDto? newConfig;

    switch (gesture) {
      case TuneGesture.verticalDrag:
        // Adjust max brightness
        final brightnessChange = (delta * 0.5).round();
        if (brightnessChange != 0) {
          newConfig = config.copyWith(
            maxBrightness:
                (config.maxBrightness + brightnessChange).clamp(1, 100),
          );
          setState(() =>
              _tuneParameter = 'Brightness: ${newConfig!.maxBrightness}%');
        }
        break;

      case TuneGesture.pinch:
        // Adjust shape (peak flatness)
        final shapeChange = delta * 0.05;
        newConfig = config.copyWith(
          shapeP: (config.shapeP + shapeChange).clamp(2.0, 10.0),
        );
        final shapeLabel = config.shapeP <= 3
            ? 'round'
            : (config.shapeP >= 5 ? 'flat' : 'moderate');
        setState(() => _tuneParameter =
            'Shape: ${newConfig!.shapeP.toStringAsFixed(1)} ($shapeLabel)');
        break;

      case TuneGesture.horizontalDrag:
        // Adjust width (ramp speed)
        // width < 1 = larger sigma = gentler slope = slower ramp
        // width > 1 = smaller sigma = steeper slope = faster ramp
        final widthChange = delta * 0.005;
        if (isMorning) {
          newConfig = config.copyWith(
            widthLeftBri: (config.widthLeftBri + widthChange).clamp(0.2, 2.0),
          );
          final speedLabel = newConfig.widthLeftBri < 0.7
              ? 'slow'
              : (newConfig.widthLeftBri > 1.3 ? 'fast' : 'normal');
          setState(() => _tuneParameter =
              'Width: ${newConfig!.widthLeftBri.toStringAsFixed(2)} ($speedLabel)');
        } else {
          newConfig = config.copyWith(
            widthRightBri: (config.widthRightBri + widthChange).clamp(0.2, 2.0),
          );
          final speedLabel = newConfig.widthRightBri < 0.7
              ? 'slow'
              : (newConfig.widthRightBri > 1.3 ? 'fast' : 'normal');
          setState(() => _tuneParameter =
              'Width: ${newConfig!.widthRightBri.toStringAsFixed(2)} ($speedLabel)');
        }
        break;
    }

    if (newConfig != null) {
      configModel.updateConfig(newConfig);
      _updateCurveData(newConfig);
    }
  }

  void _onTuneGestureEnd() {
    setState(() {
      _tuneParameter = null;
    });
  }

  Future<void> _saveConfig() async {
    final configModel = context.read<ConfigModel>();
    final api = context.read<RhythmApi>();

    configModel.setLoading(true);
    try {
      await api.saveConfig(configModel.rawConfig);
      // Track config save
      AnalyticsService().logConfigSaved();
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: const Text('Configuration saved'),
            backgroundColor: CelestialColors.accentBlue.withValues(alpha: 0.9),
            behavior: SnackBarBehavior.floating,
            shape: RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(8),
            ),
          ),
        );
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text('Failed to save: $e'),
            backgroundColor: Colors.red.shade700,
            behavior: SnackBarBehavior.floating,
            shape: RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(8),
            ),
          ),
        );
      }
    } finally {
      configModel.setLoading(false);
    }
  }

  int _getBrightnessAtHour(double hour) {
    if (_curveData == null) return 50;
    final data = _curveData!;
    if (data.hours.isEmpty) return 50;

    int idx = 0;
    double minDiff = double.infinity;
    for (int i = 0; i < data.hours.length; i++) {
      final diff = (data.hours[i] - hour).abs();
      if (diff < minDiff) {
        minDiff = diff;
        idx = i;
      }
    }
    return data.brightness[idx];
  }

  int _getKelvinAtHour(double hour) {
    if (_curveData == null) return 4000;
    final data = _curveData!;
    if (data.hours.isEmpty) return 4000;

    int idx = 0;
    double minDiff = double.infinity;
    for (int i = 0; i < data.hours.length; i++) {
      final diff = (data.hours[i] - hour).abs();
      if (diff < minDiff) {
        minDiff = diff;
        idx = i;
      }
    }
    return data.kelvin[idx];
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: _buildBody(),
    );
  }

  Widget _buildBody() {
    if (_isLoading) {
      return Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            SizedBox(
              width: 48,
              height: 48,
              child: CircularProgressIndicator(
                color: CelestialColors.accentBlue.withValues(alpha: 0.8),
                strokeWidth: 2,
              ),
            ),
            const SizedBox(height: 16),
            Text(
              'Loading curves...',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                fontSize: 14,
                letterSpacing: 0.5,
              ),
            ),
          ],
        ),
      );
    }

    if (_error != null) {
      return _buildErrorState();
    }

    return Consumer<ConfigModel>(
      builder: (context, model, child) {
        // Calculate exact values using sigmoid formula
        final api = context.read<RhythmApi>();
        int curveBrightness;
        int kelvin;

        if (api is HybridApiClient) {
          final values = api.calculateLighting(
            config: model.config,
            currentHour: model.selectedHour,
          );
          if (values != null) {
            curveBrightness = values.brightness;
            kelvin = values.kelvin;
          } else {
            // Fallback to array lookup
            curveBrightness = _getBrightnessAtHour(model.selectedHour);
            kelvin = _getKelvinAtHour(model.selectedHour);
          }
        } else {
          curveBrightness = _getBrightnessAtHour(model.selectedHour);
          kelvin = _getKelvinAtHour(model.selectedHour);
        }

        final brightness = _manualBrightness ?? curveBrightness;

        return SafeArea(
          bottom: false,
          child: GestureDetector(
            onTap: _isTuneMode ? _exitTuneMode : null,
            behavior: HitTestBehavior.opaque,
            child: Stack(
              children: [
                Column(
                  children: [
                    // Minimal header with actions
                    _buildHeader(),
                    // Main interaction area - swap based on mode
                    Expanded(
                      flex: 3,
                      child: Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 20),
                        child: Center(
                          child: AnimatedSwitcher(
                            duration: const Duration(milliseconds: 400),
                            switchInCurve: Curves.easeOutExpo,
                            switchOutCurve: Curves.easeInExpo,
                            child: _isEditMode
                                ? PolarDayEditor(
                                    key: const ValueKey('edit'),
                                    curveData: _curveData,
                                    config: model.config,
                                    onConfigChanged: _onEditConfigChanged,
                                    onConfigChangeEnd: () {
                                      // Optional: auto-save or show unsaved indicator
                                    },
                                  )
                                : SolarOrbit(
                                    key: const ValueKey('view'),
                                    curveData: _curveData,
                                    selectedHour: model.selectedHour,
                                    isTuneMode: _isTuneMode,
                                    isLightOn: _isLightOn,
                                    brightness: brightness,
                                    onHourChanged: _onHourChanged,
                                    onSunTap: _toggleLight,
                                    onBrightnessChanged: _onBrightnessChanged,
                                    onBrightnessChangeEnd:
                                        _onBrightnessChangeEnd,
                                    onTuneModeRequested: _enterTuneMode,
                                    onTuneGesture: _onTuneGesture,
                                    onTuneGestureEnd: _onTuneGestureEnd,
                                  ),
                          ),
                        ),
                      ),
                    ),
                    // Light output display (hidden in edit mode)
                    if (!_isEditMode)
                      Padding(
                        padding: const EdgeInsets.fromLTRB(24, 8, 24, 0),
                        child: LightOutputDisplay(
                          brightness: brightness,
                          kelvin: kelvin,
                          selectedHour: model.selectedHour,
                        ),
                      ),
                    if (!_isEditMode) const SizedBox(height: 20),
                    // Time scrubber (hidden in edit mode)
                    if (!_isEditMode)
                      Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 24),
                        child: TimeScrubber(
                          curveData: _curveData,
                          selectedHour: model.selectedHour,
                          onHourChanged: _onHourChanged,
                        ),
                      ),
                    // Edit mode hint
                    if (_isEditMode)
                      Padding(
                        padding: const EdgeInsets.fromLTRB(24, 16, 24, 0),
                        child: Text(
                          'Drag morning or evening handles to shape transitions',
                          style: TextStyle(
                            color: CelestialColors.textSecondary
                                .withValues(alpha: 0.6),
                            fontSize: 13,
                            letterSpacing: 0.3,
                          ),
                          textAlign: TextAlign.center,
                        ),
                      ),
                    // Bottom padding for nav overlay (gear button + safe area)
                    const SizedBox(height: 110),
                  ],
                ),
                // Tune mode overlay
                if (_isTuneMode) _buildTuneModeOverlay(),
              ],
            ),
          ),
        );
      },
    );
  }

  Widget _buildHeader() {
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 12, 20, 0),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.end,
        children: [
          // "Now" button - appears when user has manually dragged (only in view mode)
          if (!_isFollowingNow && !_isEditMode)
            Padding(
              padding: const EdgeInsets.only(right: 12),
              child: _buildActionButton(
                icon: Icons.my_location_rounded,
                onTap: _resetToNow,
                tooltip: 'Reset to now',
              ),
            ),
          _buildActionButton(
            icon: Icons.refresh_rounded,
            onTap: _loadData,
          ),
          const SizedBox(width: 12),
          Consumer<ConfigModel>(
            builder: (context, configModel, child) {
              return _buildActionButton(
                icon: Icons.check_rounded,
                onTap: configModel.isLoading ? null : _saveConfig,
                isLoading: configModel.isLoading,
                isPrimary: true,
              );
            },
          ),
        ],
      ),
    );
  }

  Widget _buildActionButton({
    required IconData icon,
    required VoidCallback? onTap,
    bool isLoading = false,
    bool isPrimary = false,
    bool isActive = false,
    String? tooltip,
  }) {
    // Active state uses warm sun color
    final isHighlighted = isPrimary || isActive;
    final highlightColor =
        isActive ? CelestialColors.sunWarm : CelestialColors.accentBlue;

    Widget button = GestureDetector(
      onTap: onTap,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 200),
        width: 44,
        height: 44,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          color: isHighlighted
              ? highlightColor.withValues(alpha: 0.15)
              : CelestialColors.backgroundCard.withValues(alpha: 0.6),
          border: Border.all(
            color: isHighlighted
                ? highlightColor.withValues(alpha: 0.4)
                : CelestialColors.orbitRing.withValues(alpha: 0.4),
            width: 1,
          ),
        ),
        child: isLoading
            ? Padding(
                padding: const EdgeInsets.all(12),
                child: CircularProgressIndicator(
                  strokeWidth: 2,
                  color: isHighlighted
                      ? highlightColor
                      : CelestialColors.textSecondary,
                ),
              )
            : Icon(
                icon,
                color: isHighlighted
                    ? highlightColor
                    : CelestialColors.textSecondary.withValues(alpha: 0.8),
                size: 20,
              ),
      ),
    );

    if (tooltip != null) {
      button = Tooltip(message: tooltip, child: button);
    }

    return button;
  }

  Widget _buildErrorState() {
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Container(
              width: 80,
              height: 80,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: CelestialColors.backgroundCard,
                border: Border.all(
                  color: CelestialColors.orbitRing.withValues(alpha: 0.3),
                ),
              ),
              child: Icon(
                Icons.cloud_off_rounded,
                size: 36,
                color: CelestialColors.textSecondary.withValues(alpha: 0.5),
              ),
            ),
            const SizedBox(height: 24),
            const Text(
              'Unable to connect',
              style: TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.3,
              ),
            ),
            const SizedBox(height: 8),
            Text(
              _error!,
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                fontSize: 13,
                height: 1.4,
              ),
              textAlign: TextAlign.center,
            ),
            const SizedBox(height: 32),
            GestureDetector(
              onTap: _loadData,
              child: Container(
                padding:
                    const EdgeInsets.symmetric(horizontal: 24, vertical: 12),
                decoration: BoxDecoration(
                  color: CelestialColors.accentBlue.withValues(alpha: 0.15),
                  borderRadius: BorderRadius.circular(24),
                  border: Border.all(
                    color: CelestialColors.accentBlue.withValues(alpha: 0.4),
                  ),
                ),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Icon(
                      Icons.refresh_rounded,
                      color: CelestialColors.accentBlue,
                      size: 18,
                    ),
                    const SizedBox(width: 8),
                    Text(
                      'Retry',
                      style: TextStyle(
                        color: CelestialColors.accentBlue,
                        fontSize: 14,
                        fontWeight: FontWeight.w600,
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

  Widget _buildTuneModeOverlay() {
    // Position above the bottom controls (TimeScrubber + nav padding)
    // TimeScrubber area: ~60px + 88px bottom padding = ~148px
    return Positioned(
      top: 0,
      left: 0,
      right: 0,
      bottom: 160,
      child: Container(
        color: CelestialColors.backgroundDark.withValues(alpha: 0.7),
        child: Column(
          children: [
            const Spacer(),
            Container(
              margin: const EdgeInsets.fromLTRB(24, 24, 24, 16),
              padding: const EdgeInsets.all(20),
              decoration: BoxDecoration(
                color: CelestialColors.backgroundCard,
                borderRadius: BorderRadius.circular(20),
                border: Border.all(
                  color: CelestialColors.accentBlue.withValues(alpha: 0.3),
                ),
                boxShadow: [
                  BoxShadow(
                    color: CelestialColors.accentBlue.withValues(alpha: 0.1),
                    blurRadius: 24,
                    spreadRadius: 0,
                  ),
                ],
              ),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Row(
                    children: [
                      Container(
                        width: 32,
                        height: 32,
                        decoration: BoxDecoration(
                          shape: BoxShape.circle,
                          color: CelestialColors.accentBlue
                              .withValues(alpha: 0.15),
                        ),
                        child: const Icon(
                          Icons.tune_rounded,
                          color: CelestialColors.accentBlue,
                          size: 16,
                        ),
                      ),
                      const SizedBox(width: 12),
                      const Text(
                        'Tune Mode',
                        style: TextStyle(
                          color: CelestialColors.textPrimary,
                          fontSize: 16,
                          fontWeight: FontWeight.w600,
                          letterSpacing: 0.3,
                        ),
                      ),
                      const Spacer(),
                      if (_tuneParameter != null)
                        Container(
                          padding: const EdgeInsets.symmetric(
                            horizontal: 10,
                            vertical: 4,
                          ),
                          decoration: BoxDecoration(
                            color: CelestialColors.accentBlue
                                .withValues(alpha: 0.15),
                            borderRadius: BorderRadius.circular(12),
                          ),
                          child: Text(
                            _tuneParameter!,
                            style: const TextStyle(
                              color: CelestialColors.accentBlue,
                              fontSize: 13,
                              fontWeight: FontWeight.w500,
                            ),
                          ),
                        ),
                    ],
                  ),
                  const SizedBox(height: 16),
                  Text(
                    'Drag vertically to adjust brightness\nPinch to adjust shape (peak flatness)\nDrag horizontally to adjust width (ramp speed)\n\nTap anywhere to exit',
                    style: TextStyle(
                      color:
                          CelestialColors.textSecondary.withValues(alpha: 0.8),
                      fontSize: 13,
                      height: 1.6,
                    ),
                    textAlign: TextAlign.center,
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}
