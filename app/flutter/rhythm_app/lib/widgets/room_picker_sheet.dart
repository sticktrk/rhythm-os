import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../providers/server_sync_provider.dart';
import 'solar_orbit.dart';

const _createRoomSelection = '__create_room__';
const _unassignedSelection = '__unassigned__';

class RoomPickerOption {
  const RoomPickerOption({
    required this.id,
    required this.name,
    this.subtitle,
  });

  final String id;
  final String name;
  final String? subtitle;
}

Future<RoomPickerOption?> createTopologyRoomOptionFromPrompt(
  BuildContext context,
) async {
  final roomName = await promptForRoomName(context);
  final trimmedName = roomName?.trim() ?? '';
  if (trimmedName.isEmpty || !context.mounted) return null;

  final result = await context
      .read<ServerSyncProvider>()
      .api
      .createTopologyRoom(trimmedName);
  final roomId = result?['id'] as String?;
  if (roomId != null && roomId.isNotEmpty) {
    return RoomPickerOption(
      id: roomId,
      name: result?['name'] as String? ?? trimmedName,
    );
  }

  if (context.mounted) {
    ScaffoldMessenger.of(context).showSnackBar(
      const SnackBar(content: Text('Room creation failed')),
    );
  }
  return null;
}

Future<String?> promptForRoomName(BuildContext context) async {
  final controller = TextEditingController();
  return showDialog<String>(
    context: context,
    builder: (dialogContext) => AlertDialog(
      backgroundColor: CelestialColors.backgroundCard,
      title: const Text(
        'Create Room',
        style: TextStyle(color: CelestialColors.textPrimary),
      ),
      content: TextField(
        controller: controller,
        autofocus: true,
        style: const TextStyle(color: CelestialColors.textPrimary),
        decoration: InputDecoration(
          hintText: 'Room name',
          hintStyle: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.6),
          ),
        ),
        onSubmitted: (value) => Navigator.of(dialogContext).pop(value.trim()),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(dialogContext).pop(),
          child: Text(
            'Cancel',
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.8),
            ),
          ),
        ),
        TextButton(
          onPressed: () =>
              Navigator.of(dialogContext).pop(controller.text.trim()),
          child: const Text(
            'Create',
            style: TextStyle(color: CelestialColors.sunWarm),
          ),
        ),
      ],
    ),
  );
}

Future<String?> showRoomPickerSheet(
  BuildContext context, {
  required String title,
  required List<RoomPickerOption> rooms,
  String? currentRoomId,
  bool allowUnassigned = false,
  String unassignedLabel = 'Unassigned',
  String? unassignedSubtitle,
  bool allowCreateRoom = false,
  String createRoomLabel = 'Create New Room',
  Future<String?> Function()? onCreateRoom,
  String? emptyMessage,
  String? noOptionsMessage,
}) async {
  final normalizedCurrentRoomId =
      currentRoomId == null || currentRoomId.isEmpty ? null : currentRoomId;
  final visibleRooms =
      rooms.where((room) => room.id != normalizedCurrentRoomId).toList();

  if (visibleRooms.isEmpty && !allowUnassigned && !allowCreateRoom) {
    if (noOptionsMessage != null) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text(noOptionsMessage)),
      );
    }
    return null;
  }

  final selection = await showModalBottomSheet<String>(
    context: context,
    isScrollControlled: true,
    backgroundColor: Colors.transparent,
    builder: (sheetContext) {
      final mediaQuery = MediaQuery.of(sheetContext);
      return SafeArea(
        top: false,
        child: Container(
          constraints: BoxConstraints(
            maxHeight: mediaQuery.size.height * 0.75,
          ),
          clipBehavior: Clip.antiAlias,
          decoration: const BoxDecoration(
            color: CelestialColors.backgroundCard,
            borderRadius: BorderRadius.vertical(top: Radius.circular(24)),
          ),
          child: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                Padding(
                  padding: const EdgeInsets.all(16),
                  child: Text(
                    title,
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 16,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
                if (visibleRooms.isEmpty && emptyMessage != null)
                  Padding(
                    padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
                    child: Text(
                      emptyMessage,
                      style: TextStyle(
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.7),
                        fontSize: 13,
                      ),
                    ),
                  ),
                if (allowUnassigned)
                  ListTile(
                    leading: Icon(
                      Icons.lightbulb_outline,
                      color: CelestialColors.sunWarm.withValues(alpha: 0.9),
                    ),
                    title: Text(
                      unassignedLabel,
                      style: const TextStyle(
                        color: CelestialColors.textPrimary,
                      ),
                    ),
                    subtitle: unassignedSubtitle == null
                        ? null
                        : Text(
                            unassignedSubtitle,
                            style: TextStyle(
                              color: CelestialColors.textSecondary
                                  .withValues(alpha: 0.7),
                              fontSize: 12,
                            ),
                          ),
                    onTap: () =>
                        Navigator.of(sheetContext).pop(_unassignedSelection),
                  ),
                for (final room in visibleRooms)
                  ListTile(
                    leading: const Icon(
                      Icons.meeting_room_rounded,
                      color: Color(0xFFFFC107),
                    ),
                    title: Text(
                      room.name,
                      style:
                          const TextStyle(color: CelestialColors.textPrimary),
                    ),
                    subtitle: room.subtitle == null
                        ? null
                        : Text(
                            room.subtitle!,
                            style: TextStyle(
                              color: CelestialColors.textSecondary
                                  .withValues(alpha: 0.7),
                              fontSize: 12,
                            ),
                          ),
                    onTap: () => Navigator.of(sheetContext).pop(room.id),
                  ),
                if (allowCreateRoom)
                  ListTile(
                    leading: const Icon(
                      Icons.add_circle_outline,
                      color: Color(0xFF81C784),
                    ),
                    title: Text(
                      createRoomLabel,
                      style: const TextStyle(
                        color: CelestialColors.textPrimary,
                      ),
                    ),
                    onTap: () =>
                        Navigator.of(sheetContext).pop(_createRoomSelection),
                  ),
                SizedBox(height: mediaQuery.padding.bottom + 16),
              ],
            ),
          ),
        ),
      );
    },
  );

  if (selection == _createRoomSelection) {
    return await onCreateRoom?.call();
  }
  if (selection == _unassignedSelection) {
    return '';
  }
  return selection;
}

