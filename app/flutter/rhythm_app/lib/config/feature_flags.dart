/// Compile-time feature flags configured via .env file.
///
/// These flags are set at build time using --dart-define-from-file=.env
/// and cannot be changed at runtime.
class FeatureFlags {
  /// Show login/signup during onboarding flow.
  ///
  /// When false, onboarding skips the account screen and assumes local-only.
  ///
  /// Configure in .env: ONBOARDING_SIGN_IN=true/false
  static const bool onboardingSignIn = bool.fromEnvironment(
    'ONBOARDING_SIGN_IN',
    defaultValue: false,
  );

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
}
