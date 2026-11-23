import 'dart:convert';

import 'package:dio/dio.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import 'customer_assistant_service.dart';

/// A tool call the phone can display completely before dispatch. Room identity
/// always comes from the current hub topology, never from the model.
class LightingProposal {
  LightingProposal._(this.operation, this.arguments, this.label, this.detail);
  final String operation, label, detail;
  final Map<String, dynamic> arguments;

  static const supported = {
    'adjust_room_brightness',
    'adjust_room_warmth',
    'control_room_power',
    'set_room_color',
    'try_room_offset',
  };

  static LightingProposal parse(
      Map<String, dynamic> response, Set<String> tools) {
    final op = response['operation'], args = response['arguments'];
    if (op is! String || !tools.contains(op) || args is! Map<String, dynamic>) {
      throw const FormatException('Unavailable lighting action');
    }
    int number(String key, int min, int max) {
      final v = args[key];
      if (v is! int || v < min || v > max) {
        throw const FormatException('Invalid lighting value');
      }
      return v;
    }

    late String label, detail;
    if (args.length != (op == 'set_room_color' ? 3 : 1)) {
      throw const FormatException('Unexpected arguments');
    }
    switch (op) {
      case 'adjust_room_brightness':
        final delta = number('delta_percent', -100, 100);
        label = '${delta >= 0 ? '+' : ''}$delta brightness points';
        detail = 'Adjusts adaptive brightness and may turn the room on.';
      case 'adjust_room_warmth':
        label = '${number('kelvin', 500, 25000)} K';
        detail =
            'Sets the warmth target while keeping brightness. May turn the room on.';
      case 'control_room_power':
        label = switch (args['power']) {
          'on' => 'Turn lights on',
          'off' => 'Use normal off behavior',
          'hard_off' => 'Turn all managed lights fully off',
          _ => throw const FormatException('Invalid power setting'),
        };
        detail = args['power'] == 'off'
            ? 'Uses Low Glow if the room is configured for it.'
            : 'Applies to the managed lights in this room.';
      case 'set_room_color':
        final hex = ['red', 'green', 'blue']
            .map((k) => number(k, 0, 255).toRadixString(16).padLeft(2, '0'))
            .join()
            .toUpperCase();
        label = 'Try color #$hex';
        detail =
            'A one-minute preview at the current brightness. May turn the lights on.';
      case 'try_room_offset':
        final minutes = number('offset_minutes', -720, 720);
        label =
            'Try ${minutes.abs()} minutes ${minutes < 0 ? 'earlier' : 'later'}';
        detail =
            'Previews that point on the room’s curve for one minute. May turn the lights on.';
      default:
        throw const FormatException('Unsupported lighting action');
    }
    return LightingProposal._(op, Map.unmodifiable(args), label, detail);
  }
}

class LightingChatHub {
  LightingChatHub(this.rooms, this.tools, this.call, this.close,
      {this.canUndo = false});
  final bool canUndo;
  final List<RhythmTopologyNode> rooms;
  final Set<String> tools;
  final Future<CustomerAction> Function(String, CustomerAction) call;
  final void Function() close;

  static String words(String value) => value
      .toLowerCase()
      .replaceAll(RegExp(r'[^\p{L}\p{N}]+', unicode: true), ' ')
      .trim();

  /// Match whole room names on the device. Claim longer names first so that
  /// "main bedroom" does not also select a separate room called "bedroom".
  List<RhythmTopologyNode> mentionedRooms(String message) {
    var remaining = ' ${words(message)} ';
    final names = rooms
        .map((room) => words(room.name))
        .where((n) => n.isNotEmpty)
        .toSet()
        .toList()
      ..sort((a, b) => b.length.compareTo(a.length));
    final found = <String>{};
    for (final name in names) {
      final phrase = ' $name ';
      if (remaining.contains(phrase)) {
        found.add(name);
        remaining = remaining.replaceAll(phrase, ' ');
      }
    }
    return rooms.where((room) => found.contains(words(room.name))).toList();
  }
}

enum LightingChatAccessIssue implements Exception {
  localNetwork,
  owner,
  revoked
}

