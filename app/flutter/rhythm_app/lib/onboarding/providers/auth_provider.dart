import 'dart:async';
import 'package:flutter/foundation.dart';
import 'package:supabase_flutter/supabase_flutter.dart' show AuthException;
import '../../backend/backend.dart';
import '../../services/auth_service.dart';
import '../../services/hue/hue_service_locator.dart';
import '../../services/settings_service.dart';
import 'onboarding_provider.dart';

/// Authentication states for the onboarding flow.
enum AuthState {
  initial,
  authenticating,
  authenticated,
  error,
}

enum EmailAuthResult {
  signedIn,
  created,
}

/// Manages authentication state.
///
/// Delegates all authentication operations to [AuthService] while managing
/// UI state (loading, errors, etc.) for the onboarding flow.
class AuthProvider extends ChangeNotifier {
  final AuthService _authService = AuthService();

  AuthState _state = AuthState.initial;
  String? _email;
  String? _errorMessage;
  AuthUser? _user;
  EmailAuthResult? _lastEmailAuthResult;
  StreamSubscription<AuthUser?>? _authSubscription;

  AuthState get state => _state;
  String? get email => _email;
  String? get errorMessage => _errorMessage;
  AuthUser? get user => _user;
  EmailAuthResult? get lastEmailAuthResult => _lastEmailAuthResult;
  bool get isAuthenticated => _user != null && !_user!.isAnonymous;
  bool get isAnonymous => _user?.isAnonymous ?? false;

  AuthProvider() {
    final currentUser = _authService.currentUser;
    if (currentUser != null && !currentUser.isAnonymous) {
      _user = currentUser;
      _email = currentUser.email;
      _state = AuthState.authenticated;
    }
    _authSubscription =
        _authService.authStateChanges.listen(_onAuthStateChanged);
  }

  @override
  void dispose() {
    _authSubscription?.cancel();
    super.dispose();
  }

  void _onAuthStateChanged(AuthUser? user) {
    _user = user;
    if (user != null && !user.isAnonymous) {
      _state = AuthState.authenticated;
    } else if (_state == AuthState.authenticated) {
      _state = AuthState.initial;
    }
    notifyListeners();
  }

  /// Sign in with Google.
  /// Links to anonymous account if one exists.
  Future<bool> signInWithGoogle() async {
    return _runAuthOperation(
      operationName: 'Google Sign In',
      operation: () async {
        final result = await _authService.signInWithGoogle();
        final user = _completedSocialUser(result.user, 'Google Sign-In');
        if (user == null) {
          // User cancelled - return null to indicate cancellation
          return null;
        }
        _email = result.email ?? user.email;
        return user;
      },
      onCancel: () {
        _state = AuthState.initial;
        notifyListeners();
      },
    );
  }

  /// Sign in with Apple.
  /// Links to anonymous account if one exists.
  Future<bool> signInWithApple() async {
    return _runAuthOperation(
      operationName: 'Apple Sign In',
      operation: () async {
        final result = await _authService.signInWithApple();
        final user = _completedSocialUser(result.user, 'Apple Sign-In');
        if (user == null) {
          // User cancelled - return null to indicate cancellation
          return null;
        }
        _email = result.email ?? user.email;
        return user;
      },
      onCancel: () {
        _state = AuthState.initial;
        notifyListeners();
      },
    );
  }

  AuthUser? _completedSocialUser(AuthUser? resultUser, String providerName) {
    final user = resultUser ?? _authService.currentUser;
    if (user == null) return null;
    if (user.isAnonymous) {
      throw AuthException(
        '$providerName did not finish creating an account.',
        code: 'social_sign_in_anonymous_user',
      );
    }
    return user;
  }

  /// Continue with email/password, creating a new account when the email does
  /// not exist and signing in when it does.
  Future<bool> continueWithEmailPassword(String email, String password) async {
    _email = email;
    _lastEmailAuthResult = null;

    // Demo mode check for App Store review
    debugPrint(
        'Demo check: "${email.toLowerCase()}" == "${DemoCredentials.email}" ? ${email.toLowerCase() == DemoCredentials.email}');
    if (email.toLowerCase() == DemoCredentials.email.toLowerCase()) {
      if (password == DemoCredentials.password) {
        HueServiceLocator.setDemoMode(true);
        _user = const AuthUser(
          id: 'demo-user',
          email: DemoCredentials.email,
          displayName: 'Demo User',
          isAnonymous: false,
        );
        _lastEmailAuthResult = EmailAuthResult.signedIn;
        _state = AuthState.authenticated;
        notifyListeners();
        return true;
      }
      _state = AuthState.error;
      _errorMessage = 'Invalid demo credentials';
      notifyListeners();
      return false;
    }

    return _runAuthOperation(
      operationName: 'Email account',
      operation: () => _createOrSignInWithEmailPassword(email, password),
    );
  }

