import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:rhythm_app/providers/home_provider.dart';
import 'package:rhythm_app/providers/hub_connection_provider.dart';
import 'package:rhythm_app/services/device_access_policy.dart';
import 'package:rhythm_app/services/hue_ble_auto_discovery_service.dart';
import 'package:rhythm_app/services/nearby_ble_discovery_service.dart';
import 'package:rhythm_app/services/phone_ble_wifi_service.dart';
import 'package:rhythm_core/rhythm_core.dart';

Hub server(String id, {String home = 'home'}) => Hub.create(
      id: id,
      homeId: home,
      type: HubType.server,
      name: id,
      endpoint: const HubEndpoint(host: 'fixture.invalid', port: 8080),
      serverInstanceId: id,
    );

const haConfig = HomeAssistantConfig(host: 'fixture.invalid', token: 'fixture');
const hueConfig = HueConfig(bridgeIp: 'fixture.invalid', username: 'fixture');

class _HaProbe extends HaWebSocketProvider {
  _HaProbe(super.config);
  int configReads = 0;
  @override
  bool get isConnected => true;
  @override
  Future<bool> connect() async => true;
  @override
  Future<Map<String, dynamic>> getConfig() async {
    configReads++;
    return {'latitude': 1, 'longitude': 2};
  }
}

class _PhoneTransport implements PhoneWifiTransport {
  int scans = 0;
  int connects = 0;
  int disposals = 0;
  @override
  Future<List<String>> scan(String service) async {
    scans++;
    return [];
  }

  @override
  Future<PhoneWifiGatt> connect(String address) async {
    connects++;
    throw StateError('Unexpected connection');
  }

  @override
  Future<void> dispose() async => disposals++;
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUp(() => DirectHubAccess.select(scope: 'test-local', allowed: true));
  tearDown(() => DirectHubAccess.select(scope: null, allowed: true));

  test('HA ownership survives restart, offline and a legacy hello', () {
    final saved = <String, bool>{};
    DeviceAccessPolicy create() => DeviceAccessPolicy(
          knownOwners: saved,
          persistOwner: (key, value) async => saved[key] = value,
          addonContext: false,
        );
    final addon = server('addon');
    final policy = create();
    policy.select(homeId: addon.homeId, server: addon);
    expect(DirectHubAccess.allowed, isFalse); // unknown server is pending
    policy.observeServer(addon,
        haDeviceManagement: true,
        explicitDeployment: true,
        platformContext: 'ha_addon');
    policy.select(homeId: addon.homeId, server: addon);
    expect(policy.ownedByHomeAssistant, isTrue);

    final restarted = create();
    restarted.select(homeId: addon.homeId, server: addon);
    expect(restarted.ownedByHomeAssistant, isTrue);
    expect(DirectHubAccess.allowed, isFalse);
    restarted.observeServer(addon,
        haDeviceManagement: false,
        explicitDeployment: false,
        platformContext: '');
    restarted.select(homeId: addon.homeId, server: addon);
    expect(DirectHubAccess.allowed, isFalse);
  });

  test('known rpiz remains direct offline and another home keeps its ownership',
      () {
    final policy = DeviceAccessPolicy(
        knownOwners: {}, persistOwner: (_, __) async {}, addonContext: false);
    final addon = server('addon');
    final rpiz = server('rpiz', home: 'other-home');
    policy.observeServer(addon,
        haDeviceManagement: true,
        explicitDeployment: true,
        platformContext: 'ha_addon');
    policy.observeServer(rpiz,
        haDeviceManagement: false,
        explicitDeployment: false,
        platformContext: 'rpiz');
    policy.select(homeId: rpiz.homeId, server: rpiz);
    final oldOperation = DirectHubAccess.capture();
    expect(DirectHubAccess.allowed, isTrue);
    policy.select(homeId: addon.homeId, server: addon);
    expect(DirectHubAccess.allowed, isFalse);
    policy.select(homeId: rpiz.homeId, server: rpiz);
    expect(DirectHubAccess.allowed, isTrue);
    expect(oldOperation.isCurrent, isFalse);
    expect(DirectHubAccess.capture().isCurrent, isTrue);
    policy.select(homeId: 'local-only', server: null);
    expect(DirectHubAccess.allowed, isTrue);
    policy.select(homeId: addon.homeId, server: addon);
    expect(policy.ownedByHomeAssistant, isTrue);
  });

  test('loading retains ownership and verified rpiz can replace the binding',
      () {
    final policy = DeviceAccessPolicy(
        knownOwners: {}, persistOwner: (_, __) async {}, addonContext: false);
    final hub = server('controller');
    policy.observeServer(hub,
        haDeviceManagement: true,
        explicitDeployment: true,
        platformContext: 'ha_addon');
    policy.select(homeId: hub.homeId, server: hub);
    policy.suspendWhileLoading();
    expect(DirectHubAccess.allowed, isFalse);
    expect(policy.ownedByHomeAssistant, isTrue);
    policy.observeServer(hub,
        haDeviceManagement: false,
        explicitDeployment: false,
        platformContext: 'rpiz',
        serverInstanceId: 'replacement-rpiz');
    policy.select(homeId: hub.homeId, server: hub);
    expect(DirectHubAccess.allowed, isTrue);
    expect(policy.ownedByHomeAssistant, isFalse);
  });

