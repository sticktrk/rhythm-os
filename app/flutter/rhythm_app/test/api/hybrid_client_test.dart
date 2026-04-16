import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/api/hybrid_client.dart';
import 'package:rhythm_core/rhythm_core.dart';

void main() {
  group('resolveHybridApiBaseUrl', () {
    test('returns null on native when no base URL or server hub exists', () {
      expect(
        resolveHybridApiBaseUrl(isWeb: false),
        isNull,
      );
    });

    test('normalizes an explicit native base URL', () {
      expect(
        resolveHybridApiBaseUrl(
          isWeb: false,
          baseUrl: 'http://10.0.0.8:54448',
        ),
        'http://10.0.0.8:54448/',
      );
    });

    test('uses the most recently connected enabled server hub on native', () {
      final olderHub = Hub.server(
        id: 'server-1',
        homeId: 'home-1',
        name: 'Older',
        host: '192.168.1.50',
        port: 54448,
      ).copyWith(
        lastConnected: DateTime(2026, 4, 10, 8),
        updatedAt: DateTime(2026, 4, 10, 8),
      );

      final newerHub = Hub.server(
        id: 'server-2',
        homeId: 'home-1',
        name: 'Newer',
        host: '192.168.1.99',
        port: 54449,
      ).copyWith(
        lastConnected: DateTime(2026, 4, 12, 9),
        updatedAt: DateTime(2026, 4, 12, 9),
      );

      final disabledNewestHub = Hub.server(
        id: 'server-3',
        homeId: 'home-1',
        name: 'Disabled',
        host: '192.168.1.120',
        port: 54450,
      ).copyWith(
        enabled: false,
        lastConnected: DateTime(2026, 4, 13, 10),
        updatedAt: DateTime(2026, 4, 13, 10),
      );

      expect(
        resolveHybridApiBaseUrl(
          isWeb: false,
          storedHubs: [olderHub, disabledNewestHub, newerHub],
        ),
        'http://192.168.1.99:54449/',
      );
    });

    test('uses the current web base URI when no explicit base URL is given',
        () {
      expect(
        resolveHybridApiBaseUrl(
          isWeb: true,
          webBaseUri: Uri.parse('https://example.com/rhythm'),
        ),
        'https://example.com/rhythm/',
      );
    });
  });
}
