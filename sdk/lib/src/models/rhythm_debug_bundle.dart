import 'dart:typed_data';

/// Binary debug bundle attachment exported by the server.
class RhythmDebugBundle {
  const RhythmDebugBundle({
    required this.fileName,
    required this.bytes,
    required this.contentType,
  });

  final String fileName;
  final Uint8List bytes;
  final String contentType;
}