  /// Sign in with email and password.
  Future<bool> signIn(String email, String password) async {
    _email = email;

    // Demo mode check for App Store review
    debugPrint(
        'Demo check: "${email.toLowerCase()}" == "${DemoCredentials.email}" ? ${email.toLowerCase() == DemoCredentials.email}');
    if (email.toLowerCase() == DemoCredentials.email.toLowerCase()) {
      if (password == DemoCredentials.password) {
        HueServiceLocator.setDemoMode(true);
        _user = const AuthUser(
          id: 'demo-user',
          email: DemoCredentials.email,
          displayName: 'Demo User',
          isAnonymous: false,
        );
        _state = AuthState.authenticated;
        notifyListeners();
        return true;
      }
      _state = AuthState.error;
      _errorMessage = 'Invalid demo credentials';
      notifyListeners();
      return false;
    }

    return _runAuthOperation(
      operationName: 'Sign in',
      operation: () => _authService.signInWithEmailPassword(email, password),
    );
  }

  /// Create account with email and password.
  /// If user is currently anonymous, links credentials to preserve uid.
  Future<bool> createAccount(String email, String password) async {
    _email = email;
    return _runAuthOperation(
      operationName: 'Account creation',
      operation: () =>
          _authService.createAccountWithEmailPassword(email, password),
    );
  }

  Future<bool> sendPasswordResetEmail(String email) async {
    try {
      await _authService.sendPasswordResetEmail(email);
      _errorMessage = null;
      return true;
    } on AuthException catch (e) {
      _errorMessage = _getErrorMessage(e.message);
      notifyListeners();
      return false;
    } catch (e, stackTrace) {
      debugPrint('Error sending password reset: $e');
      debugPrint('Stack trace: $stackTrace');
      _errorMessage = 'Could not send reset email. Please try again.';
      notifyListeners();
      return false;
    }
  }

  Future<bool> updatePassword(String password) async {
    try {
      _state = AuthState.authenticating;
      _errorMessage = null;
      notifyListeners();

      final user = await _authService.updatePassword(password);
      _user = user ?? _authService.currentUser;
      _state = AuthState.authenticated;
      notifyListeners();
      return true;
    } on AuthException catch (e) {
      _state = AuthState.error;
      _errorMessage = _getErrorMessage(e.message);
      notifyListeners();
      return false;
    } catch (e, stackTrace) {
      debugPrint('Error updating password: $e');
      debugPrint('Stack trace: $stackTrace');
      _state = AuthState.error;
      _errorMessage = 'Could not update password. Please try again.';
      notifyListeners();
      return false;
    }
  }

  Future<AuthUser?> _createOrSignInWithEmailPassword(
    String email,
    String password,
  ) async {
    try {
      final user =
          await _authService.createAccountWithEmailPassword(email, password);
      _lastEmailAuthResult = EmailAuthResult.created;
      return user;
    } on AuthException catch (error) {
      if (!_isExistingAccountError(error.message)) rethrow;

      final user = await _authService.signInWithEmailPassword(email, password);
      _lastEmailAuthResult = EmailAuthResult.signedIn;
      return user;
    }
  }

  bool _isExistingAccountError(String message) {
    final lowerMessage = message.toLowerCase();
    return lowerMessage.contains('already registered') ||
        lowerMessage.contains('already exists') ||
        lowerMessage.contains('email exists') ||
        lowerMessage.contains('email address has already');
  }

  /// Helper method to run authentication operations with consistent error handling.
  ///
  /// [operationName] is used for error messages (e.g., "Sign in", "Google Sign In").
  /// [operation] is the async function that performs the auth operation.
  /// [onCancel] is called when the operation returns null (user cancelled).
  Future<bool> _runAuthOperation({
    required String operationName,
    required Future<AuthUser?> Function() operation,
    VoidCallback? onCancel,
  }) async {
    try {
      _state = AuthState.authenticating;
      _errorMessage = null;
      notifyListeners();

      debugPrint('Starting $operationName...');

      _user = await operation();

      if (_user == null) {
        // Operation was cancelled (e.g., user closed Google sign-in)
        onCancel?.call();
        return false;
      }

      _state = AuthState.authenticated;
      notifyListeners();
      return true;
    } on AuthException catch (e) {
      _handleAuthError(e);
      return false;
    } catch (e, stackTrace) {
      _handleGenericError(operationName, e, stackTrace);
      return false;
    }
  }

