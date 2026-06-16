Map<String, String>? bearerAuthHeaders(
  String? authToken, {
  Map<String, String>? extra,
}) {
  final headers = <String, String>{...?extra};
  final token = authToken?.trim();
  if (token != null && token.isNotEmpty) {
    headers['Authorization'] = 'Bearer $token';
  }
  return headers.isEmpty ? null : headers;
}
