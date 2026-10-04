import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  test('previous appliance state preserves full backup and direct setup', () {
    final deployment = RhythmCapabilities.fromJson({}).deployment;
    expect(deployment.fullBackupExport, isTrue);
    expect(deployment.fullBackupImport, isTrue);
    expect(deployment.haDeviceManagement, isFalse);
    expect(deployment.portableSettings, isFalse);
  });

  test('explicit deployments fail closed for absent backup operations', () {
    final deployment = RhythmCapabilities.fromJson({
      'deployment': {
        'kind': 'home_assistant_addon',
        'ha_device_management': true,
        'direct_mobile_control': true,
        'event_streaming': true,
        'portable_profiles': true,
      },
    }).deployment;
    expect(deployment.haDeviceManagement, isTrue);
    expect(deployment.directMobileControl, isTrue);
    expect(deployment.portableProfiles, isTrue);
    expect(deployment.portableSettings, isFalse);
    expect(deployment.fullBackupExport, isFalse);
    expect(deployment.fullBackupImport, isFalse);
  });

  test('unknown or malformed deployment never enables whole backup', () {
    for (final value in [
      null,
      'invalid',
      {},
      {'kind': 'future'},
    ]) {
      final deployment = RhythmCapabilities.fromJson({
        'deployment': value,
      }).deployment;
      expect(deployment.fullBackupExport, isFalse);
      expect(deployment.fullBackupImport, isFalse);
    }
  });

  test(
    'portable settings needs explicit support and does not enable full restore',
    () {
      final deployment = RhythmCapabilities.fromJson({
        'deployment': {
          'kind': 'home_assistant_addon',
          'portable_settings': true,
        },
      }).deployment;
      expect(deployment.portableSettings, isTrue);
      expect(deployment.fullBackupImport, isFalse);
    },
  );
}
