import 'package:rhythm_app/services/apple_health_service.dart';

class FakeAppleHealthWriter implements AppleHealthWriter {
  AppleHealthAuthorization state = AppleHealthAuthorization.authorized;
  int requests = 0;
  bool failAfterSave = false;
  Future<AppleHealthAuthorization>? requestResult;
  final calls = <List<Map<String, Object>>>[];
  final samples = <String, Map<String, Object>>{};

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
      samples[interval['sync_id'] as String] = interval;
    }
    if (failAfterSave) throw StateError('simulated lost acknowledgement');
  }
}
