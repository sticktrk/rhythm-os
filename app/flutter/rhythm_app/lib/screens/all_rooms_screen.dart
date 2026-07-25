import 'dart:math' as math;
import 'dart:async';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show RhythmDispatchResult, RhythmMode, RoomModeState;
import 'package:uuid/uuid.dart';
import '../providers/room_page_provider.dart';
import '../providers/room_provider.dart';
import '../providers/server_sync_provider.dart';
import '../services/analytics_service.dart';
import '../widgets/editable_room_card.dart';
import '../widgets/room_card.dart';
import '../widgets/hub_connection_banner.dart';
import '../widgets/solar_orbit.dart'; // For CelestialColors
import 'sun_position_screen.dart';

/// All Rooms screen — horizontally paged room cards with edit-mode drag support.
///
/// Features:
/// - Multiple swipeable pages of room cards
/// - Long-press to enter edit mode (wiggle + drag between pages)
/// - Deep space background with optional starfield
/// - Pull-to-refresh on each page
class AllRoomsScreen extends StatefulWidget {
  final List<RoomDto> rooms;
  final CurveConfigDto globalConfig;
  final CurveData? curveData;
  final PageController pageController;
  final ValueChanged<int>? onPageChanged;
  final RhythmMode? activeMode;
  final RhythmMode? pendingMode;
  final ValueChanged<RhythmMode>? onModeSelected;
  final ValueChanged<RhythmMode>? onActiveModeDoubleTap;
  final VoidCallback? onHomeChooserTap;

  const AllRoomsScreen({
    super.key,
    required this.rooms,
    required this.globalConfig,
    this.curveData,
    required this.pageController,
    this.onPageChanged,
    this.activeMode,
    this.pendingMode,
    this.onModeSelected,
    this.onActiveModeDoubleTap,
    this.onHomeChooserTap,
  });

  @override
  State<AllRoomsScreen> createState() => _AllRoomsScreenState();
}

class _RoomGridItem {
  const _RoomGridItem.room(this.room, {required this.isHalfWidth})
      : isPlaceholder = false;

  const _RoomGridItem.placeholder({required this.isHalfWidth})
      : room = null,
        isPlaceholder = true;

  final RoomDto? room;
  final bool isHalfWidth;
  final bool isPlaceholder;
}

class _DropTarget {
  const _DropTarget({
    required this.index,
    required this.rect,
    required this.isHalfWidth,
  });

  final int index;
  final Rect rect;
  final bool isHalfWidth;
}

class _AllRoomsScreenState extends State<AllRoomsScreen> {
  static const _uuid = Uuid();
  Timer? _edgeScrollTimer;
  static const _edgeScrollZone = 28.0;
  static const _edgeScrollIntentThreshold = 16.0;
  static const _edgeScrollDelay = Duration(milliseconds: 375);
  static const _headerControlHeight = 38.0;
  bool _globalActionPending = false;
  bool _globalSliderExpanded = false;
  double? _globalSliderValue;
  List<_GlobalRoomBrightness>? _globalUndoSnapshot;

  // Overlay-based drag state
  OverlayEntry? _dragOverlay;
  String? _draggingRoomId;
  int? _activeDragPointer;
  Offset _dragPosition = Offset.zero;
  Offset _dragStartOffset = Offset.zero;
  Offset _edgeScrollReferencePosition = Offset.zero;
  Size _dragCardSize = Size.zero;

  /// Track the page we're auto-scrolling to, so drops during animation land correctly.
  int? _edgeScrollTargetPage;
  int? _dragSourcePage;
  int? _hoverPage;
  int? _hoverIndex;
  bool _isEdgeScrollAnimating = false;

  /// Keys for each card so we can find their positions for drop targeting.
  final Map<String, GlobalKey> _cardKeys = {};

  int _currentPage = 0;

  @override
  void dispose() {
    _edgeScrollTimer?.cancel();
    _removeTrackedPointerRoute();
    _dragOverlay?.remove();
    super.dispose();
  }

  Future<void> _onRefresh() async {
    final serverSync = context.read<ServerSyncProvider>();
    final pageController = widget.pageController;
    final pageBeforeRefresh = pageController.hasClients
        ? (pageController.page ?? _currentPage.toDouble()).round()
        : _currentPage;
    await serverSync.fullRefresh();
    _restorePageAfterRefresh(pageController, pageBeforeRefresh);
    AnalyticsService().logRoomsRefreshed(source: 'all_rooms_pull_to_refresh');
  }

  static void _restorePageAfterRefresh(
    PageController controller,
    int requestedPage, {
    int remainingFrames = 2,
  }) {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!controller.hasClients || !controller.position.hasContentDimensions) {
        if (remainingFrames > 0) {
          _restorePageAfterRefresh(
            controller,
            requestedPage,
            remainingFrames: remainingFrames - 1,
          );
        }
        return;
      }

