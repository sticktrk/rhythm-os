/// Captures README / marketing screenshots by walking the real app.
///
/// Boots the full app (real startup path, persisted local state on the
/// device), then fans through the main destinations and snapshots each one.
///
/// Run via:
///   flutter drive --driver=test_driver/integration_test.dart \
///     --target=integration_test/readme_screenshots_test.dart \
///     --dart-define-from-file=APP_BUILD_DEFINES -d DEVICE
///
/// Output: flutter/rhythm_app/screenshots/readme_*.png
// ignore_for_file: avoid_print
library;

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:rhythm_app/config/platform_capabilities.dart';
import 'package:rhythm_app/main.dart';

// Many surfaces run continuous animations, so pumpAndSettle() never returns.
// Use bounded pumps everywhere.
Future<void> pumpFor(WidgetTester tester, Duration total) async {
  const step = Duration(milliseconds: 100);
  var elapsed = Duration.zero;
  while (elapsed < total) {
    await tester.pump(step);
    elapsed += step;
  }
}

Future<bool> waitFor(
  WidgetTester tester,
  Finder finder, {
  Duration timeout = const Duration(seconds: 30),
}) async {
  const step = Duration(milliseconds: 200);
  var elapsed = Duration.zero;
  while (elapsed < timeout) {
    await tester.pump(step);
    if (finder.evaluate().isNotEmpty) return true;
    elapsed += step;
  }
  return false;
}

Future<void> openNavAndTap(WidgetTester tester, String label) async {
  final fab = find.bySemanticsLabel('Open navigation');
  if (fab.evaluate().isNotEmpty) {
    await tester.tap(fab.first);
    await pumpFor(tester, const Duration(milliseconds: 600));
  }
  final dest = find.text(label);
  if (await waitFor(tester, dest, timeout: const Duration(seconds: 5))) {
    await tester.tap(dest.first);
    await pumpFor(tester, const Duration(seconds: 2));
  } else {
    print('readme_screenshots: destination "$label" not found');
  }
}

Future<void> goHome(WidgetTester tester) async {
  // Home is the base surface; menu screens return via a close control and
  // detail pages via a back arrow. Pop until neither is present.
  for (var i = 0; i < 4; i++) {
    final close = find.byIcon(Icons.close_rounded);
    final back = find.byWidgetPredicate((w) =>
        w is Icon &&
        (w.icon == Icons.arrow_back_rounded ||
            w.icon == Icons.arrow_back_ios_new_rounded ||
            w.icon == Icons.arrow_back_ios_rounded ||
            w.icon == Icons.arrow_back ||
            w.icon == Icons.chevron_left_rounded));
    if (back.evaluate().isNotEmpty) {
      await tester.tap(back.first);
    } else if (close.evaluate().isNotEmpty) {
      await tester.tap(close.first);
    } else {
      return;
    }
    await pumpFor(tester, const Duration(seconds: 1));
  }
}

