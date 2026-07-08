import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/hubs/ble_provisioning_screen.dart';
import 'package:rhythm_app/services/account_cloud_sync_service.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  group('bleProvisioningKnownHomeNameForTesting', () {
    test('matches a specific BLE device name to a saved server hub home', () {
      final home = Home.create(
        id: 'home-1',
        name: 'Lake House',
        ownerId: 'user-1',
      );
      final hub = Hub.server(
        id: 'hub-1',
        homeId: home.id,
        name: 'rhythm-rpiz-A1B2',
        host: '192.168.1.20',
      );

      final homeName = bleProvisioningKnownHomeNameForTesting(
        deviceName: 'Rhythm RPIZ A1B2',
        homeEntries: [
          AccountHomeServerHubs(home: home, serverHubs: [hub]),
        ],
      );

      expect(homeName, 'Lake House');
    });

    test('does not match generic Rhythm Box names', () {
      final home = Home.create(
        id: 'home-1',
        name: 'Lake House',
        ownerId: 'user-1',
      );
      final hub = Hub.server(
        id: 'hub-1',
        homeId: home.id,
        name: 'Rhythm Box',
        host: '192.168.1.20',
      );

      final homeName = bleProvisioningKnownHomeNameForTesting(
        deviceName: 'Rhythm Box',
        homeEntries: [
          AccountHomeServerHubs(home: home, serverHubs: [hub]),
        ],
      );

      expect(homeName, isNull);
    });
  });
}
