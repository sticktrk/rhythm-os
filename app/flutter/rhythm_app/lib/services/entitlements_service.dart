import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:supabase_flutter/supabase_flutter.dart' hide AuthUser;

import '../backend/backend.dart';
import '../config/feature_flags.dart';
import '../config/platform_capabilities.dart';
import '../models/plan_tier.dart';
import 'auth_service.dart';
import 'employee_mode_service.dart';
import 'hue/hue_service_locator.dart';

/// Local-only key used by the plan-tier modal to grant/revoke entitlements
/// in demo mode (App Store reviewer flow, before real IAP exists). The
/// override is only honored while [HueServiceLocator.isDemoMode] or the
/// demo account is signed in — outside demo it is loaded but ignored.
const String _kDemoTierOverrideKey = 'rhythm_demo_tier_override';

/// Resolves the signed-in user's [PlanTier] from Supabase, applying defaults
/// for users that can't have a real subscription (HA add-on, demo, anonymous).
///
/// This is the only thing in the app that talks to the `subscriptions` table.
/// Everything else asks [has] or listens to [tierChanges].
class EntitlementsService {
  EntitlementsService._(this._capabilities);

  static EntitlementsService? _instance;
  static EntitlementsService get instance {
    final s = _instance;
    if (s == null) {
      throw StateError(
        'EntitlementsService not initialized. Call EntitlementsService.bootstrap() first.',
      );
    }
    return s;
  }

  /// Create the singleton. Safe to call once during app startup, after
  /// [BackendProvider.initialize] has run.
  static Future<EntitlementsService> bootstrap(
      PlatformCapabilities capabilities) async {
    final existing = _instance;
    if (existing != null) return existing;
    final service = EntitlementsService._(capabilities);
    await service._initialize();
    _instance = service;
    return service;
  }

  final PlatformCapabilities _capabilities;
  final StreamController<PlanTier> _controller =
      StreamController<PlanTier>.broadcast();
  PlanTier _currentTier = PlanTier.basic;
  PlanTier? _demoOverride;

  StreamSubscription<AuthUser?>? _authSub;
  StreamSubscription<List<Map<String, dynamic>>>? _realtimeSub;
  String? _subscribedUserId;

  PlanTier get currentTier => _currentTier;

  /// True when the resolved tier can be flipped from the plan-tier modal.
  /// Only available in demo mode (reviewer flow / pre-IAP testing).
  bool get isDemoOverrideActive => _isInDemoContext();

  /// Current demo override, if any. Only meaningful while
  /// [isDemoOverrideActive] is true.
  PlanTier? get demoOverride => _demoOverride;

  bool has(Entitlement e) {
    if (!FeatureFlags.entitlementsEnabled) return !e.isComingSoon;
    if (EmployeeModeService.instance.isActive) return !e.isComingSoon;
    return _currentTier.grants(e) && !e.isComingSoon;
  }

  bool isEligibleFor(Entitlement e) {
    if (!FeatureFlags.entitlementsEnabled) return true;
    if (EmployeeModeService.instance.isActive) return true;
    return _currentTier.grants(e);
  }

  Stream<PlanTier> get tierChanges => _controller.stream;

  Future<void> _initialize() async {
    _demoOverride = await _loadDemoOverride();
    // Resolve once synchronously-ish for the very first read (initial value
    // is `basic`; we'll emit the real one as soon as the query returns).
    await _resolveAndEmit();
    _authSub = AuthService().authStateChanges.listen(
          (_) => unawaited(_resolveAndEmit()),
        );
    HueServiceLocator.onDemoEnabled(_resolveAndEmit);
    HueServiceLocator.onDemoDisabled(_resolveAndEmit);
  }

  /// Pin the demo tier to [tier]. Only takes effect in demo mode. Pass
  /// `null` to clear and fall back to the demo default (Pro).
  Future<void> setDemoOverride(PlanTier? tier) async {
    _demoOverride = tier;
    final prefs = await SharedPreferences.getInstance();
    if (tier == null) {
      await prefs.remove(_kDemoTierOverrideKey);
    } else {
      await prefs.setString(_kDemoTierOverrideKey, tier.name);
    }
    await _resolveAndEmit();
  }

  Future<PlanTier?> _loadDemoOverride() async {
    try {
      final prefs = await SharedPreferences.getInstance();
      return PlanTierX.parse(prefs.getString(_kDemoTierOverrideKey));
    } catch (_) {
      return null;
    }
  }