Future<void> shot(
  IntegrationTestWidgetsFlutterBinding binding,
  WidgetTester tester,
  String name,
) async {
  await pumpFor(tester, const Duration(milliseconds: 500));
  if (Platform.isAndroid) {
    await binding.convertFlutterSurfaceToImage();
    await tester.pump(const Duration(milliseconds: 100));
  }
  await binding.takeScreenshot(name);
  print('readme_screenshots: captured $name');
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('README walkthrough', (tester) async {
    await tester.pumpWidget(
      RhythmBootstrap(capabilities: PlatformCapabilities.fromPlatform()),
    );

    // If the app is signed out, capture onboarding and enter the demo home.
    final virtual = find.textContaining('Virtual Experience', findRichText: true);
    if (await waitFor(tester, virtual, timeout: const Duration(seconds: 40))) {
      await pumpFor(tester, const Duration(seconds: 2));
      await shot(binding, tester, 'readme_00_onboarding');
      await tester.tap(virtual.first);
      await pumpFor(tester, const Duration(seconds: 3));
      final email = find.byWidgetPredicate((w) =>
          w is TextField && w.decoration?.hintText == 'Email');
      if (await waitFor(tester, email, timeout: const Duration(seconds: 5))) {
        await tester.enterText(email.first, 'demo@example.invalid');
        final pw = find.byWidgetPredicate((w) =>
            w is TextField && w.decoration?.hintText == 'Password');
        await tester.enterText(pw.first, 'demo');
        await pumpFor(tester, const Duration(milliseconds: 500));
        final signIn = find.textContaining('Sign in', findRichText: true);
        if (signIn.evaluate().isNotEmpty) {
          await tester.tap(signIn.last);
        }
        await pumpFor(tester, const Duration(seconds: 3));
      }
    }

    // Wait for startup to hand off to the real app and sync room state.
    final ready = await waitFor(
      tester,
      find.bySemanticsLabel('Open navigation'),
      timeout: const Duration(seconds: 300),
    );
    print('readme_screenshots: app ready=$ready');
    await pumpFor(tester, const Duration(seconds: 6));

    // 1. Home: all rooms.
    await shot(binding, tester, 'readme_01_rooms');

    // 2. Lighting: day curve, profile layers, time simulator.
    await openNavAndTap(tester, 'Lighting');
    await pumpFor(tester, const Duration(seconds: 2));
    await shot(binding, tester, 'readme_02_lighting');

    // Scroll to surface the time simulator if it is below the fold.
    final scrollable = find.byType(Scrollable);
    if (scrollable.evaluate().isNotEmpty) {
      await tester.drag(scrollable.first, const Offset(0, -700));
      for (var i = 0; i < 60; i++) {
        await pumpFor(tester, const Duration(milliseconds: 500));
        if (find.textContaining('Curve loading').evaluate().isEmpty) break;
      }
      await pumpFor(tester, const Duration(seconds: 1));
      await shot(binding, tester, 'readme_03_lighting_scrolled');
      await tester.drag(scrollable.first, const Offset(0, -700));
      await pumpFor(tester, const Duration(seconds: 1));
      await shot(binding, tester, 'readme_04_lighting_scrolled_2');
    }

    // Back home via the close control, then Schedule.
    await goHome(tester);
    await openNavAndTap(tester, 'Schedule');
    await shot(binding, tester, 'readme_05_schedule');

    // Settings.
    await goHome(tester);
    await openNavAndTap(tester, 'Settings');
    await shot(binding, tester, 'readme_06_settings');

    // Power usage.
    final power = find.textContaining('Power Usage', findRichText: true);
    if (await waitFor(tester, power, timeout: const Duration(seconds: 5))) {
      await tester.tap(power.first);
      await pumpFor(tester, const Duration(seconds: 3));
      await shot(binding, tester, 'readme_07_power');
      await goHome(tester);
      await pumpFor(tester, const Duration(seconds: 1));
    }

    // Hub page.
    final hub = find.textContaining('LightBox', findRichText: true);
    if (await waitFor(tester, hub, timeout: const Duration(seconds: 5))) {
      await tester.tap(hub.first);
      await pumpFor(tester, const Duration(seconds: 3));
      await shot(binding, tester, 'readme_08_hub');
      await goHome(tester);
      await pumpFor(tester, const Duration(seconds: 1));
    }

    // Room settings: the gear on the first room card.
    await goHome(tester);
    await pumpFor(tester, const Duration(seconds: 1));
    final gear = find.byWidgetPredicate((w) {
      final k = w.key;
      return w is Icon &&
          k is ValueKey &&
          k.value.toString().startsWith('room-card-settings-icon-');
    });
    if (await waitFor(tester, gear, timeout: const Duration(seconds: 5))) {
      await tester.tap(gear.first);
      await pumpFor(tester, const Duration(seconds: 3));
      await shot(binding, tester, 'readme_09_room');
      final sc = find.byType(Scrollable);
      if (sc.evaluate().isNotEmpty) {
        await tester.drag(sc.first, const Offset(0, -600));
        await pumpFor(tester, const Duration(seconds: 2));
        await shot(binding, tester, 'readme_10_room_scrolled');
      }
    } else {
      print('readme_screenshots: room gear not found');
    }
  });
}
