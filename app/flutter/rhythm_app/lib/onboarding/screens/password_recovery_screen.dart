import 'dart:async';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../providers/auth_provider.dart';
import '../widgets/onboarding_orbit.dart';
import '../widgets/sun_glow_button.dart';
import '../../services/analytics_service.dart';

class PasswordRecoveryScreen extends StatelessWidget {
  final FutureOr<void> Function()? onComplete;
  final Future<bool> Function(String password)? updatePassword;

  const PasswordRecoveryScreen({
    super.key,
    this.onComplete,
    this.updatePassword,
  });

  @override
  Widget build(BuildContext context) {
    return ChangeNotifierProvider(
      create: (_) => AuthProvider(),
      child: _PasswordRecoveryContent(
        onComplete: onComplete,
        updatePassword: updatePassword,
      ),
    );
  }
}

class _PasswordRecoveryContent extends StatefulWidget {
  final FutureOr<void> Function()? onComplete;
  final Future<bool> Function(String password)? updatePassword;

  const _PasswordRecoveryContent({
    this.onComplete,
    this.updatePassword,
  });

  @override
  State<_PasswordRecoveryContent> createState() =>
      _PasswordRecoveryContentState();
}

class _PasswordRecoveryContentState extends State<_PasswordRecoveryContent> {
  final _passwordController = TextEditingController();
  final _confirmPasswordController = TextEditingController();
  final _passwordFocusNode = FocusNode();
  final _confirmPasswordFocusNode = FocusNode();

  bool _isSubmitting = false;
  bool _obscurePassword = true;
  bool _obscureConfirmation = true;
  String? _passwordError;
  String? _confirmPasswordError;
  String? _errorMessage;

  @override
  void dispose() {
    _passwordController.dispose();
    _confirmPasswordController.dispose();
    _passwordFocusNode.dispose();
    _confirmPasswordFocusNode.dispose();
    super.dispose();
  }

