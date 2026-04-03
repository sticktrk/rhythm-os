import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';
import '../services/settings_service.dart';

/// Manages room-to-page assignments and ordering for multi-screen room layout.
///
/// Rooms can be organized across multiple horizontal pages (like iOS home screen).
/// Each page stores an ordered list of room IDs. During normal operation every
/// current room is materialized into exactly one page so drag/drop works against
/// the same ordering the user sees. Page layouts persist via [SettingsService].
class RoomPageProvider extends ChangeNotifier {
  /// Ordered room IDs per page. Index = page number.
  List<List<String>> _pages = [];
  bool _editMode = false;
  bool _initialized = false;

  /// Whether edit mode (wiggle + drag) is active.
  bool get editMode => _editMode;

  /// Load saved page layout from persistent storage.
  void initialize() {
    if (_initialized) return;
    _pages = SettingsService.instance.getRoomPageLayout() ?? [];
    _initialized = true;
  }

  /// Get the page index for a room (defaults to 0 if not in any page).
  int getPage(String roomId) {
    for (var i = 0; i < _pages.length; i++) {
      if (_pages[i].contains(roomId)) return i;
    }
    return 0;
  }

  /// Get rooms for a specific page in their stored order.
  List<RoomDto> getRoomsForPage(int pageIndex, List<RoomDto> allRooms) {
    final roomMap = {for (final r in allRooms) r.id: r};

    // Preserve the legacy first-frame behavior before rooms have been
    // reconciled into explicit pages.
    if (_pages.isEmpty) {
      if (pageIndex != 0) return [];
      final fallback = List<RoomDto>.from(allRooms)
        ..sort((a, b) => a.name.toLowerCase().compareTo(b.name.toLowerCase()));
      return fallback;
    }

    if (pageIndex < _pages.length) {
      final ordered = <RoomDto>[];
      for (final id in _pages[pageIndex]) {
        final room = roomMap[id];
        if (room != null) ordered.add(room);
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
    notifyListeners();
  }

  /// Reconcile page layout with the current room list.
  ///
  /// Ensures every current room appears exactly once across all pages,
  /// preserving stored order where possible and appending new rooms to page 0.
  void reconcileRooms(List<RoomDto> currentRooms) {
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
      _save();
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
        await SettingsService.instance.clearRoomPageAssignments();
      } else {
        await SettingsService.instance.saveRoomPageLayout(_pages);
      }
    } catch (e) {
      debugPrint('RoomPageProvider: Failed to save: $e');
    }
  }
}
