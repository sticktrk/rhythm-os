import 'dart:async';
import 'dart:convert';
import 'dart:io' show Platform, HttpServer, HttpStatus, ContentType;
import 'dart:math';
import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart';
import 'package:google_sign_in/google_sign_in.dart';
import 'package:http/http.dart' as http;
import 'package:sign_in_with_apple/sign_in_with_apple.dart';
import 'package:supabase_flutter/supabase_flutter.dart' hide AuthUser;
import 'package:url_launcher/url_launcher.dart';

import 'auth_backend.dart';
import 'auth_user.dart';

/// Supabase implementation of the authentication backend.
class SupabaseAuthBackend implements AuthBackend {
  final String supabaseUrl;
  final String supabaseAnonKey;

  SupabaseClient? _client;
  GoogleSignIn get _googleSignIn => GoogleSignIn.instance;
  static Future<void>? _googleSignInInitialization;

  StreamController<AuthUser?>? _authStateController;
  StreamController<AuthEvent>? _authEventController;
  AuthEvent? _latestAuthEvent;

  SupabaseAuthBackend({
    required this.supabaseUrl,
    required this.supabaseAnonKey,
  });

  SupabaseClient get client {
    if (_client == null) {
      throw StateError('SupabaseAuthBackend not initialized');
    }
    return _client!;
  }

  @override
  Future<void> initialize() async {
    await Supabase.initialize(
      url: supabaseUrl,
      anonKey: supabaseAnonKey,
    );
    _client = Supabase.instance.client;

    // Set up auth state stream
    _authStateController = StreamController<AuthUser?>.broadcast();
    _authEventController = StreamController<AuthEvent>.broadcast();
    _client!.auth.onAuthStateChange.listen((data) {
      final user = data.session?.user;
      _authStateController!.add(user != null ? _mapUser(user) : null);
      final event = _mapAuthEvent(data.event);
      if (event != null) {
        _latestAuthEvent = event;
        _authEventController!.add(event);
      }
    });

    debugPrint('SupabaseAuthBackend: Initialized');
  }

  @override
  bool get isSignedIn => client.auth.currentUser != null;

  @override
  AuthUser? get currentUser {
    final user = client.auth.currentUser;
    if (user == null) return null;
    return _mapUser(user);
  }

  @override
  String? get currentUserId => client.auth.currentUser?.id;

  @override
  bool get isAnonymous => client.auth.currentUser?.isAnonymous ?? true;

  AuthEvent? _mapAuthEvent(AuthChangeEvent event) {
    return switch (event) {
      AuthChangeEvent.passwordRecovery => AuthEvent.passwordRecovery,
      AuthChangeEvent.signedIn => AuthEvent.signedIn,
      AuthChangeEvent.signedOut => AuthEvent.signedOut,
      AuthChangeEvent.tokenRefreshed => AuthEvent.tokenRefreshed,
      AuthChangeEvent.userUpdated => AuthEvent.userUpdated,
      _ => null,
    };
  }

  @override
  Stream<AuthUser?> get authStateChanges {
    if (_authStateController == null) {
      throw StateError('SupabaseAuthBackend not initialized');
    }
    return _authStateController!.stream;
  }

  @override
  Stream<AuthEvent> get authEvents {
    if (_authEventController == null) {
      throw StateError('SupabaseAuthBackend not initialized');
    }
    return Stream.multi((controller) {
      final latestAuthEvent = _latestAuthEvent;
      if (latestAuthEvent != null) {
        controller.add(latestAuthEvent);
      }
      final subscription = _authEventController!.stream.listen(
        controller.add,
        onError: controller.addError,
        onDone: controller.close,
      );
      controller.onCancel = subscription.cancel;
    });
  }

  @override
  Future<AuthUser?> signInAnonymously() async {
    try {
      final response = await client.auth.signInAnonymously();
      if (response.user != null) {
        debugPrint(
            'SupabaseAuthBackend: Signed in anonymously: ${response.user!.id}');
        return _mapUser(response.user!);
      }
      return null;
    } catch (e) {
      debugPrint('SupabaseAuthBackend: Anonymous sign-in failed: $e');
      rethrow;
    }
  }

