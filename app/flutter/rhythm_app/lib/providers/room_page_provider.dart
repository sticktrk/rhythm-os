import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';
import '../services/server_identity.dart';
import '../services/settings_service.dart';

/// Manages room-to-page assignments and ordering for multi-screen room layout.
///
/// Rooms can be organized across multiple horizontal pages (like iOS home screen).
/// Each page stores an ordered list of room IDs. During normal operation every
/// current room is materialized into exactly one page so drag/drop works against
/// the same ordering the user sees. Page layouts persist via [SettingsService].
abstract interface class RoomPageLayoutStore {
  List<List<String>>? loadLayout({String? scopeKey});
  List<List<String>>? loadLegacyLayout();
  Future<void> saveLayout(List<List<String>> pages, {String? scopeKey});
  Future<void> clearLayout({String? scopeKey});
  Future<void> migrateLegacyLayoutToScope(String scopeKey);
}

class SettingsRoomPageLayoutStore implements RoomPageLayoutStore {
  final SettingsService _settingsService;

  SettingsRoomPageLayoutStore(this._settingsService);

  @override
  List<List<String>>? loadLayout({String? scopeKey}) {
    return _settingsService.getRoomPageLayout(scopeKey: scopeKey);
  }

  @override
  List<List<String>>? loadLegacyLayout() {
    return _settingsService.getLegacyRoomPageLayout();
  }

  @override
  Future<void> saveLayout(List<List<String>> pages, {String? scopeKey}) {
    return _settingsService.saveRoomPageLayout(pages, scopeKey: scopeKey);
  }

  @override
  Future<void> clearLayout({String? scopeKey}) {
    return _settingsService.clearRoomPageAssignments(scopeKey: scopeKey);
  }

  @override
  Future<void> migrateLegacyLayoutToScope(String scopeKey) {
    return _settingsService.migrateLegacyRoomPageLayoutToScope(scopeKey);
  }
}

class RoomPageProvider extends ChangeNotifier {
  final RoomPageLayoutStore _layoutStore;
  void Function(String? scopeKey)? _onUserLayoutChanged;
  void Function(String? scopeKey)? _onLayoutScopeChanged;

  /// Ordered room IDs per page. Index = page number.
  List<List<String>> _pages = [];
  bool _editMode = false;
  bool _initialized = false;
  String? _scopeKey;
  List<String> _scopeKeyAliases = const [];
  bool _claimedLegacyLayout = false;
  bool _pausePersistenceUntilRoomSetChanges = false;
  String? _roomSignatureAtScopeChange;

  RoomPageProvider({
    RoomPageLayoutStore? layoutStore,
    void Function(String? scopeKey)? onUserLayoutChanged,
  })  : _onUserLayoutChanged = onUserLayoutChanged,
        _layoutStore = layoutStore ??
            SettingsRoomPageLayoutStore(SettingsService.instance);

  /// Whether edit mode (wiggle + drag) is active.
  bool get editMode => _editMode;

  /// Active layout scope key.
  String? get scopeKey => _scopeKey;

  void configureUserLayoutChanged(
    void Function(String? scopeKey)? onUserLayoutChanged,
  ) {
    _onUserLayoutChanged = onUserLayoutChanged;
  }

  /// Configure a lifecycle callback for switches between physical layouts.
  ///
  /// Initial app startup is restored by AppStateRefresh. This callback only
  /// fires for later scope changes, such as selecting another home while the
  /// app is already running.
  void configureLayoutScopeChanged(
    void Function(String? scopeKey)? onLayoutScopeChanged,
  ) {
    _onLayoutScopeChanged = onLayoutScopeChanged;
  }

  /// Load saved page layout from persistent storage.
  void initialize({
    String? scopeKey,
    Iterable<String> scopeKeyAliases = const <String>[],
  }) {
    if (_initialized) return;
    _initialized = true;
    setLayoutScope(
      scopeKey,
      scopeKeyAliases: scopeKeyAliases,
      notify: false,
    );
  }

