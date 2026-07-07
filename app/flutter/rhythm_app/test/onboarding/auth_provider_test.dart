import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/backend/backend.dart';
import 'package:rhythm_app/onboarding/providers/auth_provider.dart';

void main() {
  late _FakeAuthBackend auth;

  setUp(() {
    auth = _FakeAuthBackend();
    BackendProvider.setInstanceForTesting(auth: auth);
  });

  tearDown(() {
    BackendProvider.resetForTesting();
  });

  test('starts unauthenticated when there is no backend session', () {
    final provider = AuthProvider();
    addTearDown(provider.dispose);

    expect(provider.state, AuthState.initial);
    expect(provider.isAuthenticated, isFalse);
    expect(auth.signInAnonymouslyCalls, 0);
  });

  test('does not treat an anonymous backend user as authenticated', () async {
    final anonymousUser = const AuthUser(
      id: 'anon-user',
      isAnonymous: true,
    );
    auth.currentUser = anonymousUser;

    final provider = AuthProvider();
    addTearDown(provider.dispose);

    expect(provider.state, AuthState.initial);
    expect(provider.isAuthenticated, isFalse);

    auth.emitAuthState(anonymousUser);
    await pumpEventQueue();

    expect(provider.state, AuthState.initial);
    expect(provider.isAuthenticated, isFalse);
    expect(provider.isAnonymous, isTrue);
  });

  test('Google sign-in succeeds from a signed-out state', () async {
    const user = AuthUser(
      id: 'google-user',
      email: 'user@example.com',
      isAnonymous: false,
    );
    auth.googleResult = const GoogleSignInResult(
      user: user,
      email: 'user@example.com',
    );

    final provider = AuthProvider();
    addTearDown(provider.dispose);

    final success = await provider.signInWithGoogle();

    expect(success, isTrue);
    expect(provider.state, AuthState.authenticated);
    expect(provider.isAuthenticated, isTrue);
    expect(provider.user, user);
    expect(provider.email, 'user@example.com');
    expect(auth.signInAnonymouslyCalls, 0);
  });

  test('Google sign-in errors when it leaves the user anonymous', () async {
    const anonymousUser = AuthUser(
      id: 'anon-user',
      email: 'user@example.com',
      isAnonymous: true,
    );
    auth.googleResult = const GoogleSignInResult(
      user: anonymousUser,
      email: 'user@example.com',
    );

    final provider = AuthProvider();
    addTearDown(provider.dispose);

    final success = await provider.signInWithGoogle();

    expect(success, isFalse);
    expect(provider.state, AuthState.error);
    expect(provider.isAuthenticated, isFalse);
    expect(provider.errorMessage, 'Sign-in did not finish. Please try again.');
  });
}

class _FakeAuthBackend implements AuthBackend {
  final _authStateController = StreamController<AuthUser?>.broadcast();
  final _authEventController = StreamController<AuthEvent>.broadcast();

  @override
  AuthUser? currentUser;
  GoogleSignInResult googleResult = const GoogleSignInResult();
  int signInAnonymouslyCalls = 0;

  void emitAuthState(AuthUser? user) {
    currentUser = user;
    _authStateController.add(user);
  }

  @override
  Future<void> initialize() async {}

  @override
  bool get isSignedIn => currentUser != null;

  @override
  String? get currentUserId => currentUser?.id;

  @override
  bool get isAnonymous => currentUser?.isAnonymous ?? true;

  @override
  Stream<AuthUser?> get authStateChanges => _authStateController.stream;

  @override
  Stream<AuthEvent> get authEvents => _authEventController.stream;

  @override
  Future<AuthUser?> signInAnonymously() async {
    signInAnonymouslyCalls++;
    currentUser = const AuthUser(id: 'anon-user', isAnonymous: true);
    _authStateController.add(currentUser);
    return currentUser;
  }

  @override
  Future<GoogleSignInResult> signInWithGoogle() async {
    currentUser = googleResult.user;
    if (currentUser != null) {
      _authStateController.add(currentUser);
    }
    return googleResult;
  }

  @override
  Future<AppleSignInResult> signInWithApple() async {
    throw UnimplementedError();
  }

  @override
  Future<AuthUser?> signInWithEmailPassword(
    String email,
    String password,
  ) async {
    throw UnimplementedError();
  }

  @override
  Future<AuthUser?> createAccountWithEmailPassword(
    String email,
    String password,
  ) async {
    throw UnimplementedError();
  }

  @override
  Future<AuthUser?> linkWithEmailPassword(String email, String password) async {
    throw UnimplementedError();
  }

  @override
  Future<void> sendPasswordResetEmail(String email) async {
    throw UnimplementedError();
  }

  @override
  Future<AuthUser?> verifyPasswordRecoveryTokenHash(String tokenHash) async {
    throw UnimplementedError();
  }

  @override
  Future<AuthUser?> updatePassword(String password) async {
    throw UnimplementedError();
  }

  @override
  Future<void> signOut() async {
    currentUser = null;
    _authStateController.add(null);
  }

  @override
  Future<void> deleteAccount() async {
    currentUser = null;
    _authStateController.add(null);
  }

  @override
  Future<void> disconnectProviders() async {}

  @override
  void dispose() {
    _authStateController.close();
    _authEventController.close();
  }
}