  @override
  Future<GoogleSignInResult> signInWithGoogle() async {
    // On macOS, use Supabase OAuth flow (opens browser) since native
    // google_sign_in requires a client secret for Desktop OAuth clients
    if (!kIsWeb && Platform.isMacOS) {
      return _signInWithGoogleOAuth();
    }
    return _signInWithGoogleNative();
  }

  // Native mobile Google Sign-In requires a Web OAuth client ID as the
  // serverClientId. Keep the older env name as a fallback for existing builds.
  static const _googleSignInServerClientId = String.fromEnvironment(
    'GOOGLE_SIGN_IN_SERVER_CLIENT_ID',
    defaultValue: '',
  );
  static const _legacyGoogleSignInServerClientId = String.fromEnvironment(
    'GOOGLE_WEB_OAUTH_CLIENT_ID',
    defaultValue: '',
  );

  // macOS manual OAuth uses an installed-app/Desktop OAuth client because it
  // relies on loopback redirects with an ephemeral local port.
  static const _googleDesktopOAuthClientId = String.fromEnvironment(
    'GOOGLE_DESKTOP_OAUTH_CLIENT_ID',
    defaultValue: _legacyGoogleSignInServerClientId,
  );
  static const _webClientSecret = String.fromEnvironment(
    'GOOGLE_DESKTOP_OAUTH_SECRET',
    defaultValue: _legacyGoogleDesktopOAuthSecret,
  );
  static const _legacyGoogleDesktopOAuthSecret = String.fromEnvironment(
    'GOOGLE_WEB_OAUTH_SECRET',
    defaultValue: '',
  );

  @visibleForTesting
  static ({String? clientId, String? serverClientId})
      googleSignInConfigurationForPlatform({
    required bool isWeb,
    required bool isAndroid,
    required bool isIOS,
    required bool isMacOS,
    String googleSignInServerClientId = _googleSignInServerClientId,
    String legacyGoogleSignInServerClientId = _legacyGoogleSignInServerClientId,
  }) {
    if (isWeb || isMacOS || (!isAndroid && !isIOS)) {
      return (clientId: null, serverClientId: null);
    }

    final serverClientId = googleSignInServerClientId.trim().isNotEmpty
        ? googleSignInServerClientId.trim()
        : legacyGoogleSignInServerClientId.trim();
    return (
      clientId: null,
      serverClientId: serverClientId.isEmpty ? null : serverClientId,
    );
  }

  Future<void> _initializeGoogleSignIn() async {
    final shouldInitializeNative =
        !kIsWeb && (Platform.isAndroid || Platform.isIOS);
    if (!shouldInitializeNative) return;

    final config = googleSignInConfigurationForPlatform(
      isWeb: kIsWeb,
      isAndroid: !kIsWeb && Platform.isAndroid,
      isIOS: !kIsWeb && Platform.isIOS,
      isMacOS: !kIsWeb && Platform.isMacOS,
    );

    try {
      _googleSignInInitialization ??= _googleSignIn.initialize(
        clientId: config.clientId,
        serverClientId: config.serverClientId,
      );
      await _googleSignInInitialization;
    } catch (_) {
      _googleSignInInitialization = null;
      rethrow;
    }
  }

