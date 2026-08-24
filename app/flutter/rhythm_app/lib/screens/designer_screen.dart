import 'dart:async';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import '../api/hybrid_client.dart';
import '../models/config_model.dart';
import '../providers/home_provider.dart';
import '../widgets/rhythm_tuner_chart.dart';

/// Interactive curve designer screen using draggable handles.
///
/// Curve rendering uses FFI (getCurveDataHighRes) on every config change
/// to guarantee the displayed curve is identical to what drives the lights.
class DesignerScreen extends StatefulWidget {
  const DesignerScreen({super.key});

  @override
  State<DesignerScreen> createState() => _DesignerScreenState();
}

class _DesignerScreenState extends State<DesignerScreen> {
  bool _isLoading = true;
  String? _error;
  double _sunrise = 6.5;
  double _sunset = 18.5;
  CurveData? _curveData;

  @override
  void initState() {
    super.initState();
    _loadData();
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

      final curveData = await api.getCurveData();

      if (mounted) {
        // Load config from Home (Supabase) first, fall back to API
        final homeCurveConfig = homeProvider.currentHome?.curveConfig;
        if (homeCurveConfig != null) {
          configModel.updateConfig(homeCurveConfig);
          configModel.markAsSaved();
          debugPrint('DesignerScreen: Loaded curve config from Home');
        } else {
          final configState = await api.getConfigState();
          configModel.updateFromConfigState(configState);
          debugPrint('DesignerScreen: Loaded curve config from API');
        }

        setState(() {
          _sunrise = curveData.solar.sunrise ?? 6.5;
          _sunset = curveData.solar.sunset ?? 18.5;
          _curveData = curveData;
          _isLoading = false;
        });

        // Recalculate with loaded config (initial fetch used defaults)
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

  /// Recalculate curve data via Rust FFI. Called on every config change
  /// during handle drag to keep the displayed curve in exact sync with
  /// the lighting engine.
  void _updateCurveData(CurveConfigDto config) {
    final api = context.read<RhythmApi>();

    CurveData? curveData;
    if (api is HybridApiClient) {
      curveData = api.getCurveDataHighRes(
        config: config,
        samplesPerHour: 10,
      );
    }

    if (curveData != null && mounted) {
      setState(() => _curveData = curveData);
    }
  }

  Future<void> _saveConfig() async {
    final configModel = context.read<ConfigModel>();
    final homeProvider = context.read<HomeProvider>();

    configModel.setLoading(true);
    try {
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
        configModel.markAsSaved();
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
      backgroundColor: const Color(0xFF080910),
      appBar: _buildAppBar(),
      body: _buildBody(),
    );
  }

  PreferredSizeWidget _buildAppBar() {
    return AppBar(
      title: const Text(
        'Rhythm Tuner',
        style: TextStyle(
          fontWeight: FontWeight.w600,
          letterSpacing: 0.5,
        ),
      ),
      backgroundColor: const Color(0xFF0C0D18),
      elevation: 0,
      actions: [
        Consumer<ConfigModel>(
          builder: (context, model, _) => Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              IconButton(
                icon: const Icon(Icons.undo),
                onPressed: model.canUndo
                    ? () {
                        model.undo();
                        _updateCurveData(model.config);
                      }
                    : null,
                tooltip: 'Undo',
              ),
              IconButton(
                icon: const Icon(Icons.redo),
                onPressed: model.canRedo
                    ? () {
                        model.redo();
                        _updateCurveData(model.config);
                      }
                    : null,
                tooltip: 'Redo',
              ),
            ],
          ),
        ),
        _OverflowMenu(
          onReload: _loadData,
          onResetToSaved: () {
            final model = context.read<ConfigModel>();
            model.resetToSaved();
            _updateCurveData(model.config);
          },
          onResetToDefaults: () {
            final model = context.read<ConfigModel>();
            model.resetToDefaults();
            _updateCurveData(model.config);
          },
        ),
        Consumer<ConfigModel>(
          builder: (context, model, _) => IconButton(
            icon: model.isLoading
                ? const SizedBox(
                    width: 20,
                    height: 20,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  )
                : const Icon(Icons.save),
            onPressed: model.isLoading ? null : _saveConfig,
            tooltip: 'Save',
          ),
        ),
      ],
    );
  }

  Widget _buildBody() {
    if (_isLoading) {
      return const Center(
        child: CircularProgressIndicator(
          color: Color(0xFFFFB74D),
        ),
      );
    }

    if (_error != null) {
      return Center(
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(Icons.error_outline, size: 64, color: Colors.red.shade300),
            const SizedBox(height: 16),
            Text('Failed to load',
                style: Theme.of(context).textTheme.headlineSmall),
            const SizedBox(height: 8),
            Text(_error!,
                style: const TextStyle(color: Colors.white54),
                textAlign: TextAlign.center),
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

    return Consumer<ConfigModel>(
      builder: (context, model, child) {
        return RhythmTunerChart(
          config: model.config,
          curveData: _curveData,
          sunrise: _sunrise,
          sunset: _sunset,
          showSolarContext: model.showSolarContext,
          onDragStart: () => model.pushUndoSnapshot(),
          onConfigChanged: (newConfig) {
            model.updateConfig(newConfig);
            _updateCurveData(newConfig);
          },
        );
      },
    );
  }
}

/// Extracted to its own StatefulWidget so the PopupMenuButton's element
/// is never replaced by parent rebuilds (which would close the popup).
class _OverflowMenu extends StatelessWidget {
  final VoidCallback onReload;
  final VoidCallback onResetToSaved;
  final VoidCallback onResetToDefaults;

  const _OverflowMenu({
    required this.onReload,
    required this.onResetToSaved,
    required this.onResetToDefaults,
  });

  @override
  Widget build(BuildContext context) {
    return PopupMenuButton<String>(
      icon: const Icon(Icons.more_vert),
      onSelected: (value) {
        switch (value) {
          case 'reload':
            onReload();
            break;
          case 'reset':
            onResetToSaved();
            break;
          case 'defaults':
            onResetToDefaults();
            break;
          case 'solar':
            context.read<ConfigModel>().setShowSolarContext(
                !context.read<ConfigModel>().showSolarContext);
            break;
        }
      },
      itemBuilder: (_) {
        final model = context.read<ConfigModel>();
        return [
          PopupMenuItem(
            value: 'solar',
            child: Row(
              children: [
                Icon(
                  model.showSolarContext
                      ? Icons.check_box
                      : Icons.check_box_outline_blank,
                  size: 20,
                  color: model.showSolarContext
                      ? const Color(0xFFFFD54F)
                      : Colors.white38,
                ),
                const SizedBox(width: 12),
                const Text('Solar context'),
              ],
            ),
          ),
          const PopupMenuDivider(),
          const PopupMenuItem(
            value: 'reload',
            child: Text('Reload from cloud'),
          ),
          PopupMenuItem(
            value: 'reset',
            enabled: model.canResetToSaved,
            child: const Text('Reset to saved'),
          ),
          const PopupMenuItem(
            value: 'defaults',
            child: Text('Reset to defaults'),
          ),
        ];
      },
    );
  }
}
