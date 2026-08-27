import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'ui_evidence_fonts.dart';

double _textWidth(TextStyle style) {
  final painter = TextPainter(
    text: TextSpan(text: 'Kitchen', style: style),
    textDirection: TextDirection.ltr,
  )..layout();
  return painter.width;
}

void main() {
  testWidgets('loads readable UI evidence text and icon families',
      (tester) async {
    final ahemWidth = _textWidth(const TextStyle(fontSize: 20));

    await tester.runAsync(loadUiEvidenceFonts);

    expect(
      _textWidth(
        const TextStyle(fontSize: 20, fontFamily: uiEvidenceFontFamily),
      ),
      lessThan(ahemWidth),
    );
    expect(
      _textWidth(const TextStyle(fontSize: 20, fontFamily: 'monospace')),
      lessThan(ahemWidth),
    );

    await tester.pumpWidget(
      const MaterialApp(
        home: Row(
          children: [
            Text('Kitchen'),
            Icon(Icons.power_settings_new),
          ],
        ),
      ),
    );

    final context = tester.element(find.text('Kitchen'));
    final inheritedFamily = DefaultTextStyle.of(context).style.fontFamily;
    expect(inheritedFamily, isNotNull);
    expect(
      _textWidth(TextStyle(fontSize: 20, fontFamily: inheritedFamily)),
      lessThan(ahemWidth),
    );
    expect(find.byIcon(Icons.power_settings_new), findsOneWidget);
  });
}
