/// Base exception for all Rhythm SDK errors.
class RhythmException implements Exception {
  final String message;
  final Object? cause;

  const RhythmException(this.message, {this.cause});

  @override
  String toString() => 'RhythmException: $message';
}

/// HTTP/network error when communicating with the server.
class RhythmApiException extends RhythmException {
  final int? statusCode;
  final String? serverMessage;

  const RhythmApiException(
    super.message, {
    this.statusCode,
    this.serverMessage,
    super.cause,
  });

  @override
  String toString() =>
      'RhythmApiException($statusCode): $message${serverMessage != null ? ' [$serverMessage]' : ''}';
}

/// Connection-level error (connect failed, too many retries, etc.)
class RhythmConnectionException extends RhythmException {
  const RhythmConnectionException(super.message, {super.cause});

  @override
  String toString() => 'RhythmConnectionException: $message';
}

/// OTA-specific error.
class RhythmOtaException extends RhythmException {
  const RhythmOtaException(super.message, {super.cause});

  @override
  String toString() => 'RhythmOtaException: $message';
}
