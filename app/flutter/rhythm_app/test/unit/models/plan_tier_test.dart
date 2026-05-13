import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/models/plan_tier.dart';

void main() {
  group('PlanTier.grants', () {
    test('Basic grants account-backed free features', () {
      expect(PlanTier.basic.grants(Entitlement.cloudBackupRestore), isTrue);
      expect(PlanTier.basic.grants(Entitlement.multiDeviceSync), isTrue);
      expect(PlanTier.basic.grants(Entitlement.multiUserAccess), isTrue);

      expect(PlanTier.basic.grants(Entitlement.standby), isFalse);
      expect(PlanTier.basic.grants(Entitlement.advancedDayControls), isFalse);
      expect(PlanTier.basic.grants(Entitlement.sleepPrimarySettings), isFalse);
      expect(PlanTier.basic.grants(Entitlement.remoteAccess), isFalse);
    });

    test('Pro grants every declared entitlement', () {
      for (final entitlement in Entitlement.values) {
        expect(
          PlanTier.pro.grants(entitlement),
          isTrue,
          reason: 'Pro must grant ${entitlement.name}',
        );
      }
    });

    test('Every Entitlement is decided by every PlanTier', () {
      // If this fails, _grants in plan_tier.dart is missing a tier entry —
      // a new tier was added without wiring its grants set.
      for (final tier in PlanTier.values) {
        for (final entitlement in Entitlement.values) {
          expect(
            () => tier.grants(entitlement),
            returnsNormally,
            reason: '${tier.name} must answer for ${entitlement.name}',
          );
        }
      }
    });
  });

  group('PlanTier display metadata', () {
    test('uses user-facing paid/free labels without changing wire values', () {
      expect(PlanTier.basic.displayName, 'Free');
      expect(PlanTier.pro.displayName, 'Pro');
      expect(PlanTier.basic.isPaid, isFalse);
      expect(PlanTier.pro.isPaid, isTrue);
    });
  });

  group('Entitlement display metadata', () {
    test('describes standby gating', () {
      expect(Entitlement.standby.displayName, 'Standby Lighting');
      expect(Entitlement.standby.minimumTier, PlanTier.pro);
      expect(Entitlement.standby.isComingSoon, isFalse);
      expect(Entitlement.standby.labelFor(PlanTier.basic), 'Pro only');
      expect(Entitlement.standby.labelFor(PlanTier.pro), 'Included');
    });

    test('describes account-backed free features', () {
      expect(Entitlement.cloudBackupRestore.minimumTier, PlanTier.basic);
      expect(Entitlement.multiDeviceSync.displayName, 'All Rooms Sync');
      expect(Entitlement.multiDeviceSync.minimumTier, PlanTier.basic);
      expect(Entitlement.multiDeviceSync.labelFor(PlanTier.basic), 'Included');
    });

    test('marks not-yet-launched features as coming soon', () {
      expect(Entitlement.multiUserAccess.minimumTier, PlanTier.basic);
      expect(Entitlement.multiUserAccess.isComingSoon, isTrue);
      expect(
          Entitlement.multiUserAccess.labelFor(PlanTier.basic), 'Coming soon');
      expect(Entitlement.remoteAccess.minimumTier, PlanTier.pro);
      expect(Entitlement.remoteAccess.isComingSoon, isTrue);
      expect(Entitlement.remoteAccess.labelFor(PlanTier.basic), 'Pro only');
      expect(Entitlement.remoteAccess.labelFor(PlanTier.pro), 'Coming soon');
    });
  });

  group('PlanTierX.parse', () {
    test('parses wire values', () {
      expect(PlanTierX.parse('basic'), PlanTier.basic);
      expect(PlanTierX.parse('pro'), PlanTier.pro);
    });

    test('returns null for unknown or missing tier strings', () {
      expect(PlanTierX.parse(null), isNull);
      expect(PlanTierX.parse(''), isNull);
      expect(PlanTierX.parse('enterprise'), isNull);
    });
  });
}
