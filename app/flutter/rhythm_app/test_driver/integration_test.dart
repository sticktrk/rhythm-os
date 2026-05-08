import 'dart:io';

import 'package:integration_test/integration_test_driver.dart';

Future<void> main() async {
  await integrationDriver(
    responseDataCallback: (Map<String, dynamic>? data) async {
      if (data == null) return;
      final screenshots = data['screenshots'] as List<dynamic>?;
      if (screenshots == null || screenshots.isEmpty) return;

      final dir = Directory('screenshots');
      if (!await dir.exists()) {
        await dir.create(recursive: true);
      }

      for (final entry in screenshots) {
        final map = entry as Map<String, dynamic>;
        final name = map['screenshotName'] as String;
        final bytes = (map['bytes'] as List<dynamic>).cast<int>();
        final file = File('screenshots/$name.png');
        await file.writeAsBytes(bytes);
        stdout.writeln('Saved screenshot: ${file.path}');
      }
    },
  );
}
