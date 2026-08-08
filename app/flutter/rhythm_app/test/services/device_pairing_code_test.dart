import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/device_pairing_code.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
  group('device pairing code classification', () {
    const localBleQr = 'B:0A0B0C0D0E0F%G\$S:SYNTHETIC000001\$M:TESTMODEL001';

    test('strictly parses a known local-BLE profile into bounded fields', () {
      final setup = parseLocalBleSetupCode(localBleQr);
      expect(setup?.profileId, RhythmDeviceProfileId.oreinOc02001Button);
      expect(
        locallyRegisteredLocalBleProfileIds,
        contains(RhythmDeviceProfileId.oreinOc02001Button),
      );
      expect(
        setup?.setupFields,
        {
          'ble_identity': '0A0B0C0D0E0F',
          'serial_metadata': 'SYNTHETIC000001',
          'model_metadata': 'TESTMODEL001',
        },
      );
      expect(
        setup?.pairingParams,
        {
          'profile_id': RhythmDeviceProfileId.oreinOc02001Button,
          'setup': {
            'ble_identity': '0A0B0C0D0E0F',
            'serial_metadata': 'SYNTHETIC000001',
            'model_metadata': 'TESTMODEL001',
          },
        },
      );
      final code = classifyDevicePairingCode(localBleQr);
      expect(code.kind, DevicePairingCodeKind.localBle);
      expect(code.payload, RhythmDeviceProfileId.oreinOc02001Button);
      expect(code.payload, isNot(localBleQr));
    });

    test('copies setup fields and produces an order-independent dedupe key',
        () {
      final mutableFields = <String, String>{'second': '2', 'first': '1'};
      final setup = LocalBleSetup(
        profileId: 'test.profile.v1',
        setupFields: mutableFields,
      );
      mutableFields['first'] = 'changed';

      expect(setup.setupFields, {'second': '2', 'first': '1'});
      expect(
        () => setup.setupFields['third'] = '3',
        throwsUnsupportedError,
      );
      expect(
        setup.deduplicationKey,
        LocalBleSetup(
          profileId: 'test.profile.v1',
          setupFields: const {'first': '1', 'second': '2'},
        ).deduplicationKey,
      );
    });

    test('rejects local-BLE profile QR near misses', () {
      for (final value in [
        'B:0A0B0C0D0E0%G\$S:SYNTHETIC000001\$M:TESTMODEL001',
        'B:0A0B0C0D0E0G%G\$S:SYNTHETIC000001\$M:TESTMODEL001',
        'B:0A0B0C0D0E0F\$S:SYNTHETIC000001\$M:TESTMODEL001',
        'B:0A0B0C0D0E0F%G\$S:\$M:TESTMODEL001',
        'B:0A0B0C0D0E0F%G\$S:SYNTHETIC000001\$M:TESTMODEL001\$M:EXTRA',
      ]) {
        expect(parseLocalBleSetupCode(value), isNull, reason: value);
      }
    });

    test('local-BLE code requires the matching advertised profile', () {
      expect(processDevicePairingCodes([localBleQr])?.canContinue, isFalse);
      final supported = processDevicePairingCodes(
        [localBleQr],
        supportedLocalBleProfileIds: const {
          RhythmDeviceProfileId.oreinOc02001Button,
        },
      );
      expect(supported?.canContinue, isTrue);
      expect(
        supported?.code.localBleSetup?.profileId,
        RhythmDeviceProfileId.oreinOc02001Button,
      );
    });

    test('offers valid Matter and supported local-BLE codes as choices', () {
      final supported = processDevicePairingCodes(
        [localBleQr, 'MT:Y.K908OC16750648G00'],
        supportedLocalBleProfileIds: const {
          RhythmDeviceProfileId.oreinOc02001Button,
        },
      );

      expect(supported?.requiresChoice, isTrue);
      expect(
        supported?.choices.map((code) => code.kind),
        [DevicePairingCodeKind.matter, DevicePairingCodeKind.localBle],
      );
    });

    test('Matter wins when the captured local-BLE profile is unsupported', () {
      final decision = processDevicePairingCodes([
        localBleQr,
        'MT:Y.K908OC16750648G00',
      ]);

      expect(decision?.requiresChoice, isFalse);
      expect(decision?.code.kind, DevicePairingCodeKind.matter);
      expect(decision?.code.payload, 'MT:Y.K908OC16750648G00');
    });

    test('malformed BLE-looking data cannot mask valid Matter', () {
      final decision = processDevicePairingCodes([
        'B:0A0B0C0D0E0F%G\$S:bad',
        '3497-011-2332',
      ]);

      expect(decision?.code.kind, DevicePairingCodeKind.matter);
    });

    test('recognizes Matter before considering bulb brand', () {
      final result = classifyDevicePairingCode('  MT:Y.K908OC16750648G00  ');

      expect(result.kind, DevicePairingCodeKind.matter);
      expect(result.payload, 'MT:Y.K908OC16750648G00');
    });

    test('recognizes manually entered Matter setup codes', () {
      final result = classifyDevicePairingCode('  3497-011-2332  ');

      expect(result.kind, DevicePairingCodeKind.matter);
      expect(result.payload, '3497-011-2332');
    });

    test('shared processor prioritizes a usable code', () {
      final decision = processDevicePairingCodes([
        'X-HM://0023ISYWY8H2B',
        '34970112332',
      ]);

      expect(decision?.canContinue, isTrue);
      expect(decision?.code.kind, DevicePairingCodeKind.matter);
      expect(decision?.code.payload, '34970112332');
    });

    test('recognizes HomeKit setup URIs case-insensitively', () {
      expect(
        classifyDevicePairingCode('x-hm://0023ISYWY8H2B').kind,
        DevicePairingCodeKind.homeKit,
      );
      expect(
        classifyDevicePairingCode('123-45-678').kind,
        DevicePairingCodeKind.homeKit,
      );
    });

    test('recognizes Hue setup prefixes and owned URLs', () {
      expect(
        classifyDevicePairingCode('HUE:device-setup-data').kind,
        DevicePairingCodeKind.hue,
      );
      expect(
        classifyDevicePairingCode(
          'https://www.philips-hue.com/connectproduct',
        ).kind,
        DevicePairingCodeKind.hue,
      );
      expect(
        classifyDevicePairingCode('https://discovery.meethue.com/setup').kind,
        DevicePairingCodeKind.hue,
      );
    });

    test('recognizes and normalizes six-character Hue Bridge serials', () {
      for (final serial in const ['27F706', 'e277da', 'E2-77-DA']) {
        final result = classifyDevicePairingCode(serial);
        expect(result.kind, DevicePairingCodeKind.hue);
        expect(result.payload, serial == '27F706' ? '27F706' : 'E277DA');
      }

      expect(normalizeHueBridgeSerial('HUE:e277da'), 'E277DA');
      expect(normalizeHueBridgeSerial('27F70Z'), isNull);
      expect(normalizeHueBridgeSerial('123-45-678'), isNull);
    });

    test('extracts serials from representative Hue QR suffix fixtures', () {
      const installCode = '0123456789ABCDEF0123456789ABCDEF0123';
      final firstRepresentativeQr = 'HUE:Z:$installCode M:001788010927F706';
      final secondRepresentativeQr =
          'hue:z:${installCode.toLowerCase()} m:0017880109e277da '
          'product-specific trailing fields';

      expect(hueBridgeSerialFromSetupQr(firstRepresentativeQr), '27F706');
      expect(hueBridgeSerialFromSetupQr(secondRepresentativeQr), 'E277DA');
      expect(
          classifyDevicePairingCode(secondRepresentativeQr).payload, 'E277DA');
      expect(
        processDevicePairingCodes(
          [firstRepresentativeQr],
          hueBridgeSerialSearchAvailable: true,
        )?.canContinue,
        isTrue,
      );
    });

    test('does not derive a serial from malformed Hue setup payloads', () {
      const valid = 'HUE:Z:0123456789ABCDEF0123456789ABCDEF0123 '
          'M:0017880109E277DA D:L3B A:1184';

      expect(
        hueBridgeSerialFromSetupQr(valid.replaceFirst(' M:', 'M:')),
        isNull,
      );
      expect(
        hueBridgeSerialFromSetupQr(valid.replaceFirst('E277DA', 'E277D')),
        isNull,
      );
      expect(
        hueBridgeSerialFromSetupQr(valid.replaceFirst('E277DA', 'E277DA0')),
        isNull,
      );
      expect(
        hueBridgeSerialFromSetupQr(valid.replaceFirst('E277DA', 'E27ZDA')),
        isNull,
      );
    });

    test('ignores optional Hue product fields after the EUI-64', () {
      const prefix = 'HUE:Z:0123456789ABCDEF0123456789ABCDEF0123 '
          'M:0017880109E277DA';

      expect(hueBridgeSerialFromSetupQr(prefix), 'E277DA');
      expect(
        hueBridgeSerialFromSetupQr('$prefix D:PRODUCT-VARIANT A:1184 EXTRA'),
        'E277DA',
      );
    });

    test('Hue serial continues only when bridge search is available', () {
      final decision = processDevicePairingCodes([
        'https://www.philips-hue.com/connectproduct',
        'HUE:E277DA',
      ], hueBridgeSerialSearchAvailable: true);

      expect(decision?.canContinue, isTrue);
      expect(decision?.code.kind, DevicePairingCodeKind.hue);
      expect(decision?.code.payload, 'E277DA');
    });

    test('Hue serial guides to nearby BLE scan without bridge capability', () {
      final decision = processDevicePairingCodes(['E277DA']);

      expect(decision?.canContinue, isFalse);
      expect(decision?.requiresChoice, isFalse);
      expect(decision?.guidance.title, 'Use Add Device');
      expect(decision?.guidance.message, contains('not available right now'));
    });

    test('Matter wins a dual-code frame when no Hue Bridge is available', () {
      final decision = processDevicePairingCodes(
        ['MT:Y.K908OC16750648G00', 'E277DA'],
      );

      expect(decision?.canContinue, isTrue);
      expect(decision?.requiresChoice, isFalse);
      expect(decision?.code.kind, DevicePairingCodeKind.matter);
    });

    test('dual-code frame requires a choice with Hue Bridge search', () {
      final decision = processDevicePairingCodes(
        ['MT:Y.K908OC16750648G00', 'E277DA'],
        hueBridgeSerialSearchAvailable: true,
      );

      expect(decision?.canContinue, isFalse);
      expect(decision?.requiresChoice, isTrue);
      expect(
        decision?.choices.map((code) => code.kind),
        [DevicePairingCodeKind.matter, DevicePairingCodeKind.hue],
      );
    });

    test('bridge-only intake ignores Matter and selects the Hue serial', () {
      final decision = processDevicePairingCodes(
        ['MT:Y.K908OC16750648G00', 'E277DA'],
        hueBridgeSerialSearchAvailable: true,
        hueBridgeOnly: true,
      );

      expect(decision?.canContinue, isTrue);
      expect(decision?.requiresChoice, isFalse);
      expect(decision?.code.kind, DevicePairingCodeKind.hue);
      expect(decision?.code.payload, 'E277DA');
    });

    test('bridge-only intake never continues with a Matter code', () {
      final decision = processDevicePairingCodes(
        ['MT:Y.K908OC16750648G00'],
        hueBridgeSerialSearchAvailable: true,
        hueBridgeOnly: true,
      );

      expect(decision?.canContinue, isFalse);
      expect(decision?.guidance.title, 'Hue bulb serial not found');
    });

    test('Hue Bridge serial guidance names the bridge path', () {
      final decision = processDevicePairingCodes(
        ['E277DA'],
        hueBridgeSerialSearchAvailable: true,
      );

      expect(decision?.canContinue, isTrue);
      expect(decision?.guidance.title, 'Hue Bridge serial');
    });

    test('does not mistake an unrelated URL for a device code', () {
      expect(
        classifyDevicePairingCode('https://example.com/connectproduct').kind,
        DevicePairingCodeKind.unknown,
      );
    });
  });

  group('device pairing guidance', () {
    test('routes unsupported ecosystems to their correct next step', () {
      expect(
        guidanceForDevicePairingCode(DevicePairingCodeKind.homeKit).title,
        'HomeKit isn’t supported',
      );
      expect(
        guidanceForDevicePairingCode(DevicePairingCodeKind.hue).title,
        'Use Add Device',
      );
      expect(
        guidanceForDevicePairingCode(DevicePairingCodeKind.unknown).title,
        'Code not recognized',
      );
    });
  });
}
