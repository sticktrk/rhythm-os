import 'dart:async';
import 'package:flutter/foundation.dart' show debugPrint, kIsWeb;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import '../api/hybrid_client.dart';
import '../models/config_model.dart';
import '../providers/home_provider.dart';
import '../widgets/curve_chart.dart';
import '../widgets/slider_controls.dart';

/// Legacy slider-based designer screen for curve configuration.
class DesignerScreenLegacy extends StatefulWidget {
  const DesignerScreenLegacy({super.key});

  @override
  State<DesignerScreenLegacy> createState() => _DesignerScreenLegacyState();
}

class _DesignerScreenLegacyState extends State<DesignerScreenLegacy> {
  CurveData? _curveData;
  bool _isLoading = true;
  String? _error;
  Timer? _nowTimer;
  double _currentHour = 0;

  @override
  void initState() {
    super.initState();
    _setLandscapeMode();
    _updateCurrentHour();
    _startNowTimer();
    _loadData();
  }

  @override
  void dispose() {
    _restoreOrientation();
    _nowTimer?.cancel();
    super.dispose();
  }

  /// Force landscape orientation on mobile devices.
  void _setLandscapeMode() {
    if (!kIsWeb) {
      SystemChrome.setPreferredOrientations([
        DeviceOrientation.landscapeLeft,
        DeviceOrientation.landscapeRight,
      ]).catchError((_) {});
    }
  }

  /// Restore all orientations when leaving the screen.
  void _restoreOrientation() {
    if (!kIsWeb) {
      SystemChrome.setPreferredOrientations([
        DeviceOrientation.portraitUp,
        DeviceOrientation.portraitDown,
        DeviceOrientation.landscapeLeft,
        DeviceOrientation.landscapeRight,
      ]).catchError((_) {});
    }
  }

  void _updateCurrentHour() {
    final now = DateTime.now();
    setState(() {
      _currentHour = now.hour + now.minute / 60.0;
    });
  }

  void _startNowTimer() {
    // Update every 60 seconds
    _nowTimer = Timer.periodic(const Duration(seconds: 60), (_) {
      _updateCurrentHour();
    });
  }