class LightingChatConnection {
  LightingChatConnection(
      {required this.baseUrl,
      required this.ownerToken,
      required this.local,
      required this.accountId,
      required this.instance,
      required this.current});
  final String baseUrl, accountId, instance;
  final String? ownerToken;
  final bool local;
  final bool Function() current;
  static const _storage = FlutterSecureStorage(
    iOptions:
        IOSOptions(accessibility: KeychainAccessibility.unlocked_this_device),
    aOptions: AndroidOptions(encryptedSharedPreferences: true),
  );
  String get _key =>
      'lighting-chat/${Uri.encodeComponent(accountId)}/${Uri.encodeComponent(instance)}';
  void _check() {
    if (!current()) throw StateError('Hub or account changed');
  }

  // A double tap or a second route can open the same room while the hub
  // is issuing access. Share only issuance, never the MCP client or its scope.
  static final _issuing = <String, Future<String>>{};

  Future<String> _issue(String? previous) async {
    _check();
    if (!local) throw LightingChatAccessIssue.localNetwork;
    if (ownerToken?.isNotEmpty != true) throw LightingChatAccessIssue.owner;
    final active = _issuing[_key];
    if (active != null) return active;
    final pending = _issueOnce(previous);
    _issuing[_key] = pending;
    try {
      return await pending;
    } finally {
      _issuing.remove(_key);
    }
  }

  Future<String> _issueOnce(String? previous) async {
    final stored = await _storage.read(key: _key);
    _check();
    if (stored != null && stored != previous) return stored;
    final auth = RhythmAuthApi(baseUrl: baseUrl, authToken: ownerToken);
    try {
      final issued = await auth.createAgentCredential();
      try {
        _check();
        final entry = jsonEncode({'id': issued.id, 'token': issued.token});
        await _storage.write(key: _key, value: entry);
        // Restore the prior revoked entry if the resident left during storage.
        if (!current()) {
          if (stored == null) {
            await _storage.delete(key: _key);
          } else {
            await _storage.write(key: _key, value: stored);
          }
          throw StateError('Hub or account changed');
        }
        return entry;
      } catch (_) {
        await auth.revokeAgentCredential(issued.id);
        rethrow;
      }
    } on DioException catch (error) {
      if ([401, 403].contains(error.response?.statusCode)) {
        throw LightingChatAccessIssue.owner;
      }
      rethrow;
    }
  }

  Future<LightingChatHub> open({bool reconnect = false}) async {
    _check();
    final stored = await _storage.read(key: _key);
    _check();
    final entry = stored ?? await _issue(null);
    try {
      return await _discover(entry);
    } on DioException catch (error) {
      if (error.response?.statusCode != 401) rethrow;
      // Keep a rejected credential until the resident explicitly reconnects.
      // Reopening the screen should respect an owner's revocation.
      if (!reconnect || stored == null || !local) {
        throw LightingChatAccessIssue.revoked;
      }
      return _discover(await _issue(stored));
    }
  }

  Future<LightingChatHub> _discover(String entry) async {
    _check();
    final token = (jsonDecode(entry) as Map)['token'] as String;
    final mcp = RhythmMcpApi(baseUrl: baseUrl, agentToken: token);
    try {
      final tools = await mcp.discover();
      _check();
      final topology = RhythmAssistantTopologySnapshot.fromJson(
          await mcp.call('get_topology_snapshot', {}));
      _check();
      if (topology.serverInstanceId != instance ||
          !tools.contains('explain_room_output')) {
        throw const FormatException('Incompatible lighting hub');
      }
      return LightingChatHub(topology.nodes.where((n) => n.isRoom).toList(),
          LightingProposal.supported.intersection(tools), (op, args) async {
        _check();
        final result = await mcp.call(op, args);
        _check();
        return result;
      }, mcp.close, canUndo: tools.contains('undo_room_trial'));
    } catch (_) {
      mcp.close();
      rethrow;
    }
  }
}

CustomerAction lightingReading(CustomerAction room) => {
      for (final k in [
        'canonical_power',
        'canonical_brightness',
        'canonical_kelvin',
        'time_offset_minutes',
        'brightness_offset',
        'adaptation_enabled',
        'mode',
        'state'
      ])
        if (room[k] is num ||
            room[k] is bool ||
            const {'day', 'night', 'sleep', 'active', 'off', 'low_glow', 'mood'}
                .contains(room[k]))
          k: room[k],
    };
