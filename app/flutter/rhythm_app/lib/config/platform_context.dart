/// Compile-time platform context set via `--dart-define=PLATFORM_CONTEXT=value`.
///
/// Tells the UI what context it's running in so it can show/hide
/// sections that don't apply.
///
/// Values:
///   ha_addon        — HA add-on (web via ingress)
///   standalone_web  — rhythm-server (web standalone)
///   mobile          — iOS / Android
///   desktop         — macOS / Linux / Windows
enum PlatformContext { haAddon, standaloneWeb, mobile, desktop }

class PlatformCtx {
  static const String _raw = String.fromEnvironment(
    'PLATFORM_CONTEXT',
    defaultValue: 'mobile',
  );

  static PlatformContext get current => switch (_raw) {
        'ha_addon' => PlatformContext.haAddon,
        'standalone_web' => PlatformContext.standaloneWeb,
        'desktop' => PlatformContext.desktop,
        _ => PlatformContext.mobile,
      };

  static bool get isHaAddon => current == PlatformContext.haAddon;
  static bool get isWeb =>
      current == PlatformContext.haAddon ||
      current == PlatformContext.standaloneWeb;
  static bool get isMobile => current == PlatformContext.mobile;
}
