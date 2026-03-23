import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';

/// Mock ConfigModel for testing widgets that depend on it.
class MockConfigModel extends ChangeNotifier {
  RawConfig _rawConfig = RawConfig.defaults();
  CurveConfigDto _config = CurveConfigDto.default_();
  SolarContext _solar = SolarContext.defaults();
  double _selectedHour = 12.0;
  String _activeHalf = 'morning';
  bool _calloutAutoFollow = true;
  int _stepCount = 10;
  bool _showSteps = true;
  bool _showSolarContext = true;
  bool _use12Hour = true;
  int _month = 6;
  bool _isLoading = false;
  String? _error;

  // Getters
  RawConfig get rawConfig => _rawConfig;
  CurveConfigDto get config => _config;
  SolarContext get solar => _solar;
  double get selectedHour => _selectedHour;
  String get activeHalf => _activeHalf;
  bool get calloutAutoFollow => _calloutAutoFollow;
  int get stepCount => _stepCount;
  bool get showSteps => _showSteps;
  bool get showSolarContext => _showSolarContext;
  bool get use12Hour => _use12Hour;
  int get month => _month;
  bool get isLoading => _isLoading;
  String? get error => _error;

  // Track method calls
  int notifyCount = 0;

  @override
  void notifyListeners() {
    notifyCount++;
    super.notifyListeners();
  }

  void setSelectedHour(double hour, {double? solarNoon}) {
    _selectedHour = hour.clamp(0.0, 24.0);
    _calloutAutoFollow = false;
    if (solarNoon != null) {
      _activeHalf = hour < solarNoon ? 'morning' : 'evening';
    }
    notifyListeners();
  }

  void setActiveHalf(String half) {
    _activeHalf = half;
    notifyListeners();
  }

  void setShowSteps(bool value) {
    _showSteps = value;
    notifyListeners();
  }

  void setShowSolarContext(bool value) {
    _showSolarContext = value;
    notifyListeners();
  }

  void setStepCount(int count) {
    _stepCount = count.clamp(1, 100);
    notifyListeners();
  }

  void setUse12Hour(bool value) {
    _use12Hour = value;
    notifyListeners();
  }

  void setMonth(int month) {
    _month = month.clamp(1, 12);
    notifyListeners();
  }

  void setLoading(bool loading) {
    _isLoading = loading;
    notifyListeners();
  }

  void setError(String? error) {
    _error = error;
    notifyListeners();
  }

  void resetCursor() {
    final now = DateTime.now();
    _selectedHour = now.hour + now.minute / 60.0;
    _calloutAutoFollow = true;
    notifyListeners();
  }

  /// Reset all state to defaults and clear call count.
  void reset() {
    _rawConfig = RawConfig.defaults();
    _config = CurveConfigDto.default_();
    _solar = SolarContext.defaults();
    _selectedHour = 12.0;
    _activeHalf = 'morning';
    _calloutAutoFollow = true;
    _stepCount = 10;
    _showSteps = true;
    _showSolarContext = true;
    _use12Hour = true;
    _month = 6;
    _isLoading = false;
    _error = null;
    notifyCount = 0;
  }
}

/// Mock RoomProvider for testing.
class MockRoomProvider extends ChangeNotifier {
  RunnerStateDto _state = createRunnerState();
  int _currentIndex = 0;
  bool _initialized = false;

  List<RoomDto> get rooms => _state.rooms;
  List<RoomDto> get enabledRooms => runnerGetEnabledRooms(state: _state);
  RoomDto? get currentRoom => rooms.isNotEmpty && _currentIndex < rooms.length
      ? rooms[_currentIndex]
      : null;
  int get currentIndex => _currentIndex;
  bool get initialized => _initialized;
  RunnerStateDto get state => _state;
  bool get hasRooms => rooms.isNotEmpty;
  int get roomCount => rooms.length;

  // Track method calls
  int addRoomCallCount = 0;
  int removeRoomCallCount = 0;

  Future<void> initialize() async {
    _initialized = true;
    notifyListeners();
  }

  void addRoom(RoomDto room) {
    addRoomCallCount++;
    _state = runnerAddRoom(state: _state, room: room);
    notifyListeners();
  }

  void removeRoom(String roomId) {
    removeRoomCallCount++;
    _state = runnerRemoveRoom(state: _state, roomId: roomId);
    if (_currentIndex >= _state.rooms.length && _state.rooms.isNotEmpty) {
      _currentIndex = _state.rooms.length - 1;
    }
    notifyListeners();
  }

  void setCurrentIndex(int index) {
    if (index >= 0 && index < rooms.length && index != _currentIndex) {
      _currentIndex = index;
      notifyListeners();
    }
  }

  void clearAllRooms() {
    _state = createRunnerState();
    _currentIndex = 0;
    notifyListeners();
  }

  void reset() {
    _state = createRunnerState();
    _currentIndex = 0;
    _initialized = false;
    addRoomCallCount = 0;
    removeRoomCallCount = 0;
  }
}

/// Factory for creating test rooms.
class TestRoomFactory {
  static int _idCounter = 0;

  /// Create a test room with default values.
  static RoomDto createRoom({
    String? id,
    String name = 'Test Room',
    RoomSourceDto source = RoomSourceDto.hue,
    List<String> deviceIds = const [],
    bool disabled = false,
  }) {
    _idCounter++;
    return RoomDto.raw(
      id: id ?? 'room_$_idCounter',
      name: name,
      source: source,
      deviceIds: deviceIds,
      disabled: disabled,
      rhythmEnabled: false,
      lightsOn: false,
      timeOffsetMinutes: 0.0,
      brightnessOffset: 0.0,
      curveConfig: null,
    );
  }

  /// Reset the ID counter.
  static void resetCounter() {
    _idCounter = 0;
  }
}
