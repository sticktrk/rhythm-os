import 'package:rhythm_admin_api/src/env_loader.dart';
import 'package:test/test.dart';

void main() {
  test('uses the validated process environment', () async {
    final values = await loadAdminApiEnvironment(
      environment: {
        'SUPABASE_URL': 'https://example.invalid',
        'SUPABASE_ANON_KEY': 'anon-key',
        'ADMIN_API_PORT': '9999',
      },
    );

    expect(values['SUPABASE_URL'], 'https://example.invalid');
    expect(values['SUPABASE_ANON_KEY'], 'anon-key');
    expect(values['ADMIN_API_PORT'], '9999');
  });
}
