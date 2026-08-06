import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';

import '../data/local_data_source.dart';

/// A Rhythm server the user has connected to before.
///
/// Stored locally only — never synced to Supabase — so recents don't leak
/// across accounts. `lastConnected` orders the list; `token` lets us
/// reconnect without re-pairing over BLE.
@immutable
class RecentServer {
  final String name;
  final String host;
  final int port;
  final String? token;
  final String? serverInstanceId;
  final DateTime lastConnected;

  const RecentServer({
    required this.name,
    required this.host,
    required this.port,
    required this.lastConnected,
    this.token,
    this.serverInstanceId,
  });

  String get id => '$host:$port';

  RecentServer copyWith({
    String? name,
    String? token,
    String? serverInstanceId,
    DateTime? lastConnected,
  }) {
    return RecentServer(
      name: name ?? this.name,
      host: host,
      port: port,
      token: token ?? this.token,
      serverInstanceId:
          _cleanOptional(serverInstanceId) ?? this.serverInstanceId,
      lastConnected: lastConnected ?? this.lastConnected,
    );
  }

  Map<String, dynamic> toJson() => {
        'name': name,
        'host': host,
        'port': port,
        if (token != null) 'token': token,
        if (serverInstanceId != null) 'serverInstanceId': serverInstanceId,
        'lastConnected': lastConnected.toIso8601String(),
      };

  static RecentServer? fromJson(Map<String, dynamic> json) {
    final host = (json['host'] as String?)?.trim();
    final port = json['port'] as int?;
    if (host == null || host.isEmpty || port == null) return null;
    final lastConnectedStr = json['lastConnected'] as String?;
    final lastConnected = lastConnectedStr != null
        ? DateTime.tryParse(lastConnectedStr) ?? DateTime.now()
        : DateTime.now();
    return RecentServer(
      name: (json['name'] as String?)?.trim().isNotEmpty == true
          ? (json['name'] as String).trim()
          : 'RhythmServer',
      host: host,
      port: port,
      token: (json['token'] as String?)?.trim().isNotEmpty == true
          ? (json['token'] as String).trim()
          : null,
      serverInstanceId: _cleanOptional(
        json['serverInstanceId'] as String? ??
            json['server_instance_id'] as String?,
      ),
      lastConnected: lastConnected,
    );
  }
}

/// Local-only cache of Rhythm servers we've successfully reached.
///
/// The synced Hub list represents whatever box is currently paired; recents
/// outlive disconnect/reset so the user can re-pair with one tap without
/// waiting on a fresh mDNS sweep. Online/offline status is in-memory only —
/// background discovery refreshes it via [setOnlineIds] / [markOnline].
class RecentServersService extends ChangeNotifier {
  static final RecentServersService instance = RecentServersService._();
  RecentServersService._();

  // Bumped if the on-disk shape ever changes incompatibly.
  static const String _storageKey = 'recent_servers_v1';
  static const int _maxEntries = 12;

  final LocalDataSource _dataSource = LocalDataSource();
  List<RecentServer> _servers = const [];
  final Set<String> _onlineIds = <String>{};
  bool _initialized = false;

  bool get isInitialized => _initialized;

  /// Recent servers, newest first.
  List<RecentServer> get servers => List.unmodifiable(_servers);

  bool isOnline(String id) => _onlineIds.contains(id);

  Future<void> initialize() async {
    if (_initialized) return;
    if (!_dataSource.isInitialized) {
      await _dataSource.initialize();
    }
    final raw = _dataSource.getSettingsValue(_storageKey);
    if (raw is String && raw.isNotEmpty) {
      try {
        final decoded = jsonDecode(raw);
        if (decoded is List) {
          final parsed = <RecentServer>[];
          for (final entry in decoded) {
            Map<String, dynamic>? map;
            if (entry is Map<String, dynamic>) {
              map = entry;
            } else if (entry is Map) {
              map = entry.cast<String, dynamic>();
            }
            if (map == null) continue;
            final server = RecentServer.fromJson(map);
            if (server != null) parsed.add(server);
          }
          parsed.sort((a, b) => b.lastConnected.compareTo(a.lastConnected));
          _servers = parsed;
        }
      } catch (e) {
        debugPrint('RecentServersService: failed to decode storage: $e');
      }
    }
    _initialized = true;
    notifyListeners();
  }

  /// Record a successful connection. Upserts by `host:port`, bumps the
  /// timestamp, and marks the entry online.
  Future<void> record({
    required String name,
    required String host,
    required int port,
    String? token,
    String? serverInstanceId,
  }) async {
    if (!_initialized) await initialize();
    final id = '$host:$port';
    final now = DateTime.now();
    final existingIndex = _servers.indexWhere((s) => s.id == id);
    final displayName = name.trim().isNotEmpty ? name.trim() : 'RhythmServer';
    final cleanToken = token?.trim();
    final cleanServerInstanceId = _cleanOptional(serverInstanceId);

    final RecentServer updated;
    if (existingIndex >= 0) {
      final existing = _servers[existingIndex];
      updated = RecentServer(
        name: displayName,
        host: existing.host,
        port: existing.port,
        token: cleanToken != null && cleanToken.isNotEmpty
            ? cleanToken
            : existing.token,
        serverInstanceId: cleanServerInstanceId,
        lastConnected: now,
      );
    } else {
      updated = RecentServer(
        name: displayName,
        host: host,
        port: port,
        token: cleanToken != null && cleanToken.isNotEmpty ? cleanToken : null,
        serverInstanceId: cleanServerInstanceId,
        lastConnected: now,
      );
    }

    final next = List<RecentServer>.from(_servers);
    if (existingIndex >= 0) {
      next.removeAt(existingIndex);
    }
    next.insert(0, updated);
    if (next.length > _maxEntries) {
      next.removeRange(_maxEntries, next.length);
    }
    _servers = next;
    _onlineIds.add(id);
    await _persist();
    notifyListeners();
  }

  Future<void> remove(String id) async {
    if (!_initialized) await initialize();
    final next = _servers.where((s) => s.id != id).toList(growable: false);
    if (next.length == _servers.length) return;
    _servers = next;
    _onlineIds.remove(id);
    await _persist();
    notifyListeners();
  }

  /// Replace the set of currently-reachable IDs. Use this at the end of a
  /// discovery sweep so previously-online entries that didn't show up this
  /// round get dimmed back to offline.
  void setOnlineIds(Iterable<String> ids) {
    final next = ids.toSet();
    if (setEquals(next, _onlineIds)) return;
    _onlineIds
      ..clear()
      ..addAll(next);
    notifyListeners();
  }

  /// Mark a single server reachable. Useful during a streaming probe so
  /// recents light up before the full sweep completes.
  void markOnline(String id) {
    if (_onlineIds.add(id)) notifyListeners();
  }

  void markOffline(String id) {
    if (_onlineIds.remove(id)) notifyListeners();
  }

  Future<void> _persist() async {
    if (!_dataSource.isInitialized) return;
    final encoded = jsonEncode(_servers.map((s) => s.toJson()).toList());
    await _dataSource.saveSettingsValue(_storageKey, encoded);
  }
}

String? _cleanOptional(String? value) {
  final clean = value?.trim();
  return clean != null && clean.isNotEmpty ? clean : null;
}
