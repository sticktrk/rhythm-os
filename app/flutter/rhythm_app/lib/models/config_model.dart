import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';

/// State management for curve configuration.
///
/// Stores raw config and a CurveConfigDto for direct use in calculations.
/// With the super-Gaussian curve, RawConfig maps directly to CurveConfigDto.
class ConfigModel extends ChangeNotifier {
  /// Raw user configuration
  RawConfig _rawConfig = RawConfig.defaults();

  /// Solar context for resolving dynamic values
  SolarContext _solar = SolarContext.defaults();

  /// Config DTO for calculations (mirrors rawConfig)
  CurveConfigDto _config = CurveConfigDto.default_();

  // Undo/redo history
  final List<CurveConfigDto> _undoStack = [];
  final List<CurveConfigDto> _redoStack = [];
  static const int _maxUndoEntries = 30;
  CurveConfigDto? _savedConfig;

  bool _isLoading = false;
  String? _error;

  // Cursor state
  double _selectedHour = DateTime.now().hour + DateTime.now().minute / 60.0;
  String _activeHalf = 'morning';
  bool _calloutAutoFollow = true;

  // Display state
  bool _showSolarContext = true;

  // Display settings
  int _month = DateTime.now().month;

  /// Get raw config (for editing/saving)
  RawConfig get rawConfig => _rawConfig;

  /// Get solar context
  SolarContext get solar => _solar;

  /// Get config DTO (for calculations)
  CurveConfigDto get config => _config;

  bool get isLoading => _isLoading;
  String? get error => _error;

  // Cursor getters
  double get selectedHour => _selectedHour;
  String get activeHalf => _activeHalf;
  bool get calloutAutoFollow => _calloutAutoFollow;

  // Display getters
  bool get showSolarContext => _showSolarContext;

  // Display settings getters
  int get month => _month;

  /// Update from a full ConfigState (from API response).
  void updateFromConfigState(ConfigState state) {
    _rawConfig = state.config;
    _solar = state.solar;
    // Convert RawConfig to CurveConfigDto
    _config = ConfigState.rawConfigToDto(state.config);
    _savedConfig = _config;
    notifyListeners();
  }

  /// Update config DTO directly (for live preview during edits).
  /// Also syncs changes to rawConfig.
  void updateConfig(CurveConfigDto config) {
    _config = config;

    // Sync changes to rawConfig
    _rawConfig = _rawConfig.copyWith(
      minColorTemp: config.minColorTemp,
      maxColorTemp: config.maxColorTemp,
      minBrightness: config.minBrightness,
      maxBrightness: config.maxBrightness,
      widthLeftBri: config.widthLeftBri,
      widthRightBri: config.widthRightBri,
      widthLeftCct: config.widthLeftCct,
      widthRightCct: config.widthRightCct,
      shapeP: config.shapeP,
      maxDimSteps: config.maxDimSteps,
    );
    notifyListeners();
  }

  /// Update raw config (for saving and editing).
  void updateRawConfig(RawConfig config) {
    _rawConfig = config;
    // Convert to CurveConfigDto
    _config = ConfigState.rawConfigToDto(config);
    notifyListeners();
  }

  // Undo/redo getters
  bool get canUndo => _undoStack.isNotEmpty;
  bool get canRedo => _redoStack.isNotEmpty;
  bool get canResetToSaved => _savedConfig != null;
  bool get canResetToDefaults => true;

  /// Push current config onto undo stack (call before a drag begins).
  void pushUndoSnapshot() {
    _undoStack.add(_config);
    if (_undoStack.length > _maxUndoEntries) {
      _undoStack.removeAt(0);
    }
    _redoStack.clear();
  }

  /// Undo: pop from undo stack, push current to redo stack.
  void undo() {
    if (_undoStack.isEmpty) return;
    _redoStack.add(_config);
    final prev = _undoStack.removeLast();
    _config = prev;
    _syncRawConfigFromDto(prev);
    notifyListeners();
  }

  /// Redo: pop from redo stack, push current to undo stack.
  void redo() {
    if (_redoStack.isEmpty) return;
    _undoStack.add(_config);
    final next = _redoStack.removeLast();
    _config = next;
    _syncRawConfigFromDto(next);
    notifyListeners();
  }

  /// Reset to last saved config (pushes undo first so it's reversible).
  void resetToSaved() {
    if (_savedConfig == null) return;
    pushUndoSnapshot();
    _config = _savedConfig!;
    _syncRawConfigFromDto(_savedConfig!);
    notifyListeners();
  }

  /// Reset to factory defaults (pushes undo first so it's reversible).
  void resetToDefaults() {
    pushUndoSnapshot();
    _config = CurveConfigDto.default_();
    _syncRawConfigFromDto(_config);
    notifyListeners();
  }

  /// Mark current config as the saved baseline.
  void markAsSaved() {
    _savedConfig = _config;
  }

  void _syncRawConfigFromDto(CurveConfigDto config) {
    _rawConfig = _rawConfig.copyWith(
      minColorTemp: config.minColorTemp,
      maxColorTemp: config.maxColorTemp,
      minBrightness: config.minBrightness,
      maxBrightness: config.maxBrightness,
      widthLeftBri: config.widthLeftBri,
      widthRightBri: config.widthRightBri,
      widthLeftCct: config.widthLeftCct,
      widthRightCct: config.widthRightCct,
      shapeP: config.shapeP,
      maxDimSteps: config.maxDimSteps,
    );
  }

  void setLoading(bool loading) {
    _isLoading = loading;
    notifyListeners();
  }

  void setError(String? error) {
    _error = error;
    notifyListeners();
  }

  /// Update selected hour on chart cursor
  void setSelectedHour(double hour, {double? solarNoon}) {
    _selectedHour = hour.clamp(0.0, 24.0);
    _calloutAutoFollow = false;

    // Update active half based on solar noon
    if (solarNoon != null) {
      _activeHalf = hour < solarNoon ? 'morning' : 'evening';
    }

    notifyListeners();
  }

  /// Update active half directly
  void setActiveHalf(String half) {
    _activeHalf = half;
    notifyListeners();
  }

  /// Enable/disable auto-follow mode for callouts
  void setCalloutAutoFollow(bool value) {
    _calloutAutoFollow = value;
    notifyListeners();
  }

  /// Toggle solar context visibility on the chart.
  void setShowSolarContext(bool value) {
    _showSolarContext = value;
    notifyListeners();
  }

  /// Set test month
  void setMonth(int month) {
    _month = month.clamp(1, 12);
    notifyListeners();
  }

  /// Reset cursor to current time
  void resetCursor() {
    final now = DateTime.now();
    _selectedHour = now.hour + now.minute / 60.0;
    _calloutAutoFollow = true;
    notifyListeners();
  }
}
