import 'dart:async';

import 'package:flutter/foundation.dart';

import '../models/plan_tier.dart';
import '../services/entitlements_service.dart';

/// Provider façade over [EntitlementsService] so widgets can `context.watch`
/// for tier changes and rebuild automatically.
class SubscriptionProvider extends ChangeNotifier {
  SubscriptionProvider(this._service) : _tier = _service.currentTier {
    _sub = _service.tierChanges.listen((tier) {
      _tier = tier;
      notifyListeners();
    });
  }

  final EntitlementsService _service;
  StreamSubscription<PlanTier>? _sub;
  PlanTier _tier;

  PlanTier get tier => _tier;
  bool get isPro => _tier == PlanTier.pro;
  bool has(Entitlement e) => _tier.grants(e) && !e.isComingSoon;
  bool isEligibleFor(Entitlement e) => _tier.grants(e);

  /// True when the plan-tier modal should let the user flip between Basic
  /// and Pro locally (demo-mode reviewer flow / pre-IAP testing).
  bool get isDemoOverrideActive => _service.isDemoOverrideActive;

  /// Active demo override, if any.
  PlanTier? get demoOverride => _service.demoOverride;

  /// Flip the demo-mode tier locally. No-op in production tier resolution
  /// because [EntitlementsService] only honors this in a demo context.
  Future<void> setDemoOverride(PlanTier? tier) =>
      _service.setDemoOverride(tier);

  /// Server-side plan change for the signed-in (non-demo) user. Routes
  /// through the `set-subscription-tier` edge function and refreshes.
  Future<void> changePlan(PlanTier tier) => _service.changePlan(tier);

  Future<void> refresh() => _service.refresh();

  @override
  void dispose() {
    _sub?.cancel();
    super.dispose();
  }
}