  /// Handle AuthException with user-friendly error messages.
  void _handleAuthError(AuthException e) {
    debugPrint('AuthException: code=${e.statusCode}, message=${e.message}');
    _state = AuthState.error;
    _errorMessage = _getErrorMessage(e.message);
    notifyListeners();
  }

  /// Handle generic errors.
  void _handleGenericError(
      String operationName, Object e, StackTrace stackTrace) {
    debugPrint('Error with $operationName: $e');
    debugPrint('Stack trace: $stackTrace');
    _state = AuthState.error;
    _errorMessage = '$operationName failed. Please try again.';
    notifyListeners();
  }

  /// Save user preferences.
  ///
  /// Only the device-specific notification preference and the onboarding-
  /// complete flag are persisted here. Location is no longer collected during
  /// onboarding — it's captured when a Home is created (see AddHomeFlow).
  Future<bool> savePreferences(OnboardingPreferences preferences) async {
    try {
      final settings = SettingsService.instance;

      // Store notification preference (device-specific)
      await settings.setNotificationsEnabled(preferences.notificationsEnabled);

      // Mark onboarding as complete
      await settings.setOnboardingComplete(true);

      return true;
    } catch (e) {
      debugPrint('Failed to save preferences: $e');
      return false;
    }
  }

  /// Check if onboarding is complete for current user.
  Future<bool> isOnboardingComplete() async {
    return SettingsService.instance.onboardingComplete;
  }

  /// Sign out.
  Future<void> signOut() async {
    // Note: Demo mode is cleared in AuthService.signOut()
    // The per-user data-encryption key intentionally stays in the keychain so
    // account data can be decrypted again after re-login.
    await _authService.signOut();
    _state = AuthState.initial;
    _email = null;
    _errorMessage = null;
    notifyListeners();
  }

  /// Reset state for retry.
  void resetState() {
    _state = AuthState.initial;
    _errorMessage = null;
    _lastEmailAuthResult = null;
    notifyListeners();
  }

  String _getErrorMessage(String message) {
    // Map common Supabase error messages to user-friendly messages
    final lowerMessage = message.toLowerCase();

    if (lowerMessage.contains('invalid email')) {
      return 'Please enter a valid email address.';
    }
    if (lowerMessage.contains('user disabled') ||
        lowerMessage.contains('banned')) {
      return 'This account has been disabled.';
    }
    if (lowerMessage.contains('user not found') ||
        lowerMessage.contains('no user found')) {
      return 'No account found with this email.';
    }
    if (lowerMessage.contains('invalid password') ||
        lowerMessage.contains('wrong password')) {
      return 'Incorrect password.';
    }
    if (lowerMessage.contains('invalid credentials') ||
        lowerMessage.contains('invalid login')) {
      return 'Invalid email or password.';
    }
    if (lowerMessage.contains('already registered') ||
        lowerMessage.contains('already exists')) {
      return 'An account already exists with this email.';
    }
    if (lowerMessage.contains('weak password') ||
        lowerMessage.contains('password should be')) {
      return 'Password must be at least 6 characters.';
    }
    if (lowerMessage.contains('only request this after') ||
        lowerMessage.contains('over_email_send_rate_limit') ||
        lowerMessage.contains('email send rate limit')) {
      return 'A reset email was just sent. Use the newest email or try again shortly.';
    }
    if (lowerMessage.contains('otp_expired') ||
        lowerMessage.contains('email link is invalid') ||
        lowerMessage.contains('link is invalid') ||
        lowerMessage.contains('expired')) {
      return 'This reset link is invalid or expired. Request a new reset email.';
    }
    if (lowerMessage.contains('rate limit') ||
        lowerMessage.contains('too many requests')) {
      return 'Too many attempts. Please try again later.';
    }
    if (lowerMessage.contains('google sign-in is not configured') ||
        lowerMessage.contains('google sign-in did not return an id token') ||
        lowerMessage.contains('google_sign_in_configuration_error') ||
        lowerMessage.contains('google_sign_in_missing_id_token') ||
        lowerMessage.contains('serverclientid')) {
      return 'Google Sign-In is not configured for this app build. Please update the app or contact support.';
    }
    if (lowerMessage.contains('did not finish creating an account') ||
        lowerMessage.contains('social_sign_in_anonymous_user')) {
      return 'Sign-in did not finish. Please try again.';
    }

    return 'An error occurred. Please try again.';
  }
}
