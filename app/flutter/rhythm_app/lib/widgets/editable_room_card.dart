import 'dart:math' as math;
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'room_card.dart';

/// Wraps a [RoomCard] with edit-mode behavior: wiggle animation and drag support.
///
/// In normal mode, a long-press triggers edit mode entry (normal taps pass through).
/// In edit mode, the card wiggles and shows a drag handle. Touch the handle to
/// pick up the card and drag it to reorder or move between pages.
class EditableRoomCard extends StatefulWidget {
  final String roomId;
  final CurveConfigDto globalConfig;
  final CurveData? curveData;
  final bool powerSave;
  final bool editMode;
  final VoidCallback onEnterEditMode;

  /// Called when the user starts dragging this card via the handle.
  /// Parent should use this to create a drag overlay.
  final void Function(String roomId, int pointer, Offset globalPosition)?
      onDragStart;

  /// Called as the user moves the dragged card.
  final void Function(Offset globalPosition)? onDragUpdate;

  /// Called when the user releases the dragged card.
  final void Function()? onDragEnd;

  /// Whether this card is currently being dragged.
  final bool isDragging;
  final bool collapseWhileDragging;

  const EditableRoomCard({
    super.key,
    required this.roomId,
    required this.globalConfig,
    this.curveData,
    this.powerSave = false,
    required this.editMode,
    required this.onEnterEditMode,
    this.onDragStart,
    this.onDragUpdate,
    this.onDragEnd,
    this.isDragging = false,
    this.collapseWhileDragging = false,
  });

  @override
  State<EditableRoomCard> createState() => _EditableRoomCardState();
}

class _EditableRoomCardState extends State<EditableRoomCard>
    with SingleTickerProviderStateMixin {
  late AnimationController _wiggleController;
  late double _phaseOffset;
  int? _dragPointerId;

  @override
  void initState() {
    super.initState();
    _phaseOffset = (widget.roomId.hashCode % 1000) / 1000.0 * 2 * math.pi;
    _wiggleController = AnimationController(
      duration: const Duration(milliseconds: 300),
      vsync: this,
    );
    if (widget.editMode) {
      _wiggleController.repeat(reverse: true);
    }
  }

  @override
  void didUpdateWidget(EditableRoomCard oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.editMode && !oldWidget.editMode) {
      _wiggleController.repeat(reverse: true);
    } else if (!widget.editMode && oldWidget.editMode) {
      _wiggleController.stop();
      _wiggleController.reset();
    }
  }

  @override
  void dispose() {
    _wiggleController.dispose();
    super.dispose();
  }

  Widget _buildCard() {
    return RoomCard(
      key: ValueKey(widget.roomId),
      roomId: widget.roomId,
      globalConfig: widget.globalConfig,
      curveData: widget.curveData,
      powerSave: widget.powerSave,
    );
  }

  @override
  Widget build(BuildContext context) {
    if (!widget.editMode) {
      return GestureDetector(
        onLongPress: () {
          HapticFeedback.heavyImpact();
          widget.onEnterEditMode();
        },
        behavior: HitTestBehavior.translucent,
        child: _buildCard(),
      );
    }

    // Edit mode: wiggle + full-card drag target.
    final card = AnimatedBuilder(
      animation: _wiggleController,
      builder: (context, child) {
        final angle = math.sin(
              _wiggleController.value * 2 * math.pi + _phaseOffset,
            ) *
            0.012;
        return Transform.rotate(angle: angle, child: child);
      },
      child: Opacity(
        opacity: widget.isDragging ? 0.25 : 1.0,
        child: Stack(
          children: [
            IgnorePointer(child: _buildCard()),
            // The whole card acts as the drag hit target in edit mode. The
            // card itself is the affordance, so no separate handle is needed.
            Positioned.fill(
              child: Listener(
                onPointerDown: (event) {
                  _dragPointerId = event.pointer;
                },
                onPointerCancel: (_) {
                  _dragPointerId = null;
                },
                onPointerUp: (_) {
                  _dragPointerId = null;
                },
                child: RawGestureDetector(
                  behavior: HitTestBehavior.opaque,
                  gestures: <Type, GestureRecognizerFactory>{
                    _EagerPanRecognizer: GestureRecognizerFactoryWithHandlers<
                        _EagerPanRecognizer>(
                      () => _EagerPanRecognizer(),
                      (recognizer) {
                        recognizer.onStart = (details) {
                          HapticFeedback.mediumImpact();
                          final pointer = _dragPointerId;
                          if (pointer != null) {
                            widget.onDragStart?.call(
                              widget.roomId,
                              pointer,
                              details.globalPosition,
                            );
                          }
                        };
                      },
                    ),
                  },
                  child: const SizedBox.expand(),
                ),
              ),
            ),
          ],
        ),
      ),
    );

    final shouldCollapse = widget.isDragging && widget.collapseWhileDragging;
    return IgnorePointer(
      ignoring: shouldCollapse,
      child: ClipRect(
        child: Align(
          alignment: Alignment.topCenter,
          heightFactor: shouldCollapse ? 0.001 : 1.0,
          child: card,
        ),
      ),
    );
  }
}

/// Pan recognizer that immediately wins the gesture arena.
///
/// Prevents the parent [ListView] scroll from claiming drag gestures that
/// start on a card, while still reporting both horizontal and
/// vertical movement for cross-page dragging.
class _EagerPanRecognizer extends PanGestureRecognizer {
  @override
  void addAllowedPointer(PointerDownEvent event) {
    super.addAllowedPointer(event);
    resolve(GestureDisposition.accepted);
  }
}
