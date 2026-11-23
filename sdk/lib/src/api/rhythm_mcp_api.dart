import 'dart:convert';

import 'package:dio/dio.dart';

/// Native, stateless Streamable HTTP transport for a hub-issued agent token.
class RhythmMcpApi {
  RhythmMcpApi({
    required String baseUrl,
    required String agentToken,
    Dio? client,
  })  : _client = client ??
            Dio(BaseOptions(connectTimeout: const Duration(seconds: 8))),
        _url = Uri.parse(baseUrl).resolve('/api/assistant/mcp').toString(),
        _token = agentToken;

  final Dio _client;
  final String _url, _token;
  var _nextId = 0;
  static const _version = '2025-06-18';

  Future<Map<String, dynamic>> _send(
    String method,
    Map<String, dynamic> params, {
    bool notification = false,
  }) async {
    final id = ++_nextId;
    final response = await _client.post<Object?>(
      _url,
      data: {
        'jsonrpc': '2.0',
        if (!notification) 'id': id,
        'method': method,
        'params': params,
      },
      options: Options(
        headers: {
          'Authorization': 'Bearer $_token',
          'Accept': 'application/json, text/event-stream',
          'MCP-Protocol-Version': _version,
        },
        followRedirects: false,
        sendTimeout: const Duration(seconds: 10),
        receiveTimeout: const Duration(seconds: 12),
      ),
    );
    if (notification && response.statusCode == 202) return {};
    final envelope = response.data;
    if (envelope is! Map ||
        envelope['jsonrpc'] != '2.0' ||
        envelope['id'] != id ||
        envelope['result'] is! Map ||
        envelope.containsKey('error')) {
      throw const FormatException('Invalid MCP response');
    }
    return Map<String, dynamic>.from(envelope['result'] as Map);
  }

  Future<Set<String>> discover() async {
    final init = await _send('initialize', {
      'protocolVersion': _version,
      'capabilities': <String, dynamic>{},
      'clientInfo': {'name': 'Rhythm app', 'version': '1'},
    });
    if (init['protocolVersion'] != _version) {
      throw UnsupportedError('MCP version');
    }
    await _send('notifications/initialized', {}, notification: true);
    final list = await _send('tools/list', {});
    return (list['tools'] as List)
        .whereType<Map>()
        .map((t) => t['name'])
        .whereType<String>()
        .toSet();
  }

  Future<Map<String, dynamic>> call(
    String name,
    Map<String, dynamic> arguments,
  ) async {
    final result = await _send('tools/call', {
      'name': name,
      'arguments': arguments,
    });
    if (result['isError'] == true) {
      throw StateError('The hub could not verify this operation.');
    }
    final content = (result['content'] as List).whereType<Map>().singleWhere(
          (c) => c['type'] == 'text',
        );
    return jsonDecode(content['text'] as String) as Map<String, dynamic>;
  }

  void close() => _client.close(force: true);
}