  /// Build a room-layout scope key from the currently active home and hubs.
  ///
  /// This is intentionally derived from the local hub topology for now. When
  /// cloud-backed hub/account IDs solidify, this method is the single place to
  /// swap over to that canonical identity.
  static String? layoutScopeFor({
    required Home? home,
    required List<Hub> hubs,
  }) {
    final enabledHubs = hubs.where((hub) => hub.enabled).toList();

    final serverHub =
        enabledHubs.where((hub) => hub.type == HubType.server).firstOrNull;
    if (serverHub != null) {
      return 'server:${hubLayoutKey(serverHub)}';
    }

    if (enabledHubs.isNotEmpty) {
      final fingerprints = enabledHubs.map(_hubFingerprint).toList()..sort();
      return 'hubs:${fingerprints.join('|')}';
    }

    return home == null ? null : 'home:${home.id}:default';
  }

  /// Previous local keys that may contain the layout for the active scope.
  ///
  /// Layouts used to be keyed by the server's mutable network endpoint. Keep
  /// that key as a read-through alias while durable server identities take
  /// over as the canonical local scope.
  static List<String> layoutScopeAliasesFor({
    required Home? home,
    required List<Hub> hubs,
  }) {
    final serverHub = hubs
        .where((hub) => hub.enabled && hub.type == HubType.server)
        .firstOrNull;
    if (serverHub == null) return const <String>[];

    final canonical = 'server:${hubLayoutKey(serverHub)}';
    final endpointScope = 'server:${_hubFingerprint(serverHub)}';
    return canonical == endpointScope
        ? const <String>[]
        : <String>[endpointScope];
  }

  /// Stable key used by cloud layout sync to match an All Rooms layout to the
  /// same physical/logical hub on another phone.
  static String hubLayoutKey(Hub hub) {
    final identity = normalizeServerIdentity(hub.serverInstanceId);
    if (serverIdentityKind(identity) == ServerIdentityKind.durable) {
      return 'server_instance:${Uri.encodeComponent(identity!)}';
    }
    return _hubFingerprint(hub);
  }

  /// Keys accepted while restoring account layouts. The endpoint fingerprint
  /// keeps cloud bundles written before durable server identity compatible.
  static List<String> hubLayoutKeyAliases(Hub hub) {
    final primary = hubLayoutKey(hub);
    final endpoint = _hubFingerprint(hub);
    return primary == endpoint ? const <String>[] : <String>[endpoint];
  }

  /// Reload the persisted layout for a new room source scope.
  void setLayoutScope(
    String? scopeKey, {
    Iterable<String> scopeKeyAliases = const <String>[],
    bool notify = true,
  }) {
    final normalizedScopeKey =
        scopeKey != null && scopeKey.isEmpty ? null : scopeKey;
    final normalizedAliases = scopeKeyAliases
        .where((alias) => alias.isNotEmpty && alias != normalizedScopeKey)
        .toSet()
        .toList(growable: false);

    if (_initialized &&
        normalizedScopeKey == _scopeKey &&
        listEquals(normalizedAliases, _scopeKeyAliases)) {
      return;
    }

    // A mutable endpoint alias can change while the durable scope remains the
    // same. Keep the already-loaded canonical layout in memory instead of
    // briefly replacing it with an empty lookup while an alias copy is still
    // being persisted.
    if (_initialized && normalizedScopeKey == _scopeKey) {
      _scopeKeyAliases = normalizedAliases;
      if (_pages.isNotEmpty) return;

      _pages = _loadPagesForScope(normalizedScopeKey, normalizedAliases);
      if (!_editMode) {
        _compactPages();
      }
      if (notify) {
        notifyListeners();
      }
      return;
    }

    final wasInitialized = _initialized;
    _initialized = true;
    _scopeKey = normalizedScopeKey;
    _scopeKeyAliases = normalizedAliases;
    _pages = _loadPagesForScope(normalizedScopeKey, normalizedAliases);
    if (!_editMode) {
      _compactPages();
    }
    _pausePersistenceUntilRoomSetChanges = normalizedScopeKey != null;
    _roomSignatureAtScopeChange = null;

    if (notify) {
      notifyListeners();
    }
    if (wasInitialized) {
      _onLayoutScopeChanged?.call(_scopeKey);
    }
  }