  /// Manual OAuth flow for macOS using local HTTP server callback.
  /// This handles the token exchange with the Desktop client secret,
  /// then uses the ID token with Supabase (same as iOS native flow).
  ///
  /// Google Desktop OAuth clients automatically allow loopback redirects
  /// on any port, so we use an ephemeral port for security.
  Future<GoogleSignInResult> _signInWithGoogleOAuth() async {
    if (_googleDesktopOAuthClientId.isEmpty || _webClientSecret.isEmpty) {
      throw Exception(
        'Google OAuth credentials not configured. '
        'Set GOOGLE_DESKTOP_OAUTH_CLIENT_ID and GOOGLE_DESKTOP_OAUTH_SECRET in .env',
      );
    }

    debugPrint('SupabaseAuthBackend: Starting manual OAuth flow for macOS');

    // Generate PKCE code verifier and challenge
    final codeVerifier = _generateNonce(64);
    final codeChallenge = base64Url
        .encode(sha256.convert(utf8.encode(codeVerifier)).bytes)
        .replaceAll('=', '');

    // Start local server on ephemeral port (OS assigns available port)
    final server = await HttpServer.bind('127.0.0.1', 0);
    final redirectUri = 'http://127.0.0.1:${server.port}/callback';

    debugPrint('SupabaseAuthBackend: Listening on $redirectUri');

    try {
      // Build Google OAuth URL
      final authUrl = Uri.https('accounts.google.com', '/o/oauth2/v2/auth', {
        'client_id': _googleDesktopOAuthClientId,
        'redirect_uri': redirectUri,
        'response_type': 'code',
        'scope': 'openid email profile',
        'code_challenge': codeChallenge,
        'code_challenge_method': 'S256',
        'access_type': 'offline',
      });

      // Open browser
      if (!await launchUrl(authUrl, mode: LaunchMode.externalApplication)) {
        throw Exception('Could not open browser for Google Sign-In');
      }

      // Wait for callback with timeout
      String? authCode;
      await for (final request in server.timeout(const Duration(minutes: 5))) {
        if (request.uri.path == '/callback') {
          authCode = request.uri.queryParameters['code'];
          final error = request.uri.queryParameters['error'];

          if (error != null) {
            request.response
              ..statusCode = HttpStatus.ok
              ..headers.contentType = ContentType.html
              ..write('<html><body><h1>Sign-in cancelled</h1>'
                  '<p>You can close this window.</p></body></html>');
            await request.response.close();
            debugPrint('SupabaseAuthBackend: OAuth cancelled: $error');
            return const GoogleSignInResult();
          }

          if (authCode != null) {
            request.response
              ..statusCode = HttpStatus.ok
              ..headers.contentType = ContentType.html
              ..write('<html><body><h1>Sign-in successful!</h1>'
                  '<p>You can close this window and return to the app.</p></body></html>');
            await request.response.close();
            break;
          }
        }
        request.response.statusCode = HttpStatus.notFound;
        await request.response.close();
      }

      if (authCode == null) {
        throw Exception('No authorization code received');
      }

      debugPrint('SupabaseAuthBackend: Got auth code, exchanging for tokens');

      // Exchange code for tokens
      final tokenResponse = await http.post(
        Uri.https('oauth2.googleapis.com', '/token'),
        headers: {'Content-Type': 'application/x-www-form-urlencoded'},
        body: {
          'client_id': _googleDesktopOAuthClientId,
          'client_secret': _webClientSecret,
          'code': authCode,
          'code_verifier': codeVerifier,
          'grant_type': 'authorization_code',
          'redirect_uri': redirectUri,
        },
      );

      if (tokenResponse.statusCode != 200) {
        debugPrint(
            'SupabaseAuthBackend: Token exchange failed: ${tokenResponse.body}');
        throw Exception(
            'Failed to exchange code for tokens: ${tokenResponse.body}');
      }

      final tokenData = jsonDecode(tokenResponse.body) as Map<String, dynamic>;
      final idToken = tokenData['id_token'] as String?;

      if (idToken == null) {
        throw Exception('No ID token in response');
      }

      debugPrint('SupabaseAuthBackend: Got ID token, signing in with Supabase');

      final accessToken = tokenData['access_token'] as String?;
      final response = await _signInOrLinkWithIdToken(
        provider: OAuthProvider.google,
        idToken: idToken,
        accessToken: accessToken,
        providerName: 'Google',
      );

      // Extract email from ID token
      final parts = idToken.split('.');
      String? email;
      if (parts.length == 3) {
        final payload = jsonDecode(
          utf8.decode(base64Url.decode(base64Url.normalize(parts[1]))),
        ) as Map<String, dynamic>;
        email = payload['email'] as String?;
      }

      final resultUser = _mapAuthResponseUser(response);
      debugPrint(
          'SupabaseAuthBackend: macOS Google sign-in successful: $email');
      return GoogleSignInResult(user: resultUser, email: email);
    } on TimeoutException {
      throw Exception('Google Sign-In timed out');
    } catch (e) {
      debugPrint('SupabaseAuthBackend: Google OAuth failed: $e');
      rethrow;
    } finally {
      // Always close the server
      await server.close();
      debugPrint('SupabaseAuthBackend: Local OAuth server closed');
    }
  }

