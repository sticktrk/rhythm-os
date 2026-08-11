import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/providers/room_page_provider.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  group('RoomPageProvider.layoutScopeFor', () {
    test('keys server-backed layouts by server endpoint', () {
      final home = Home.create(id: 'home-1', name: 'Home', ownerId: 'user-1');
      final server = Hub.server(
        id: 'server-1',
        homeId: home.id,
        name: 'RhythmServer',
        host: '10.0.0.15',
        port: 54448,
      );

      final scope = RoomPageProvider.layoutScopeFor(
        home: home,
        hubs: [server],
      );

      expect(
        scope,
        'server:server:10.0.0.15:54448:plain',
      );
    });

    test('uses the same server scope across homes for phone restore', () {
      final firstHome =
          Home.create(id: 'home-1', name: 'Home', ownerId: 'user-1');
      final secondHome =
          Home.create(id: 'home-2', name: 'Home', ownerId: 'user-1');
      final firstServer = Hub.server(
        id: 'server-1',
        homeId: firstHome.id,
        name: 'RhythmServer',
        host: '10.0.0.15',
        port: 54448,
      );
      final secondServer = Hub.server(
        id: 'server-2',
        homeId: secondHome.id,
        name: 'RhythmServer',
        host: '10.0.0.15',
        port: 54448,
      );

      final first = RoomPageProvider.layoutScopeFor(
        home: firstHome,
        hubs: [firstServer],
      );
      final second = RoomPageProvider.layoutScopeFor(
        home: secondHome,
        hubs: [secondServer],
      );

      expect(first, second);
      expect(
        RoomPageProvider.hubLayoutKey(firstServer),
        RoomPageProvider.hubLayoutKey(secondServer),
      );
    });

    test('uses durable server identity for account-cloud layout matching', () {
      final first = Hub.server(
        id: 'server-1',
        homeId: 'home-1',
        name: 'RhythmServer',
        host: '10.0.0.15',
        port: 54448,
        serverInstanceId: 'BOX-INSTANCE-1',
      );
      final second = Hub.create(
        id: 'server-2',
        homeId: 'home-2',
        type: HubType.server,
        name: 'RhythmServer',
        endpoint: const HubEndpoint(
          host: 'remote.rhythm.test',
          port: 443,
          useSsl: true,
        ),
        serverInstanceId: 'box-instance-1',
      );

      expect(RoomPageProvider.hubLayoutKey(first),
          RoomPageProvider.hubLayoutKey(second));
      expect(
        RoomPageProvider.hubLayoutKeyAliases(first),
        ['server:10.0.0.15:54448:plain'],
      );
    });

    test('keeps the local layout scope stable when a server endpoint changes',
        () {
      final local = Hub.server(
        id: 'server-1',
        homeId: 'home-1',
        name: 'RhythmServer',
        host: '10.0.0.15',
        port: 54448,
        serverInstanceId: 'box-instance-1',
      );
      final refreshed = Hub.create(
        id: 'server-1',
        homeId: 'home-1',
        type: HubType.server,
        name: 'RhythmServer',
        endpoint: const HubEndpoint(
          host: 'rhythm-box.local',
          port: 54448,
          useSsl: true,
        ),
        serverInstanceId: 'box-instance-1',
      );

      final localScope = RoomPageProvider.layoutScopeFor(
        home: null,
        hubs: [local],
      );
      final refreshedScope = RoomPageProvider.layoutScopeFor(
        home: null,
        hubs: [refreshed],
      );

      expect(localScope, 'server:server_instance:box-instance-1');
      expect(refreshedScope, localScope);
      expect(
        RoomPageProvider.layoutScopeAliasesFor(home: null, hubs: [local]),
        ['server:server:10.0.0.15:54448:plain'],
      );
      expect(
        RoomPageProvider.layoutScopeAliasesFor(home: null, hubs: [refreshed]),
        ['server:server:rhythm-box.local:54448:ssl'],
      );
    });

    test('keys direct layouts by enabled hub set regardless of order', () {
      final home = Home.create(id: 'home-1', name: 'Home', ownerId: 'user-1');
      final hue = Hub.hue(
        id: 'hue-1',
        homeId: home.id,
        name: 'Hue',
        bridgeIp: '192.168.1.2',
        appKey: 'key',
      );
      final ha = Hub.homeAssistant(
        id: 'ha-1',
        homeId: home.id,
        name: 'HA',
        host: 'homeassistant.local',
        token: 'token',
      );

      final first = RoomPageProvider.layoutScopeFor(
        home: home,
        hubs: [hue, ha],
      );
      final second = RoomPageProvider.layoutScopeFor(
        home: home,
        hubs: [ha, hue],
      );

      expect(first, second);
    });
  });

  group('RoomPageProvider', () {
    test('loads a different layout for each scope', () {
      final store = _FakeRoomPageLayoutStore(
        scopedLayouts: {
          'scope-a': [
            ['room-a']
          ],
          'scope-b': [
            ['room-b']
          ],
        },
      );
      final provider = RoomPageProvider(layoutStore: store);

      provider.setLayoutScope('scope-a');
      expect(
        provider.getRoomsForPage(0, [_room('room-a'), _room('room-b')]),
        [_room('room-a')],
      );

      provider.setLayoutScope('scope-b');
      expect(
        provider.getRoomsForPage(0, [_room('room-a'), _room('room-b')]),
        [_room('room-b')],
      );
    });

    test('migrates the legacy global layout into the first scoped layout only',
        () async {
      final store = _FakeRoomPageLayoutStore(
        legacyLayout: [
          ['legacy-room']
        ],
      );
      final provider = RoomPageProvider(layoutStore: store);

      provider.setLayoutScope('scope-a');
      await Future<void>.delayed(Duration.zero);

      provider.setLayoutScope('scope-b');
      provider.moveRoom('new-room', 0);

      expect(store.migratedScopes, ['scope-a']);
      expect(store.scopedLayouts['scope-a'], [
        ['legacy-room']
      ]);
      expect(store.scopedLayouts['scope-b'], [
        ['new-room']
      ]);
    });

    test('copies an endpoint-keyed layout into its durable server scope',
        () async {
      final store = _FakeRoomPageLayoutStore(
        scopedLayouts: {
          'server:server:10.0.0.15:54448:plain': [
            ['saved-room']
          ],
        },
      );
      final provider = RoomPageProvider(layoutStore: store);

      provider.setLayoutScope(
        'server:server_instance:box-instance-1',
        scopeKeyAliases: const [
          'server:server:10.0.0.15:54448:plain',
        ],
      );
      await Future<void>.delayed(Duration.zero);

      expect(
        provider.getRoomsForPage(0, [_room('saved-room')]),
        [_room('saved-room')],
      );
      expect(
        store.scopedLayouts['server:server_instance:box-instance-1'],
        [
          ['saved-room']
        ],
      );

      provider.setLayoutScope(
        'server:server_instance:box-instance-1',
        scopeKeyAliases: const [
          'server:server:rhythm-box.local:54448:ssl',
        ],
      );
      expect(
        provider.getRoomsForPage(0, [_room('saved-room')]),
        [_room('saved-room')],
      );
    });

    test('recovers when a later endpoint alias contains the saved layout', () {
      final store = _FakeRoomPageLayoutStore(
        scopedLayouts: {
          'server:server:rhythm-box.local:54448:ssl': [
            ['other-room'],
            ['saved-room']
          ],
        },
      );
      final provider = RoomPageProvider(layoutStore: store);

      provider.setLayoutScope(
        'server:server_instance:box-instance-1',
        scopeKeyAliases: const [
          'server:server:10.0.0.15:54448:plain',
        ],
      );
      expect(provider.getPage('saved-room'), 0);

      provider.setLayoutScope(
        'server:server_instance:box-instance-1',
        scopeKeyAliases: const [
          'server:server:rhythm-box.local:54448:ssl',
        ],
      );
      expect(provider.getPage('saved-room'), 1);
    });

    test(
        'does not persist the previous room set immediately after a scope switch',
        () {
      final store = _FakeRoomPageLayoutStore(
        scopedLayouts: {
          'scope-a': [
            ['room-a']
          ],
        },
      );
      final provider = RoomPageProvider(layoutStore: store);

      provider.setLayoutScope('scope-a');
      provider.reconcileRooms([_room('room-a')]);

      provider.setLayoutScope('scope-b');
      provider.reconcileRooms([_room('room-a')]);
      expect(store.savedScopes, isNot(contains('scope-b')));

      provider.reconcileRooms([_room('room-b')]);
      expect(store.scopedLayouts['scope-b'], [
        ['room-b']
      ]);
    });

    test('only user-authored layout moves request account sync', () {
      final changes = <String?>[];
      final provider = RoomPageProvider(
        layoutStore: _FakeRoomPageLayoutStore(),
        onUserLayoutChanged: changes.add,
      );
      provider.setLayoutScope('scope-a');

      provider.reconcileRooms([_room('room-a'), _room('room-b')]);
      expect(changes, isEmpty);

      provider.moveRoom('room-b', 1);
      expect(changes, ['scope-a']);

      provider.reorderInPage('room-a', 0, 0);
      expect(changes, ['scope-a']);
    });
  });
}

