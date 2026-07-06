import 'dart:async';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:url_launcher/url_launcher.dart';
import '../widgets/onboarding_orbit.dart';
import '../widgets/sun_glow_button.dart';
import '../providers/auth_provider.dart';
import '../providers/onboarding_provider.dart';
import '../../services/analytics_service.dart';

/// Account screen with Google Sign In as primary and email/password as secondary.
class AccountScreen extends StatefulWidget {
  final FutureOr<void> Function()? onComplete;
  final FutureOr<void> Function()? onSignedInComplete;

  /// Shown to users who already have a local (anonymous) setup from before
  /// accounts became mandatory: swaps the copy to explain that sign-in is now
  /// required and that their existing setup is preserved.
  final bool existingUserPrompt;

  const AccountScreen({
    super.key,
    this.onComplete,
    this.onSignedInComplete,
    this.existingUserPrompt = false,
  });

  @override
  State<AccountScreen> createState() => _AccountScreenState();
}

class _AccountScreenState extends State<AccountScreen>
    with SingleTickerProviderStateMixin {
  final _emailController = TextEditingController();
  final _passwordController = TextEditingController();
  final _emailFocusNode = FocusNode();
  final _passwordFocusNode = FocusNode();
  late AnimationController _pulseController;
  late Animation<double> _pulseAnimation;
  String? _emailError;
  String? _passwordError;
  String? _passwordResetMessage;
  String? _passwordResetError;
  bool _showEmailForm = false;
  bool _isSendingPasswordReset = false;
  Timer? _passwordResetCooldownTimer;
  DateTime? _passwordResetCooldownUntil;
  bool _obscurePassword = true;

  @override
  void initState() {
    super.initState();
    _pulseController = AnimationController(
      duration: const Duration(milliseconds: 2000),
      vsync: this,
    )..repeat(reverse: true);

    _pulseAnimation = Tween<double>(begin: 0.4, end: 0.7).animate(
      CurvedAnimation(parent: _pulseController, curve: Curves.easeInOut),
    );
  }

  @override
  void dispose() {
    _emailController.dispose();
    _passwordController.dispose();
    _emailFocusNode.dispose();
    _passwordFocusNode.dispose();
    _pulseController.dispose();
    _passwordResetCooldownTimer?.cancel();
    super.dispose();
  }

  bool _isValidEmail(String email) {
    return RegExp(r'^[\w-\.]+@([\w-]+\.)+[\w-]{2,}$').hasMatch(email);
  }

  Future<void> _signInWithGoogle() async {
    final authProvider = context.read<AuthProvider>();
    final onboardingProvider = context.read<OnboardingProvider>();

    final success = await authProvider.signInWithGoogle();

    if (success && mounted) {
      // Identify user and track account choice
      final userId = authProvider.user?.id;
      if (userId != null) {
        AnalyticsService().identifyUser(userId);
      }
      AnalyticsService().logOnboardingAccountChoice('google');
      AnalyticsService().logSignIn('google');

      // Save preferences locally
      await authProvider.savePreferences(onboardingProvider.preferences);
      if (!mounted) return;
      await _completeSignedIn();
    }
  }

  Future<void> _signInWithApple() async {
    final authProvider = context.read<AuthProvider>();
    final onboardingProvider = context.read<OnboardingProvider>();

    final success = await authProvider.signInWithApple();

    if (success && mounted) {
      // Identify user and track account choice
      final userId = authProvider.user?.id;
      if (userId != null) {
        AnalyticsService().identifyUser(userId);
      }
      AnalyticsService().logOnboardingAccountChoice('apple');
      AnalyticsService().logSignIn('apple');

      // Save preferences locally
      await authProvider.savePreferences(onboardingProvider.preferences);
      if (!mounted) return;
      await _completeSignedIn();
    }
  }

  Future<void> _submitEmailForm() async {
    final email = _emailController.text.trim();
    final password = _passwordController.text;

    // Validate email
    if (email.isEmpty) {
      setState(() => _emailError = 'Please enter your email');
      return;
    }
    if (!_isValidEmail(email)) {
      setState(() => _emailError = 'Please enter a valid email address');
      return;
    }

    // Validate password
    if (password.isEmpty) {
      setState(() => _passwordError = 'Please enter a password');
      return;
    }
    if (password.length < 6) {
      setState(() => _passwordError = 'Password must be at least 6 characters');
      return;
    }

    setState(() {
      _emailError = null;
      _passwordError = null;
      _passwordResetMessage = null;
      _passwordResetError = null;
    });
    _emailFocusNode.unfocus();
    _passwordFocusNode.unfocus();

    final authProvider = context.read<AuthProvider>();
    final onboardingProvider = context.read<OnboardingProvider>();

    final success = await authProvider.continueWithEmailPassword(
      email,
      password,
    );

    if (success && mounted) {
      final emailResult = authProvider.lastEmailAuthResult;
      final method = emailResult == EmailAuthResult.created
          ? 'email_create'
          : 'email_signin';

      // Identify user and track account choice
      final userId = authProvider.user?.id;
      if (userId != null) {
        AnalyticsService().identifyUser(userId);
      }
      AnalyticsService().logOnboardingAccountChoice(method);
      AnalyticsService().logSignIn(method);

      // Save preferences locally for this device before app-state refresh
      // restores or creates the Home.
      await authProvider.savePreferences(onboardingProvider.preferences);
      if (!mounted) return;
      await _completeSignedIn();
    }
  }

  void _resetAndTryAgain() {
    context.read<AuthProvider>().resetState();
    _emailController.clear();
    _passwordController.clear();
    setState(() {
      _showEmailForm = false;
    });
  }

  Future<void> _sendPasswordResetEmail() async {
    final email = _emailController.text.trim();
    if (_isPasswordResetCoolingDown) {
      setState(() {
        _passwordResetMessage =
            'Use the newest reset email. You can request another link shortly.';
        _passwordResetError = null;
      });
      return;
    }

    if (email.isEmpty) {
      setState(() {
        _emailError = 'Enter your email to reset your password';
        _passwordResetMessage = null;
        _passwordResetError = null;
      });
      _emailFocusNode.requestFocus();
      return;
    }
    if (!_isValidEmail(email)) {
      setState(() {
        _emailError = 'Please enter a valid email address';
        _passwordResetMessage = null;
        _passwordResetError = null;
      });
      _emailFocusNode.requestFocus();
      return;
    }

    setState(() {
      _emailError = null;
      _passwordResetMessage = null;
      _passwordResetError = null;
      _isSendingPasswordReset = true;
    });

    final success =
        await context.read<AuthProvider>().sendPasswordResetEmail(email);
    if (!mounted) return;

    final authProvider = context.read<AuthProvider>();
    setState(() {
      _isSendingPasswordReset = false;
      if (success) {
        _startPasswordResetCooldown();
        _passwordResetMessage = 'Check your newest email for a reset link.';
      } else {
        _passwordResetError =
            authProvider.errorMessage ?? 'Could not send reset email.';
      }
    });
  }

  bool get _isPasswordResetCoolingDown {
    final cooldownUntil = _passwordResetCooldownUntil;
    return cooldownUntil != null && DateTime.now().isBefore(cooldownUntil);
  }

  void _startPasswordResetCooldown() {
    _passwordResetCooldownTimer?.cancel();
    _passwordResetCooldownUntil = DateTime.now().add(
      const Duration(seconds: 30),
    );
    _passwordResetCooldownTimer = Timer(const Duration(seconds: 30), () {
      if (!mounted) return;
      setState(() {
        _passwordResetCooldownUntil = null;
      });
    });
  }

  void _showEmailSignIn() {
    setState(() {
      _showEmailForm = true;
    });
  }

  void _backToSocialButtons() {
    setState(() {
      _showEmailForm = false;
      _emailError = null;
      _passwordError = null;
      _passwordResetMessage = null;
      _passwordResetError = null;
      _isSendingPasswordReset = false;
      _passwordResetCooldownUntil = null;
    });
    _passwordResetCooldownTimer?.cancel();
    context.read<AuthProvider>().resetState();
  }

  Future<void> _completeSignedIn() async {
    final callback = widget.onSignedInComplete ?? widget.onComplete;
    await Future<void>.sync(() => callback?.call());
  }

  @override
  Widget build(BuildContext context) {
    return Container(
      color: OnboardingColors.backgroundDark,
      child: SafeArea(
        child: Stack(
          children: [
            // Main content
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 32),
              child: Consumer<AuthProvider>(
                builder: (context, authProvider, child) {
                  // Show error state
                  if (authProvider.state == AuthState.error) {
                    return _buildErrorState(authProvider);
                  }

                  // Show email form or social buttons
                  if (_showEmailForm) {
                    return _buildEmailForm(authProvider);
                  } else {
                    return _buildSocialButtons(authProvider);
                  }
                },
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildSocialButtons(AuthProvider authProvider) {
    final isLoading = authProvider.state == AuthState.authenticating;

    return SingleChildScrollView(
      child: Column(
        children: [
          const SizedBox(height: 32),
          // Floating icon with warm glow
          AnimatedBuilder(
            animation: _pulseAnimation,
            builder: (context, child) {
              return Container(
                width: 120,
                height: 120,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  gradient: RadialGradient(
                    colors: [
                      OnboardingColors.sunWarm
                          .withValues(alpha: _pulseAnimation.value),
                      OnboardingColors.sunWarm
                          .withValues(alpha: _pulseAnimation.value * 0.3),
                      Colors.transparent,
                    ],
                  ),
                ),
                child: Center(
                  child: Container(
                    width: 70,
                    height: 70,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      color: OnboardingColors.backgroundCard,
                      border: Border.all(
                        color: OnboardingColors.sunWarm.withValues(alpha: 0.5),
                        width: 2,
                      ),
                    ),
                    child: Icon(
                      widget.existingUserPrompt
                          ? Icons.lock_person_rounded
                          : Icons.person_add_rounded,
                      color: OnboardingColors.sunWarm,
                      size: 32,
                    ),
                  ),
                ),
              );
            },
          ),
          const SizedBox(height: 24),
          // Header
          Text(
            widget.existingUserPrompt
                ? 'Sign In Required'
                : 'Create Your Account',
            style: const TextStyle(
              color: OnboardingColors.textPrimary,
              fontSize: 28,
              fontWeight: FontWeight.bold,
              letterSpacing: 0.5,
            ),
          ),
          const SizedBox(height: 8),
          Text(
            widget.existingUserPrompt
                ? 'Rhythm now requires an account. Your rooms and '
                    'settings stay on this device and will be linked '
                    'to your account.'
                : 'Sync your settings across all devices',
            textAlign: TextAlign.center,
            style: const TextStyle(
              color: OnboardingColors.textSecondary,
              fontSize: 16,
              height: 1.5,
            ),
          ),
          const SizedBox(height: 40),

          // Google Sign In Button (Primary)
          _GoogleSignInButton(
            onPressed: isLoading ? null : _signInWithGoogle,
            isLoading: isLoading,
          ),

          const SizedBox(height: 12),

          // Apple Sign In Button
          _AppleSignInButton(
            onPressed: isLoading ? null : _signInWithApple,
            isLoading: isLoading,
          ),

          const SizedBox(height: 12),

          // Email/Password option
          _SecondaryButton(
            icon: Icons.email_outlined,
            text: 'Continue with Email',
            onPressed: isLoading ? null : _showEmailSignIn,
          ),

          const SizedBox(height: 32),

          // Terms and Privacy links
          Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              GestureDetector(
                onTap: () =>
                    launchUrl(Uri.parse('https://rhythm.lighting/terms')),
                child: Text(
                  'Terms',
                  style: TextStyle(
                    color:
                        OnboardingColors.textSecondary.withValues(alpha: 0.6),
                    fontSize: 13,
                    decoration: TextDecoration.underline,
                    decorationColor:
                        OnboardingColors.textSecondary.withValues(alpha: 0.4),
                  ),
                ),
              ),
              Padding(
                padding: const EdgeInsets.symmetric(horizontal: 12),
                child: Text(
                  '·',
                  style: TextStyle(
                    color:
                        OnboardingColors.textSecondary.withValues(alpha: 0.4),
                    fontSize: 13,
                  ),
                ),
              ),
              GestureDetector(
                onTap: () =>
                    launchUrl(Uri.parse('https://rhythm.lighting/privacy')),
                child: Text(
                  'Privacy',
                  style: TextStyle(
                    color:
                        OnboardingColors.textSecondary.withValues(alpha: 0.6),
                    fontSize: 13,
                    decoration: TextDecoration.underline,
                    decorationColor:
                        OnboardingColors.textSecondary.withValues(alpha: 0.4),
                  ),
                ),
              ),
            ],
          ),

          const SizedBox(height: 60),
        ],
      ),
    );
  }

  Widget _buildEmailForm(AuthProvider authProvider) {
    final isLoading = authProvider.state == AuthState.authenticating;

    return SingleChildScrollView(
      child: Column(
        children: [
          const SizedBox(height: 16),
          // Back button
          Align(
            alignment: Alignment.centerLeft,
            child: GestureDetector(
              onTap: _backToSocialButtons,
              child: Container(
                padding: const EdgeInsets.all(8),
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: OnboardingColors.orbitRing.withValues(alpha: 0.2),
                ),
                child: const Icon(
                  Icons.arrow_back,
                  color: OnboardingColors.textSecondary,
                  size: 20,
                ),
              ),
            ),
          ),
          const SizedBox(height: 16),
          // Header
          const Text(
            'Continue with Email',
            style: TextStyle(
              color: OnboardingColors.textPrimary,
              fontSize: 28,
              fontWeight: FontWeight.bold,
              letterSpacing: 0.5,
            ),
          ),
          const SizedBox(height: 8),
          const Text(
            'We\'ll sign you in or create an account if you\'re new',
            textAlign: TextAlign.center,
            style: TextStyle(
              color: OnboardingColors.textSecondary,
              fontSize: 16,
              height: 1.5,
            ),
          ),
          const SizedBox(height: 32),

          // Email input
          Container(
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(16),
              border: Border.all(
                color: _emailError != null
                    ? Colors.red.withValues(alpha: 0.5)
                    : OnboardingColors.orbitRing,
                width: 2,
              ),
            ),
            child: TextField(
              controller: _emailController,
              focusNode: _emailFocusNode,
              keyboardType: TextInputType.emailAddress,
              autocorrect: false,
              enabled: !isLoading,
              style: const TextStyle(
                color: OnboardingColors.textPrimary,
                fontSize: 16,
              ),
              decoration: InputDecoration(
                hintText: 'Email',
                hintStyle: TextStyle(
                  color: OnboardingColors.textSecondary.withValues(alpha: 0.5),
                ),
                prefixIcon: Icon(
                  Icons.email_outlined,
                  color: _emailError != null
                      ? Colors.red.shade300
                      : OnboardingColors.textSecondary,
                ),
                border: InputBorder.none,
                contentPadding: const EdgeInsets.all(16),
              ),
              onChanged: (_) {
                if (_emailError != null ||
                    _passwordResetMessage != null ||
                    _passwordResetError != null) {
                  _passwordResetCooldownTimer?.cancel();
                  setState(() {
                    _emailError = null;
                    _passwordResetMessage = null;
                    _passwordResetError = null;
                    _passwordResetCooldownUntil = null;
                  });
                }
              },
              onSubmitted: (_) => _passwordFocusNode.requestFocus(),
            ),
          ),
          if (_emailError != null)
            Padding(
              padding: const EdgeInsets.only(top: 8, left: 4),
              child: Align(
                alignment: Alignment.centerLeft,
                child: Text(
                  _emailError!,
                  style: TextStyle(
                    color: Colors.red.shade300,
                    fontSize: 13,
                  ),
                ),
              ),
            ),

          const SizedBox(height: 16),

          // Password input
          Container(
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(16),
              border: Border.all(
                color: _passwordError != null
                    ? Colors.red.withValues(alpha: 0.5)
                    : OnboardingColors.orbitRing,
                width: 2,
              ),
            ),
            child: TextField(
              controller: _passwordController,
              focusNode: _passwordFocusNode,
              obscureText: _obscurePassword,
              enabled: !isLoading,
              style: const TextStyle(
                color: OnboardingColors.textPrimary,
                fontSize: 16,
              ),
              decoration: InputDecoration(
                hintText: 'Password',
                hintStyle: TextStyle(
                  color: OnboardingColors.textSecondary.withValues(alpha: 0.5),
                ),
                prefixIcon: Icon(
                  Icons.lock_outline,
                  color: _passwordError != null
                      ? Colors.red.shade300
                      : OnboardingColors.textSecondary,
                ),
                suffixIcon: IconButton(
                  icon: Icon(
                    _obscurePassword ? Icons.visibility_off : Icons.visibility,
                    color: OnboardingColors.textSecondary,
                  ),
                  onPressed: () {
                    setState(() => _obscurePassword = !_obscurePassword);
                  },
                ),
                border: InputBorder.none,
                contentPadding: const EdgeInsets.all(16),
              ),
              onChanged: (_) {
                if (_passwordError != null) {
                  setState(() => _passwordError = null);
                }
              },
              onSubmitted: (_) => _submitEmailForm(),
            ),
          ),
          if (_passwordError != null)
            Padding(
              padding: const EdgeInsets.only(top: 8, left: 4),
              child: Align(
                alignment: Alignment.centerLeft,
                child: Text(
                  _passwordError!,
                  style: TextStyle(
                    color: Colors.red.shade300,
                    fontSize: 13,
                  ),
                ),
              ),
            ),

          const SizedBox(height: 12),

          Align(
            alignment: Alignment.centerRight,
            child: _isSendingPasswordReset
                ? const SizedBox(
                    width: 18,
                    height: 18,
                    child: CircularProgressIndicator(
                      strokeWidth: 2,
                      valueColor: AlwaysStoppedAnimation<Color>(
                        OnboardingColors.sunWarm,
                      ),
                    ),
                  )
                : GestureDetector(
                    onTap: isLoading || _isPasswordResetCoolingDown
                        ? null
                        : _sendPasswordResetEmail,
                    child: Text(
                      _isPasswordResetCoolingDown
                          ? 'Reset email sent'
                          : 'Forgot password?',
                      style: TextStyle(
                        color: OnboardingColors.sunWarm.withValues(
                          alpha: _isPasswordResetCoolingDown ? 0.5 : 0.9,
                        ),
                        fontSize: 14,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ),
          ),

          if (_passwordResetMessage != null || _passwordResetError != null)
            Padding(
              padding: const EdgeInsets.only(top: 12),
              child: Align(
                alignment: Alignment.centerLeft,
                child: Text(
                  _passwordResetMessage ?? _passwordResetError!,
                  style: TextStyle(
                    color: _passwordResetMessage != null
                        ? OnboardingColors.textSecondary
                        : Colors.red.shade300,
                    fontSize: 13,
                  ),
                ),
              ),
            ),

          const SizedBox(height: 32),

          // Submit button
          SunGlowButton(
            text: 'Continue',
            isLoading: isLoading,
            onPressed: _submitEmailForm,
          ),

          const SizedBox(height: 80),
        ],
      ),
    );
  }

  Widget _buildErrorState(AuthProvider authProvider) {
    return Column(
      children: [
        const SizedBox(height: 48),
        // Header
        const Text(
          'Something Went Wrong',
          style: TextStyle(
            color: OnboardingColors.textPrimary,
            fontSize: 28,
            fontWeight: FontWeight.bold,
            letterSpacing: 0.5,
          ),
        ),
        const Spacer(),
        // Error icon
        Container(
          width: 100,
          height: 100,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: Colors.red.withValues(alpha: 0.1),
            border: Border.all(
              color: Colors.red.withValues(alpha: 0.3),
              width: 2,
            ),
          ),
          child: Icon(
            Icons.error_outline,
            color: Colors.red.shade300,
            size: 48,
          ),
        ),
        const SizedBox(height: 24),
        // Error message
        Text(
          authProvider.errorMessage ?? 'An error occurred. Please try again.',
          textAlign: TextAlign.center,
          style: TextStyle(
            color: Colors.red.shade300,
            fontSize: 16,
          ),
        ),
        const Spacer(),
        // Try again button
        SunGlowButton(
          text: 'Try Again',
          onPressed: _resetAndTryAgain,
        ),
        const SizedBox(height: 48),
      ],
    );
  }
}

