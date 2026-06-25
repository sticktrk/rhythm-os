import 'package:bonsoir/bonsoir.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/account_cloud_sync_service.dart';
import 'package:rhythm_app/services/recent_servers_service.dart';
import 'package:rhythm_app/widgets/connect_hub_screen.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  group('Rhythm mDNS discovery helpers', () {
    test('uses Android TXT ip attribute before the resolved host', () {
      const service = BonsoirService.ignoreNorms(
        name: 'AMC0945FFD075E099B',
        type: '_http._tcp',
        host: '192.168.5.10',
        port: 80,
        attributes: {
          'ip': '192.168.5.10',
          'host': 'AMC0945FFD075E099B',
        },
      );

      expect(
        rhythmDiscoveryHostCandidates(service),
        ['192.168.5.10'],
      );
    });

    test('keeps generic HTTP services out of the non-Android path', () {
      const service = BonsoirService.ignoreNorms(
        name: 'AMC0945FFD075E099B',
        type: '_http._tcp',
        host: '192.168.5.10',
        port: 80,
        attributes: {
          'ip': '192.168.5.10',
        },
      );

      expect(
        rhythmDiscoveryHostCandidates(
          service,
          includeGenericHttpServices: false,
        ),
        isEmpty,
      );
    });

    test('keeps Rhythm-looking services discoverable on the non-Android path',
        () {
      const service = BonsoirService.ignoreNorms(
        name: 'Rhythm Box',
        type: '_http._tcp',
        host: 'rhythm-box.local.',
        port: 80,
        attributes: {},
      );

      expect(
        rhythmDiscoveryHostCandidates(
          service,
          includeGenericHttpServices: false,
        ),
        ['rhythm-box.local'],
      );
    });

    test('probes advertised port and Rhythm server default port', () {
      const service = BonsoirService.ignoreNorms(
        name: 'AMC0945FFD075E099B',
        type: '_http._tcp',
        host: '192.168.5.10',
        port: 80,
        attributes: {},
      );

      expect(
        rhythmDiscoveryPortCandidates(service),
        [80, rhythmServerDefaultPort],
      );
    });

    test('uses only the advertised port on the non-Android path', () {
      const service = BonsoirService.ignoreNorms(
        name: 'Rhythm Box',
        type: '_http._tcp',
        host: 'rhythm-box.local.',
        port: 80,
        attributes: {},
      );

      expect(
        rhythmDiscoveryPortCandidates(service, includeDefaultPort: false),
        [80],
      );
    });

    test('does not duplicate the default port', () {
      const service = BonsoirService.ignoreNorms(
        name: 'rhythm-box',
        type: '_http._tcp',
        host: 'rhythm-box.local.',
        port: rhythmServerDefaultPort,
        attributes: {},
      );

      expect(rhythmDiscoveryPortCandidates(service), [rhythmServerDefaultPort]);
      expect(rhythmDiscoveryHostCandidates(service), ['rhythm-box.local']);
    });

    test('scopes connecting state to the selected endpoint', () {
      const connectingEndpoint = '192.168.5.10:54448';

      expect(
        rhythmServerEndpointIsConnectingForTesting(
          connectingEndpoint: connectingEndpoint,
          host: '192.168.5.10',
          port: rhythmServerDefaultPort,
        ),
        isTrue,
      );
      expect(
        rhythmServerEndpointIsConnectingForTesting(
          connectingEndpoint: connectingEndpoint,
          host: '192.168.5.11',
          port: rhythmServerDefaultPort,
        ),
        isFalse,
      );
    });

    test('builds same-subnet fallback candidates from private local IPs', () {
      final candidates = rhythmSubnetScanCandidates([
        '192.168.5.42',
        '10.0.3.12',
        '8.8.8.8',
      ]);

      expect(candidates, contains('192.168.5.123'));
      expect(candidates, contains('10.0.3.1'));
      expect(candidates, isNot(contains('192.168.5.42')));
      expect(candidates, isNot(contains('8.8.8.1')));
    });

    test('merges local and cloud Home entries for the same Box', () {
      final localHome = Home.create(
        id: 'local-home',
        name: 'Kitchen',
        ownerId: 'local-user',
      );
      final cloudHome = Home.create(
        id: 'cloud-home',
        name: 'Kitchen',
        ownerId: 'cloud-user',
      );
      final localHub = Hub.server(
        id: 'server-1',
        homeId: localHome.id,
        name: 'Kitchen Box',
        host: '192.168.5.10',
        token: 'local-owner-token',
      );
      final cloudHub = Hub.server(
        id: 'server-1',
        homeId: cloudHome.id,
        name: 'Kitchen Box',
        host: '192.168.5.10',
        remoteEndpoint: const HubEndpoint(
          host: 'server-1.devices.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
      );

      final homes = rhythmMergedHomeEntriesForTesting(
        localHomes: [
          AccountHomeServerHubs(home: localHome, serverHubs: [localHub]),
        ],
        cloudHomes: [
          AccountHomeServerHubs(home: cloudHome, serverHubs: [cloudHub]),
        ],
      );

      expect(homes, hasLength(1));
      expect(homes.single.home.id, localHome.id);
      expect(homes.single.serverHubs.single.homeId, localHome.id);
      expect(homes.single.serverHubs.single.token, 'local-owner-token');
      expect(
        homes.single.serverHubs.single.remoteEndpoint?.host,
        'server-1.devices.rhythm.lighting',
      );

      expect(
        rhythmHomeIdsRepresentedByForTesting(
          snapshot: homes.single,
          localHomes: [
            AccountHomeServerHubs(home: localHome, serverHubs: [localHub]),
          ],
          cloudHomes: [
            AccountHomeServerHubs(home: cloudHome, serverHubs: [cloudHub]),
          ],
        ),
        {localHome.id, cloudHome.id},
      );
    });

    test('merges same-token local and tunneled Box entries', () {
      final localHome = Home.create(
        id: 'local-home',
        name: 'Kitchen',
        ownerId: 'local-user',
      );
      final cloudHome = Home.create(
        id: 'cloud-home',
        name: 'Kitchen',
        ownerId: 'cloud-user',
      );
      final localHub = Hub.server(
        id: 'server-local',
        homeId: localHome.id,
        name: 'Kitchen Box',
        host: '192.168.5.123',
        token: 'local-owner-token',
      );
      final cloudHub = Hub.server(
        id: 'server-cloud',
        homeId: cloudHome.id,
        name: 'Kitchen Box',
        host: '100.64.0.12',
        token: 'local-owner-token',
        remoteEndpoint: const HubEndpoint(
          host: 'kitchen.devices.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
      );

      final homes = rhythmMergedHomeEntriesForTesting(
        localHomes: [
          AccountHomeServerHubs(home: localHome, serverHubs: [localHub]),
        ],
        cloudHomes: [
          AccountHomeServerHubs(home: cloudHome, serverHubs: [cloudHub]),
        ],
      );

      expect(homes, hasLength(1));
      expect(homes.single.serverHubs, hasLength(1));
      final hub = homes.single.serverHubs.single;
      expect(hub.id, 'server-local');
      expect(hub.token, 'local-owner-token');
      expect(hub.remoteEndpoint?.host, 'kitchen.devices.rhythm.lighting');
    });

    test('keeps same-named remote Box entries separate without shared identity',
        () {
      final localHome = Home.create(
        id: 'local-home',
        name: 'Home',
        ownerId: 'local-user',
      );
      final cloudHome = Home.create(
        id: 'cloud-home',
        name: 'Home',
        ownerId: 'cloud-user',
      );
      final localHub = Hub.server(
        id: 'server-local',
        homeId: localHome.id,
        name: 'Rhythm OS',
        host: '192.168.5.123',
        token: 'local-owner-token',
        remoteEndpoint: const HubEndpoint(
          host: 'local.devices.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
      );
      final cloudHub = Hub.server(
        id: 'server-cloud',
        homeId: cloudHome.id,
        name: 'Rhythm OS',
        host: '192.168.5.124',
        token: 'cloud-owner-token',
        remoteEndpoint: const HubEndpoint(
          host: 'cloud.devices.rhythm.lighting',
          port: 443,
          useSsl: true,
        ),
      );

      final homes = rhythmMergedHomeEntriesForTesting(
        localHomes: [
          AccountHomeServerHubs(home: localHome, serverHubs: [localHub]),
        ],
        cloudHomes: [
          AccountHomeServerHubs(home: cloudHome, serverHubs: [cloudHub]),
        ],
      );

      expect(homes, hasLength(2));
      expect(homes.map((entry) => entry.home.id), [
        localHome.id,
        cloudHome.id,
      ]);
      expect(homes.first.serverHubs.single.id, localHub.id);
      expect(homes.last.serverHubs.single.id, cloudHub.id);
    });

    test('keeps different Homes separate even when they share a Box', () {
      final localHome = Home.create(
        id: 'local-home',
        name: 'Kitchen',
        ownerId: 'local-user',
      );
      final cloudHome = Home.create(
        id: 'cloud-home',
        name: 'Cabin',
        ownerId: 'cloud-user',
      );
      final localHub = Hub.server(
        id: 'server-1',
        homeId: localHome.id,
        name: 'Kitchen Box',
        host: '192.168.5.10',
      );
      final cloudHub = Hub.server(
        id: 'server-1',
        homeId: cloudHome.id,
        name: 'Cabin Box',
        host: '192.168.5.10',
      );

      final homes = rhythmMergedHomeEntriesForTesting(
        localHomes: [
          AccountHomeServerHubs(home: localHome, serverHubs: [localHub]),
        ],
        cloudHomes: [
          AccountHomeServerHubs(home: cloudHome, serverHubs: [cloudHub]),
        ],
      );

      expect(homes, hasLength(2));
      expect(homes.map((entry) => entry.home.id), [
        localHome.id,
        cloudHome.id,
      ]);

      expect(
        rhythmHomeIdsRepresentedByForTesting(
          snapshot: homes.first,
          localHomes: [
            AccountHomeServerHubs(home: localHome, serverHubs: [localHub]),
          ],
          cloudHomes: [
            AccountHomeServerHubs(home: cloudHome, serverHubs: [cloudHub]),
          ],
        ),
        {localHome.id},
      );
    });

    test('hides recent hardware already represented by a Home', () {
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'local-user',
      );
      final hub = Hub.server(
        id: 'server-1',
        homeId: home.id,
        name: 'Kitchen Box',
        host: '192.168.5.10',
      );
      final matchingRecent = RecentServer(
        name: 'Kitchen Box',
        host: '192.168.5.10',
        port: rhythmServerDefaultPort,
        lastConnected: DateTime.utc(2026, 6, 6),
      );
      final otherRecent = RecentServer(
        name: 'Garage Box',
        host: '192.168.5.11',
        port: rhythmServerDefaultPort,
        lastConnected: DateTime.utc(2026, 6, 6),
      );

      final homes = [
        AccountHomeServerHubs(home: home, serverHubs: [hub]),
      ];

      expect(
        rhythmRecentServerIsRepresentedByHomeForTesting(
          server: matchingRecent,
          homes: homes,
        ),
        isTrue,
      );
      expect(
        rhythmRecentServerIsRepresentedByHomeForTesting(
          server: otherRecent,
          homes: homes,
        ),
        isFalse,
      );
    });

    test('uses recent server identity before endpoint reuse', () {
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'local-user',
      );
      final hub = Hub.server(
        id: 'server-1',
        homeId: home.id,
        name: 'Kitchen Box',
        host: '192.168.5.10',
        serverInstanceId: 'srv-kitchen',
      );
      final sameBoxAtNewIp = RecentServer(
        name: 'Kitchen Box',
        host: '192.168.5.99',
        port: rhythmServerDefaultPort,
        serverInstanceId: 'srv-kitchen',
        lastConnected: DateTime.utc(2026, 6, 6),
      );
      final differentBoxAtOldIp = RecentServer(
        name: 'Garage Box',
        host: '192.168.5.10',
        port: rhythmServerDefaultPort,
        serverInstanceId: 'srv-garage',
        lastConnected: DateTime.utc(2026, 6, 6),
      );

      final homes = [
        AccountHomeServerHubs(home: home, serverHubs: [hub]),
      ];

      expect(
        rhythmRecentServerIsRepresentedByHomeForTesting(
          server: sameBoxAtNewIp,
          homes: homes,
        ),
        isTrue,
      );
      expect(
        rhythmRecentServerIsRepresentedByHomeForTesting(
          server: differentBoxAtOldIp,
          homes: homes,
        ),
        isFalse,
      );
    });

    test('hides discovered hardware already represented by a Home', () {
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'local-user',
      );
      final hub = Hub.server(
        id: 'server-1',
        homeId: home.id,
        name: 'Kitchen Box',
        host: '192.168.5.10',
      );
      final matchingDiscovered = DiscoveredHub(
        host: '192.168.5.10',
        address: '192.168.5.10',
        port: rhythmServerDefaultPort,
        name: 'Kitchen Box',
        type: HubType.server,
      );
      final otherDiscovered = DiscoveredHub(
        host: '192.168.5.11',
        address: '192.168.5.11',
        port: rhythmServerDefaultPort,
        name: 'Garage Box',
        type: HubType.server,
      );

      final homes = [
        AccountHomeServerHubs(home: home, serverHubs: [hub]),
      ];

      expect(
        rhythmDiscoveredServerIsRepresentedByHomeForTesting(
          server: matchingDiscovered,
          homes: homes,
        ),
        isTrue,
      );
      expect(
        rhythmDiscoveredServerIsRepresentedByHomeForTesting(
          server: otherDiscovered,
          homes: homes,
        ),
        isFalse,
      );
    });

    test('updates existing Home entry when discovered server IP changed', () {
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'local-user',
      );
      final hub = Hub.server(
        id: 'server-1',
        homeId: home.id,
        name: 'Kitchen Box',
        host: '100.64.0.12',
        token: 'owner-token',
      );
      final discovered = DiscoveredHub(
        host: '192.168.5.123',
        address: '192.168.5.123',
        port: rhythmServerDefaultPort,
        name: 'Kitchen Box',
        type: HubType.server,
      );

      final entry = rhythmHomeEntryForDiscoveredServerForTesting(
        server: discovered,
        homes: [
          AccountHomeServerHubs(home: home, serverHubs: [hub]),
        ],
        authToken: 'owner-token',
      );

      expect(entry, isNotNull);
      final updatedHub = entry!.serverHubs.single;
      expect(updatedHub.id, hub.id);
      expect(updatedHub.endpoint.host, '192.168.5.123');
      expect(updatedHub.endpoint.port, rhythmServerDefaultPort);
      expect(updatedHub.token, 'owner-token');
      expect(updatedHub.pendingSync, isTrue);
    });

    test('updates existing Home entry by server identity when IP changed', () {
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'local-user',
      );
      final hub = Hub.server(
        id: 'server-1',
        homeId: home.id,
        name: 'Kitchen Box',
        host: '192.168.5.10',
        token: 'owner-token',
        serverInstanceId: 'srv-kitchen',
      );
      final discovered = DiscoveredHub(
        host: '192.168.5.99',
        address: '192.168.5.99',
        port: rhythmServerDefaultPort,
        name: 'RhythmServer',
        type: HubType.server,
      );

      final entry = rhythmHomeEntryForDiscoveredServerForTesting(
        server: discovered,
        homes: [
          AccountHomeServerHubs(home: home, serverHubs: [hub]),
        ],
        serverInstanceId: 'srv-kitchen',
      );

      expect(entry, isNotNull);
      final updatedHub = entry!.serverHubs.single;
      expect(updatedHub.id, hub.id);
      expect(updatedHub.endpoint.host, '192.168.5.99');
      expect(updatedHub.serverInstanceId, 'srv-kitchen');
      expect(updatedHub.token, 'owner-token');
    });

    test('keeps known-different server identity separate at the same endpoint',
        () {
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'local-user',
      );
      final hub = Hub.server(
        id: 'server-1',
        homeId: home.id,
        name: 'Kitchen Box',
        host: '192.168.5.123',
        token: 'owner-token',
        serverInstanceId: 'srv-kitchen',
      );
      final discovered = DiscoveredHub(
        host: '192.168.5.123',
        address: '192.168.5.123',
        port: rhythmServerDefaultPort,
        name: 'Garage Box',
        type: HubType.server,
      );

      final entry = rhythmHomeEntryForDiscoveredServerForTesting(
        server: discovered,
        homes: [
          AccountHomeServerHubs(home: home, serverHubs: [hub]),
        ],
        authToken: 'owner-token',
        serverInstanceId: 'srv-garage',
      );

      expect(entry, isNull);
    });

    test(
        'keeps direct-IP Box separate when endpoint is reused by another token',
        () {
      final home = Home.create(
        id: 'home-1',
        name: 'Kitchen',
        ownerId: 'local-user',
      );
      final hub = Hub.server(
        id: 'server-1',
        homeId: home.id,
        name: 'Kitchen Box',
        host: '192.168.5.123',
        token: 'old-owner-token',
      );
      final discovered = DiscoveredHub(
        host: '192.168.5.123',
        address: '192.168.5.123',
        port: rhythmServerDefaultPort,
        name: 'RhythmServer',
        type: HubType.server,
      );

      final entry = rhythmHomeEntryForDiscoveredServerForTesting(
        server: discovered,
        homes: [
          AccountHomeServerHubs(home: home, serverHubs: [hub]),
        ],
        authToken: 'new-owner-token',
      );

      expect(entry, isNull);
    });

    test('builds detached server hub for local hardware inspection', () {
      final discovered = DiscoveredHub(
        host: '192.168.5.123',
        address: '192.168.5.123',
        port: rhythmServerDefaultPort,
        name: 'Kitchen Box',
        type: HubType.server,
      );

      final hub = rhythmDetachedServerHubForTesting(
        server: discovered,
        authToken: 'owner-token',
      );

      expect(hub.id, 'detached-192.168.5.123:54448');
      expect(hub.homeId, 'detached-home');
      expect(hub.name, 'Kitchen Box');
      expect(hub.endpoint.host, '192.168.5.123');
      expect(hub.endpoint.port, rhythmServerDefaultPort);
      expect(hub.token, 'owner-token');
      expect(hub.pendingSync, isFalse);
    });
  });
}