class _FakeRoomPageLayoutStore implements RoomPageLayoutStore {
  final Map<String, List<List<String>>> scopedLayouts;
  List<List<String>>? legacyLayout;
  final List<String> migratedScopes = [];
  final List<String?> savedScopes = [];

  _FakeRoomPageLayoutStore({
    Map<String, List<List<String>>>? scopedLayouts,
    this.legacyLayout,
  }) : scopedLayouts = scopedLayouts ?? {};

  @override
  Future<void> clearLayout({String? scopeKey}) async {
    savedScopes.add(scopeKey);
    if (scopeKey != null) {
      scopedLayouts.remove(scopeKey);
      return;
    }
    legacyLayout = null;
  }

  @override
  List<List<String>>? loadLayout({String? scopeKey}) {
    final pages = scopeKey == null ? legacyLayout : scopedLayouts[scopeKey];
    return pages == null ? null : _copyPages(pages);
  }

  @override
  List<List<String>>? loadLegacyLayout() {
    return legacyLayout == null ? null : _copyPages(legacyLayout!);
  }

  @override
  Future<void> migrateLegacyLayoutToScope(String scopeKey) async {
    migratedScopes.add(scopeKey);
    if (legacyLayout != null) {
      scopedLayouts[scopeKey] = _copyPages(legacyLayout!);
      legacyLayout = null;
    }
  }

  @override
  Future<void> saveLayout(List<List<String>> pages, {String? scopeKey}) async {
    savedScopes.add(scopeKey);
    if (scopeKey == null) {
      legacyLayout = _copyPages(pages);
      return;
    }
    scopedLayouts[scopeKey] = _copyPages(pages);
  }

  List<List<String>> _copyPages(List<List<String>> pages) {
    return pages.map((page) => List<String>.from(page)).toList();
  }
}

RoomDto _room(String id, {String? name}) {
  return RoomDto(
    id: id,
    name: name ?? id,
    source: RoomSourceDto.hue,
    deviceIds: const [],
    rhythmEnabled: true,
    disabled: false,
    lightsOn: true,
    timeOffsetMinutes: 0,
    brightnessOffset: 0,
  );
}
