import '../config/feature_flags.dart';

/// User-facing paid tier.
///
/// New features are gated on [Entitlement], not on tier directly — consumers
/// should always go through [PlanTierX.grants] so adding a new feature is a
/// single-table change here.
enum PlanTier { basic, pro }

/// A named capability that may or may not be available to a given tier.
///
enum Entitlement {
  /// Signed-in Free feature: cloud backup and restore.
  cloudBackupRestore,

  /// Signed-in Free feature: account-backed layout sync across phones.
  multiDeviceSync,

  /// Signed-in Free feature: shared account users. Product surface is coming
  /// soon.
  multiUserAccess,

  /// Standby lighting: per-room standby state and the standby section in the
  /// light profile editor.
  standby,

  /// Advanced day-mode timing controls: motion timeout, light transition, and
  /// background interval.
  advancedDayControls,

  /// Sleep profile primary brightness/color controls.
  sleepPrimarySettings,

  /// Physical-button trigger for Day ⇄ Sleep transitions on the Transition
  /// screen.
  transitionButton,

  /// Day-profile time simulator — drag-to-scrub a preview of the lighting
  /// curve at any hour.
  timeSimulator,

  /// Remote access. Product surface is coming soon.
  remoteAccess,
}

const Map<PlanTier, Set<Entitlement>> _grants = {
  PlanTier.basic: <Entitlement>{
    Entitlement.cloudBackupRestore,
    Entitlement.multiDeviceSync,
    Entitlement.multiUserAccess,
    Entitlement.remoteAccess,
  },
  PlanTier.pro: {
    Entitlement.cloudBackupRestore,
    Entitlement.multiDeviceSync,
    Entitlement.multiUserAccess,
    Entitlement.standby,
    Entitlement.advancedDayControls,
    Entitlement.sleepPrimarySettings,
    Entitlement.transitionButton,
    Entitlement.timeSimulator,
    Entitlement.remoteAccess,
  },
};

extension PlanTierX on PlanTier {
  /// User-facing plan label. The wire value remains `basic`; the product
  /// surface calls it Free so the paid/free split is obvious.
  String get displayName => switch (this) {
        PlanTier.basic => 'Free',
        PlanTier.pro => 'Pro',
      };

  bool get isPaid => this == PlanTier.pro;

  Set<Entitlement> get entitlements => _grants[this]!;

  bool grants(Entitlement entitlement) => entitlements.contains(entitlement);

  /// Parse a wire-format tier string (matches the Supabase enum).
  static PlanTier? parse(String? raw) {
    switch (raw) {
      case 'basic':
        return PlanTier.basic;
      case 'pro':
        return PlanTier.pro;
      default:
        return null;
    }
  }
}

extension EntitlementX on Entitlement {
  String get displayName => switch (this) {
        Entitlement.cloudBackupRestore => 'Cloud Backup & Restore',
        Entitlement.multiDeviceSync => 'All Rooms Sync',
        Entitlement.multiUserAccess => 'Multi-user Access',
        Entitlement.standby => 'Standby Lighting',
        Entitlement.advancedDayControls => 'Advanced Day Controls',
        Entitlement.sleepPrimarySettings => 'Sleep Primary Settings',
        Entitlement.transitionButton => 'Button Trigger',
        Entitlement.timeSimulator => 'Time Simulator',
        Entitlement.remoteAccess => 'Remote Access',
      };

  String get shortDescription => switch (this) {
        Entitlement.cloudBackupRestore => 'Back up and restore your Rhythm hub',
        Entitlement.multiDeviceSync =>
          'Restore All Rooms layout on a new phone',
        Entitlement.multiUserAccess => 'Share a hub with multiple users',
        Entitlement.standby => 'Standby scenes for day and sleep modes',
        Entitlement.advancedDayControls =>
          'Custom motion timeout, transition, and interval',
        Entitlement.sleepPrimarySettings => 'Custom sleep brightness and color',
        Entitlement.transitionButton =>
          'Bind a physical button to Day ⇄ Sleep transitions',
        Entitlement.timeSimulator =>
          'Drag to preview light at any hour of the day',
        Entitlement.remoteAccess => 'Control Rhythm away from home',
      };

  PlanTier get minimumTier => switch (this) {
        Entitlement.cloudBackupRestore => PlanTier.basic,
        Entitlement.multiDeviceSync => PlanTier.basic,
        Entitlement.multiUserAccess => PlanTier.basic,
        Entitlement.standby => PlanTier.pro,
        Entitlement.advancedDayControls => PlanTier.pro,
        Entitlement.sleepPrimarySettings => PlanTier.pro,
        Entitlement.transitionButton => PlanTier.pro,
        Entitlement.timeSimulator => PlanTier.pro,
        Entitlement.remoteAccess => PlanTier.basic,
      };

  bool get isComingSoon => switch (this) {
        Entitlement.multiUserAccess => true,
        Entitlement.remoteAccess => !FeatureFlags.remoteAccessTunnel,
        _ => false,
      };

  String labelFor(PlanTier tier) {
    if (!tier.grants(this)) return '${minimumTier.displayName} only';
    if (isComingSoon) return 'Coming soon';
    return 'Included';
  }
}
