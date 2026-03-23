import 'package:flutter_test/flutter_test.dart';

/// Extension methods for WidgetTester to simplify common test operations.
extension PumpHelpers on WidgetTester {
  /// Pump the widget tree and wait for animations to settle.
  ///
  /// Use this after triggering animations or state changes.
  Future<void> pumpAndSettle2({
    Duration duration = const Duration(milliseconds: 100),
    int maxIterations = 10,
  }) async {
    for (int i = 0; i < maxIterations; i++) {
      await pump(duration);
      if (!hasRunningAnimations) break;
    }
  }

  /// Pump frames for a specific duration.
  Future<void> pumpDuration(Duration duration) async {
    final end = DateTime.now().add(duration);
    while (DateTime.now().isBefore(end)) {
      await pump(const Duration(milliseconds: 16)); // ~60fps
    }
  }

  /// Wait for async operations to complete.
  Future<void> pumpAsync() async {
    await pumpAndSettle();
    await pump(Duration.zero);
  }
}

/// Matcher for finding widgets by semantic label.
Finder findBySemanticsLabel(String label) {
  return find.bySemanticsLabel(label);
}

/// Matcher for finding widgets containing specific text.
Finder findTextContaining(String substring) {
  return find.byWidgetPredicate(
    (widget) => widget.toString().contains(substring),
  );
}
