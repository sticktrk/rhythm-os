import 'package:rhythm_app/backend/backend.dart';

class CapturedAnalyticsEvent {
  const CapturedAnalyticsEvent(this.name, this.properties);

  final String name;
  final Map<String, Object> properties;
}

class CapturingAnalyticsBackend implements AnalyticsBackend {
  bool _initialized = false;
  final List<CapturedAnalyticsEvent> events = [];
  final List<String> screens = [];

  @override
  bool get isInitialized => _initialized;

  @override
  Future<void> initialize() async {
    _initialized = true;
  }

  @override
  Future<void> logEvent(
    String name, [
    Map<String, Object>? params,
  ]) async {
    events.add(CapturedAnalyticsEvent(name, params ?? const {}));
  }

  @override
  Future<void> logScreenView(String screenName) async {
    screens.add(screenName);
  }

  @override
  Future<void> setUserId(String? userId) async {}

  @override
  Future<void> setUserProperty(String name, String? value) async {}

  @override
  void dispose() {
    _initialized = false;
  }
}
