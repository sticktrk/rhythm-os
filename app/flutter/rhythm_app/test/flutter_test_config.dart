import 'dart:async';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

import 'helpers/ui_evidence_fonts.dart';

Future<void> testExecutable(FutureOr<void> Function() testMain) async {
  if (Platform.environment['RHYTHM_UI_EVIDENCE_RENDERER'] ==
      uiEvidenceFontRenderer) {
    TestWidgetsFlutterBinding.ensureInitialized();
    await loadUiEvidenceFonts();
  }
  await testMain();
}
