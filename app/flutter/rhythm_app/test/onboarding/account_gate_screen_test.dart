import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/onboarding/screens/account_gate_screen.dart';
import 'package:rhythm_app/onboarding/widgets/sun_glow_button.dart';

void main() {
  testWidgets('offers sign-in options and no local-only escape hatch',
      (tester) async {
    await tester.pumpWidget(
      MaterialApp(home: AccountGateScreen(onSignedIn: () {})),
    );
    await tester.pump(const Duration(milliseconds: 500));

    expect(find.text('Create Your Account'), findsOneWidget);
    expect(find.text('Continue with Google'), findsOneWidget);
    expect(find.text('Continue with Apple'), findsOneWidget);
    expect(find.text('Continue with Email'), findsOneWidget);

    // The account requirement must not be skippable.
    expect(find.text('Continue local-only'), findsNothing);
    expect(find.byType(TextLinkButton), findsNothing);
  });

  testWidgets('shows migration prompt for existing anonymous users',
      (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: AccountGateScreen(onSignedIn: () {}, existingUser: true),
      ),
    );
    await tester.pump(const Duration(milliseconds: 500));

    expect(find.text('Sign In Required'), findsOneWidget);
    expect(
      find.textContaining('Your rooms and settings stay on this device'),
      findsOneWidget,
    );
    expect(find.text('Continue with Google'), findsOneWidget);
    expect(find.text('Continue with Apple'), findsOneWidget);
    expect(find.text('Continue with Email'), findsOneWidget);

    // Still no way past the gate without an account.
    expect(find.text('Continue local-only'), findsNothing);
    expect(find.byType(TextLinkButton), findsNothing);
  });
}
