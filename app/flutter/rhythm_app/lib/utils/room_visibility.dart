import 'package:rhythm_core/rhythm_core.dart';

/// Matches the homepage All Rooms selection rules.
///
/// Show topology rooms plus standalone light devices that are not assigned to a
/// parent room yet. Child bulbs and non-light devices stay hidden.
bool showsInAllRooms(RoomDto room) {
  if (room.kind.isRoom) return true;
  if (!room.kind.isLightDevice) return false;
  final parentId = room.parentId;
  return parentId == null || parentId.isEmpty;
}
