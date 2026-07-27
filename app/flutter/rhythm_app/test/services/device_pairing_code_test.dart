import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/device_pairing_code.dart';

void main() {
  group('device pairing code classification', () {
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
        'Pair this device in the Hue app',
      );
      expect(
        guidanceForDevicePairingCode(DevicePairingCodeKind.unknown).title,
        'Code not recognized',
      );
    });
  });
}
