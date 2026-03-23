/// Compile-time feature flags configured via .env file.
///
/// These flags are set at build time using --dart-define-from-file=.env
/// and cannot be changed at runtime.
class FeatureFlags {
  /// Login/signup functionality enabled.
  ///
  /// When false:
  /// - Account section hidden from settings
  /// - Onboarding auto-continues in local-only mode
  /// - Sign-in modal won't open
  ///
  /// Configure in .env: LOGIN_ENABLED=true/false
  static const bool loginEnabled = bool.fromEnvironment(
    'LOGIN_ENABLED',
    defaultValue: true,
  );
}
