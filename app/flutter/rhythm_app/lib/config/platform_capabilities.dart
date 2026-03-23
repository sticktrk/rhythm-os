import 'platform_context.dart';

/// Describes what features are available on the current platform.
/// Injected via Provider at the app root — widgets check capabilities,
/// not platform identity.
class PlatformCapabilities {
  /// User can sign in / manage a cloud account (Supabase).
  final bool hasAccounts;

  /// User can pair hubs (Hue bridge, RhythmServer) manually.
  final bool hasHubPairing;

  /// User needs to set location manually (vs auto-provided by HA).
  final bool hasLocationSetup;

  /// Platform manages rooms externally (auto-imported, not user-created).
  final bool autoImportsRooms;

  /// Should initialize Supabase backend and auth services.
  final bool hasCloudBackend;

  const PlatformCapabilities({
    required this.hasAccounts,
    required this.hasHubPairing,
    required this.hasLocationSetup,
    required this.autoImportsRooms,
    required this.hasCloudBackend,
  });

  /// Resolve capabilities from the compile-time platform context.
  factory PlatformCapabilities.fromPlatform() {
    return switch (PlatformCtx.current) {
      PlatformContext.haAddon => const PlatformCapabilities(
        hasAccounts: false,
        hasHubPairing: true,
        hasLocationSetup: false,
        autoImportsRooms: false,
        hasCloudBackend: false,
      ),
      PlatformContext.standaloneWeb => const PlatformCapabilities(
        hasAccounts: true,
        hasHubPairing: true,
        hasLocationSetup: true,
        autoImportsRooms: false,
        hasCloudBackend: true,
      ),
      _ => const PlatformCapabilities(
        hasAccounts: true,
        hasHubPairing: true,
        hasLocationSetup: true,
        autoImportsRooms: false,
        hasCloudBackend: true,
      ),
    };
  }
}