  /// Reload the current scope from storage.
  ///
  /// Used after cloud settings restore writes the layout outside this provider.
  void reloadLayout({bool notify = true}) {
    _pages = _loadPagesForScope(_scopeKey, _scopeKeyAliases);
    if (!_editMode) {
      _compactPages();
    }
    if (notify) {
      notifyListeners();
    }
  }

  /// Get the page index for a room (defaults to 0 if not in any page).
  int getPage(String roomId) {
    for (var i = 0; i < _pages.length; i++) {
      if (_pages[i].contains(roomId)) return i;
    }
    return 0;
  }

  /// Get rooms for a specific page.
  ///
  /// Normal presentation is alphabetical within each swipeable page. Edit mode
  /// keeps the stored order so drag targets remain stable while page membership
  /// and positions are being changed.
  List<RoomDto> getRoomsForPage(int pageIndex, List<RoomDto> allRooms) {
    final roomMap = {for (final r in allRooms) r.id: r};

    // Preserve the legacy first-frame behavior before rooms have been
    // reconciled into explicit pages.
    if (_pages.isEmpty) {
      if (pageIndex != 0) return [];
      final fallback = List<RoomDto>.from(allRooms)..sort(_compareRoomsByName);
      return fallback;
    }

    if (pageIndex < _pages.length) {
      final ordered = <RoomDto>[];
      for (final id in _pages[pageIndex]) {
        final room = roomMap[id];
        if (room != null) ordered.add(room);
      }
      if (!_editMode) {
        ordered.sort(_compareRoomsByName);
      }
      return ordered;
    }

    // Page beyond stored pages — empty
    return [];
  }

  /// Number of pages. In edit mode, includes one extra empty page.
  int get pageCount {
    final base = _pages.isEmpty ? 1 : _pages.length;
    return _editMode ? base + 1 : base;
  }

  /// Enter edit mode — cards wiggle and become draggable.
  void enterEditMode() {
    if (_editMode) return;
    _editMode = true;
    notifyListeners();
  }

  /// Exit edit mode — stop wiggle, compact empty pages, persist.
  void exitEditMode() {
    if (!_editMode) return;
    _editMode = false;
    _compactPages();
    _save();
    notifyListeners();
  }

  /// Move a room to a specific page and position.
  ///
  /// If [insertIndex] is null, appends to the end of the target page.
  void moveRoom(String roomId, int toPage, {int? insertIndex}) {
    // Remove from current location
    for (final page in _pages) {
      page.remove(roomId);
    }

    // Ensure target page exists
    while (_pages.length <= toPage) {
      _pages.add([]);
    }

    // Insert at position
    final targetPage = _pages[toPage];
    if (insertIndex != null && insertIndex <= targetPage.length) {
      targetPage.insert(insertIndex, roomId);
    } else {
      targetPage.add(roomId);
    }

    _save();
    _onUserLayoutChanged?.call(_scopeKey);
    notifyListeners();
  }

  /// Reorder a room within its current page.
  void reorderInPage(String roomId, int pageIndex, int newIndex) {
    if (pageIndex >= _pages.length) return;
    final page = _pages[pageIndex];
    final oldIndex = page.indexOf(roomId);
    if (oldIndex == -1) return;
    if (oldIndex == newIndex) return;

    page.removeAt(oldIndex);
    final clampedIndex = newIndex.clamp(0, page.length);
    page.insert(clampedIndex, roomId);

    _save();
    _onUserLayoutChanged?.call(_scopeKey);
    notifyListeners();
  }

