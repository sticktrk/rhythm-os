import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/auth/supabase_auth_backend.dart';

void main() {
  group('Google Sign-In native configuration', () {
    test('uses the web OAuth client as Android serverClientId', () {
      final config = SupabaseAuthBackend.googleSignInConfigurationForPlatform(
        isWeb: false,
        isAndroid: true,
        isIOS: false,
        isMacOS: false,
        googleSignInServerClientId: ' web-client.apps.googleusercontent.com ',
      );

      expect(config.clientId, isNull);
      expect(
        config.serverClientId,
        'web-client.apps.googleusercontent.com',
      );
    });

    test('does not initialize native Google Sign-In for macOS OAuth flow', () {
      final config = SupabaseAuthBackend.googleSignInConfigurationForPlatform(
        isWeb: false,
        isAndroid: false,
        isIOS: false,
        isMacOS: true,
        googleSignInServerClientId: 'web-client.apps.googleusercontent.com',
      );

      expect(config.clientId, isNull);
      expect(config.serverClientId, isNull);
    });

    test('falls back to platform configuration when no web OAuth client exists',
        () {
      final config = SupabaseAuthBackend.googleSignInConfigurationForPlatform(
        isWeb: false,
        isAndroid: true,
        isIOS: false,
        isMacOS: false,
        googleSignInServerClientId: '',
        legacyGoogleSignInServerClientId: '',
      );

      expect(config.clientId, isNull);
      expect(config.serverClientId, isNull);
    });

    test('falls back to the legacy OAuth env name for existing builds', () {
      final config = SupabaseAuthBackend.googleSignInConfigurationForPlatform(
        isWeb: false,
        isAndroid: true,
        isIOS: false,
        isMacOS: false,
        googleSignInServerClientId: '',
        legacyGoogleSignInServerClientId:
            'legacy-web-client.apps.googleusercontent.com',
      );

      expect(config.clientId, isNull);
      expect(
        config.serverClientId,
        'legacy-web-client.apps.googleusercontent.com',
      );
    });
  });
}
