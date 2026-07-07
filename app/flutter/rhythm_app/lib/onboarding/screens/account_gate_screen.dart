import 'dart:async';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../../services/analytics_service.dart';
import '../providers/auth_provider.dart';
import '../providers/onboarding_provider.dart';
import '../widgets/onboarding_orbit.dart';
import 'account_screen.dart';

/// Full-screen gate shown at launch until the user signs in with a real
/// (non-anonymous) account.
///
/// Shown only for legacy anonymous sessions from before accounts became
/// mandatory — the session must be linked to a real account and there is no
/// local-only path around it. Fresh installs never see this gate: they run
/// the hardware onboarding funnel, which requires sign-in at the connect
/// step instead.
class AccountGateScreen extends StatefulWidget {
  final FutureOr<void> Function() onSignedIn;

  /// True when the device already has an anonymous session from before
  /// accounts became mandatory — shows migration copy instead of the
  /// first-run copy.
  final bool existingUser;

  const AccountGateScreen({
    super.key,
    required this.onSignedIn,
    this.existingUser = false,
  });

  @override
  State<AccountGateScreen> createState() => _AccountGateScreenState();
}

class _AccountGateScreenState extends State<AccountGateScreen> {
  @override
  void initState() {
    super.initState();
    unawaited(AnalyticsService().logScreenView(
      widget.existingUser ? 'account_gate_existing_user' : 'account_gate',
    ));
  }

  @override
  Widget build(BuildContext context) {
    return MultiProvider(
      providers: [
        ChangeNotifierProvider(create: (_) => OnboardingProvider()),
        ChangeNotifierProvider(create: (_) => AuthProvider()),
      ],
      child: Scaffold(
        backgroundColor: OnboardingColors.backgroundDark,
        body: TweenAnimationBuilder<double>(
          tween: Tween(begin: 0, end: 1),
          duration: const Duration(milliseconds: 450),
          curve: Curves.easeOutCubic,
          builder: (context, value, child) {
            return Opacity(
              opacity: value,
              child: Transform.translate(
                offset: Offset(0, 16 * (1 - value)),
                child: child,
              ),
            );
          },
          child: AccountScreen(
            onComplete: widget.onSignedIn,
            onSignedInComplete: widget.onSignedIn,
            existingUserPrompt: widget.existingUser,
          ),
        ),
      ),
    );
  }
}
