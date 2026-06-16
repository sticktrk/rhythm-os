import 'dart:async';
import 'auth_backend.dart';
import 'auth_user.dart';

/// Offline auth backend that works without a remote server.
///
/// Returns a local "offline" user and stores state in memory only.
/// Used when Supabase credentials are not configured.
class OfflineAuthBackend implements AuthBackend {
  AuthUser? _currentUser;
  final _authController = StreamController<AuthUser?>.broadcast();
  final _authEventController = StreamController<AuthEvent>.broadcast();

  @override
  Future<void> initialize() async {
    // Create a persistent offline user
    _currentUser = const AuthUser(
      id: 'offline-user',
      email: null,
      displayName: 'Offline User',
      isAnonymous: true,
    );
    _authController.add(_currentUser);
  }

  @override
  void dispose() {
    _authController.close();
    _authEventController.close();
  }

  // This backend is always ready after construction.
  bool get isInitialized => true;

  @override
  AuthUser? get currentUser => _currentUser;

  @override
  String? get currentUserId => _currentUser?.id;

  @override
  bool get isSignedIn => _currentUser != null;

  @override
  bool get isAnonymous => _currentUser?.isAnonymous ?? true;

  @override
  Stream<AuthUser?> get authStateChanges => _authController.stream;

  @override
  Stream<AuthEvent> get authEvents => _authEventController.stream;

  @override
  Future<AuthUser?> signInAnonymously() async {
    _currentUser = const AuthUser(
      id: 'offline-user',
      email: null,
      displayName: 'Offline User',
      isAnonymous: true,
    );
    _authController.add(_currentUser);
    return _currentUser;
  }

  @override
  Future<GoogleSignInResult> signInWithGoogle() async {
    // Can't do Google sign-in offline
    throw UnsupportedError('Google sign-in requires an online connection');
  }

  @override
  Future<AppleSignInResult> signInWithApple() async {
    // Can't do Apple sign-in offline
    throw UnsupportedError('Apple sign-in requires an online connection');
  }

  @override
  Future<AuthUser?> signInWithEmailPassword(
      String email, String password) async {
    throw UnsupportedError('Email sign-in requires an online connection');
  }

  @override
  Future<AuthUser?> createAccountWithEmailPassword(
      String email, String password) async {
    throw UnsupportedError('Account creation requires an online connection');
  }

  @override
  Future<AuthUser?> linkWithEmailPassword(String email, String password) async {
    throw UnsupportedError('Account linking requires an online connection');
  }

  @override
  Future<void> sendPasswordResetEmail(String email) async {
    throw UnsupportedError('Password reset requires an online connection');
  }

  @override
  Future<AuthUser?> verifyPasswordRecoveryTokenHash(String tokenHash) async {
    throw UnsupportedError('Password recovery requires an online connection');
  }

  @override
  Future<AuthUser?> updatePassword(String password) async {
    throw UnsupportedError('Password update requires an online connection');
  }

  @override
  Future<void> signOut() async {
    // Keep the offline user - just reset to anonymous state
    _currentUser = const AuthUser(
      id: 'offline-user',
      email: null,
      displayName: 'Offline User',
      isAnonymous: true,
    );
    _authController.add(_currentUser);
    _authEventController.add(AuthEvent.signedOut);
  }

  @override
  Future<void> deleteAccount() async {
    // Can't delete account offline - just reset to anonymous state
    throw UnsupportedError('Account deletion requires an online connection');
  }

  @override
  Future<void> disconnectProviders() async {
    // No-op for offline
  }
}