  Future<void> _submit() async {
    final password = _passwordController.text;
    final confirmation = _confirmPasswordController.text;

    setState(() {
      _passwordError = null;
      _confirmPasswordError = null;
      _errorMessage = null;
    });

    if (password.isEmpty) {
      setState(() => _passwordError = 'Enter a new password');
      _passwordFocusNode.requestFocus();
      return;
    }
    if (password.length < 6) {
      setState(() => _passwordError = 'Password must be at least 6 characters');
      _passwordFocusNode.requestFocus();
      return;
    }
    if (confirmation.isEmpty) {
      setState(() => _confirmPasswordError = 'Confirm your new password');
      _confirmPasswordFocusNode.requestFocus();
      return;
    }
    if (password != confirmation) {
      setState(() => _confirmPasswordError = 'Passwords do not match');
      _confirmPasswordFocusNode.requestFocus();
      return;
    }

    setState(() => _isSubmitting = true);
    final authProvider = context.read<AuthProvider>();
    final updatePassword = widget.updatePassword ?? authProvider.updatePassword;
    final success = await updatePassword(password);
    if (!mounted) return;

    if (!success) {
      setState(() {
        _isSubmitting = false;
        _errorMessage =
            authProvider.errorMessage ?? 'Could not update password.';
      });
      return;
    }

    AnalyticsService().logEvent('password_reset_completed');
    await Future<void>.sync(() => widget.onComplete?.call());
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: OnboardingColors.backgroundDark,
      body: SafeArea(
        child: Center(
          child: SingleChildScrollView(
            padding: const EdgeInsets.symmetric(horizontal: 32, vertical: 24),
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 420),
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  const OnboardingOrbit(size: 140),
                  const SizedBox(height: 32),
                  const Text(
                    'Reset Password',
                    textAlign: TextAlign.center,
                    style: TextStyle(
                      color: OnboardingColors.textPrimary,
                      fontSize: 28,
                      fontWeight: FontWeight.bold,
                      letterSpacing: 0.5,
                    ),
                  ),
                  const SizedBox(height: 8),
                  const Text(
                    'Set a new password for your Rhythm account',
                    textAlign: TextAlign.center,
                    style: TextStyle(
                      color: OnboardingColors.textSecondary,
                      fontSize: 16,
                      height: 1.5,
                    ),
                  ),
                  const SizedBox(height: 32),
                  _PasswordField(
                    controller: _passwordController,
                    focusNode: _passwordFocusNode,
                    errorText: _passwordError,
                    hintText: 'New password',
                    obscureText: _obscurePassword,
                    enabled: !_isSubmitting,
                    onToggleVisibility: () {
                      setState(() => _obscurePassword = !_obscurePassword);
                    },
                    onChanged: () {
                      if (_passwordError != null || _errorMessage != null) {
                        setState(() {
                          _passwordError = null;
                          _errorMessage = null;
                        });
                      }
                    },
                    onSubmitted: () => _confirmPasswordFocusNode.requestFocus(),
                  ),
                  const SizedBox(height: 16),
                  _PasswordField(
                    controller: _confirmPasswordController,
                    focusNode: _confirmPasswordFocusNode,
                    errorText: _confirmPasswordError,
                    hintText: 'Confirm password',
                    obscureText: _obscureConfirmation,
                    enabled: !_isSubmitting,
                    onToggleVisibility: () {
                      setState(() {
                        _obscureConfirmation = !_obscureConfirmation;
                      });
                    },
                    onChanged: () {
                      if (_confirmPasswordError != null ||
                          _errorMessage != null) {
                        setState(() {
                          _confirmPasswordError = null;
                          _errorMessage = null;
                        });
                      }
                    },
                    onSubmitted: _submit,
                  ),
                  if (_errorMessage != null)
                    Padding(
                      padding: const EdgeInsets.only(top: 16),
                      child: Text(
                        _errorMessage!,
                        textAlign: TextAlign.center,
                        style: TextStyle(
                          color: Colors.red.shade300,
                          fontSize: 14,
                        ),
                      ),
                    ),
                  const SizedBox(height: 32),
                  SunGlowButton(
                    text: 'Update Password',
                    isLoading: _isSubmitting,
                    onPressed: _submit,
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _PasswordField extends StatelessWidget {
  final TextEditingController controller;
  final FocusNode focusNode;
  final String? errorText;
  final String hintText;
  final bool obscureText;
  final bool enabled;
  final VoidCallback onToggleVisibility;
  final VoidCallback onChanged;
  final VoidCallback onSubmitted;

  const _PasswordField({
    required this.controller,
    required this.focusNode,
    required this.errorText,
    required this.hintText,
    required this.obscureText,
    required this.enabled,
    required this.onToggleVisibility,
    required this.onChanged,
    required this.onSubmitted,
  });

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        Container(
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(16),
            border: Border.all(
              color: errorText != null
                  ? Colors.red.withValues(alpha: 0.5)
                  : OnboardingColors.orbitRing,
              width: 2,
            ),
          ),
          child: TextField(
            controller: controller,
            focusNode: focusNode,
            obscureText: obscureText,
            enabled: enabled,
            style: const TextStyle(
              color: OnboardingColors.textPrimary,
              fontSize: 16,
            ),
            decoration: InputDecoration(
              hintText: hintText,
              hintStyle: TextStyle(
                color: OnboardingColors.textSecondary.withValues(alpha: 0.5),
              ),
              prefixIcon: Icon(
                Icons.lock_outline,
                color: errorText != null
                    ? Colors.red.shade300
                    : OnboardingColors.textSecondary,
              ),
              suffixIcon: IconButton(
                icon: Icon(
                  obscureText ? Icons.visibility_off : Icons.visibility,
                  color: OnboardingColors.textSecondary,
                ),
                onPressed: enabled ? onToggleVisibility : null,
              ),
              border: InputBorder.none,
              contentPadding: const EdgeInsets.all(16),
            ),
            onChanged: (_) => onChanged(),
            onSubmitted: (_) => onSubmitted(),
          ),
        ),
        if (errorText != null)
          Padding(
            padding: const EdgeInsets.only(top: 8, left: 4),
            child: Align(
              alignment: Alignment.centerLeft,
              child: Text(
                errorText!,
                style: TextStyle(
                  color: Colors.red.shade300,
                  fontSize: 13,
                ),
              ),
            ),
          ),
      ],
    );
  }
}