      final position = controller.position;
      final viewport = position.viewportDimension;
      final maxPage =
          viewport <= 0 ? 0 : (position.maxScrollExtent / viewport).round();
      final targetPage = requestedPage.clamp(0, maxPage);
      if ((controller.page ?? 0).round() != targetPage) {
        controller.jumpToPage(targetPage);
      }
    });
  }

  double _headerVerticalPadding(bool isLandscape) => isLandscape ? 4.0 : 10.0;

  // -- Drag handle callbacks --------------------------------------------------

  void _onHandleDragStart(String roomId, int pointer, Offset globalPosition) {
    _removeTrackedPointerRoute();
    _activeDragPointer = pointer;
    GestureBinding.instance.pointerRouter
        .addRoute(pointer, _handleTrackedPointerEvent);

    final pageProvider = context.read<RoomPageProvider>();
    final currentPage = pageProvider.getPage(roomId);
    final currentRooms =
        pageProvider.getRoomsForPage(currentPage, widget.rooms);
    final dragStartIndex = currentRooms.indexWhere((room) => room.id == roomId);

    // Anchor the drag preview to the card center horizontally so the card
    // stays centered under the finger instead of feeling right-grabbed from
    // the handle edge.
    final key = _cardKeys[roomId];
    final box = key?.currentContext?.findRenderObject() as RenderBox?;
    if (box != null) {
      final cardTopLeft = box.localToGlobal(Offset.zero);
      _dragCardSize = box.size;
      _dragStartOffset = Offset(
        _dragCardSize.width / 2,
        globalPosition.dy - cardTopLeft.dy,
      );
    } else {
      _dragCardSize = Size(MediaQuery.of(context).size.width - 32, 112);
      _dragStartOffset = Offset(_dragCardSize.width / 2, 40);
    }

    setState(() {
      _draggingRoomId = roomId;
      _dragSourcePage = currentPage;
      _hoverPage = currentPage;
      _hoverIndex = dragStartIndex == -1 ? currentRooms.length : dragStartIndex;
    });
    _dragPosition = globalPosition;
    _edgeScrollReferencePosition = globalPosition;
    _edgeScrollTargetPage = null;

    _dragOverlay = OverlayEntry(builder: (_) {
      return Positioned(
        left: _dragPosition.dx - _dragStartOffset.dx,
        top: _dragPosition.dy - _dragStartOffset.dy,
        width: _dragCardSize.width,
        child: IgnorePointer(
          child: Material(
            type: MaterialType.transparency,
            child: Transform.scale(
              scale: 1.06,
              child: Opacity(
                opacity: 0.9,
                child: RoomCard(
                  roomId: roomId,
                  globalConfig: widget.globalConfig,
                  curveData: widget.curveData,
                ),
              ),
            ),
          ),
        ),
      );
    });
    Overlay.of(context).insert(_dragOverlay!);
  }

  void _onHandleDragUpdate(Offset globalPosition) {
    _dragPosition = globalPosition;
    _dragOverlay?.markNeedsBuild();
    _checkEdgeScroll(globalPosition);
    _updateHoverTarget();
  }

  void _handleTrackedPointerEvent(PointerEvent event) {
    if (event.pointer != _activeDragPointer) return;

    if (event is PointerMoveEvent) {
      _onHandleDragUpdate(event.position);
      return;
    }

    if (event is PointerUpEvent || event is PointerCancelEvent) {
      _onHandleDragEnd();
    }
  }

  void _removeTrackedPointerRoute() {
    final pointer = _activeDragPointer;
    if (pointer == null) return;
    GestureBinding.instance.pointerRouter.removeRoute(
      pointer,
      _handleTrackedPointerEvent,
    );
    _activeDragPointer = null;
  }

  void _onHandleDragEnd() {
    _removeTrackedPointerRoute();
    _edgeScrollTimer?.cancel();
    _edgeScrollTimer = null;
    _dragOverlay?.remove();
    _dragOverlay = null;

    final draggedId = _draggingRoomId;
    if (draggedId == null) {
      setState(() {
        _dragSourcePage = null;
        _hoverPage = null;
        _hoverIndex = null;
        _dragCardSize = Size.zero;
      });
      _edgeScrollReferencePosition = Offset.zero;
      return;
    }

    final pageProvider = context.read<RoomPageProvider>();
    final currentPage = _hoverPage ?? _resolvedDropPage(pageProvider);
    final dropIndex = _hoverIndex ??
        _computeDropIndex(
          pageProvider: pageProvider,
          pageIndex: currentPage,
          draggedId: draggedId,
        );
    _edgeScrollTargetPage = null;

    final fromPage = pageProvider.getPage(draggedId);
    final existingRooms =
        pageProvider.getRoomsForPage(currentPage, widget.rooms);
    final existingIndex =
        existingRooms.indexWhere((room) => room.id == draggedId);
    final didChange = fromPage != currentPage || existingIndex != dropIndex;
    if (fromPage != currentPage) {
      pageProvider.moveRoom(draggedId, currentPage, insertIndex: dropIndex);
    } else {
      pageProvider.reorderInPage(draggedId, currentPage, dropIndex);
    }
    if (didChange) {
      AnalyticsService().logRoomLayoutChanged(
        action: fromPage == currentPage ? 'reorder' : 'move_page',
        fromPage: fromPage,
        toPage: currentPage,
        toIndex: dropIndex,
        roomCount: widget.rooms.length,
        pageCount: math.max(1, pageProvider.pageCount - 1),
      );
    }

    HapticFeedback.selectionClick();
    setState(() {
      _draggingRoomId = null;
      _dragSourcePage = null;
      _hoverPage = null;
      _hoverIndex = null;
      _dragCardSize = Size.zero;
    });
    _edgeScrollReferencePosition = Offset.zero;
  }

  void _checkEdgeScroll(Offset globalPosition) {
    final screenWidth = MediaQuery.of(context).size.width;
    final dx = globalPosition.dx;
    final pageProvider = context.read<RoomPageProvider>();
    final currentPage = _resolvedDropPage(pageProvider);
    final horizontalIntent = dx - _edgeScrollReferencePosition.dx;

    if (dx < _edgeScrollZone &&
        horizontalIntent <= -_edgeScrollIntentThreshold &&
        currentPage > 0) {
      _startEdgeScroll(-1, pageProvider.pageCount);
    } else if (dx > screenWidth - _edgeScrollZone &&
        horizontalIntent >= _edgeScrollIntentThreshold &&
        currentPage < pageProvider.pageCount - 1) {
      _startEdgeScroll(1, pageProvider.pageCount);
    } else {
      _edgeScrollTimer?.cancel();
      _edgeScrollTimer = null;
      if (!_isEdgeScrollAnimating) {
        _edgeScrollTargetPage = null;
      }
    }
  }

  void _startEdgeScroll(int direction, int pageCount) {
    if (_edgeScrollTimer != null || _isEdgeScrollAnimating) return;
    _edgeScrollTimer = Timer(_edgeScrollDelay, () async {
      _edgeScrollTimer = null;
      final pageProvider = context.read<RoomPageProvider>();
      final currentPage = _resolvedDropPage(pageProvider);
      final targetPage = (currentPage + direction).clamp(0, pageCount - 1);
      if (targetPage != currentPage) {
        _edgeScrollTargetPage = targetPage;
        _isEdgeScrollAnimating = true;
        _updateHoverTarget();
        try {
          await widget.pageController.animateToPage(
            targetPage,
            duration: const Duration(milliseconds: 300),
            curve: Curves.easeInOut,
          );
        } finally {
          _isEdgeScrollAnimating = false;
          if (_edgeScrollTargetPage == targetPage) {
            _edgeScrollTargetPage = null;
          }
        }
        if (!mounted) return;
        _edgeScrollReferencePosition = _dragPosition;
        _checkEdgeScroll(_dragPosition);
        _updateHoverTarget();
      }
    });
  }

  int _resolvedDropPage(RoomPageProvider pageProvider) {
    if (_edgeScrollTargetPage != null) return _edgeScrollTargetPage!;
    if (!widget.pageController.hasClients) {
      return widget.pageController.initialPage;
    }
    return widget.pageController.page?.round() ??
        widget.pageController.initialPage;
  }

  int _computeDropIndex({
    required RoomPageProvider pageProvider,
    required int pageIndex,
    required String draggedId,
  }) {
    final pageRooms = pageProvider
        .getRoomsForPage(pageIndex, widget.rooms)
        .where((room) => room.id != draggedId)
        .toList();

    final targets = <_DropTarget>[];
    for (var i = 0; i < pageRooms.length; i++) {
      final room = pageRooms[i];
      final key = _cardKeys[room.id];
      final box = key?.currentContext?.findRenderObject() as RenderBox?;
      if (box == null) continue;
      final topLeft = box.localToGlobal(Offset.zero);
      targets.add(
        _DropTarget(
          index: i,
          rect: topLeft & box.size,
          isHalfWidth: _isHalfWidthCard(room),
        ),
      );
    }

    if (targets.isEmpty) return pageRooms.length;

    targets.sort(_compareTargetsByPosition);
    final rows = _groupDropTargetsIntoRows(targets);
    for (final row in rows) {
      final rowTop = row.map((target) => target.rect.top).reduce(math.min);
      final rowBottom =
          row.map((target) => target.rect.bottom).reduce(math.max);

      if (_dragPosition.dy < rowTop) {
        return row.map((target) => target.index).reduce(math.min);
      }

      if (_dragPosition.dy <= rowBottom) {
        final fullWidthTarget = row.length == 1 && !row.first.isHalfWidth;
        if (fullWidthTarget) {
          final target = row.first;
          return _dragPosition.dy < target.rect.center.dy
              ? target.index
              : target.index + 1;
        }

        final orderedRow = List<_DropTarget>.from(row)
          ..sort((a, b) => a.rect.left.compareTo(b.rect.left));
        for (final target in orderedRow) {
          if (_dragPosition.dx < target.rect.center.dx) {
            return target.index;
          }
        }
        return orderedRow.last.index + 1;
      }
    }

    return pageRooms.length;
  }

  int _compareTargetsByPosition(_DropTarget a, _DropTarget b) {
    const rowTolerance = 8.0;
    final topDelta = a.rect.top - b.rect.top;
    if (topDelta.abs() > rowTolerance) {
      return topDelta.sign.toInt();
    }
    return a.rect.left.compareTo(b.rect.left);
  }

  List<List<_DropTarget>> _groupDropTargetsIntoRows(
    List<_DropTarget> targets,
  ) {
    const rowTolerance = 8.0;
    final rows = <List<_DropTarget>>[];
    for (final target in targets) {
      if (rows.isEmpty) {
        rows.add([target]);
        continue;
      }

      final row = rows.last;
      final rowTop = row.map((entry) => entry.rect.top).reduce(math.min);
      if ((target.rect.top - rowTop).abs() <= rowTolerance) {
        row.add(target);
      } else {
        rows.add([target]);
      }
    }
    return rows;
  }

  void _updateHoverTarget() {
    final draggedId = _draggingRoomId;
    if (draggedId == null) return;

    final pageProvider = context.read<RoomPageProvider>();
    final pageIndex = _resolvedDropPage(pageProvider);
    final dropIndex = _computeDropIndex(
      pageProvider: pageProvider,
      pageIndex: pageIndex,
      draggedId: draggedId,
    );

    if (pageIndex == _hoverPage && dropIndex == _hoverIndex) return;
    setState(() {
      _hoverPage = pageIndex;
      _hoverIndex = dropIndex;
    });
  }

  @override
  Widget build(BuildContext context) {
    final isLandscape =
        MediaQuery.of(context).orientation == Orientation.landscape;
    final bottomSafeArea = MediaQuery.of(context).padding.bottom;
    final bottomPad = bottomSafeArea + 104.0;
    final pageProvider = context.watch<RoomPageProvider>();
    final pageCount = pageProvider.pageCount;
    final clampedPage =
        pageCount == 0 ? 0 : _currentPage.clamp(0, pageCount - 1);

    return Stack(
      children: [
        _CelestialBackground(curveData: widget.curveData),
        SafeArea(
          bottom: false,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _buildHeader(isLandscape, pageProvider.editMode),
              if (!pageProvider.editMode) const HubConnectionBanner(),
              Expanded(
                child: PageView.builder(
                  controller: widget.pageController,
                  onPageChanged: (page) {
                    setState(() => _currentPage = page);
                    widget.onPageChanged?.call(page);
                  },
                  physics: _draggingRoomId != null
                      ? const NeverScrollableScrollPhysics()
                      : null,
                  itemCount: pageCount,
                  itemBuilder: (context, pageIndex) {
                    final pageRooms = pageProvider.getRoomsForPage(
                      pageIndex,
                      widget.rooms,
                    );
                    return _buildPageContent(
                      pageIndex: pageIndex,
                      rooms: pageRooms,
                      bottomPad: bottomPad,
                      editMode: pageProvider.editMode,
                    );
                  },
                ),
              ),
            ],
          ),
        ),
        if (pageCount > 1)
          Positioned(
            left: 0,
            right: 0,
            bottom: 12,
            child: _PageDots(
              currentPage: clampedPage,
              totalPages: pageCount,
              pageController: widget.pageController,
            ),
          ),
      ],
    );
  }

  bool _isHalfWidthCard(RoomDto room) => false;

  List<_RoomGridItem> _roomGridItems(List<RoomDto> rooms) {
    return [
      for (final room in rooms)
        _RoomGridItem.room(room, isHalfWidth: _isHalfWidthCard(room)),
    ];
  }

  List<List<_RoomGridItem>> _buildRows(List<_RoomGridItem> items) {
    final rows = <List<_RoomGridItem>>[];
    var index = 0;
    while (index < items.length) {
      final item = items[index];
      if (!item.isHalfWidth) {
        rows.add([item]);
        index++;
        continue;
      }

      final nextIndex = index + 1;
      if (nextIndex < items.length && items[nextIndex].isHalfWidth) {
        rows.add([item, items[nextIndex]]);
        index += 2;
        continue;
      }

      rows.add([item]);
      index++;
    }
    return rows;
  }

  void _enterEditMode() {
    final pageProvider = context.read<RoomPageProvider>();
    pageProvider.reconcileRooms(widget.rooms);
    AnalyticsService().logRoomLayoutEditStarted(
      roomCount: widget.rooms.length,
      pageCount: pageProvider.pageCount,
    );
    pageProvider.enterEditMode();
  }

  void _exitEditMode() {
    HapticFeedback.lightImpact();
    final pageProvider = context.read<RoomPageProvider>();
    pageProvider.exitEditMode();
    AnalyticsService().logRoomLayoutEditCompleted(
      roomCount: widget.rooms.length,
      pageCount: pageProvider.pageCount,
    );
  }

  Widget _buildEditModeDismissRegion(Widget child) {
    return GestureDetector(
      behavior: HitTestBehavior.translucent,
      onDoubleTap: _exitEditMode,
      child: child,
    );
  }

  Widget _buildRoomCard({
    required RoomDto room,
    required bool editMode,
    bool isDragging = false,
    bool collapseWhileDragging = false,
  }) {
    final key =
        editMode ? _cardKeys.putIfAbsent(room.id, () => GlobalKey()) : null;
    return Container(
      key: key,
      child: EditableRoomCard(
        key: ValueKey(room.id),
        roomId: room.id,
        globalConfig: widget.globalConfig,
        curveData: widget.curveData,
        editMode: editMode,
        onEnterEditMode: editMode ? () {} : _enterEditMode,
        isDragging: isDragging,
        collapseWhileDragging: collapseWhileDragging,
        onDragStart: editMode ? _onHandleDragStart : null,
      ),
    );
  }

  Widget _buildGridItem({
    required _RoomGridItem item,
    required bool editMode,
  }) {
    if (item.isPlaceholder) {
      return _buildDropPlaceholder();
    }

    final room = item.room!;
    final isDraggedRoom =
        editMode && room.id == _draggingRoomId && _dragSourcePage != null;
    return _buildRoomCard(
      room: room,
      editMode: editMode,
      isDragging: isDraggedRoom,
      collapseWhileDragging: isDraggedRoom,
    );
  }

  Widget _buildRoomRowsList({
    required List<RoomDto> rooms,
    required double bottomPad,
    required bool editMode,
  }) {
    return _buildRoomGridList(
      items: _roomGridItems(rooms),
      bottomPad: bottomPad,
      editMode: editMode,
    );
  }

  Widget _buildRoomGridList({
    required List<_RoomGridItem> items,
    required double bottomPad,
    required bool editMode,
  }) {
    final rows = _buildRows(items);
    return ListView.builder(
      padding: EdgeInsets.fromLTRB(16, 4, 16, bottomPad),
      itemCount: rows.length,
      itemBuilder: (context, index) {
        final row = rows[index];
        if (row.length == 1 && !row.first.isHalfWidth) {
          return Padding(
            padding: const EdgeInsets.only(bottom: 12),
            child: _buildGridItem(
              item: row.first,
              editMode: editMode,
            ),
          );
        }

        return Padding(
          padding: const EdgeInsets.only(bottom: 12),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              for (var i = 0; i < 2; i++) ...[
                if (i > 0) const SizedBox(width: 12),
                Expanded(
                  child: i < row.length
                      ? _buildGridItem(
                          item: row[i],
                          editMode: editMode,
                        )
                      : const SizedBox.shrink(),
                ),
              ],
            ],
          ),
        );
      },
    );
  }

  Widget _buildPageContent({
    required int pageIndex,
    required List<RoomDto> rooms,
    required double bottomPad,
    required bool editMode,
  }) {
    if (rooms.isEmpty && editMode && pageIndex != _hoverPage) {
      return _buildEditModeDismissRegion(
        Center(
          child: Text(
            'Drag rooms here',
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.5),
              fontSize: 16,
              fontWeight: FontWeight.w500,
            ),
          ),
        ),
      );
    }

    if (rooms.isEmpty && !editMode) return const SizedBox.shrink();

    if (!editMode) {
      return RefreshIndicator(
        onRefresh: _onRefresh,
        color: CelestialColors.accentBlue,
        backgroundColor: const Color(0xFF1A2E45),
        child: _buildRoomRowsList(
          rooms: rooms,
          bottomPad: bottomPad,
          editMode: false,
        ),
      );
    }

    if (_draggingRoomId == null) {
      return _buildEditModeDismissRegion(
        _buildRoomRowsList(
          rooms: rooms,
          bottomPad: bottomPad,
          editMode: true,
        ),
      );
    }

    final visibleRooms = rooms
        .where((room) => room.id != _draggingRoomId)
        .toList(growable: false);
    final nonDraggedCount = visibleRooms.length;
    final placeholderIndex = pageIndex == _hoverPage
        ? (_hoverIndex ?? nonDraggedCount).clamp(0, nonDraggedCount)
        : null;

    final draggedRoom = _roomById(_draggingRoomId);
    final items = <_RoomGridItem>[];
    for (var i = 0; i <= visibleRooms.length; i++) {
      if (placeholderIndex != null && i == placeholderIndex) {
        items.add(
          _RoomGridItem.placeholder(
            isHalfWidth:
                draggedRoom == null ? true : _isHalfWidthCard(draggedRoom),
          ),
        );
      }
      if (i < visibleRooms.length) {
        final room = visibleRooms[i];
        items
            .add(_RoomGridItem.room(room, isHalfWidth: _isHalfWidthCard(room)));
      }
    }

    return _buildEditModeDismissRegion(
      _buildRoomGridList(
        items: items,
        bottomPad: bottomPad,
        editMode: true,
      ),
    );
  }

  RoomDto? _roomById(String? roomId) {
    if (roomId == null) return null;
    for (final room in widget.rooms) {
      if (room.id == roomId) return room;
    }
    return null;
  }

  Widget _buildDropPlaceholder() {
    final height = _dragCardSize.height > 0 ? _dragCardSize.height : 112.0;
    return AnimatedContainer(
      duration: const Duration(milliseconds: 160),
      curve: Curves.easeOut,
      height: height,
      decoration: BoxDecoration(
        color: CelestialColors.accentBlue.withValues(alpha: 0.08),
        borderRadius: BorderRadius.circular(20),
        border: Border.all(
          color: CelestialColors.accentBlue.withValues(alpha: 0.45),
          width: 1.5,
        ),
      ),
      child: Center(
        child: Icon(
          Icons.add_rounded,
          color: CelestialColors.accentBlue.withValues(alpha: 0.7),
          size: 24,
        ),
      ),
    );
  }

  Widget _buildHeader(bool isLandscape, bool editMode) {
    final vPad = _headerVerticalPadding(isLandscape);

    if (editMode) {
      return Padding(
        padding: EdgeInsets.fromLTRB(20, vPad, 20, vPad),
        child: Row(
          children: [
            Expanded(
              child: Text(
                'Edit Rooms',
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: isLandscape ? 16 : 20,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 0.3,
                ),
              ),
            ),
            GestureDetector(
              onTap: _exitEditMode,
              child: Container(
                padding:
                    const EdgeInsets.symmetric(horizontal: 16, vertical: 6),
                decoration: BoxDecoration(
                  color: CelestialColors.accentBlue.withValues(alpha: 0.2),
                  borderRadius: BorderRadius.circular(16),
                ),
                child: Text(
                  'Done',
                  style: TextStyle(
                    color: CelestialColors.accentBlue,
                    fontSize: 14,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ),
          ],
        ),
      );
    }

    return Padding(
      padding: EdgeInsets.fromLTRB(20, vPad, 20, vPad),
      child: Column(
        children: [
          Row(
            children: [
              _HomeChooserButton(onTap: widget.onHomeChooserTap),
              const SizedBox(width: 8),
              Expanded(child: _buildGlobalActionDock()),
              const SizedBox(width: 8),
              _SunButton(onTap: () => SunPositionScreen.show(context)),
            ],
          ),
          AnimatedSize(
            duration: const Duration(milliseconds: 220),
            curve: Curves.easeOutCubic,
            alignment: Alignment.topCenter,
            child: _globalSliderExpanded
                ? Padding(
                    padding: const EdgeInsets.only(top: 8),
                    child: _buildGlobalBrightnessPanel(),
                  )
                : const SizedBox.shrink(),
          ),
        ],
      ),
    );
  }

  List<_GlobalRoomTarget> _eligibleGlobalRoomTargets() {
    final roomProvider = context.read<RoomProvider>();
    final provisional = <_GlobalRoomTarget>[];

    for (final listedRoom in widget.rooms) {
      final room = roomProvider.getRoom(listedRoom.id) ?? listedRoom;
      final isLightNode = room.kind == RoomNodeKind.room ||
          room.kind == RoomNodeKind.lightDevice;
      final state = roomProvider.getDisplayRoomState(room.id);
      final isAdaptiveOn = state == RoomModeState.active ||
          state == RoomModeState.wake ||
          state == RoomModeState.warning;
      if (!isLightNode ||
          room.disabled ||
          !room.rhythmEnabled ||
          !room.lightsOn ||
          !isAdaptiveOn ||
          roomProvider.isRoomTransitioning(room.id) ||
          roomProvider.isNodeDispatchPending(room.id)) {
        continue;
      }
      provisional.add(
        _GlobalRoomTarget(
          room: room,
          brightness:
              (roomProvider.getBrightness(room.id) ?? 50).clamp(1, 100).toInt(),
        ),
      );
    }

    final parentIds = provisional
        .where((target) => target.room.kind == RoomNodeKind.room)
        .map((target) => target.room.id)
        .toSet();
    return provisional
        .where(
          (target) =>
              target.room.kind != RoomNodeKind.lightDevice ||
              !parentIds.contains(target.room.parentId),
        )
        .toList(growable: false);
  }

  int _completedDispatchCount(
    RhythmDispatchResult? result,
    List<_GlobalRoomTarget> attempted,
  ) {
    if (result == null) return 0;
    final metadataCount = result.dispatchCount;
    if (metadataCount != null) {
      return metadataCount.clamp(0, attempted.length).toInt();
    }
    if (result.states.isEmpty) return 0;
    final attemptedIds = attempted.map((target) => target.room.id).toSet();
    return result.states
        .map((state) => state.nodeId)
        .where(attemptedIds.contains)
        .toSet()
        .length;
  }

  void _showGlobalProgress(String message) {
    final messenger = ScaffoldMessenger.of(context);
    messenger.hideCurrentSnackBar();
    messenger.showSnackBar(
      SnackBar(
        duration: const Duration(seconds: 30),
        content: Row(
          key: const ValueKey('global-room-action-progress'),
          children: [
            const SizedBox(
              width: 16,
              height: 16,
              child: CircularProgressIndicator(strokeWidth: 2),
            ),
            const SizedBox(width: 12),
            Text(message),
          ],
        ),
      ),
    );
  }

  void _showGlobalResult({
    required String verb,
    required int completed,
    required int eligible,
    bool offerUndo = true,
  }) {
    final messenger = ScaffoldMessenger.of(context);
    messenger.hideCurrentSnackBar();
    messenger.showSnackBar(
      SnackBar(
        duration: const Duration(seconds: 8),
        persist: false,
        content: Text('$verb $completed of $eligible rooms.'),
        action: offerUndo && completed > 0 && _globalUndoSnapshot != null
            ? SnackBarAction(
                key: const ValueKey('global-room-action-undo'),
                label: 'UNDO',
                textColor: const Color(0xFFFFD166),
                onPressed: _scheduleUndoGlobalRoomAction,
              )
            : null,
      ),
    );
  }

  void _showNoEligibleRooms() {
    final messenger = ScaffoldMessenger.of(context);
    messenger.hideCurrentSnackBar();
    messenger.showSnackBar(
      const SnackBar(
        content: Text('No adaptive rooms are currently on.'),
      ),
    );
  }

  Future<void> _runGlobalRoomAction(_GlobalRoomAction action) async {
    if (_globalActionPending) return;
    final journeyId = 'global-room-${_uuid.v4()}';
    final analyticsAction = switch (action) {
      _GlobalRoomAction.soften => 'soften',
      _GlobalRoomAction.boost => 'boost',
      _GlobalRoomAction.reset => 'reset',
    };
    final eligible = _eligibleGlobalRoomTargets();
    if (eligible.isEmpty) {
      AnalyticsService().logGlobalRoomActionCompleted(
        journeyId: journeyId,
        action: analyticsAction,
        eligibleCount: 0,
        attemptedCount: 0,
        completedCount: 0,
        outcome: 'no_eligible_rooms',
      );
      _showNoEligibleRooms();
      return;
    }

    final serverSync = context.read<ServerSyncProvider>();
    final dispatchable = eligible
        .where((target) => serverSync.isRoomHubConnected(target.room.source))
        .toList(growable: false);
    final actionName = switch (action) {
      _GlobalRoomAction.soften => 'Softening',
      _GlobalRoomAction.boost => 'Boosting',
      _GlobalRoomAction.reset => 'Resetting',
    };
    final resultVerb = action == _GlobalRoomAction.reset ? 'Reset' : 'Adjusted';

    HapticFeedback.lightImpact();
    setState(() => _globalActionPending = true);
    _showGlobalProgress('$actionName ${eligible.length} rooms…');

    RhythmDispatchResult? result;
    if (dispatchable.isNotEmpty) {
      final wireAction = switch (action) {
        _GlobalRoomAction.soften => 'step_down',
        _GlobalRoomAction.boost => 'step_up',
        _GlobalRoomAction.reset => 'reset',
      };
      result = await serverSync.dispatchBatchNodeActionsResult([
        for (final target in dispatchable)
          (nodeId: target.room.id, action: wireAction),
      ], correlationId: journeyId);
    }
    if (!mounted) return;

    final completed = _completedDispatchCount(result, dispatchable);
    setState(() {
      _globalActionPending = false;
      _globalSliderValue = null;
      _globalUndoSnapshot = completed == 0
          ? null
          : [
              for (final target in dispatchable.take(completed))
                _GlobalRoomBrightness(
                  nodeId: target.room.id,
                  brightness: target.brightness,
                ),
            ];
    });
    _showGlobalResult(
      verb: resultVerb,
      completed: completed,
      eligible: eligible.length,
    );
    AnalyticsService().logGlobalRoomActionCompleted(
      journeyId: journeyId,
      action: analyticsAction,
      eligibleCount: eligible.length,
      attemptedCount: dispatchable.length,
      completedCount: completed,
      outcome: _globalRoomActionOutcome(
        eligibleCount: eligible.length,
        attemptedCount: dispatchable.length,
        completedCount: completed,
      ),
    );
  }

  Future<void> _setGlobalBrightness(double value) async {
    if (_globalActionPending) return;
    final journeyId = 'global-room-${_uuid.v4()}';
    final eligible = _eligibleGlobalRoomTargets();
    if (eligible.isEmpty) {
      AnalyticsService().logGlobalRoomActionCompleted(
        journeyId: journeyId,
        action: 'set_brightness',
        eligibleCount: 0,
        attemptedCount: 0,
        completedCount: 0,
        outcome: 'no_eligible_rooms',
      );
      _showNoEligibleRooms();
      return;
    }

    final serverSync = context.read<ServerSyncProvider>();
    final dispatchable = eligible
        .where((target) => serverSync.isRoomHubConnected(target.room.source))
        .toList(growable: false);
    final brightness = value.round().clamp(1, 100).toInt();

    HapticFeedback.selectionClick();
    setState(() => _globalActionPending = true);
    _showGlobalProgress('Setting ${eligible.length} rooms to $brightness%…');

    RhythmDispatchResult? result;
    if (dispatchable.isNotEmpty) {
      result = await serverSync.dispatchBatchNodeCurveBrightnessResult([
        for (final target in dispatchable)
          (nodeId: target.room.id, brightness: brightness),
      ], correlationId: journeyId);
    }
    if (!mounted) return;

    final completed = _completedDispatchCount(result, dispatchable);
    setState(() {
      _globalActionPending = false;
      _globalSliderValue = brightness.toDouble();
      _globalUndoSnapshot = completed == 0
          ? null
          : [
              for (final target in dispatchable.take(completed))
                _GlobalRoomBrightness(
                  nodeId: target.room.id,
                  brightness: target.brightness,
                ),
            ];
    });
    _showGlobalResult(
      verb: 'Adjusted',
      completed: completed,
      eligible: eligible.length,
    );
    AnalyticsService().logGlobalRoomActionCompleted(
      journeyId: journeyId,
      action: 'set_brightness',
      eligibleCount: eligible.length,
      attemptedCount: dispatchable.length,
      completedCount: completed,
      outcome: _globalRoomActionOutcome(
        eligibleCount: eligible.length,
        attemptedCount: dispatchable.length,
        completedCount: completed,
      ),
    );
  }

  Future<void> _undoGlobalRoomAction() async {
    if (_globalActionPending || _globalUndoSnapshot == null) return;
    final journeyId = 'global-room-${_uuid.v4()}';
    final snapshot = List<_GlobalRoomBrightness>.of(_globalUndoSnapshot!);
    final currentlyEligible = {
      for (final target in _eligibleGlobalRoomTargets()) target.room.id: target,
    };
    final serverSync = context.read<ServerSyncProvider>();
    final restorable = snapshot
        .where(
          (saved) =>
              currentlyEligible.containsKey(saved.nodeId) &&
              serverSync.isRoomHubConnected(
                currentlyEligible[saved.nodeId]!.room.source,
              ),
        )
        .toList(growable: false);

    if (restorable.isEmpty) {
      _globalUndoSnapshot = null;
      AnalyticsService().logGlobalRoomActionCompleted(
        journeyId: journeyId,
        action: 'undo',
        eligibleCount: snapshot.length,
        attemptedCount: 0,
        completedCount: 0,
        outcome: 'no_restorable_rooms',
      );
      _showNoEligibleRooms();
      return;
    }

    setState(() {
      _globalActionPending = true;
      _globalUndoSnapshot = null;
    });
    _showGlobalProgress('Restoring ${restorable.length} rooms…');
    final result = await serverSync.dispatchBatchNodeCurveBrightnessResult([
      for (final saved in restorable)
        (nodeId: saved.nodeId, brightness: saved.brightness),
    ], correlationId: journeyId);
    if (!mounted) return;

    final attemptedTargets = [
      for (final saved in restorable) currentlyEligible[saved.nodeId]!,
    ];
    final completed = _completedDispatchCount(result, attemptedTargets);
    setState(() {
      _globalActionPending = false;
      _globalSliderValue = null;
    });
    _showGlobalResult(
      verb: 'Restored',
      completed: completed,
      eligible: snapshot.length,
      offerUndo: false,
    );
    AnalyticsService().logGlobalRoomActionCompleted(
      journeyId: journeyId,
      action: 'undo',
      eligibleCount: snapshot.length,
      attemptedCount: restorable.length,
      completedCount: completed,
      outcome: _globalRoomActionOutcome(
        eligibleCount: snapshot.length,
        attemptedCount: restorable.length,
        completedCount: completed,
      ),
    );
  }

  void _scheduleUndoGlobalRoomAction() {
    // SnackBarAction dismisses its parent after invoking onPressed. Let that
    // exit animation finish so it cannot also dismiss Undo's progress/result.
    Future<void>.delayed(const Duration(milliseconds: 260), () {
      if (mounted) _undoGlobalRoomAction();
    });
  }

  void _toggleGlobalBrightnessPanel() {
    if (_globalActionPending) return;
    HapticFeedback.selectionClick();
    setState(() {
      _globalSliderExpanded = !_globalSliderExpanded;
      if (!_globalSliderExpanded) _globalSliderValue = null;
    });
  }

  Widget _buildGlobalActionDock() {
    final enabled = !_globalActionPending;
    return Container(
      key: const ValueKey('global-room-action-dock'),
      height: _headerControlHeight,
      clipBehavior: Clip.antiAlias,
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard.withValues(alpha: 0.88),
        borderRadius: BorderRadius.circular(20),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.82),
        ),
        boxShadow: [
          BoxShadow(
            color: Colors.black.withValues(alpha: 0.16),
            blurRadius: 12,
            offset: const Offset(0, 4),
          ),
        ],
      ),
      child: Row(
        children: [
          Expanded(
            child: _GlobalActionButton(
              key: const ValueKey('global-room-action-dim'),
              icon: Icons.brightness_low_rounded,
              label: 'Soften',
              enabled: enabled,
              onTap: () => _runGlobalRoomAction(_GlobalRoomAction.soften),
            ),
          ),
          const _GlobalActionDivider(),
          Expanded(
            child: _GlobalActionButton(
              key: const ValueKey('global-room-action-bright'),
              icon: Icons.brightness_high_rounded,
              label: 'Boost',
              enabled: enabled,
              onTap: () => _runGlobalRoomAction(_GlobalRoomAction.boost),
            ),
          ),
          const _GlobalActionDivider(),
          Expanded(
            child: _GlobalActionButton(
              key: const ValueKey('global-room-action-reset'),
              icon: Icons.restart_alt_rounded,
              label: 'Reset',
              enabled: enabled,
              onTap: () => _runGlobalRoomAction(_GlobalRoomAction.reset),
            ),
          ),
          const _GlobalActionDivider(),
          SizedBox(
            width: 36,
            child: _GlobalActionButton(
              key: const ValueKey('global-room-action-expand'),
              icon: _globalSliderExpanded
                  ? Icons.expand_less_rounded
                  : Icons.tune_rounded,
              label: _globalSliderExpanded
                  ? 'Hide exact brightness'
                  : 'Set exact brightness',
              showLabel: false,
              enabled: enabled,
              selected: _globalSliderExpanded,
              onTap: _toggleGlobalBrightnessPanel,
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildGlobalBrightnessPanel() {
    context.watch<RoomProvider>();
    final targets = _eligibleGlobalRoomTargets();
    final average = targets.isEmpty
        ? 50.0
        : targets
                .map((target) => target.brightness)
                .reduce((left, right) => left + right) /
            targets.length;
    final value = (_globalSliderValue ?? average).clamp(1.0, 100.0).toDouble();
    final enabled = !_globalActionPending && targets.isNotEmpty;

    return Container(
      key: const ValueKey('global-room-slider-panel'),
      padding: const EdgeInsets.fromLTRB(14, 8, 14, 8),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard.withValues(alpha: 0.92),
        borderRadius: BorderRadius.circular(18),
        border: Border.all(
          color: CelestialColors.accentBlue.withValues(alpha: 0.26),
        ),
      ),
      child: Row(
        children: [
          const Icon(
            Icons.wb_sunny_rounded,
            size: 18,
            color: Color(0xFFFFC857),
          ),
          const SizedBox(width: 8),
          Expanded(
            child: SliderTheme(
              data: SliderTheme.of(context).copyWith(
                trackHeight: 6,
                activeTrackColor: const Color(0xFFFFC857),
                inactiveTrackColor:
                    CelestialColors.orbitRing.withValues(alpha: 0.55),
                thumbColor: const Color(0xFFFFD978),
                overlayColor: const Color(0xFFFFC857).withValues(alpha: 0.16),
              ),
              child: Slider(
                key: const ValueKey('global-room-brightness-slider'),
                value: value,
                min: 1,
                max: 100,
                onChanged: enabled
                    ? (next) => setState(() => _globalSliderValue = next)
                    : null,
                onChangeEnd: enabled ? _setGlobalBrightness : null,
              ),
            ),
          ),
          SizedBox(
            width: 42,
            child: Text(
              '${value.round()}%',
              textAlign: TextAlign.end,
              style: TextStyle(
                color: enabled
                    ? CelestialColors.textPrimary
                    : CelestialColors.textSecondary.withValues(alpha: 0.6),
                fontSize: 13,
                fontWeight: FontWeight.w700,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

String _globalRoomActionOutcome({
  required int eligibleCount,
  required int attemptedCount,
  required int completedCount,
}) {
  if (attemptedCount == 0) return 'unavailable';
  if (completedCount == 0) return 'failed';
  if (completedCount < eligibleCount) return 'partial';
  return 'succeeded';
}

enum _GlobalRoomAction { soften, boost, reset }

class _GlobalRoomTarget {
  const _GlobalRoomTarget({
    required this.room,
    required this.brightness,
  });

  final RoomDto room;
  final int brightness;
}

class _GlobalRoomBrightness {
  const _GlobalRoomBrightness({
    required this.nodeId,
    required this.brightness,
  });

  final String nodeId;
  final int brightness;
}

class _GlobalActionDivider extends StatelessWidget {
  const _GlobalActionDivider();

  @override
  Widget build(BuildContext context) {
    return Container(
      width: 1,
      height: 20,
      color: CelestialColors.orbitRing.withValues(alpha: 0.55),
    );
  }
}

class _GlobalActionButton extends StatelessWidget {
  const _GlobalActionButton({
    super.key,
    required this.icon,
    required this.label,
    required this.enabled,
    required this.onTap,
    this.showLabel = true,
    this.selected = false,
  });

  final IconData icon;
  final String label;
  final bool enabled;
  final VoidCallback onTap;
  final bool showLabel;
  final bool selected;

  @override
  Widget build(BuildContext context) {
    final color = enabled
        ? selected
            ? CelestialColors.accentBlue
            : CelestialColors.textPrimary
        : CelestialColors.textSecondary.withValues(alpha: 0.38);
    return Semantics(
      button: true,
      enabled: enabled,
      selected: selected,
      label: label,
      child: Tooltip(
        message: label,
        child: InkWell(
          onTap: enabled ? onTap : null,
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 160),
            color: selected
                ? CelestialColors.accentBlue.withValues(alpha: 0.14)
                : Colors.transparent,
            alignment: Alignment.center,
            padding: EdgeInsets.symmetric(horizontal: showLabel ? 5 : 0),
            child: Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Icon(icon, size: 16, color: color),
                if (showLabel) ...[
                  const SizedBox(width: 3),
                  Flexible(
                    child: Text(
                      label,
                      overflow: TextOverflow.fade,
                      softWrap: false,
                      style: TextStyle(
                        color: color,
                        fontSize: 10.5,
                        fontWeight: FontWeight.w700,
                        letterSpacing: -0.15,
                      ),
                    ),
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// Circular Home action button, sized to match the header's other 34-px
/// controls (the sun orb beside it).
class _HomeChooserButton extends StatelessWidget {
  const _HomeChooserButton({required this.onTap});

  final VoidCallback? onTap;

  static const _teal = Color(0xFF00BCD4);

  @override
  Widget build(BuildContext context) {
    return Semantics(
      button: true,
      label: 'Choose Home',
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: onTap == null
            ? null
            : () {
                HapticFeedback.lightImpact();
                onTap!();
              },
        child: Container(
          width: 34,
          height: 34,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: CelestialColors.backgroundCard.withValues(alpha: 0.85),
            border: Border.all(
              color: _teal.withValues(alpha: 0.35),
              width: 1,
            ),
          ),
          child: const Icon(
            Icons.home_rounded,
            color: _teal,
            size: 20,
          ),
        ),
      ),
    );
  }
}

/// Warm sun orb in the header, sitting next to the Home button. Opens the
/// [SunPositionScreen] celestial visualization. Sized to match the 34-px header
/// controls but keeps the sun's amber gradient so it still reads as "the sun".
class _SunButton extends StatelessWidget {
  const _SunButton({required this.onTap});

  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    const warm = Color(0xFFFFB74D);
    const deep = Color(0xFFE6892E);
    return Semantics(
      button: true,
      label: 'Open sun position',
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: () {
          HapticFeedback.selectionClick();
          onTap();
        },
        child: Container(
          width: 34,
          height: 34,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            gradient: const LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: [warm, deep],
            ),
            boxShadow: [
              BoxShadow(
                color: warm.withValues(alpha: 0.4),
                blurRadius: 12,
                spreadRadius: -2,
              ),
            ],
            border: Border.all(
              color: Colors.white.withValues(alpha: 0.18),
            ),
          ),
          child: const Icon(
            Icons.wb_sunny_rounded,
            color: Colors.white,
            size: 18,
          ),
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Sky phase system — continuous daylight transitions
// ---------------------------------------------------------------------------

/// Represents a sky gradient at a specific phase of the day.
class _SkyPalette {
  /// Top-to-bottom gradient colors.
  final List<Color> colors;

  /// Gradient stops (must match colors length).
  final List<double> stops;

  /// Star visibility (0 = invisible, 1 = full).
  final double starOpacity;

  /// Sun/horizon glow intensity.
  final double glowIntensity;

  /// Color of the horizon glow.
  final Color glowColor;

  /// Vertical position of glow center (0 = top, 1 = bottom).
  final double glowY;

  const _SkyPalette({
    required this.colors,
    required this.stops,
    this.starOpacity = 0.0,
    this.glowIntensity = 0.0,
    this.glowColor = const Color(0x00000000),
    this.glowY = 0.3,
  });

  /// Linearly interpolate between two palettes.
  static _SkyPalette lerp(_SkyPalette a, _SkyPalette b, double t) {
    final clampedT = t.clamp(0.0, 1.0);
    // Interpolate colors (both must have same length — we normalize to 4 stops)
    final colors = <Color>[];
    final stops = <double>[];
    final count = math.min(a.colors.length, b.colors.length);
    for (int i = 0; i < count; i++) {
      colors.add(Color.lerp(a.colors[i], b.colors[i], clampedT)!);
      stops.add(a.stops[i] + (b.stops[i] - a.stops[i]) * clampedT);
    }
    return _SkyPalette(
      colors: colors,
      stops: stops,
      starOpacity: a.starOpacity + (b.starOpacity - a.starOpacity) * clampedT,
      glowIntensity:
          a.glowIntensity + (b.glowIntensity - a.glowIntensity) * clampedT,
      glowColor: Color.lerp(a.glowColor, b.glowColor, clampedT)!,
      glowY: a.glowY + (b.glowY - a.glowY) * clampedT,
    );
  }
}

/// All sky palettes keyed by phase of day.
class _SkyPalettes {
  // Deep night — inky blue-black, stars at full
  static const deepNight = _SkyPalette(
    colors: [
      Color(0xFF06080D), // Near-black zenith
      Color(0xFF0B0F18), // Deep indigo
      Color(0xFF0D1220), // Faint navy
      Color(0xFF0F1525), // Horizon hint
    ],
    stops: [0.0, 0.35, 0.7, 1.0],
    starOpacity: 1.0,
  );

  // Astronomical twilight — first hint of indigo at horizon
  static const astronomicalTwilight = _SkyPalette(
    colors: [
      Color(0xFF080C16), // Still very dark zenith
      Color(0xFF0E1428), // Deep indigo
      Color(0xFF162040), // Navy wash
      Color(0xFF1C2A52), // Indigo-blue horizon
    ],
    stops: [0.0, 0.3, 0.65, 1.0],
    starOpacity: 0.75,
    glowIntensity: 0.02,
    glowColor: Color(0xFF2A3B6E),
    glowY: 0.95,
  );

  // Nautical twilight — steel-navy sky, horizon warming
  static const nauticalTwilight = _SkyPalette(
    colors: [
      Color(0xFF0E1524), // Dark steel zenith
      Color(0xFF182840), // Steel navy
      Color(0xFF253A58), // Warming navy
      Color(0xFF3B4D6E), // Dusty blue horizon
    ],
    stops: [0.0, 0.3, 0.65, 1.0],
    starOpacity: 0.35,
    glowIntensity: 0.06,
    glowColor: Color(0xFF5A6B8E),
    glowY: 0.9,
  );

  // Civil twilight — rose-peach at horizon, stars fading
  static const civilTwilight = _SkyPalette(
    colors: [
      Color(0xFF162338), // Muted navy zenith
      Color(0xFF263A52), // Slate blue
      Color(0xFF4A5468), // Warm grey
      Color(0xFF7A6258), // Dusty rose horizon
    ],
    stops: [0.0, 0.3, 0.6, 1.0],
    starOpacity: 0.08,
    glowIntensity: 0.15,
    glowColor: Color(0xFFD4956A),
    glowY: 0.85,
  );

  // Golden sunrise — warm burst at horizon
  static const goldenSunrise = _SkyPalette(
    colors: [
      Color(0xFF1E3048), // Deep blue zenith
      Color(0xFF3A5068), // Steel-blue mid
      Color(0xFF6E6858), // Warm grey-gold
      Color(0xFFC48A50), // Amber-gold horizon
    ],
    stops: [0.0, 0.25, 0.6, 1.0],
    starOpacity: 0.0,
    glowIntensity: 0.30,
    glowColor: Color(0xFFE8A54B),
    glowY: 0.8,
  );

  // Morning — bright, clear sky settling in
  static const morning = _SkyPalette(
    colors: [
      Color(0xFF1A2E45), // Clean deep blue zenith
      Color(0xFF2A4260), // Open blue
      Color(0xFF3A556F), // Soft mid-blue
      Color(0xFF4A647A), // Pale horizon
    ],
    stops: [0.0, 0.3, 0.65, 1.0],
    starOpacity: 0.0,
    glowIntensity: 0.08,
    glowColor: Color(0xFFFFE4B5),
    glowY: 0.4,
  );

  // Midday — brightest, most open sky
  static const midday = _SkyPalette(
    colors: [
      Color(0xFF1C3550), // Rich blue zenith
      Color(0xFF284A68), // Strong blue
      Color(0xFF355D78), // Open blue
      Color(0xFF4A7088), // Luminous horizon
    ],
    stops: [0.0, 0.3, 0.65, 1.0],
    starOpacity: 0.0,
    glowIntensity: 0.12,
    glowColor: Color(0xFFE0D8C8),
    glowY: 0.2,
  );

  // Afternoon — slightly warmer, sun descending
  static const afternoon = _SkyPalette(
    colors: [
      Color(0xFF1A3048), // Deep blue zenith
      Color(0xFF2A4560), // Warm blue
      Color(0xFF3D5870), // Warming mid
      Color(0xFF506878), // Soft warm horizon
    ],
    stops: [0.0, 0.3, 0.65, 1.0],
    starOpacity: 0.0,
    glowIntensity: 0.10,
    glowColor: Color(0xFFEEC890),
    glowY: 0.35,
  );

  // Golden hour — rich warm light
  static const goldenHour = _SkyPalette(
    colors: [
      Color(0xFF1E2E42), // Darkening blue zenith
      Color(0xFF3A4858), // Muted blue-grey
      Color(0xFF6A5848), // Warm amber-grey
      Color(0xFFBE7A3E), // Rich amber horizon
    ],
    stops: [0.0, 0.25, 0.6, 1.0],
    starOpacity: 0.0,
    glowIntensity: 0.28,
    glowColor: Color(0xFFD4843A),
    glowY: 0.8,
  );

  // Sunset — dramatic crimson-amber
  static const sunset = _SkyPalette(
    colors: [
      Color(0xFF1A2236), // Deepening blue zenith
      Color(0xFF2E3448), // Purple-navy
      Color(0xFF6E4840), // Warm crimson-brown
      Color(0xFFC86030), // Burning orange horizon
    ],
    stops: [0.0, 0.25, 0.6, 1.0],
    starOpacity: 0.0,
    glowIntensity: 0.35,
    glowColor: Color(0xFFE0603A),
    glowY: 0.85,
  );

  // Civil dusk — violet-rose afterglow
  static const civilDusk = _SkyPalette(
    colors: [
      Color(0xFF141C30), // Deep navy zenith
      Color(0xFF24304A), // Purple-navy
      Color(0xFF4A4058), // Mauve
      Color(0xFF7A5858), // Dusty rose horizon
    ],
    stops: [0.0, 0.3, 0.6, 1.0],
    starOpacity: 0.10,
    glowIntensity: 0.12,
    glowColor: Color(0xFFA06858),
    glowY: 0.88,
  );

  // Nautical dusk — stars emerging, deep blue
  static const nauticalDusk = _SkyPalette(
    colors: [
      Color(0xFF0C1420), // Dark zenith
      Color(0xFF162236), // Deep steel
      Color(0xFF203050), // Navy
      Color(0xFF2A3A5A), // Cool blue horizon
    ],
    stops: [0.0, 0.3, 0.65, 1.0],
    starOpacity: 0.40,
    glowIntensity: 0.04,
    glowColor: Color(0xFF4A5878),
    glowY: 0.92,
  );

  // Astronomical dusk — nearly full dark
  static const astronomicalDusk = _SkyPalette(
    colors: [
      Color(0xFF080C16), // Near-black
      Color(0xFF0E1428), // Deep indigo
      Color(0xFF162040), // Navy wash
      Color(0xFF1C2A52), // Indigo horizon
    ],
    stops: [0.0, 0.3, 0.65, 1.0],
    starOpacity: 0.80,
    glowIntensity: 0.01,
    glowColor: Color(0xFF2A3560),
    glowY: 0.95,
  );
}

/// Computes the interpolated sky palette for the current time using solar data.
_SkyPalette _computeSkyPalette(double currentHour, CurveData? curveData) {
  // Extract solar times with sensible fallbacks
  final sunrise = curveData?.solar.sunrise ?? 6.5;
  final sunset = curveData?.solar.sunset ?? 18.0;
  final solarNoon = curveData?.solar.solarNoon ?? 12.0;

  // Twilight phases (fallback to offsets from sunrise/sunset)
  final dawnCivil = curveData?.solar.dawn?.civil ?? (sunrise - 0.5);
  final dawnNautical = curveData?.solar.dawn?.nautical ?? (sunrise - 1.0);
  final dawnAstro = curveData?.solar.dawn?.astronomical ?? (sunrise - 1.5);

  final duskCivil = curveData?.solar.dusk?.civil ?? (sunset + 0.5);
  final duskNautical = curveData?.solar.dusk?.nautical ?? (sunset + 1.0);
  final duskAstro = curveData?.solar.dusk?.astronomical ?? (sunset + 1.5);

  // Derived transition points
  final goldenMorningEnd = sunrise + 0.5; // 30 min after sunrise
  final morningEnd = sunrise + 1.5; // settling into day
  final afternoonStart = solarNoon + 1.5; // past peak
  final goldenEveStart = sunset - 1.0; // golden hour begins

  // Build ordered phase timeline
  // Each entry: (hour, palette)
  final phases = <(double, _SkyPalette)>[
    (dawnAstro, _SkyPalettes.deepNight),
    (dawnNautical, _SkyPalettes.astronomicalTwilight),
    (dawnCivil, _SkyPalettes.nauticalTwilight),
    (sunrise, _SkyPalettes.civilTwilight),
    (goldenMorningEnd, _SkyPalettes.goldenSunrise),
    (morningEnd, _SkyPalettes.morning),
    (solarNoon, _SkyPalettes.midday),
    (afternoonStart, _SkyPalettes.afternoon),
    (goldenEveStart, _SkyPalettes.goldenHour),
    (sunset, _SkyPalettes.sunset),
    (duskCivil, _SkyPalettes.civilDusk),
    (duskNautical, _SkyPalettes.nauticalDusk),
    (duskAstro, _SkyPalettes.astronomicalDusk),
    (duskAstro + 0.01, _SkyPalettes.deepNight), // snaps to night
  ];

  // Before first phase → deep night
  if (currentHour <= phases.first.$1) {
    return _SkyPalettes.deepNight;
  }
  // After last phase → deep night
  if (currentHour >= phases.last.$1) {
    return _SkyPalettes.deepNight;
  }

  // Find which two phases we're between
  for (int i = 0; i < phases.length - 1; i++) {
    final (startHour, startPalette) = phases[i];
    final (endHour, endPalette) = phases[i + 1];
    if (currentHour >= startHour && currentHour < endHour) {
      final t = (currentHour - startHour) / (endHour - startHour);
      return _SkyPalette.lerp(startPalette, endPalette, t);
    }
  }

  return _SkyPalettes.deepNight;
}

/// Celestial background with continuous daylight transitions.
///
/// Smoothly blends through: deep night → astronomical dawn → nautical twilight
/// → civil twilight → sunrise → morning → midday → afternoon → golden hour
/// → sunset → dusk phases → night. Uses actual solar/twilight times from
/// CurveData when available.
class _CelestialBackground extends StatefulWidget {
  final CurveData? curveData;

  const _CelestialBackground({this.curveData});

  @override
  State<_CelestialBackground> createState() => _CelestialBackgroundState();
}

class _CelestialBackgroundState extends State<_CelestialBackground>
    with SingleTickerProviderStateMixin {
  late AnimationController _twinkleController;
  late List<_Star> _stars;
  Timer? _timeCheckTimer;
  _SkyPalette _palette = _SkyPalettes.deepNight;

  @override
  void initState() {
    super.initState();
    _twinkleController = AnimationController(
      duration: const Duration(seconds: 10),
      vsync: this,
    )..repeat();

    // Generate random stars
    final random = math.Random(42);
    _stars = List.generate(60, (i) {
      return _Star(
        x: random.nextDouble(),
        y: random.nextDouble(),
        size: 0.5 + random.nextDouble() * 1.5,
        twinkleOffset: random.nextDouble() * 2 * math.pi,
        twinkleSpeed: 0.5 + random.nextDouble() * 1.5,
      );
    });

    _updateSkyPalette();
    // Re-compute every minute for smooth real-time transitions
    _timeCheckTimer = Timer.periodic(const Duration(minutes: 1), (_) {
      _updateSkyPalette();
    });
  }

  @override
  void didUpdateWidget(_CelestialBackground oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.curveData != oldWidget.curveData) {
      _updateSkyPalette();
    }
  }

  void _updateSkyPalette() {
    final now = DateTime.now();
    final currentHour = now.hour + now.minute / 60.0;
    final newPalette = _computeSkyPalette(currentHour, widget.curveData);
    if (mounted) {
      setState(() {
        _palette = newPalette;
      });
    }
  }

  @override
  void dispose() {
    _twinkleController.dispose();
    _timeCheckTimer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedContainer(
      duration: const Duration(milliseconds: 1200),
      curve: Curves.easeInOut,
      decoration: BoxDecoration(
        gradient: LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: _palette.colors,
          stops: _palette.stops,
        ),
      ),
      child: AnimatedBuilder(
        animation: _twinkleController,
        builder: (context, child) {
          return CustomPaint(
            painter: _CelestialPainter(
              stars: _stars,
              animationValue: _twinkleController.value,
              palette: _palette,
            ),
            size: Size.infinite,
          );
        },
      ),
    );
  }
}

class _Star {
  final double x;
  final double y;
  final double size;
  final double twinkleOffset;
  final double twinkleSpeed;

  _Star({
    required this.x,
    required this.y,
    required this.size,
    required this.twinkleOffset,
    required this.twinkleSpeed,
  });
}

class _CelestialPainter extends CustomPainter {
  final List<_Star> stars;
  final double animationValue;
  final _SkyPalette palette;

  _CelestialPainter({
    required this.stars,
    required this.animationValue,
    required this.palette,
  });

  @override
  void paint(Canvas canvas, Size size) {
    // Stars — opacity controlled by palette
    if (palette.starOpacity > 0.01) {
      _paintStars(canvas, size);
    }

    // Horizon / sun glow
    if (palette.glowIntensity > 0.005) {
      _paintGlow(canvas, size);
    }
  }

  void _paintStars(Canvas canvas, Size size) {
    final paint = Paint()..style = PaintingStyle.fill;

    for (final star in stars) {
      final twinkle = math.sin(
        animationValue * 2 * math.pi * star.twinkleSpeed + star.twinkleOffset,
      );
      // Base twinkle range scaled by palette's star opacity
      final opacity = (0.3 + (twinkle + 1) / 2 * 0.5) * palette.starOpacity;

      paint.color = CelestialColors.textPrimary.withValues(alpha: opacity);

      final x = star.x * size.width;
      final y = star.y * size.height;

      canvas.drawCircle(Offset(x, y), star.size, paint);
    }
  }

  void _paintGlow(Canvas canvas, Size size) {
    final paint = Paint()..style = PaintingStyle.fill;

    final glowCenter = Offset(size.width * 0.5, size.height * palette.glowY);
    final glowRadius = size.width * 1.2;

    // Subtle breathing animation on the glow
    final breathe = 0.85 + math.sin(animationValue * 2 * math.pi * 0.3) * 0.15;
    final intensity = palette.glowIntensity * breathe;

    paint.shader = RadialGradient(
      colors: [
        palette.glowColor.withValues(alpha: intensity),
        palette.glowColor.withValues(alpha: intensity * 0.4),
        palette.glowColor.withValues(alpha: 0),
      ],
      stops: const [0.0, 0.35, 1.0],
    ).createShader(Rect.fromCircle(center: glowCenter, radius: glowRadius));

    canvas.drawCircle(glowCenter, glowRadius, paint);
  }

  @override
  bool shouldRepaint(covariant _CelestialPainter oldDelegate) {
    return animationValue != oldDelegate.animationValue ||
        !identical(palette, oldDelegate.palette);
  }
}

/// Floating page indicator dots, rendered just above the global bottom nav
/// over the celestial background — no opaque strip, no own background color.
class _PageDots extends StatelessWidget {
  final int currentPage;
  final int totalPages;
  final PageController pageController;

  const _PageDots({
    required this.currentPage,
    required this.totalPages,
    required this.pageController,
  });

  @override
  Widget build(BuildContext context) {
    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: List.generate(totalPages, (index) {
        final isActive = index == currentPage;
        return GestureDetector(
          onTap: () {
            pageController.animateToPage(
              index,
              duration: const Duration(milliseconds: 300),
              curve: Curves.easeInOut,
            );
          },
          behavior: HitTestBehavior.opaque,
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 4),
            child: AnimatedContainer(
              duration: const Duration(milliseconds: 200),
              width: isActive ? 8 : 6,
              height: isActive ? 8 : 6,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: isActive
                    ? CelestialColors.textPrimary
                    : CelestialColors.textSecondary.withValues(alpha: 0.4),
              ),
            ),
          ),
        );
      }),
    );
  }
}
