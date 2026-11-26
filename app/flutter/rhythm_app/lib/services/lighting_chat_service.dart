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
      {this.canUndo = false, this.catalog = const {}, this.nodes = const []});
  final bool canUndo;
  final Set<String> catalog;
  final List<RhythmTopologyNode> nodes;
  final List<RhythmTopologyNode> rooms;
  final Set<String> tools;
  final Future<CustomerAction> Function(String, CustomerAction) call;
  final void Function() close;

  static String words(String value) => value
      .toLowerCase()
      .replaceAll(RegExp(r'[^\p{L}\p{N}]+', unicode: true), ' ')
      .trim();

  /// A scene can be designed without a current room. Explicit room targets
  /// and exclusions stay on the scoped lighting path.
  static bool requestsSceneDesign(String prompt,
      {bool mentionsRoom = false, bool mentionsSavedScene = false}) {
    final text = words(prompt);
    if (mentionsRoom || hasExclusion(text)) return false;
    final theme =
        RegExp(r'\b(scene|vibe|theme|palette|look|mood)\b|\bfeel like\b');
    if (!theme.hasMatch(text)) return false;
    final create = RegExp(
        r'\b(create|design|generate|build|compose|invent)\b|\bmake\b.*\bscene\b');
    return create.hasMatch(text) ||
        (!mentionsSavedScene && refersToWholeHome(text));
  }

  static const _dwelling = r'(?:home|house|apartment|flat)';
  static const _allRooms = r'(?:all (?:the |my |our )?rooms|every room)';
  static const _wholeHome = '(?:(?:whole|entire) $_dwelling'
      '|$_allRooms'
      r'(?! (?:in|on|within)\b)'
      '|$_dwelling ?wide'
      '|(?:throughout|across|(?:everywhere|$_allRooms) in) (?:the |my |our )?$_dwelling'
      r'|everywhere(?! (?:in|on|within|around|inside|else)\b))';
  static final _homeScope = RegExp(r'\b' + _wholeHome + r'\b');
  static final _homeAnswer = RegExp(
      r'^(?:(?:in|for) )?(?:(?:the|our|my) )?' + _wholeHome + r'(?: please)?$');

  static bool refersToWholeHome(String prompt) =>
      _homeScope.hasMatch(words(prompt));

  static bool isWholeHomeAnswer(String text) =>
      _homeAnswer.hasMatch(words(text));

  static const _except =
      '(?:except(?: for)?|exclude|excluding|skip|leave out|but not|other than|apart from|$_wholeHome but)';
  static const exclusionPrefix = r'\b(?:' + _except + r'|leave)\s+';
  static bool hasExclusion(String prompt) => RegExp(
          r'\b' + _except + r'\b|\bleave\b.*\b(?:alone|unchanged|as is|out)\b')
      .hasMatch(words(prompt));

  static String _displayName(RhythmTopologyNode room) =>
      room.name.trim().isEmpty ? 'Unnamed room' : room.name.trim();

  late final Map<String, String> _roomLabels = _buildRoomLabels();

  Map<String, String> _buildRoomLabels() {
    final groups = <String, List<RhythmTopologyNode>>{};
    for (final room in rooms) {
      groups.putIfAbsent(words(_displayName(room)), () => []).add(room);
    }
    final labels = <String, String>{};
    for (final group in groups.entries) {
      final matching = group.value..sort((a, b) => a.id.compareTo(b.id));
      var number = 0;
      for (final room in matching) {
        var label = _displayName(room);
        if (matching.length > 1) {
          do {
            number++;
          } while (groups.containsKey('${group.key} $number'));
          label = '$label ($number)';
        }
        labels[room.id] = label;
      }
    }
    return labels;
  }

  String roomLabel(RhythmTopologyNode room) =>
      _roomLabels[room.id] ?? _displayName(room);

  static int roomOrder(RhythmTopologyNode a, RhythmTopologyNode b) {
    final names = words(_displayName(a)).compareTo(words(_displayName(b)));
    return names == 0 ? a.id.compareTo(b.id) : names;
  }

  /// A number or ordinal refers to the order of the choices shown in this turn.
  static int? roomAnswerNumber(String text) {
    final answer = words(text)
        .replaceFirst(RegExp(r'^the '), '')
        .replaceFirst(RegExp(r' please$'), '');
    const ordinals = [
      'first',
      'second',
      'third',
      'fourth',
      'fifth',
      'sixth',
      'seventh',
      'eighth',
      'ninth',
      'tenth'
    ];
    final ordinal = ordinals.indexOf(answer.replaceFirst(RegExp(r' one$'), ''));
    if (ordinal >= 0) return ordinal + 1;
    final numeric =
        RegExp(r'^(?:(?:number|option) )?(\d+)$').firstMatch(answer);
    if (numeric == null) return null;
    final number = int.tryParse(numeric[1]!);
    if (number == null) return null;
    return RegExp(r'(?:^|\s)[-−]\s*\d').hasMatch(text) ? -number : number;
  }

  RhythmTopologyNode? roomAnswer(
      String text, List<RhythmTopologyNode> choices) {
    final number = roomAnswerNumber(text);
    if (number != null && number > 0 && number <= choices.length) {
      return choices[number - 1];
    }
    final original = words(text).replaceFirst(RegExp(r' please$'), '');
    final location = original.replaceFirst(RegExp(r'^in '), '');
    final answer = location.replaceFirst(RegExp(r'^the '), '');
    if (answer.isEmpty) return null;
    for (final phrase in {original, location, answer}) {
      final named = choices.where((room) =>
          words(roomLabel(room)) == phrase ||
          words(_displayName(room)) == phrase);
      if (named.isNotEmpty) return named.length == 1 ? named.single : null;
    }
    return null;
  }

  // Keep numbered labels distinct from a lighting value after a room name.
  static String _mentionWords(String text) => text
      .toLowerCase()
      .replaceAllMapped(RegExp(r'\(\s*(\d+)\s*\)|[^\p{L}\p{N}]', unicode: true),
          (m) => m[1] == null ? ' ' : ' (${m[1]}) ')
      .replaceAll(RegExp(r' +'), ' ')
      .trim();

  /// Match whole room names on the device. Claim longer names first so that
  /// "main bedroom" does not also select a separate room called "bedroom".
  List<RhythmTopologyNode> mentionedRooms(String message) {
    var remaining = ' ${_mentionWords(message)} ';
    final aliases = <String, Set<String>>{};
    for (final room in rooms) {
      for (final name in {
        _mentionWords(_displayName(room)),
        _mentionWords(roomLabel(room))
      }) {
        if (name.isNotEmpty) aliases.putIfAbsent(name, () => {}).add(room.id);
      }
    }
    final names = aliases.keys.toList()
      ..sort((a, b) => b.length.compareTo(a.length));
    final found = <String>{};
    for (final name in names) {
      final phrase = ' $name ';
      if (remaining.contains(phrase)) {
        found.addAll(aliases[name]!);
        remaining = remaining.replaceAll(phrase, ' ');
      }
    }
    return rooms.where((room) => found.contains(room.id)).toList();
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
      }, mcp.close,
          canUndo: tools.contains('undo_room_trial'),
          catalog: tools,
          nodes: topology.nodes);
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
