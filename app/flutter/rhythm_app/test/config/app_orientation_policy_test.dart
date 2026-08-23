import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/config/app_orientation_policy.dart';

void main() {
  test('application orientation policy permits only landscape directions', () {
    expect(appPreferredOrientations, const [
      DeviceOrientation.landscapeLeft,
      DeviceOrientation.landscapeRight,
    ]);
    expect(appOrientationPolicyAnalyticsValue, 'landscape');
  });
}
