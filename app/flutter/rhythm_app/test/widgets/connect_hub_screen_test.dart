import 'package:bonsoir/bonsoir.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/account_cloud_sync_service.dart';
import 'package:rhythm_app/services/cloud_home_join_service.dart';
import 'package:rhythm_app/services/recent_servers_service.dart';
import 'package:rhythm_app/widgets/connect_hub_screen.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show RhythmAuthStatus, RhythmCloudJoinProof;

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

    test('normalizes Rhythm OS mDNS instance names for display', () {
      const hub = DiscoveredHub(
        host: 'rhythm-server-31810e88.local',
        address: '192.168.5.99',
        port: rhythmServerDefaultPort,
        name: 'Rhythm OS (rhythm-server-31810e88)',
        type: HubType.server,
      );

      expect(rhythmDisplayNameForDiscoveredServerForTesting(hub), 'Rhythm Box');
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

    test('does not mix a cloud tunnel with a conflicting local Box identity',
        () {
      final localHome = Home.create(
        id: 'anna-lake',
        name: 'Anna Lake',
        ownerId: 'user-1',
      );
      final cloudHome = localHome.copyWith();
      final localHub = Hub.server(
        id: 'shared-hub-id',
        homeId: localHome.id,
        name: 'Local Box',
        host: '192.168.0.13',
        token: 'other-house-token',
        serverInstanceId: 'srv-other-house',
      );
      final cloudHub = Hub.server(
        id: 'shared-hub-id',
        homeId: cloudHome.id,
        name: 'Anna Lake Box',
        host: '192.168.5.10',
        serverInstanceId: 'srv-anna-lake',
        remoteEndpoint: const HubEndpoint(
          host: 'anna-lake.rhythm.lighting',
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
      expect(hub.endpoint, cloudHub.endpoint);
      expect(hub.remoteEndpoint, cloudHub.remoteEndpoint);
      expect(hub.serverInstanceId, cloudHub.serverInstanceId);
      expect(hub.token, isNull);
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

    test('promotes a provisional identity when owner-token evidence matches',
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
        host: '100.64.0.12',
        token: 'owner-token',
        serverInstanceId: 'endpoint:http://192.168.5.123:54448',
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
        authToken: 'owner-token',
        serverInstanceId: 'srv-kitchen',
      );

      expect(entry, isNotNull);
      expect(entry!.serverHubs.single.id, hub.id);
      expect(entry.serverHubs.single.serverInstanceId, 'srv-kitchen');
      expect(entry.serverHubs.single.endpoint.host, '192.168.5.99');
    });

    test('requests signed cloud promotion for a matched provisional Home', () {
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
        serverInstanceId: 'endpoint:http://192.168.5.123:54448',
      );
      final discovered = DiscoveredHub(
        host: '192.168.5.123',
        address: '192.168.5.123',
        port: rhythmServerDefaultPort,
        name: 'RhythmServer',
        type: HubType.server,
      );
      final homes = [
        AccountHomeServerHubs(home: home, serverHubs: [hub]),
      ];

      expect(
        rhythmExistingHomeNeedsCloudIdentityPromotionForTesting(
          server: discovered,
          homes: homes,
          authToken: 'owner-token',
          serverInstanceId: 'srv-kitchen',
          hasJoinProof: true,
        ),
        isTrue,
      );
      expect(
        rhythmExistingHomeNeedsCloudIdentityPromotionForTesting(
          server: discovered,
          homes: homes,
          authToken: 'owner-token',
          serverInstanceId: 'srv-kitchen',
          hasJoinProof: false,
        ),
        isFalse,
      );

      final promotedHub = hub.copyWith(serverInstanceId: 'srv-kitchen');
      expect(
        rhythmExistingHomeNeedsCloudIdentityPromotionForTesting(
          server: discovered,
          homes: [
            AccountHomeServerHubs(home: home, serverHubs: [promotedHub]),
          ],
          authToken: 'owner-token',
          serverInstanceId: 'srv-kitchen',
          hasJoinProof: true,
        ),
        isFalse,
      );
    });

    test('enters the cloud Home returned by identity promotion', () async {
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
      final localSnapshot = AccountHomeServerHubs(
        home: localHome,
        serverHubs: const [],
      );
      final cloudSnapshot = AccountHomeServerHubs(
        home: cloudHome,
        serverHubs: const [],
      );
      final enteredHomes = <String>[];

      await rhythmEnterExistingHomeWithCloudIdentityPromotionForTesting(
        existingHome: localSnapshot,
        shouldPromote: true,
        promote: () async => CloudHomeJoinResult.joined(cloudSnapshot),
        enterHome: (snapshot) async {
          enteredHomes.add(snapshot.home.id);
          return null;
        },
      );

      expect(enteredHomes, ['cloud-home']);
    });

    test('rejected identity promotion does not enter the local Home', () async {
      final localHome = Home.create(
        id: 'local-home',
        name: 'Kitchen',
        ownerId: 'local-user',
      );
      final localSnapshot = AccountHomeServerHubs(
        home: localHome,
        serverHubs: const [],
      );
      var enterCalls = 0;

      await expectLater(
        rhythmEnterExistingHomeWithCloudIdentityPromotionForTesting(
          existingHome: localSnapshot,
          shouldPromote: true,
          promote: () async =>
              const CloudHomeJoinResult.blocked(code: 'identity_conflict'),
          enterHome: (_) async {
            enterCalls += 1;
            return null;
          },
        ),
        throwsA(
          isA<CloudHomeJoinBlockedException>().having(
            (error) => error.code,
            'code',
            'identity_conflict',
          ),
        ),
      );
      expect(enterCalls, 0);
    });

    test('unavailable cloud promotion still enters the existing Home',
        () async {
      final localHome = Home.create(
        id: 'local-home',
        name: 'Kitchen',
        ownerId: 'local-user',
      );
      final localSnapshot = AccountHomeServerHubs(
        home: localHome,
        serverHubs: const [],
      );
      final enteredHomes = <String>[];

      await rhythmEnterExistingHomeWithCloudIdentityPromotionForTesting(
        existingHome: localSnapshot,
        shouldPromote: true,
        promote: () async => const CloudHomeJoinResult.notAttempted(),
        enterHome: (snapshot) async {
          enteredHomes.add(snapshot.home.id);
          return null;
        },
      );

      expect(enteredHomes, ['local-home']);
    });

    test('skips promotion when the existing Home does not need it', () async {
      final localHome = Home.create(
        id: 'local-home',
        name: 'Kitchen',
        ownerId: 'local-user',
      );
      final localSnapshot = AccountHomeServerHubs(
        home: localHome,
        serverHubs: const [],
      );
      var promotionCalls = 0;
      final enteredHomes = <String>[];

      await rhythmEnterExistingHomeWithCloudIdentityPromotionForTesting(
        existingHome: localSnapshot,
        shouldPromote: false,
        promote: () async {
          promotionCalls += 1;
          return const CloudHomeJoinResult.notAttempted();
        },
        enterHome: (snapshot) async {
          enteredHomes.add(snapshot.home.id);
          return null;
        },
      );

      expect(promotionCalls, 0);
      expect(enteredHomes, ['local-home']);
    });

    test('Home entry reconciles a provisional Box without an owner token', () {
      final home = Home.create(
        id: 'local-home',
        name: 'Evington',
        ownerId: 'local-user',
      );
      final hub = Hub.server(
        id: 'local-hub',
        homeId: home.id,
        name: 'Rhythm Box',
        host: '192.168.0.169',
        serverInstanceId: 'endpoint:http://192.168.0.169:54448',
      );

      expect(
        rhythmHomeEntryNeedsLocalIdentityReconciliationForTesting(
          AccountHomeServerHubs(home: home, serverHubs: [hub]),
        ),
        isTrue,
      );
    });

    test('Home entry reconciles a durable Box whose owner token is missing',
        () {
      final home = Home.create(
        id: 'local-home',
        name: 'Evington',
        ownerId: 'local-user',
      );
      final hub = Hub.server(
        id: 'local-hub',
        homeId: home.id,
        name: 'Rhythm Box',
        host: '192.168.0.169',
        serverInstanceId: 'srv-evington',
      );

      expect(
        rhythmHomeEntryNeedsLocalIdentityReconciliationForTesting(
          AccountHomeServerHubs(home: home, serverHubs: [hub]),
        ),
        isTrue,
      );
    });

    test('Home entry skips reconciliation for a durable authenticated Box', () {
      final home = Home.create(
        id: 'canonical-home',
        name: 'Evington',
        ownerId: 'local-user',
      );
      final hub = Hub.server(
        id: 'canonical-hub',
        homeId: home.id,
        name: 'Rhythm Box',
        host: '192.168.0.169',
        token: 'owner-token',
        serverInstanceId: 'srv-evington',
      );

      expect(
        rhythmHomeEntryNeedsLocalIdentityReconciliationForTesting(
          AccountHomeServerHubs(home: home, serverHubs: [hub]),
        ),
        isFalse,
      );
    });

    test('Box proof prevents a stale local Home from claiming another house',
        () {
      final staleHome = Home.create(
        id: 'anna-lake',
        name: 'Anna Lake',
        ownerId: 'user-1',
      );
      final staleSnapshot = AccountHomeServerHubs(
        home: staleHome,
        serverHubs: [
          Hub.server(
            id: 'anna-hub',
            homeId: staleHome.id,
            name: 'Anna Lake Box',
            host: '192.168.0.13',
            serverInstanceId: 'srv-other-house',
          ),
        ],
      );
      const proof = RhythmCloudJoinProof(
        proofVersion: 'activity-token-hmac-v1',
        algorithm: 'hmac-sha256',
        serverInstanceId: 'srv-other-house',
        homeId: 'deleted-other-home',
        hubId: 'deleted-other-hub',
        issuedAtEpochMs: 1000,
        expiresAtEpochMs: 121000,
        nonce: '0123456789abcdef',
        signature:
            '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef',
      );

      expect(
        rhythmCloudJoinProofTargetsHomeForTesting(proof, staleSnapshot),
        isFalse,
      );
      expect(
        rhythmShouldReuseExistingHomeForCloudProofForTesting(
          existingHome: staleSnapshot,
          proof: proof,
        ),
        isFalse,
      );
    });

    test('fresh LAN claim wins before tokens saved for another house',
        () async {
      final calls = <String>[];
      const status = RhythmAuthStatus(
        requiresAuth: true,
        ownerConfigured: true,
        tokenCount: 1,
        claimAvailable: true,
      );

      final token = await rhythmResolveDiscoveredAuthTokenForTesting(
        status: status,
        claimLanToken: () async {
          calls.add('claim');
          return 'fresh-house-token';
        },
        loadStoredToken: () async {
          calls.add('stored');
          return 'other-house-token';
        },
        requestBleToken: () async {
          calls.add('ble');
          return 'ble-token';
        },
      );

      expect(token, 'fresh-house-token');
      expect(calls, ['claim']);
    });

    test('falls back to BLE when a fresh LAN claim cannot be issued', () async {
      final calls = <String>[];
      const status = RhythmAuthStatus(
        requiresAuth: true,
        ownerConfigured: true,
        tokenCount: 1,
        claimAvailable: true,
      );

      final token = await rhythmResolveDiscoveredAuthTokenForTesting(
        status: status,
        claimLanToken: () async {
          calls.add('claim');
          return null;
        },
        loadStoredToken: () async {
          calls.add('stored');
          return 'other-house-token';
        },
        requestBleToken: () async {
          calls.add('ble');
          return 'ble-token';
        },
      );

      expect(token, 'ble-token');
      expect(calls, ['claim', 'ble']);
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

  group('connectHubContinueAfterBleProvisioningForTesting', () {
    test('opens the three-option hub picker after Box setup completes',
        () async {
      final events = <String>[];

      await connectHubContinueAfterBleProvisioningForTesting(
        provisionBox: () async {
          events.add('box');
          return true;
        },
        isMounted: () => true,
        openHubPicker: () async => events.add('hub-picker'),
      );

      expect(events, ['box', 'hub-picker']);
    });

    test('does not open the hub picker when Box setup is cancelled', () async {
      var openedHubPicker = false;

      await connectHubContinueAfterBleProvisioningForTesting(
        provisionBox: () async => null,
        isMounted: () => true,
        openHubPicker: () async => openedHubPicker = true,
      );

      expect(openedHubPicker, isFalse);
    });

    test('does not navigate after the Connect Hub screen is disposed',
        () async {
      var openedHubPicker = false;

      await connectHubContinueAfterBleProvisioningForTesting(
        provisionBox: () async => true,
        isMounted: () => false,
        openHubPicker: () async => openedHubPicker = true,
      );

      expect(openedHubPicker, isFalse);
    });
  });
}
