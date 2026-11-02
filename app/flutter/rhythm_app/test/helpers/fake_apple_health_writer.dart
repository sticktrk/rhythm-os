import 'package:rhythm_app/services/apple_health_service.dart';

class FakeAppleHealthWriter implements AppleHealthWriter {
  AppleHealthAuthorization state = AppleHealthAuthorization.authorized;
  int requests = 0;
  bool failAfterSave = false;
  Future<AppleHealthAuthorization>? requestResult;
  final calls = <List<Map<String, Object>>>[];
  final samples = <String, Map<String, Object>>{};

  // HealthKit replaces an existing sync identifier only for a greater version.
  // Keep the native version projection covered by AppleHealthIntervalTests.swift.
  int version(Map<String, Object> interval) =>
      0x7fffffffffffffff - (interval['end_epoch_ms'] as int) + 1;

  @override
  Future<AppleHealthAuthorization> authorization() async => state;
  @override
  Future<AppleHealthAuthorization> requestAuthorization() async {
    requests++;
    return requestResult == null ? state : await requestResult!;
  }

  @override
  Future<void> saveIntervals(List<Map<String, Object>> intervals) async {
    calls.add(intervals);
    for (final interval in intervals) {
      final id = interval['sync_id'] as String;
      final previous = samples[id];
      if (previous == null || version(interval) > version(previous)) {
        samples[id] = interval;
      }
    }
    if (failAfterSave) throw StateError('simulated lost acknowledgement');
  }
}