  test('an HA handoff lease tracks selection independently of direct access',
      () {
    DirectHubAccess.select(scope: 'ha-first', allowed: false);
    final handoff = DirectHubAccess.capture();
    expect(handoff.isSameSelection, isTrue);
    expect(handoff.isCurrent, isFalse);
    DirectHubAccess.select(scope: 'ha-second', allowed: false);
    expect(handoff.isSameSelection, isFalse);
    DirectHubAccess.select(scope: 'ha-first', allowed: false);
    expect(handoff.isSameSelection, isFalse);
  });

  test('already-created REST services send zero requests after a switch',
      () async {
    var requests = 0;
    http.Client client() => MockClient((_) async {
          requests++;
          return http.Response('[]', 200);
        });
    final hue = HueProvider(hueConfig, client: client());
    final ha = HomeAssistantProvider(haConfig, client: client());
    DirectHubAccess.select(scope: 'addon', allowed: false);
    await expectLater(hue.turnOff('light'), throwsStateError);
    await expectLater(ha.turnOff('light.fixture'), throwsStateError);
    expect(requests, 0);

    DirectHubAccess.select(scope: 'rpiz', allowed: true);
    await expectLater(hue.turnOff('light'), throwsStateError);
    await expectLater(ha.turnOff('light.fixture'), throwsStateError);
    final freshHue = HueProvider(hueConfig, client: client());
    final freshHa = HomeAssistantProvider(haConfig, client: client());
    await freshHue.turnOff('light');
    await freshHa.turnOff('light.fixture');
    expect(requests, 2);
    await hue.dispose();
    await ha.dispose();
    await freshHue.dispose();
    await freshHa.dispose();
  });

  test('saved Hue and HA credentials do not trigger connection probes in HA',
      () async {
    var hueProbes = 0;
    var haProbes = 0;
    final provider = HubConnectionProvider(
      hueConnectionTest: ({required bridgeIp, required username}) async {
        hueProbes++;
        return true;
      },
      haWebSocketFactory: (config) {
        haProbes++;
        return _HaProbe(config);
      },
    );
    addTearDown(provider.dispose);
    provider.configureHubs([
      server('hue').copyWith(type: HubType.hue, token: 'saved-hue'),
      server('ha').copyWith(type: HubType.homeAssistant, token: 'saved-ha'),
    ]);
    DirectHubAccess.select(scope: 'addon', allowed: false);
    expect(await provider.verifyConnection(), isFalse);
    expect(await provider.verifyHueConnection(), isFalse);
    expect(hueProbes, 0);
    expect(haProbes, 0);
    DirectHubAccess.select(scope: 'rpiz', allowed: true);
    expect(await provider.verifyConnection(), isTrue);
    expect(hueProbes, 1);
    expect(haProbes, 1);
  });

  test('offline HA skips discovery, pairing, and saved-hub geolocation',
      () async {
    final home = HomeProvider(
        deviceAccessPolicy: DeviceAccessPolicy(
            knownOwners: {},
            persistOwner: (_, __) async {},
            addonContext: false));
    final ha = _HaProbe(haConfig);
    addTearDown(home.dispose);
    addTearDown(ha.dispose);
    DirectHubAccess.select(scope: 'addon-offline', allowed: false);
    expect(await HueProvider.discoverBridges(), isEmpty);
    expect(await HueProvider.pair('fixture.invalid'), isNull);
    expect(
        await HueProvider.testBridgeConnection(
            bridgeIp: 'fixture.invalid', username: 'saved'),
        isFalse);
    expect(await HubDiscoveryService.discoverAll(), isEmpty);
    final location =
        await home.resolveLocation(hueConfig: hueConfig, haWebSocket: ha);
    expect(location.source, 'default');
    expect(ha.configReads, 0);
  });

  test('phone discovery transports are never called in HA mode', () async {
    var scans = 0;
    final hue = HueBleAutoDiscoveryService(platformScan: (_) async {
      scans++;
      return const HueBleDiscoveryResult(HueBleDiscoveryOutcome.none);
    });
    final nearby = NearbyBleDiscoveryService(platformScan: (_, __) async {
      scans++;
      return const NearbyBleDiscoveryResult(NearbyBleDiscoveryOutcome.none);
    });
    final transport = _PhoneTransport();
    final wifi = AylaPhoneBleWifiService(transport: transport);
    DirectHubAccess.select(scope: 'addon', allowed: false);
    expect((await hue.discover(source: 'test')).outcome,
        HueBleDiscoveryOutcome.unsupported);
    expect(
        (await nearby
                .discover(source: 'test', families: {NearbyBleFamily.hueBle}))
            .outcome,
        NearbyBleDiscoveryOutcome.unsupported);
    await expectLater(wifi.discover(), throwsA(isA<PhoneBleWifiFailure>()));
    expect(scans, 0);
    expect(transport.scans, 0);
    expect(transport.connects, 0);
    expect(transport.disposals, 1);
    DirectHubAccess.select(scope: 'rpiz', allowed: true);
    await expectLater(wifi.discover(), throwsA(isA<PhoneBleWifiFailure>()));
    expect(transport.scans, 0); // a stale screen cannot resume
    await wifi.dispose();
  });

  test('a scan result from a previous selection cannot reopen device setup',
      () async {
    final scan = Completer<HueBleDiscoveryResult>();
    final service =
        HueBleAutoDiscoveryService(platformScan: (_) => scan.future);
    final result = service.discover(source: 'test');
    DirectHubAccess.select(scope: 'addon', allowed: false);
    DirectHubAccess.select(scope: 'rpiz', allowed: true);
    scan.complete(const HueBleDiscoveryResult(HueBleDiscoveryOutcome.found,
        deviceCount: 1));
    expect((await result).outcome, HueBleDiscoveryOutcome.unsupported);
  });
}
