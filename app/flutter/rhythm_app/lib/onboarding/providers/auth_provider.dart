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
  StreamSubscription<AuthUser?>? _authSubscription;

  AuthState get state => _state;
  String? get email => _email;
  String? get errorMessage => _errorMessage;
  AuthUser? get user => _user;
  bool get isAuthenticated => _user != null;
  bool get isAnonymous => _user?.isAnonymous ?? false;

  AuthProvider() {
    _authSubscription = _authService.authStateChanges.listen(_onAuthStateChanged);
  }

  @override
  void dispose() {
    _authSubscription?.cancel();
    super.dispose();
  }

  void _onAuthStateChanged(AuthUser? user) {
    _user = user;
    if (user != null) {
      _state = AuthState.authenticated;
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
        if (result.user == null) {
          // User cancelled - return null to indicate cancellation
          return null;
        }
        _email = result.email;
        return result.user;
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
        if (result.user == null) {
          // User cancelled - return null to indicate cancellation
          return null;
        }
        _email = result.email;
        return result.user;
      },
      onCancel: () {
        _state = AuthState.initial;
        notifyListeners();
      },
    );
  }

  /// Sign in with email and password.
  Future<bool> signIn(String email, String password) async {
    _email = email;

    // Demo mode check for App Store review
    debugPrint('Demo check: "${email.toLowerCase()}" == "${DemoCredentials.email}" ? ${email.toLowerCase() == DemoCredentials.email}');
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
      operation: () => _authService.createAccountWithEmailPassword(email, password),
    );
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
  void _handleGenericError(String operationName, Object e, StackTrace stackTrace) {
    debugPrint('Error with $operationName: $e');
    debugPrint('Stack trace: $stackTrace');
    _state = AuthState.error;
    _errorMessage = '$operationName failed. Please try again.';
    notifyListeners();
  }

  /// Save user preferences.
  ///
  /// Note: Location, sleep schedule, and timezone are passed to HomeProvider
  /// via SettingsService for creating the Home model in onUserSignIn().
  Future<bool> savePreferences(OnboardingPreferences preferences) async {
    try {
      final settings = SettingsService.instance;

      // Store notification preference (device-specific)
      await settings.setNotificationsEnabled(preferences.notificationsEnabled);

      // Store onboarding data for HomeProvider to consume
      settings.setOnboardingPreferences(OnboardingPreferencesData(
        latitude: preferences.location?.latitude,
        longitude: preferences.location?.longitude,
        cityName: preferences.locationName,
        timezone: preferences.timezone,
        bedtimeHour: preferences.bedtimeHour,
        bedtimeMinute: preferences.bedtimeMinute,
        wakeTimeHour: preferences.wakeTimeHour,
        wakeTimeMinute: preferences.wakeTimeMinute,
      ));

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
    notifyListeners();
  }

  String _getErrorMessage(String message) {
    // Map common Supabase error messages to user-friendly messages
    final lowerMessage = message.toLowerCase();

    if (lowerMessage.contains('invalid email')) {
      return 'Please enter a valid email address.';
    }
    if (lowerMessage.contains('user disabled') || lowerMessage.contains('banned')) {
      return 'This account has been disabled.';
    }
    if (lowerMessage.contains('user not found') || lowerMessage.contains('no user found')) {
      return 'No account found with this email.';
    }
    if (lowerMessage.contains('invalid password') || lowerMessage.contains('wrong password')) {
      return 'Incorrect password.';
    }
    if (lowerMessage.contains('invalid credentials') || lowerMessage.contains('invalid login')) {
      return 'Invalid email or password.';
    }
    if (lowerMessage.contains('already registered') || lowerMessage.contains('already exists')) {
      return 'An account already exists with this email.';
    }
    if (lowerMessage.contains('weak password') || lowerMessage.contains('password should be')) {
      return 'Password must be at least 6 characters.';
    }
    if (lowerMessage.contains('rate limit') || lowerMessage.contains('too many requests')) {
      return 'Too many attempts. Please try again later.';
    }

    return 'An error occurred. Please try again.';
  }
}
