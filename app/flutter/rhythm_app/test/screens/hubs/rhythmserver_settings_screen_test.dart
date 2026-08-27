import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/hubs/rhythmserver_settings_screen.dart';

void main() {
  group('factoryResetFailureMessage', () {
    test('claims a safe pre-reset stop only for HTTP 400', () {
      expect(
        factoryResetFailureMessage(
          httpStatus: 400,
          error: 'Hue Bluetooth bulb is unreachable',
          hubName: 'Rhythm Light Box',
        ),
        'Factory reset stopped safely: Hue Bluetooth bulb is unreachable',
      );
      expect(
        factoryResetFailureMessage(
          httpStatus: 400,
          error: null,
          hubName: 'Rhythm Light Box',
        ),
        'Factory reset stopped safely before any settings were erased.',
      );
    });

    test('does not claim an unconfirmed reset stopped before changes', () {
      expect(
        factoryResetFailureMessage(
          httpStatus: 500,
          error: 'Request timed out',
          hubName: 'Rhythm Light Box',
        ),
        'Factory reset could not be confirmed: Request timed out',
      );
      expect(
        factoryResetFailureMessage(
          httpStatus: null,
          error: null,
          hubName: 'Rhythm Light Box',
        ),
        'Factory reset could not be confirmed for Rhythm Light Box.',
      );
    });
  });

  group('removed Matter recovery state', () {
    test('enables retry only when the secret-free projection says available',
        () {
      final available = <String, dynamic>{'recovery_available': true};
      final missing = <String, dynamic>{'recovery_available': false};

      expect(removedMatterRecoveryAvailable('matter', available), isTrue);
      expect(removedMatterRecoveryAvailable('matter', missing), isFalse);
      expect(removedMatterRecoveryAvailable('hue', available), isFalse);
      expect(
        removedDeviceRecoveryStatus('matter', available),
        'Saved for Matter retry',
      );
      expect(
        removedDeviceRecoveryStatus('matter', missing),
        'Saved setup code unavailable — add again with its code',
      );
      expect(
        removedDeviceRecoveryStatus('hue', missing),
        'Archived from active rooms',
      );
    });
  });
}