  bool _isInDemoContext() {
    return HueServiceLocator.isDemoMode ||
        AuthService().currentUser?.email == DemoCredentials.email;
  }

  Future<void> _resolveAndEmit() async {
    final next = await _resolveTier();
    if (next != _currentTier) {
      _currentTier = next;
      debugPrint('EntitlementsService: tier resolved to ${next.name}');
      if (!_controller.isClosed) _controller.add(next);
    }
  }

  Future<PlanTier> _resolveTier() async {
    // HA add-on / fully self-hosted: no SaaS model applies.
    if (!_capabilities.hasCloudBackend) {
      _unsubscribeRealtime();
      return PlanTier.pro;
    }

    // Demo account: App Store reviewers need to see everything. In demo
    // mode the plan-tier modal can flip the tier locally so reviewers (and
    // we ourselves before IAP ships) can exercise both Basic and Pro
    // surfaces.
    if (_isInDemoContext()) {
      _unsubscribeRealtime();
      return _demoOverride ?? PlanTier.pro;
    }

    final auth = AuthService();
    if (!auth.isSignedIn || auth.isAnonymous) {
      _unsubscribeRealtime();
      return PlanTier.basic;
    }

    final userId = auth.currentUserId;
    final client = _client;
    if (userId == null || client == null) {
      _unsubscribeRealtime();
      return PlanTier.basic;
    }

    _ensureRealtimeSubscription(userId, client);

    try {
      final rows = await client
          .from('subscriptions')
          .select('tier, status, ended_at')
          .eq('user_id', userId)
          .eq('status', 'active')
          .order('started_at', ascending: false)
          .limit(1);
      if (rows.isEmpty) return PlanTier.basic;
      final row = Map<String, dynamic>.from(rows.first as Map);
      final endedAtRaw = row['ended_at'];
      if (endedAtRaw is String) {
        final endedAt = DateTime.tryParse(endedAtRaw);
        if (endedAt != null && endedAt.isBefore(DateTime.now())) {
          return PlanTier.basic;
        }
      }
      return PlanTierX.parse(row['tier'] as String?) ?? PlanTier.basic;
    } catch (e) {
      debugPrint(
        'EntitlementsService: query failed, keeping ${_currentTier.name}: $e',
      );
      return _currentTier;
    }
  }

  void _ensureRealtimeSubscription(String userId, SupabaseClient client) {
    if (_subscribedUserId == userId && _realtimeSub != null) return;
    _unsubscribeRealtime();
    _realtimeSub = client
        .from('subscriptions')
        .stream(primaryKey: ['id'])
        .eq('user_id', userId)
        .listen(
          (_) => unawaited(_resolveAndEmit()),
          onError: (Object e) =>
              debugPrint('EntitlementsService: realtime error: $e'),
        );
    _subscribedUserId = userId;
  }

  void _unsubscribeRealtime() {
    _realtimeSub?.cancel();
    _realtimeSub = null;
    _subscribedUserId = null;
  }

  SupabaseClient? get _client {
    if (!BackendProvider.isInitialized) return null;
    final auth = BackendProvider.instance.auth;
    if (auth is SupabaseAuthBackend) return auth.client;
    return null;
  }

  /// Force a re-query. Useful from UI after a known external change.
  Future<void> refresh() => _resolveAndEmit();

  /// Server-side plan change for the signed-in user. Calls the
  /// `set-subscription-tier` edge function, which cancels existing active
  /// rows and inserts a new one. No real billing yet — this is the pre-IAP
  /// path so we can exercise the full Subscribe/Cancel flow end-to-end.
  ///
  /// Throws if no Supabase client is available or the function returns an
  /// error. Successful calls re-emit the tier through [tierChanges] via the
  /// realtime listener; we also force a [refresh] for snappy UI.
  Future<void> changePlan(PlanTier tier) async {
    final client = _client;
    if (client == null) {
      throw StateError('Cannot change plan: no Supabase client available.');
    }
    final response = await client.functions.invoke(
      'set-subscription-tier',
      body: {'tier': tier.name},
    );
    if (response.status != 200) {
      final data = response.data;
      final message = data is Map && data['error'] is String
          ? data['error'] as String
          : 'set-subscription-tier failed (${response.status})';
      throw Exception(message);
    }
    await refresh();
  }

  void dispose() {
    _authSub?.cancel();
    _realtimeSub?.cancel();
    _controller.close();
  }
}
