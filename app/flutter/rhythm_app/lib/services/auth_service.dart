import 'dart:async';
import 'package:flutter/foundation.dart';

import '../backend/backend.dart';
import 'employee_mode_service.dart';
import 'hue/hue_service_locator.dart';
import 'support_access_service.dart';

/// Result of a Google sign-in operation.
/// Re-exported from backend for backwards compatibility.
export '../backend/auth/auth_backend.dart'
    show GoogleSignInResult, AppleSignInResult;

/// Service for handling authentication.
///
/// This service delegates to the configured backend (Supabase, Firebase, etc.)
/// through the BackendProvider abstraction layer.
class AuthService {
  static final AuthService _instance = AuthService._internal();
  factory AuthService() => _instance;
  AuthService._internal();

  // TODO: TEMPORARY - Set this to recover an old guest account, then remove
  // Example: 'abc123xyz...' (the old anonymous UID)
  static const String? _overrideUserId = null;

  AuthBackend? get _auth =>
      BackendProvider.isInitialized ? BackendProvider.instance.auth : null;

  /// Initialize auth service.
  Future<void> initialize() async {
    debugPrint('AuthService initialized');
  }

  /// Check if user is currently signed in.
  bool get isSignedIn => _auth?.isSignedIn ?? false;

  /// Get current user.
  AuthUser? get currentUser => _auth?.currentUser;

  /// Get current user ID.
  String? get currentUserId => _overrideUserId ?? _auth?.currentUserId;

  /// Check if current user is anonymous.
  bool get isAnonymous => _auth?.isAnonymous ?? true;

  /// Stream of auth state changes.
  Stream<AuthUser?> get authStateChanges =>
      _auth?.authStateChanges ?? const Stream.empty();

  /// Stream of high-level auth events.
  Stream<AuthEvent> get authEvents => _auth?.authEvents ?? const Stream.empty();

  /// Check if the current user has a specific feature flag enabled.
  /// Feature flags are stored in Supabase raw_user_meta_data.
  bool hasFeature(String feature) {
    final userMetadata =
        currentUser?.metadata?['userMetadata'] as Map<String, dynamic>?;
    return userMetadata?[feature] == true;
  }

  /// Sign in anonymously (creates user with uid but no credentials).
  Future<AuthUser?> signInAnonymously() async {
    return await _auth!.signInAnonymously();
  }

  /// Sign in with Google.
  /// If user is anonymous, links Google credentials to preserve uid.
  /// Returns GoogleSignInResult with user on success, null user if cancelled.
  /// Throws on error.
  Future<GoogleSignInResult> signInWithGoogle() async {
    return await _auth!.signInWithGoogle();
  }

  /// Sign in with Apple.
  /// If user is anonymous, links Apple credentials to preserve uid.
  /// Returns AppleSignInResult with user on success, null user if cancelled.
  /// Throws on error.
  Future<AppleSignInResult> signInWithApple() async {
    return await _auth!.signInWithApple();
  }

  /// Sign in with email and password.
  Future<AuthUser?> signInWithEmailPassword(
      String email, String password) async {
    return await _auth!.signInWithEmailPassword(email, password);
  }

  /// Create account with email and password.
  /// If user is currently anonymous, links credentials to preserve uid.
  Future<AuthUser?> createAccountWithEmailPassword(
      String email, String password) async {
    return await _auth!.createAccountWithEmailPassword(email, password);
  }

  /// Link email/password credentials to current anonymous user.
  /// Throws if user is not signed in or email already exists.
  Future<AuthUser?> linkWithEmailPassword(String email, String password) async {
    return await _auth!.linkWithEmailPassword(email, password);
  }

  /// Send a password reset email.
  Future<void> sendPasswordResetEmail(String email) async {
    await _auth!.sendPasswordResetEmail(email);
  }

  /// Verify a password recovery email token hash and create a recovery session.
  Future<AuthUser?> verifyPasswordRecoveryTokenHash(String tokenHash) async {
    return await _auth!.verifyPasswordRecoveryTokenHash(tokenHash);
  }

  /// Update the current user's password.
  Future<AuthUser?> updatePassword(String password) async {
    return await _auth!.updatePassword(password);
  }

  /// Sign out from both backend and Google.
  /// Also clears demo mode if active. Employee mode is normally cleared too,
  /// except during staff-session bootstrap where we need to discard a stale
  /// anonymous account without losing the active support grant.
  Future<void> signOut({bool preserveEmployeeMode = false}) async {
    HueServiceLocator.setDemoMode(false);
    if (!preserveEmployeeMode) {
      await _revokeEmployeeGrantIfPresent(EmployeeModeService.instance.grantId);
      EmployeeModeService.instance.exit();
    }
    await _auth!.signOut();
  }

  /// Delete the current user's account permanently.
  /// This will delete all user data and sign out.
  /// Also clears demo mode if active.
  Future<void> deleteAccount() async {
    HueServiceLocator.setDemoMode(false);
    await _revokeEmployeeGrantIfPresent(EmployeeModeService.instance.grantId);
    EmployeeModeService.instance.exit();
    await _auth!.deleteAccount();
  }

  /// Disconnect Google account (revokes access).
  Future<void> disconnectGoogle() async {
    await _auth!.disconnectProviders();
  }

  Future<void> _revokeEmployeeGrantIfPresent(String? grantId) async {
    final cleanGrantId = grantId?.trim();
    if (cleanGrantId == null || cleanGrantId.isEmpty) return;

    try {
      await SupportAccessService.instance.revokeGrant(cleanGrantId);
    } catch (error, stackTrace) {
      debugPrint('AuthService: support grant revoke skipped: $error');
      debugPrint('$stackTrace');
    }
  }
}