Future<Set<String>?> showMultiRoomPickerSheet(
  BuildContext context, {
  required String title,
  required List<RoomPickerOption> rooms,
  required Set<String> selectedRoomIds,
  String description = 'Choose one or more rooms.',
  String noOptionsMessage = 'No rooms available',
}) async {
  if (rooms.isEmpty) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text(noOptionsMessage)),
    );
    return null;
  }

  final selected = Set<String>.of(selectedRoomIds);
  return showModalBottomSheet<Set<String>>(
    context: context,
    isScrollControlled: true,
    backgroundColor: Colors.transparent,
    builder: (sheetContext) {
      final mediaQuery = MediaQuery.of(sheetContext);
      return StatefulBuilder(
        builder: (context, setSheetState) => SafeArea(
          top: false,
          child: Container(
            constraints: BoxConstraints(
              maxHeight: mediaQuery.size.height * 0.75,
            ),
            clipBehavior: Clip.antiAlias,
            decoration: const BoxDecoration(
              color: CelestialColors.backgroundCard,
              borderRadius: BorderRadius.vertical(top: Radius.circular(24)),
            ),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                Padding(
                  padding: const EdgeInsets.fromLTRB(20, 16, 20, 8),
                  child: Column(
                    children: [
                      Text(
                        title,
                        style: const TextStyle(
                          color: CelestialColors.textPrimary,
                          fontSize: 16,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                      const SizedBox(height: 4),
                      Text(
                        description,
                        textAlign: TextAlign.center,
                        style: TextStyle(
                          color: CelestialColors.textSecondary
                              .withValues(alpha: 0.7),
                          fontSize: 12,
                        ),
                      ),
                    ],
                  ),
                ),
                Flexible(
                  child: ListView.builder(
                    shrinkWrap: true,
                    padding: EdgeInsets.zero,
                    itemCount: rooms.length,
                    itemBuilder: (context, index) {
                      final room = rooms[index];
                      return CheckboxListTile(
                        value: selected.contains(room.id),
                        onChanged: (checked) {
                          setSheetState(() {
                            if (checked == true) {
                              selected.add(room.id);
                            } else {
                              selected.remove(room.id);
                            }
                          });
                        },
                        controlAffinity: ListTileControlAffinity.leading,
                        activeColor: CelestialColors.sunWarm,
                        checkColor: CelestialColors.backgroundDark,
                        title: Text(
                          room.name,
                          style: const TextStyle(
                            color: CelestialColors.textPrimary,
                          ),
                        ),
                        subtitle: room.subtitle == null
                            ? null
                            : Text(
                                room.subtitle!,
                                style: TextStyle(
                                  color: CelestialColors.textSecondary
                                      .withValues(alpha: 0.7),
                                  fontSize: 12,
                                ),
                              ),
                      );
                    },
                  ),
                ),
                Padding(
                  padding: EdgeInsets.fromLTRB(
                    16,
                    8,
                    16,
                    mediaQuery.padding.bottom + 12,
                  ),
                  child: Row(
                    children: [
                      Expanded(
                        child: TextButton(
                          onPressed: () => Navigator.of(sheetContext).pop(),
                          child: const Text('CANCEL'),
                        ),
                      ),
                      const SizedBox(width: 12),
                      Expanded(
                        child: ElevatedButton(
                          onPressed: selected.isEmpty
                              ? null
                              : () => Navigator.of(sheetContext).pop(
                                    Set<String>.of(selected),
                                  ),
                          style: ElevatedButton.styleFrom(
                            backgroundColor: CelestialColors.sunWarm,
                            foregroundColor: CelestialColors.backgroundDark,
                          ),
                          child: const Text('SAVE'),
                        ),
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
      );
    },
  );
}
