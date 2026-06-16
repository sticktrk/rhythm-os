import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/onboarding/screens/password_recovery_screen.dart';

void main() {
  Widget buildSubject({
    Future<bool> Function(String password)? updatePassword,
    Future<void> Function()? onComplete,
  }) {
    return MaterialApp(
      home: PasswordRecoveryScreen(
        updatePassword: updatePassword,
        onComplete: onComplete,
      ),
    );
  }

  testWidgets('validates matching passwords', (tester) async {
    await tester.pumpWidget(buildSubject());

    await tester.enterText(
      find.widgetWithText(TextField, 'New password'),
      'new-password',
    );
    await tester.enterText(
      find.widgetWithText(TextField, 'Confirm password'),
      'different-password',
    );
    await tester.tap(find.text('Update Password'));
    await tester.pump();

    expect(find.text('Passwords do not match'), findsOneWidget);
  });

  testWidgets('updates password and completes recovery', (tester) async {
    String? updatedPassword;
    var completed = false;

    await tester.pumpWidget(
      buildSubject(
        updatePassword: (password) async {
          updatedPassword = password;
          return true;
        },
        onComplete: () async {
          completed = true;
        },
      ),
    );

    await tester.enterText(
      find.widgetWithText(TextField, 'New password'),
      'new-password',
    );
    await tester.enterText(
      find.widgetWithText(TextField, 'Confirm password'),
      'new-password',
    );
    await tester.tap(find.text('Update Password'));
    await tester.pump();

    expect(updatedPassword, 'new-password');
    expect(completed, isTrue);
  });
}
