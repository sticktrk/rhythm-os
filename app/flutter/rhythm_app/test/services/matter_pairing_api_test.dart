import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/matter_setup_payload.dart';

void main() {
  group('Matter setup payload detection', () {
    test('detects Matter QR payloads', () {
      expect(
        detectMatterSetupPayloadKind('  MT:Y.K908OC16750648G00  '),
        MatterSetupPayloadKind.qr,
      );
    });

    test('detects manual pairing codes with separators', () {
      expect(
        detectMatterSetupPayloadKind('3497-123-4567'),
        MatterSetupPayloadKind.manual,
      );
    });

    test('treats unrelated content as unknown', () {
      expect(
        detectMatterSetupPayloadKind('https://example.com'),
        MatterSetupPayloadKind.unknown,
      );
    });
  });
}
