import 'dart:io';

import 'package:flutter/services.dart';

/// Renderer identity required by the exact-head Flutter UI evidence pipeline.
const uiEvidenceFontRenderer = 'rhythm_flutter_test_fonts_v1';

/// The text family loaded for deterministic Flutter UI evidence.
const uiEvidenceFontFamily = 'Roboto';

Future<ByteData> _readFont(File file) async {
  if (!file.existsSync()) {
    throw StateError('Flutter UI evidence font is unavailable: ${file.path}');
  }
  final bytes = await file.readAsBytes();
  return bytes.buffer.asByteData(bytes.offsetInBytes, bytes.lengthInBytes);
}

Future<void> _loadFamily(String family, Iterable<File> files) async {
  final loader = FontLoader(family);
  for (final file in files) {
    loader.addFont(_readFont(file));
  }
  await loader.load();
}

/// Loads real text and icon fonts before a headless Flutter screenshot test.
///
/// `flutter test` normally substitutes the square Ahem font. UI evidence must
/// call this before pumping widgets (the canonical renderer does so through
/// `test/flutter_test_config.dart`) or screenshots will contain blank bars and
/// missing-glyph boxes instead of text and icons.
Future<void> loadUiEvidenceFonts() async {
  final executable = File(Platform.resolvedExecutable);
  final artifacts = executable.parent.parent.parent;
  final cache = artifacts.parent;
  final materialFonts = Directory('${artifacts.path}/material_fonts');

  await Future.wait([
    _loadFamily(uiEvidenceFontFamily, [
      File('${materialFonts.path}/Roboto-Regular.ttf'),
      File('${materialFonts.path}/Roboto-Medium.ttf'),
      File('${materialFonts.path}/Roboto-Bold.ttf'),
    ]),
    _loadFamily('MaterialIcons', [
      File('${materialFonts.path}/MaterialIcons-Regular.otf'),
    ]),
    _loadFamily('monospace', [
      _firstAvailableFont([
        File(
          '${cache.path}/dart-sdk/bin/resources/devtools/assets/fonts/'
          'Roboto_Mono/RobotoMono-Regular.ttf',
        ),
        File(
          '${cache.path}/dart-sdk/bin/resources/devtools/assets/packages/'
          'devtools_app_shared/fonts/Roboto_Mono/RobotoMono-Regular.ttf',
        ),
        File('${materialFonts.path}/Roboto-Regular.ttf'),
      ]),
    ]),
  ]);
}

File _firstAvailableFont(Iterable<File> candidates) {
  return candidates.firstWhere(
    (candidate) => candidate.existsSync(),
    orElse: () => candidates.last,
  );
}