  Future<void> _loadData() async {
    setState(() {
      _isLoading = true;
      _error = null;
    });

    try {
      final api = context.read<RhythmApi>();
      final configModel = context.read<ConfigModel>();
      final homeProvider = context.read<HomeProvider>();

      // Load curve data from API (computed values based on solar position)
      final curveData = await api.getCurveData();

      if (mounted) {
        // Load curve config from Home (Supabase) first, fall back to API
        final homeCurveConfig = homeProvider.currentHome?.curveConfig;
        if (homeCurveConfig != null) {
          // Use curve config from Supabase
          configModel.updateConfig(homeCurveConfig);
          debugPrint(
              'DesignerScreen: Loaded curve config from Home (Supabase)');
        } else {
          // Fall back to API config state
          final configState = await api.getConfigState();
          configModel.updateFromConfigState(configState);
          debugPrint('DesignerScreen: Loaded curve config from API (fallback)');
        }

        // Update active half based on solar noon
        final solarNoon = curveData.solar.solarNoon;
        configModel.setSelectedHour(configModel.selectedHour,
            solarNoon: solarNoon);

        setState(() {
          _curveData = curveData;
          _isLoading = false;
        });

        // Recalculate curve data with loaded config (initial fetch used defaults)
        _updateCurveData(configModel.config);
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

      // Try high-res curves first for smoother rendering (sync call)
      CurveData? curveData;
      if (api is HybridApiClient) {
        curveData = api.getCurveDataHighRes(
          config: config,
          samplesPerHour: 4,
        );
      }

      // Fall back to standard resolution if high-res not available
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

  void _onHourSelected(double hour) {
    final configModel = context.read<ConfigModel>();
    final solarNoon = _curveData?.solar.solarNoon;

    configModel.setSelectedHour(hour, solarNoon: solarNoon);
  }

  Future<void> _saveConfig() async {
    final configModel = context.read<ConfigModel>();
    final homeProvider = context.read<HomeProvider>();

    configModel.setLoading(true);
    try {
      // Save to Supabase via HomeProvider
      if (homeProvider.currentHome != null) {
        final success =
            await homeProvider.updateCurrentHomeCurveConfig(configModel.config);
        if (!success) {
          throw Exception('Failed to update curve config in Supabase');
        }
      } else {
        throw Exception('No home selected');
      }

      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(
            content: Text('Configuration saved'),
            backgroundColor: Colors.green,
          ),
        );
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text('Failed to save: $e'),
            backgroundColor: Colors.red,
          ),
        );
      }
    } finally {
      configModel.setLoading(false);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('Rhythm Tuner'),
        backgroundColor: const Color(0xFF16213E),
        actions: [
          IconButton(
            icon: const Icon(Icons.refresh),
            onPressed: _loadData,
            tooltip: 'Refresh',
          ),
          Consumer<ConfigModel>(
            builder: (context, model, child) {
              return IconButton(
                icon: model.isLoading
                    ? const SizedBox(
                        width: 20,
                        height: 20,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : const Icon(Icons.save),
                onPressed: model.isLoading ? null : _saveConfig,
                tooltip: 'Save',
              );
            },
          ),
        ],
      ),
      body: _buildBody(),
    );
  }

  Widget _buildBody() {
    if (_isLoading) {
      return const Center(
        child: CircularProgressIndicator(),
      );
    }

    if (_error != null) {
      return Center(
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(
              Icons.error_outline,
              size: 64,
              color: Colors.red.shade300,
            ),
            const SizedBox(height: 16),
            Text(
              'Failed to load',
              style: Theme.of(context).textTheme.headlineSmall,
            ),
            const SizedBox(height: 8),
            Text(
              _error!,
              style: const TextStyle(color: Colors.white54),
              textAlign: TextAlign.center,
            ),
            const SizedBox(height: 24),
            ElevatedButton.icon(
              onPressed: _loadData,
              icon: const Icon(Icons.refresh),
              label: const Text('Retry'),
            ),
          ],
        ),
      );
    }

    return LayoutBuilder(
      builder: (context, constraints) {
        final isWide = constraints.maxWidth > 800;
        final chartHeight = isWide ? 400.0 : 280.0;

        return SingleChildScrollView(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              // Full-width chart section
              SizedBox(
                height: chartHeight,
                child: _buildChartSection(),
              ),
              // Controls section below (already has internal padding)
              _buildControlsSection(),
            ],
          ),
        );
      },
    );
  }

  Widget _buildChartSection() {
    return Consumer<ConfigModel>(
      builder: (context, model, child) {
        return Container(
          padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  const Spacer(),
                  // Active half indicator
                  Container(
                    padding:
                        const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
                    decoration: BoxDecoration(
                      color: model.activeHalf == 'morning'
                          ? Colors.amber.withValues(alpha: 0.2)
                          : Colors.indigo.withValues(alpha: 0.2),
                      borderRadius: BorderRadius.circular(4),
                    ),
                    child: Text(
                      model.activeHalf == 'morning'
                          ? '☀ Morning'
                          : '🌙 Evening',
                      style: TextStyle(
                        color: model.activeHalf == 'morning'
                            ? Colors.amber
                            : Colors.indigo.shade200,
                        fontSize: 12,
                      ),
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 12),
              Expanded(
                child: CurveChart(
                  data: _curveData,
                  currentHour: _currentHour,
                  selectedHour: model.selectedHour,
                  showNowMarker: true,
                  showSolarContext: model.showSolarContext,
                  onHourSelected: _onHourSelected,
                ),
              ),
            ],
          ),
        );
      },
    );
  }

  Widget _buildControlsSection() {
    return Consumer<ConfigModel>(
      builder: (context, model, child) {
        return SliderControls(
          config: model.config,
          solarNoonHour: _curveData?.solar.solarNoon,
          onConfigChanged: (newConfig) {
            model.updateConfig(newConfig);
            _updateCurveData(newConfig);
          },
        );
      },
    );
  }
}
