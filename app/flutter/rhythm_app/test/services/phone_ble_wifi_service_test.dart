import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/phone_ble_wifi_service.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

const candidate = BleWifiDiscoveredCandidate(
    dsn: 'ACFIXTURE123456', address: 'ios-opaque-uuid');
const wifi = RhythmCommissioningWifi(
    ssid: 'Fixture Wi-Fi', password: 'fixture-password');
const token = '0123456789abcdef0123456789abcdef';

class FakeGatt implements PhoneWifiGatt {
  String dsn = candidate.dsn;
  String statusSsid = wifi.ssid;
  bool failWrite = false;
  bool failStatus = false;
  int disconnects = 0;
  Completer<void>? writeBarrier;
  final writes = <(String, String, List<int>)>[];
  @override
  Future<List<int>> read(String service, String characteristic) async {
    if (service == AylaPhoneBleWifiService.idService) {
      expect(characteristic, AylaPhoneBleWifiService.dsnCharacteristic);
      return utf8.encode('$dsn\u0000');
    }
    expect(service, AylaPhoneBleWifiService.wifiService);
    expect(characteristic, AylaPhoneBleWifiService.statusCharacteristic);
    if (failStatus) throw StateError('raw-secret-status');
    final bytes = List<int>.filled(35, 0);
    final name = utf8.encode(statusSsid);
    bytes.setRange(0, name.length, name);
    bytes[32] = name.length;
    bytes[34] = 5;
    return bytes;
  }

  @override
  Future<void> write(
      String service, String characteristic, List<int> value) async {
    writes.add((service, characteristic, List.of(value)));
    if (failWrite) throw StateError('raw-secret-write');
    await writeBarrier?.future;
  }

  @override
  Future<void> disconnect() async {
    disconnects++;
  }
}

class FakeTransport implements PhoneWifiTransport {
  final gatt = FakeGatt();
  int disposed = 0;
  final addresses = <String>[];
  @override
  Future<List<String>> scan(String service) async {
    expect(service, AylaPhoneBleWifiService.idService);
    return [candidate.address];
  }

  @override
  Future<PhoneWifiGatt> connect(String address) async {
    addresses.add(address);
    return gatt;
  }

  @override
  Future<void> dispose() async {
    disposed++;
    await gatt.disconnect();
  }
}

void main() {
  test('fresh phone discovery reads identity and releases the connection',
      () async {
    final transport = FakeTransport();
    final service = AylaPhoneBleWifiService(transport: transport);
    final result = await service.discover();
    expect(result.single.dsn, candidate.dsn);
    expect(result.single.address, candidate.address);
    expect(transport.gatt.disconnects, 1);
  });

  test(
      'rechecks identity, writes exact Ayla services and confirms requested Wi-Fi',
      () async {
    final transport = FakeTransport();
    final service = AylaPhoneBleWifiService(transport: transport);
    await service.provision(candidate, token, wifi);
    final writes = transport.gatt.writes;
    expect(writes.length, 2);
    expect(writes[0].$1, AylaPhoneBleWifiService.tokenService);
    expect(writes[0].$2, AylaPhoneBleWifiService.tokenCharacteristic);
    expect(writes[0].$3, utf8.encode(token));
    expect(writes[1].$1, AylaPhoneBleWifiService.wifiService);
    expect(writes[1].$2, AylaPhoneBleWifiService.connectCharacteristic);
    final bytes = writes[1].$3;
    expect(bytes.length, 105);
    expect(bytes.sublist(0, 13), utf8.encode(wifi.ssid));
    expect(bytes[32], 13);
    expect(bytes.sublist(39, 55), utf8.encode(wifi.password));
    expect(bytes[103], 16);
    expect(bytes[104], 3);
    expect(transport.gatt.disconnects, 1);
  });

  test('wrong identity makes zero writes', () async {
    final transport = FakeTransport()..gatt.dsn = 'ACDIFFERENT1234';
    final service = AylaPhoneBleWifiService(transport: transport);
    await expectLater(
        service.provision(candidate, token, wifi),
        throwsA(isA<PhoneBleWifiFailure>()
            .having((e) => e.uncertain, 'uncertain', false)));
    expect(transport.gatt.writes, isEmpty);
    expect(transport.gatt.disconnects, 1);
  });

  for (final failure in ['write', 'status', 'wrong_network']) {
    test('$failure after write is uncertain, sanitized and disconnected',
        () async {
      final transport = FakeTransport();
      transport.gatt.failWrite = failure == 'write';
      transport.gatt.failStatus = failure == 'status';
      if (failure == 'wrong_network') {
        transport.gatt.statusSsid = 'Other network';
      }
      final service = AylaPhoneBleWifiService(transport: transport);
      await expectLater(
          service.provision(candidate, token, wifi),
          throwsA(isA<PhoneBleWifiFailure>()
              .having((e) => e.uncertain, 'uncertain', true)
              .having((e) => e.toString(), 'sanitized',
                  isNot(contains('raw-secret')))));
      expect(transport.gatt.disconnects, 1);
      expect(transport.gatt.writes.length, failure == 'write' ? 1 : 2);
    });
  }

  test('disposal during token write fences the later Wi-Fi write', () async {
    final transport = FakeTransport();
    final barrier = transport.gatt.writeBarrier = Completer<void>();
    final service = AylaPhoneBleWifiService(transport: transport);
    final attempt = service.provision(candidate, token, wifi);
    final assertion = expectLater(attempt, throwsA(isA<PhoneBleWifiFailure>()));
    await Future<void>.delayed(Duration.zero);
    expect(transport.gatt.writes.length, 1);
    await service.dispose();
    barrier.complete();
    await assertion;
    expect(transport.gatt.writes.length, 1);
    expect(transport.disposed, 1);
  });

  test(
      'Wi-Fi encoding uses UTF-8 lengths, supports open and rejects invalid data',
      () {
    final payload = AylaPhoneBleWifiService.encodeWifi(
        const RhythmCommissioningWifi(ssid: 'café', password: ''));
    expect(payload[32], 5);
    expect(payload[104], 0);
    for (final invalid in [
      const RhythmCommissioningWifi(ssid: '', password: ''),
      RhythmCommissioningWifi(ssid: 'é' * 17, password: ''),
      const RhythmCommissioningWifi(ssid: 'fixture', password: 'short'),
      const RhythmCommissioningWifi(ssid: 'fixture\u0000', password: ''),
    ]) {
      expect(() => AylaPhoneBleWifiService.encodeWifi(invalid),
          throwsA(isA<PhoneBleWifiFailure>()));
    }
  });

  test('missing and unknown protocols never activate phone provisioning', () {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    expect(AylaPhoneBleWifiService.supports(null), false);
    expect(AylaPhoneBleWifiService.supports('ayla_v2'), false);
    expect(AylaPhoneBleWifiService.supports('ayla_v1'), true);
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    expect(AylaPhoneBleWifiService.supports('ayla_v1'), false);
  });
}