/// Google Sign In button with Google branding.
class _GoogleSignInButton extends StatelessWidget {
  final VoidCallback? onPressed;
  final bool isLoading;

  const _GoogleSignInButton({
    required this.onPressed,
    this.isLoading = false,
  });

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onPressed,
      child: Container(
        height: 56,
        decoration: BoxDecoration(
          color: Colors.white,
          borderRadius: BorderRadius.circular(16),
          boxShadow: [
            BoxShadow(
              color: Colors.black.withValues(alpha: 0.1),
              blurRadius: 8,
              offset: const Offset(0, 2),
            ),
          ],
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            if (isLoading)
              const SizedBox(
                width: 24,
                height: 24,
                child: CircularProgressIndicator(
                  strokeWidth: 2,
                  valueColor: AlwaysStoppedAnimation<Color>(Colors.black54),
                ),
              )
            else ...[
              // Google "G" logo
              Container(
                width: 24,
                height: 24,
                decoration: BoxDecoration(
                  color: Colors.white,
                  borderRadius: BorderRadius.circular(4),
                ),
                child: Center(
                  child: Text(
                    'G',
                    style: TextStyle(
                      fontSize: 18,
                      fontWeight: FontWeight.bold,
                      foreground: Paint()
                        ..shader = const LinearGradient(
                          colors: [
                            Color(0xFF4285F4), // Blue
                            Color(0xFF34A853), // Green
                            Color(0xFFFBBC05), // Yellow
                            Color(0xFFEA4335), // Red
                          ],
                          stops: [0.0, 0.33, 0.66, 1.0],
                        ).createShader(const Rect.fromLTWH(0, 0, 24, 24)),
                    ),
                  ),
                ),
              ),
              const SizedBox(width: 12),
              const Text(
                'Continue with Google',
                style: TextStyle(
                  color: Colors.black87,
                  fontSize: 16,
                  fontWeight: FontWeight.w500,
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// Apple Sign In button with Apple branding.
class _AppleSignInButton extends StatelessWidget {
  final VoidCallback? onPressed;
  final bool isLoading;

  const _AppleSignInButton({
    required this.onPressed,
    this.isLoading = false,
  });

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onPressed,
      child: Container(
        height: 56,
        decoration: BoxDecoration(
          color: Colors.black,
          borderRadius: BorderRadius.circular(16),
          boxShadow: [
            BoxShadow(
              color: Colors.black.withValues(alpha: 0.2),
              blurRadius: 8,
              offset: const Offset(0, 2),
            ),
          ],
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            if (isLoading)
              const SizedBox(
                width: 24,
                height: 24,
                child: CircularProgressIndicator(
                  strokeWidth: 2,
                  valueColor: AlwaysStoppedAnimation<Color>(Colors.white70),
                ),
              )
            else ...[
              // Apple logo
              const Icon(
                Icons.apple,
                color: Colors.white,
                size: 24,
              ),
              const SizedBox(width: 12),
              const Text(
                'Continue with Apple',
                style: TextStyle(
                  color: Colors.white,
                  fontSize: 16,
                  fontWeight: FontWeight.w500,
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// Secondary button for less prominent options.
class _SecondaryButton extends StatelessWidget {
  final IconData icon;
  final String text;
  final VoidCallback? onPressed;

  const _SecondaryButton({
    required this.icon,
    required this.text,
    required this.onPressed,
  });

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onPressed,
      child: Container(
        height: 56,
        decoration: BoxDecoration(
          color: OnboardingColors.backgroundCard,
          borderRadius: BorderRadius.circular(16),
          border: Border.all(
            color: OnboardingColors.orbitRing,
            width: 2,
          ),
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(
              icon,
              color: OnboardingColors.textSecondary,
              size: 22,
            ),
            const SizedBox(width: 12),
            Text(
              text,
              style: const TextStyle(
                color: OnboardingColors.textPrimary,
                fontSize: 16,
                fontWeight: FontWeight.w500,
              ),
            ),
          ],
        ),
      ),
    );
  }
}
