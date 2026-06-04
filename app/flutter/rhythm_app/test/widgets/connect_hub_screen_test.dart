import 'package:bonsoir/bonsoir.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/widgets/connect_hub_screen.dart';

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
  });
}
