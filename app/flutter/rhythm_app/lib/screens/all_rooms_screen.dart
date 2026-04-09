import 'dart:math' as math;
import 'dart:async';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmMode;
import '../providers/room_page_provider.dart';
import '../providers/server_sync_provider.dart';
import '../widgets/editable_room_card.dart';
import '../widgets/room_card.dart';
import '../widgets/hub_connection_banner.dart';
import '../widgets/solar_orbit.dart'; // For CelestialColors

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
  final ValueChanged<int> onPageChanged;
  final RhythmMode? activeMode;
  final ValueChanged<RhythmMode>? onModeSelected;

  const AllRoomsScreen({
    super.key,
    required this.rooms,
    required this.globalConfig,
    this.curveData,
    required this.pageController,
    required this.onPageChanged,
    this.activeMode,
    this.onModeSelected,
  });

  @override
  State<AllRoomsScreen> createState() => _AllRoomsScreenState();
}

class _AllRoomsScreenState extends State<AllRoomsScreen> {
  Timer? _edgeScrollTimer;
  static const _edgeScrollZone = 28.0;
  static const _edgeScrollIntentThreshold = 16.0;
  static const _edgeScrollDelay = Duration(milliseconds: 375);

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

  @override
  void dispose() {
    _edgeScrollTimer?.cancel();
    _removeTrackedPointerRoute();
    _dragOverlay?.remove();
    super.dispose();
  }

  Future<void> _onRefresh() async {
    final serverSync = context.read<ServerSyncProvider>();
    await serverSync.fullRefresh();
  }

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

