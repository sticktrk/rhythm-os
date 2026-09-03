import 'matter_bulb_tester_screen.dart';

/// Canonical Bulb Audition entry point. The deprecated implementation class
/// remains behind this wrapper for one app release so existing deep links and
/// tests continue to compile.
class BulbAuditionScreen extends MatterBulbTesterScreen {
  const BulbAuditionScreen({
    super.key,
    required super.device,
    required super.nativeDeviceId,
  });
}