  /// Native sign-in flow for iOS/Android using google_sign_in package.
  Future<GoogleSignInResult> _signInWithGoogleNative() async {
    try {
      // Native Google setup is only needed when the user chooses this sign-in
      // method; it must not delay every recovered-session app launch.
      await _initializeGoogleSignIn();

      // Use google_sign_in v7 API
      final GoogleSignInAccount googleUser;
      try {
        googleUser = await _googleSignIn.authenticate();
      } on GoogleSignInException catch (e) {
        if (e.code == GoogleSignInExceptionCode.canceled) {
          return const GoogleSignInResult();
        }
        if (e.code == GoogleSignInExceptionCode.clientConfigurationError) {
          throw AuthException(
            'Google Sign-In is not configured for this app build. '
            'Check GOOGLE_SIGN_IN_SERVER_CLIENT_ID, the Android package '
            'name, and the signing SHA in Google Cloud.',
            code: 'google_sign_in_configuration_error',
          );
        }
        rethrow;
      }

      debugPrint('SupabaseAuthBackend: Google user: ${googleUser.email}');

      // Get ID token (v7 API - authentication is synchronous property)
      final googleAuth = googleUser.authentication;
      final idToken = googleAuth.idToken;

      if (idToken == null) {
        throw const AuthException(
          'Google Sign-In did not return an ID token. '
          'Configure GOOGLE_SIGN_IN_SERVER_CLIENT_ID as the Google web OAuth '
          'server client ID for this build.',
          code: 'google_sign_in_missing_id_token',
        );
      }

      // NOTE: "Skip nonce checks" must be enabled in Supabase Dashboard:
      // Authentication → Providers → Google → Skip nonce checks
      final response = await _signInOrLinkWithIdToken(
        provider: OAuthProvider.google,
        idToken: idToken,
        providerName: 'Google',
      );

      final resultUser = _mapAuthResponseUser(response);
      return GoogleSignInResult(user: resultUser, email: googleUser.email);
    } catch (e) {
      debugPrint('SupabaseAuthBackend: Google sign-in failed: $e');
      rethrow;
    }
  }

  @override
  Future<AppleSignInResult> signInWithApple() async {
    try {
      // Generate a secure random nonce
      final rawNonce = _generateNonce();
      final hashedNonce = sha256.convert(utf8.encode(rawNonce)).toString();

      // Request Apple credentials
      final AuthorizationCredentialAppleID credential;
      try {
        credential = await SignInWithApple.getAppleIDCredential(
          scopes: [
            AppleIDAuthorizationScopes.email,
            AppleIDAuthorizationScopes.fullName,
          ],
          nonce: hashedNonce,
        );
      } on SignInWithAppleAuthorizationException catch (e) {
        if (e.code == AuthorizationErrorCode.canceled) {
          return const AppleSignInResult();
        }
        rethrow;
      }

      final idToken = credential.identityToken;
      if (idToken == null) {
        throw Exception('No identity token received from Apple');
      }

      debugPrint(
          'SupabaseAuthBackend: Apple credential received, email: ${credential.email}');

      final response = await _signInOrLinkWithIdToken(
        provider: OAuthProvider.apple,
        idToken: idToken,
        nonce: rawNonce,
        providerName: 'Apple',
      );

      final resultUser = _mapAuthResponseUser(response);
      // Note: Apple only provides email on first sign-in, may be null on subsequent
      return AppleSignInResult(user: resultUser, email: credential.email);
    } catch (e) {
      debugPrint('SupabaseAuthBackend: Apple sign-in failed: $e');
      rethrow;
    }
  }

