import 'auth_user.dart';

/// High-level authentication events surfaced by backend implementations.
enum AuthEvent {
  passwordRecovery,
  signedIn,
  signedOut,
  tokenRefreshed,
  userUpdated,
}

/// Result of a Google sign-in operation.
class GoogleSignInResult {
  final AuthUser? user;
  final String? email;

  const GoogleSignInResult({this.user, this.email});
}

/// Result of an Apple sign-in operation.
class AppleSignInResult {
  final AuthUser? user;
  final String? email;

  const AppleSignInResult({this.user, this.email});
}

/// Abstract interface for authentication backend.
///
/// Implementations should handle:
/// - Anonymous sign-in (guest users)
/// - Google OAuth sign-in
/// - Email/password authentication
/// - Account linking (anonymous → permanent)
abstract class AuthBackend {
  /// Initialize the auth backend.
  Future<void> initialize();

  /// Check if user is currently signed in.
  bool get isSignedIn;

  /// Get current authenticated user.
  AuthUser? get currentUser;

  /// Get current user ID (convenience getter).
  String? get currentUserId => currentUser?.id;

  /// Check if current user is anonymous.
  bool get isAnonymous => currentUser?.isAnonymous ?? true;

  /// Stream of auth state changes.
  Stream<AuthUser?> get authStateChanges;

  /// Stream of high-level auth events.
  Stream<AuthEvent> get authEvents;

  /// Sign in anonymously (creates guest user).
  ///
  /// Returns the created user, or null if failed.
  Future<AuthUser?> signInAnonymously();

  /// Sign in with Google.
  ///
  /// If user is anonymous, links Google credentials to preserve UID.
  /// Returns GoogleSignInResult with user on success, null user if cancelled.
  /// Throws on error.
  Future<GoogleSignInResult> signInWithGoogle();

  /// Sign in with Apple.
  ///
  /// If user is anonymous, links Apple credentials to preserve UID.
  /// Returns AppleSignInResult with user on success, null user if cancelled.
  /// Throws on error.
  Future<AppleSignInResult> signInWithApple();

  /// Sign in with email and password.
  ///
  /// Returns the user on success, throws on error.
  Future<AuthUser?> signInWithEmailPassword(String email, String password);

  /// Create account with email and password.
  ///
  /// If user is currently anonymous, links credentials to preserve UID.
  /// Returns the user on success, throws on error.
  Future<AuthUser?> createAccountWithEmailPassword(
      String email, String password);

  /// Link email/password credentials to current anonymous user.
  ///
  /// Throws if user is not signed in or email already exists.
  Future<AuthUser?> linkWithEmailPassword(String email, String password);

  /// Send a password reset email.
  Future<void> sendPasswordResetEmail(String email);

  /// Verify a password recovery email token hash and create a recovery session.
  Future<AuthUser?> verifyPasswordRecoveryTokenHash(String tokenHash);

  /// Update the current user's password.
  Future<AuthUser?> updatePassword(String password);

  /// Sign out the current user.
  Future<void> signOut();

  /// Delete the current user's account permanently.
  ///
  /// This will delete all user data from the backend and sign out.
  /// Throws on error.
  Future<void> deleteAccount();

  /// Disconnect any third-party providers (e.g., revoke Google access).
  Future<void> disconnectProviders();

  /// Dispose of resources.
  void dispose();
}