  /// Reconcile page layout with the current room list.
  ///
  /// Ensures every current room appears exactly once across all pages,
  /// preserving stored order where possible and appending new rooms to page 0.
  void reconcileRooms(List<RoomDto> currentRooms) {
    final allowPersistence = _allowPersistenceForRooms(currentRooms);
    final currentSet = currentRooms.map((room) => room.id).toSet();
    final normalizedPages = <List<String>>[];
    final seen = <String>{};
    var changed = false;

    for (final page in _pages) {
      final nextPage = <String>[];
      for (final id in page) {
        if (!currentSet.contains(id)) {
          changed = true;
          continue;
        }
        if (!seen.add(id)) {
          changed = true;
          continue;
        }
        nextPage.add(id);
      }
      normalizedPages.add(nextPage);
    }

    if (normalizedPages.isEmpty && currentRooms.isNotEmpty) {
      normalizedPages.add([]);
      changed = true;
    }

    final missingRooms = currentRooms
        .where((room) => !seen.contains(room.id))
        .toList()
      ..sort((a, b) => a.name.toLowerCase().compareTo(b.name.toLowerCase()));
    if (missingRooms.isNotEmpty) {
      if (normalizedPages.isEmpty) {
        normalizedPages.add([]);
      }
      normalizedPages.first.addAll(missingRooms.map((room) => room.id));
      changed = true;
    }

    if (!_editMode && normalizedPages.any((page) => page.isEmpty)) {
      changed = true;
    }

    if (changed) {
      _pages = normalizedPages;
      if (!_editMode) {
        _compactPages();
      }
      if (allowPersistence) {
        _save();
      }
      notifyListeners();
    }
  }

  /// Remove empty pages and renumber the remaining pages compactly.
  void _compactPages() {
    _pages = _pages.where((page) => page.isNotEmpty).toList();
  }

  Future<void> _save() async {
    try {
      if (_pages.isEmpty || _pages.every((p) => p.isEmpty)) {
        await _layoutStore.clearLayout(scopeKey: _scopeKey);
      } else {
        await _layoutStore.saveLayout(_pages, scopeKey: _scopeKey);
      }
    } catch (e) {
      debugPrint('RoomPageProvider: Failed to save: $e');
    }
  }

  List<List<String>> _loadPagesForScope(
    String? scopeKey,
    Iterable<String> scopeKeyAliases,
  ) {
    final scoped = _layoutStore.loadLayout(scopeKey: scopeKey);
    if (scoped != null) {
      return scoped;
    }

    if (scopeKey != null) {
      for (final alias in scopeKeyAliases) {
        final aliased = _layoutStore.loadLayout(scopeKey: alias);
        if (aliased == null) continue;
        unawaited(_copyAliasedLayoutToScope(aliased, scopeKey));
        return aliased;
      }
    }

    if (scopeKey == null || _claimedLegacyLayout) {
      return [];
    }

    final legacy = _layoutStore.loadLegacyLayout();
    if (legacy == null) {
      return [];
    }

    _claimedLegacyLayout = true;
    unawaited(_layoutStore.migrateLegacyLayoutToScope(scopeKey));
    return legacy;
  }

  Future<void> _copyAliasedLayoutToScope(
    List<List<String>> pages,
    String scopeKey,
  ) async {
    try {
      await _layoutStore.saveLayout(pages, scopeKey: scopeKey);
    } catch (e) {
      debugPrint(
        'RoomPageProvider: Failed to migrate endpoint-keyed layout: $e',
      );
    }
  }

  bool _allowPersistenceForRooms(List<RoomDto> rooms) {
    if (!_pausePersistenceUntilRoomSetChanges) {
      return true;
    }

    final signature = _roomSignature(rooms);
    if (_roomSignatureAtScopeChange == null) {
      _roomSignatureAtScopeChange = signature;
      return false;
    }

    if (_roomSignatureAtScopeChange == signature) {
      return false;
    }

    _pausePersistenceUntilRoomSetChanges = false;
    _roomSignatureAtScopeChange = null;
    return true;
  }

  static String _roomSignature(List<RoomDto> rooms) {
    final ids = rooms.map((room) => room.id).toList()..sort();
    return ids.join('|');
  }

  static int _compareRoomsByName(RoomDto left, RoomDto right) {
    final folded = left.name.toLowerCase().compareTo(right.name.toLowerCase());
    if (folded != 0) return folded;
    final exact = left.name.compareTo(right.name);
    if (exact != 0) return exact;
    return left.id.compareTo(right.id);
  }

  static String _hubFingerprint(Hub hub) {
    final endpoint = hub.endpoint;
    final ssl = endpoint.useSsl ? 'ssl' : 'plain';
    final host = Uri.encodeComponent(endpoint.host);
    return '${hub.type.name}:$host:${endpoint.port}:$ssl';
  }
}