    // Find the card's render box to compute the offset from touch to card origin
    final key = _cardKeys[roomId];
    final box = key?.currentContext?.findRenderObject() as RenderBox?;
    if (box != null) {
      final cardTopLeft = box.localToGlobal(Offset.zero);
      _dragStartOffset = globalPosition - cardTopLeft;
      _dragCardSize = box.size;
    } else {
      _dragStartOffset = const Offset(0, 40);
      _dragCardSize = Size(MediaQuery.of(context).size.width - 32, 112);
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
    if (fromPage != currentPage) {
      pageProvider.moveRoom(draggedId, currentPage, insertIndex: dropIndex);
    } else {
      pageProvider.reorderInPage(draggedId, currentPage, dropIndex);
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

    for (var i = 0; i < pageRooms.length; i++) {
      final room = pageRooms[i];
      final key = _cardKeys[room.id];
      final box = key?.currentContext?.findRenderObject() as RenderBox?;
      if (box == null) continue;
      final cardTop = box.localToGlobal(Offset.zero).dy;
      final cardHeight = box.size.height;
      if (_dragPosition.dy < cardTop + cardHeight / 2) {
        return i;
      }
    }

    return pageRooms.length;
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
                child: Selector<ServerSyncProvider, bool>(
                  selector: (_, p) => p.powerSave,
                  builder: (context, powerSave, _) {
                    return PageView.builder(
                      controller: widget.pageController,
                      onPageChanged: widget.onPageChanged,
                      physics: _draggingRoomId != null
                          ? const NeverScrollableScrollPhysics()
                          : null,
                      itemCount: pageProvider.pageCount,
                      itemBuilder: (context, pageIndex) {
                        final pageRooms = pageProvider.getRoomsForPage(
                          pageIndex,
                          widget.rooms,
                        );
                        return _buildPageContent(
                          pageIndex: pageIndex,
                          rooms: pageRooms,
                          powerSave: powerSave,
                          bottomPad: bottomPad,
                          editMode: pageProvider.editMode,
                        );
                      },
                    );
                  },
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }

  Widget _buildPageContent({
    required int pageIndex,
    required List<RoomDto> rooms,
    required bool powerSave,
    required double bottomPad,
    required bool editMode,
  }) {
    if (rooms.isEmpty && editMode && pageIndex != _hoverPage) {
      return Center(
        child: Text(
          'Drag rooms here',
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.5),
            fontSize: 16,
            fontWeight: FontWeight.w500,
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
        child: ListView.builder(
          padding: EdgeInsets.fromLTRB(16, 4, 16, bottomPad),
          itemCount: rooms.length,
          itemBuilder: (context, index) {
            final room = rooms[index];
            return Padding(
              padding: const EdgeInsets.only(bottom: 12),
              child: EditableRoomCard(
                key: ValueKey(room.id),
                roomId: room.id,
                globalConfig: widget.globalConfig,
                curveData: widget.curveData,
                powerSave: powerSave,
                editMode: false,
                onEnterEditMode: () {
                  final pageProvider = context.read<RoomPageProvider>();
                  pageProvider.reconcileRooms(widget.rooms);
                  pageProvider.enterEditMode();
                },
              ),
            );
          },
        ),
      );
    }

    // Edit mode: keep the dragged card mounted so its gesture stream survives,
    // but collapse it in-place while a separate placeholder marks the drop slot.
    final draggedIndex = _draggingRoomId == null
        ? -1
        : rooms.indexWhere((room) => room.id == _draggingRoomId);
    final nonDraggedCount =
        draggedIndex == -1 ? rooms.length : rooms.length - 1;
    final rawPlaceholderIndex = pageIndex == _hoverPage
        ? (_hoverIndex ?? nonDraggedCount).clamp(0, nonDraggedCount)
        : null;
    final placeholderIndex = rawPlaceholderIndex == null
        ? null
        : draggedIndex != -1 && rawPlaceholderIndex > draggedIndex
            ? rawPlaceholderIndex + 1
            : rawPlaceholderIndex;

    return ListView.builder(
      padding: EdgeInsets.fromLTRB(16, 4, 16, bottomPad),
      itemCount: rooms.length + (placeholderIndex == null ? 0 : 1),
      itemBuilder: (context, index) {
        if (placeholderIndex != null && index == placeholderIndex) {
          return Padding(
            padding: const EdgeInsets.only(bottom: 12),
            child: _buildDropPlaceholder(),
          );
        }

        final roomIndex = placeholderIndex != null && index > placeholderIndex
            ? index - 1
            : index;
        final room = rooms[roomIndex];
        final key = _cardKeys.putIfAbsent(room.id, () => GlobalKey());
        final isDraggedRoom =
            room.id == _draggingRoomId && pageIndex == _dragSourcePage;
        return Padding(
          key: key,
          padding: const EdgeInsets.only(bottom: 12),
          child: EditableRoomCard(
            roomId: room.id,
            globalConfig: widget.globalConfig,
            curveData: widget.curveData,
            powerSave: powerSave,
            editMode: true,
            onEnterEditMode: () {},
            isDragging: isDraggedRoom,
            collapseWhileDragging: isDraggedRoom,
            onDragStart: _onHandleDragStart,
          ),
        );
      },
    );
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
    final vPad = isLandscape ? 4.0 : 10.0;

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
              onTap: () {
                HapticFeedback.lightImpact();
                context.read<RoomPageProvider>().exitEditMode();
              },
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
      child: Row(
        children: [
          Text(
            'Rooms',
            style: TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: isLandscape ? 16 : 20,
              fontWeight: FontWeight.w600,
              letterSpacing: 0.3,
            ),
          ),
          const Spacer(),
          if (widget.activeMode != null)
            _CurveProfileToggle(
              activeMode: widget.activeMode!,
              onModeSelected: widget.onModeSelected,
            ),
        ],
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

// ─── Curve Profile Toggle ──────────────────────────────────────────────

/// Moon glow color — matches celestial palette.
const _moonGlow = Color(0xFF7C8EBF);

/// Animated segmented toggle for switching between day and sleep modes.
class _CurveProfileToggle extends StatelessWidget {
  final RhythmMode activeMode;
  final ValueChanged<RhythmMode>? onModeSelected;

  const _CurveProfileToggle({
    required this.activeMode,
    this.onModeSelected,
  });

  static const _duration = Duration(milliseconds: 350);

  static const _modeVisuals = <RhythmMode, (String, IconData, Color)>{
    RhythmMode.day: ('Day', Icons.wb_sunny_rounded, CelestialColors.sunWarm),
    RhythmMode.sleep: ('Sleep', Icons.nightlight_round, _moonGlow),
  };

  @override
  Widget build(BuildContext context) {
    return Container(
      height: 34,
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(17),
        color: CelestialColors.backgroundCard.withValues(alpha: 0.85),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.25),
          width: 1,
        ),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (final mode in RhythmMode.values)
            _buildSegment(
              label: _modeVisuals[mode]!.$1,
              icon: _modeVisuals[mode]!.$2,
              isActive: mode == activeMode,
              activeColor: _modeVisuals[mode]!.$3,
              onTap: () => onModeSelected?.call(mode),
            ),
        ],
      ),
    );
  }

  Widget _buildSegment({
    required String label,
    required IconData icon,
    required bool isActive,
    required Color activeColor,
    required VoidCallback onTap,
  }) {
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: () {
        if (!isActive) {
          HapticFeedback.lightImpact();
          onTap();
        }
      },
      child: AnimatedContainer(
        duration: _duration,
        curve: Curves.easeInOut,
        margin: const EdgeInsets.all(3),
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 2),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          color: isActive
              ? activeColor.withValues(alpha: 0.15)
              : Colors.transparent,
          border: Border.all(
            color: isActive
                ? activeColor.withValues(alpha: 0.25)
                : Colors.transparent,
            width: 0.5,
          ),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            AnimatedSwitcher(
              duration: _duration,
              child: Icon(
                icon,
                key: ValueKey(isActive),
                size: 14,
                color: isActive
                    ? activeColor
                    : CelestialColors.textSecondary.withValues(alpha: 0.4),
              ),
            ),
            const SizedBox(width: 5),
            AnimatedDefaultTextStyle(
              duration: _duration,
              style: TextStyle(
                color: isActive
                    ? activeColor
                    : CelestialColors.textSecondary.withValues(alpha: 0.4),
                fontSize: 13,
                fontWeight: isActive ? FontWeight.w600 : FontWeight.w400,
                letterSpacing: 0.2,
              ),
              child: Text(label),
            ),
          ],
        ),
      ),
    );
  }
}
