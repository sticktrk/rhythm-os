/// Compile-time feature flags configured via .env file.
///
/// These flags are set at build time using --dart-define-from-file=.env
/// and cannot be changed at runtime.
class FeatureFlags {
  /// Show login/signup in settings and other auxiliary screens.
  ///
  /// When false:
  /// - Account section hidden from settings
  /// - Sign-in modal won't open
  ///
  /// Configure in .env: AUX_SIGN_IN=true/false
  static const bool auxSignIn = bool.fromEnvironment(
    'AUX_SIGN_IN',
    defaultValue: true,
  );

  /// Enforce plan-tier entitlements throughout the app.
  ///
  /// When false, every [Entitlement] check returns true (except features
  /// marked `isComingSoon`, which remain hidden because they're not yet
  /// implemented). The plan-tier modal and Supabase subscription resolution
  /// still run, but no feature surface is gated.
  ///
  /// Configure in .env: ENTITLEMENTS_ENABLED=true/false
  static const bool entitlementsEnabled = bool.fromEnvironment(
    'ENTITLEMENTS_ENABLED',
    defaultValue: false,
  );

  /// Show the per-profile "Advanced" (timing fine-tune) section on the Light
  /// tab. Hidden for now while the Light tab is reworked around layered
  /// profiles; the underlying UI is retained and can be switched back on.
  ///
  /// Configure in .env: SHOW_ADVANCED_LIGHT_SECTION=true/false
  static const bool showAdvancedLightSection = bool.fromEnvironment(
    'SHOW_ADVANCED_LIGHT_SECTION',
    defaultValue: false,
  );

  /// Enable Cloudflare Tunnel-backed remote access for Rhythm Server hubs.
  ///
  /// When false, the app ignores remote endpoints, hides remote access setup,
  /// and uses the existing LAN-only server connection behavior.
  ///
  /// Configure in .env: REMOTE_ACCESS_TUNNEL_ENABLED=true/false
  static const bool remoteAccessTunnel = bool.fromEnvironment(
    'REMOTE_ACCESS_TUNNEL_ENABLED',
    defaultValue: true,
  );
}
