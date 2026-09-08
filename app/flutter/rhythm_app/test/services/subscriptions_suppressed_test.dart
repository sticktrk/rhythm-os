import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/config/platform_capabilities.dart';
import 'package:rhythm_app/models/plan_tier.dart';
import 'package:rhythm_app/providers/subscription_provider.dart';
import 'package:rhythm_app/screens/settings/sections/account_section.dart';
import 'package:rhythm_app/services/entitlements_service.dart';
import 'package:rhythm_app/widgets/plan_tier_modal.dart';

// Any attempt to inspect a user or subscribe to auth during tier resolution
// fails, so an offline fallback cannot mask a cloud dependency in this test.
class _UnusedAuth extends OfflineAuthBackend {
  @override
  AuthUser? get currentUser => throw StateError('Unexpected auth read');

  @override
  Stream<AuthUser?> get authStateChanges =>
      throw StateError('Unexpected auth subscription');
}

void main() {
  testWidgets(
      'plans stay suppressed and implemented features need no subscription backend',
      (tester) async {
    BackendProvider.setInstanceForTesting(auth: _UnusedAuth());
    addTearDown(BackendProvider.resetForTesting);
    final service = await EntitlementsService.bootstrap(
      PlatformCapabilities.fromPlatform(),
    );
    final provider = SubscriptionProvider(service);
    addTearDown(provider.dispose);
    addTearDown(service.dispose);

    await service.refresh();
    await service.setDemoOverride(PlanTier.pro);
    expect(service.demoOverride, isNull);
    await expectLater(service.changePlan(PlanTier.pro), throwsStateError);
    for (final feature in Entitlement.values) {
      expect(service.has(feature), !feature.isComingSoon);
      expect(provider.has(feature), !feature.isComingSoon);
      expect(provider.isEligibleFor(feature), isTrue);
    }

    for (final user in <AuthUser?>[
      null,
      const AuthUser(id: 'guest', isAnonymous: true),
      const AuthUser(
          id: 'member', email: 'member@example.test', isAnonymous: false),
    ]) {
      await tester.pumpWidget(ChangeNotifierProvider.value(
        value: provider,
        child: MaterialApp(
            home: Scaffold(
                body: Column(children: [
          AccountSection(user: user),
          Builder(
              builder: (context) => TextButton(
                    onPressed: () => PlanTierModal.show(context),
                    child: const Text('Legacy plan entry'),
                  )),
        ]))),
      ));
      expect(find.text('Plan'), findsNothing);
      expect(find.text('Account & Plan'), findsNothing);
      await tester.tap(find.text('Legacy plan entry'));
      await tester.pumpAndSettle();
      expect(find.byType(PlanTierModal), findsNothing);
    }
  });
}