  /// Generate a cryptographically secure random nonce.
  String _generateNonce([int length = 32]) {
    const charset =
        '0123456789ABCDEFGHIJKLMNOPQRSTUVXYZabcdefghijklmnopqrstuvwxyz-._';
    final random = Random.secure();
    return List.generate(length, (_) => charset[random.nextInt(charset.length)])
        .join();
  }

  Future<AuthResponse> _signInOrLinkWithIdToken({
    required OAuthProvider provider,
    required String idToken,
    required String providerName,
    String? accessToken,
    String? nonce,
  }) async {
    final currentUser = client.auth.currentUser;

    if (currentUser != null && currentUser.isAnonymous) {
      debugPrint(
          'SupabaseAuthBackend: Linking $providerName to anonymous user: ${currentUser.id}');
      try {
        return await client.auth.linkIdentityWithIdToken(
          provider: provider,
          idToken: idToken,
          accessToken: accessToken,
          nonce: nonce,
        );
      } on AuthException catch (e) {
        if (!_isExistingProviderAccountError(e)) rethrow;

        debugPrint(
            'SupabaseAuthBackend: $providerName account exists, signing in to existing account');
        await client.auth.signOut();
        return await client.auth.signInWithIdToken(
          provider: provider,
          idToken: idToken,
          accessToken: accessToken,
          nonce: nonce,
        );
      }
    }

    return await client.auth.signInWithIdToken(
      provider: provider,
      idToken: idToken,
      accessToken: accessToken,
      nonce: nonce,
    );
  }

  bool _isExistingProviderAccountError(AuthException error) {
    final code = error.code;
    if (code == 'identity_already_exists' ||
        code == 'email_exists' ||
        code == 'user_already_exists') {
      return true;
    }

    final message = error.message.toLowerCase();
    return message.contains('already registered') ||
        message.contains('already exists') ||
        message.contains('identity already') ||
        message.contains('email exists');
  }

  AuthUser? _mapAuthResponseUser(AuthResponse response) {
    final user = response.user ?? client.auth.currentUser;
    return user != null ? _mapUser(user) : null;
  }

  @override
  Future<AuthUser?> signInWithEmailPassword(
      String email, String password) async {
    try {
      final response = await client.auth.signInWithPassword(
        email: email,
        password: password,
      );
      if (response.user != null) {
        debugPrint(
            'SupabaseAuthBackend: Signed in with email: ${response.user!.email}');
        return _mapUser(response.user!);
      }
      return null;
    } catch (e) {
      debugPrint('SupabaseAuthBackend: Email sign-in failed: $e');
      rethrow;
    }
  }

  @override
  Future<AuthUser?> createAccountWithEmailPassword(
      String email, String password) async {
    try {
      final currentUser = client.auth.currentUser;

      if (currentUser != null && currentUser.isAnonymous) {
        // Link email to anonymous account
        debugPrint(
            'SupabaseAuthBackend: Linking email to anonymous user: ${currentUser.id}');
        final response = await client.auth.updateUser(
          UserAttributes(
            email: email,
            password: password,
          ),
        );
        if (response.user != null) {
          return _mapUser(response.user!);
        }
        return null;
      } else {
        // Create new account
        final response = await client.auth.signUp(
          email: email,
          password: password,
        );
        if (response.user != null) {
          debugPrint(
              'SupabaseAuthBackend: Created account: ${response.user!.email}');
          return _mapUser(response.user!);
        }
        return null;
      }
    } catch (e) {
      debugPrint('SupabaseAuthBackend: Account creation failed: $e');
      rethrow;
    }
  }

  @override
  Future<AuthUser?> linkWithEmailPassword(String email, String password) async {
    final user = client.auth.currentUser;
    if (user == null) {
      throw Exception('No user signed in');
    }

    try {
      final response = await client.auth.updateUser(
        UserAttributes(
          email: email,
          password: password,
        ),
      );
      if (response.user != null) {
        debugPrint('SupabaseAuthBackend: Linked email: $email');
        return _mapUser(response.user!);
      }
      return null;
    } catch (e) {
      debugPrint('SupabaseAuthBackend: Email link failed: $e');
      rethrow;
    }
  }

  @override
  Future<void> sendPasswordResetEmail(String email) async {
    try {
      debugPrint(
          'SupabaseAuthBackend: Password reset redirect: $_passwordResetRedirectTo');
      await client.auth.resetPasswordForEmail(
        email,
        redirectTo: _passwordResetRedirectTo,
      );
      debugPrint('SupabaseAuthBackend: Sent password reset email: $email');
    } catch (e) {
      debugPrint('SupabaseAuthBackend: Password reset failed: $e');
      rethrow;
    }
  }

  String? get _passwordResetRedirectTo {
    if (kIsWeb) {
      return Uri.base.toString();
    }
    return 'rhythmapp://password-reset';
  }

  @override
  Future<AuthUser?> verifyPasswordRecoveryTokenHash(String tokenHash) async {
    try {
      final response = await client.auth.verifyOTP(
        type: OtpType.recovery,
        tokenHash: tokenHash,
      );
      if (response.user != null) {
        debugPrint(
            'SupabaseAuthBackend: Verified password recovery for ${response.user!.email}');
        return _mapUser(response.user!);
      }
      return currentUser;
    } catch (e) {
      debugPrint('SupabaseAuthBackend: Password recovery verify failed: $e');
      rethrow;
    }
  }

  @override
  Future<AuthUser?> updatePassword(String password) async {
    try {
      final response = await client.auth.updateUser(
        UserAttributes(password: password),
      );
      if (response.user != null) {
        debugPrint(
            'SupabaseAuthBackend: Updated password for ${response.user!.email}');
        return _mapUser(response.user!);
      }
      return currentUser;
    } catch (e) {
      debugPrint('SupabaseAuthBackend: Password update failed: $e');
      rethrow;
    }
  }

  @override
  Future<void> signOut() async {
    try {
      await _googleSignIn.signOut();
    } catch (e) {
      debugPrint('SupabaseAuthBackend: Google sign out error: $e');
    }
    await client.auth.signOut();
    debugPrint('SupabaseAuthBackend: Signed out');
  }

  @override
  Future<void> deleteAccount() async {
    final user = client.auth.currentUser;
    if (user == null) {
      throw Exception('No user signed in');
    }

    try {
      debugPrint('SupabaseAuthBackend: Deleting account for user: ${user.id}');

      // Call the Edge Function to delete user data and auth account
      final response = await client.functions.invoke('delete-user');

      if (response.status != 200) {
        final error = response.data?['error'] ?? 'Unknown error';
        throw Exception('Failed to delete account: $error');
      }

      debugPrint('SupabaseAuthBackend: Account deleted successfully');

      // Sign out locally (the server-side deletion already invalidates the session)
      try {
        await _googleSignIn.signOut();
      } catch (e) {
        debugPrint(
            'SupabaseAuthBackend: Google sign out error during delete: $e');
      }

      // Force local sign out
      await client.auth.signOut();
    } catch (e) {
      debugPrint('SupabaseAuthBackend: Delete account failed: $e');
      rethrow;
    }
  }

  @override
  Future<void> disconnectProviders() async {
    try {
      await _googleSignIn.disconnect();
    } catch (e) {
      debugPrint('SupabaseAuthBackend: Google disconnect error: $e');
    }
  }

  @override
  void dispose() {
    _authStateController?.close();
    _authStateController = null;
    _authEventController?.close();
    _authEventController = null;
  }

  /// Map Supabase User to AuthUser.
  AuthUser _mapUser(User user) {
    return AuthUser(
      id: user.id,
      email: user.email,
      displayName: user.userMetadata?['full_name'] as String? ??
          user.userMetadata?['name'] as String?,
      isAnonymous: user.isAnonymous,
      createdAt: DateTime.tryParse(user.createdAt),
      metadata: {
        'providers': user.appMetadata['providers'] ?? [],
        'lastSignIn': user.lastSignInAt,
        // Include user metadata for feature flags (raw_user_meta_data in Supabase)
        'userMetadata': user.userMetadata ?? {},
      },
    );
  }
}
