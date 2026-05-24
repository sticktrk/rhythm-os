import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import '../../../widgets/settings_row.dart';
import '../../../widgets/beta_badge.dart';
import '../../../widgets/solar_orbit.dart';
import '../../../providers/room_provider.dart';
import '../../../providers/server_sync_provider.dart';

/// Rooms section for managing synced rooms.
///
/// Features:
/// - Lists all synced rooms grouped by source
/// - Toggle to enable/disable rooms
/// - Visual indicator for disabled rooms
class RoomsSection extends StatelessWidget {
  const RoomsSection({super.key});

  @override
  Widget build(BuildContext context) {
    return Consumer<RoomProvider>(
      builder: (context, roomProvider, child) {
        if (!roomProvider.hasRooms) {
          return const SizedBox.shrink();
        }

        // Group rooms by source
        final matterRooms = roomProvider.getRoomsBySource(RoomSourceDto.matter);
        final hueRooms = roomProvider.getRoomsBySource(RoomSourceDto.hue);
        final haRooms =
            roomProvider.getRoomsBySource(RoomSourceDto.homeAssistant);
        final bridgeRooms = roomProvider.getRoomsBySource(RoomSourceDto.bridge);

        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            const SettingsSectionHeader(title: 'Rooms'),
            // Summary row in a card
            SettingsGroup(
              children: [
                SettingsRow(
                  icon: Icons.meeting_room_rounded,
                  iconColor: const Color(0xFF5C6BC0),
                  label: '${roomProvider.roomCount} rooms',
                  value: '${roomProvider.enabledRoomCount} active',
                  showChevron: false,
                ),
              ],
            ),
            const SizedBox(height: 16),
            // Matter rooms
            if (matterRooms.isNotEmpty) ...[
              _buildSourceHeader(
                'Matter',
                const Color(0xFF26A69A),
                showBetaBadge: true,
              ),
              const SizedBox(height: 8),
              _buildRoomGroup(context, roomProvider, matterRooms),
              const SizedBox(height: 16),
            ],
            // Hue rooms
            if (hueRooms.isNotEmpty) ...[
              _buildSourceHeader('Philips Hue', const Color(0xFFFFB900)),
              const SizedBox(height: 8),
              _buildRoomGroup(context, roomProvider, hueRooms),
              const SizedBox(height: 16),
            ],
            // Home Assistant rooms
            if (haRooms.isNotEmpty) ...[
              _buildSourceHeader(
                'Home Assistant',
                const Color(0xFF03A9F4),
                showBetaBadge: true,
              ),
              const SizedBox(height: 8),
              _buildRoomGroup(context, roomProvider, haRooms),
              const SizedBox(height: 16),
            ],
            // Bridge rooms
            if (bridgeRooms.isNotEmpty) ...[
              _buildSourceHeader('Bridge', const Color(0xFF4CAF50)),
              const SizedBox(height: 8),
              _buildRoomGroup(context, roomProvider, bridgeRooms),
              const SizedBox(height: 16),
            ],
          ],
        );
      },
    );
  }

  Widget _buildSourceHeader(
    String title,
    Color color, {
    bool showBetaBadge = false,
  }) {
    return Padding(
      padding: const EdgeInsets.only(left: 4),
      child: Row(
        children: [
          Container(
            width: 8,
            height: 8,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: color,
            ),
          ),
          const SizedBox(width: 8),
          if (showBetaBadge)
            BetaLabel(
              label: title,
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                fontSize: 13,
                fontWeight: FontWeight.w500,
              ),
              spacing: 6,
            )
          else
            Text(
              title,
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                fontSize: 13,
                fontWeight: FontWeight.w500,
              ),
            ),
        ],
      ),
    );
  }

  Widget _buildRoomGroup(
    BuildContext context,
    RoomProvider roomProvider,
    List<RoomDto> rooms,
  ) {
    return Container(
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(14),
      ),
      clipBehavior: Clip.antiAlias,
      child: Column(
        children: [
          for (int i = 0; i < rooms.length; i++) ...[
            _RoomRow(
              room: rooms[i],
              isEnabled: !rooms[i].disabled,
              onToggle: () {
                roomProvider.toggleDisabled(rooms[i].id);
                HapticFeedback.selectionClick();
              },
            ),
            if (i < rooms.length - 1)
              Padding(
                padding: const EdgeInsets.only(left: 62),
                child: Container(
                  height: 0.5,
                  color: CelestialColors.orbitRing.withValues(alpha: 0.2),
                ),
              ),
          ],
        ],
      ),
    );
  }
}

class _RoomRow extends StatelessWidget {
  final RoomDto room;
  final bool isEnabled;
  final VoidCallback onToggle;

  const _RoomRow({
    required this.room,
    required this.isEnabled,
    required this.onToggle,
  });

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onToggle,
      behavior: HitTestBehavior.opaque,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
        child: Row(
          children: [
            // Room icon - solid colored circle
            AnimatedOpacity(
              duration: const Duration(milliseconds: 200),
              opacity: isEnabled ? 1.0 : 0.4,
              child: Container(
                width: 32,
                height: 32,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: const Color(0xFFFFC107),
                ),
                child: const Icon(
                  Icons.meeting_room_rounded,
                  color: Color(0xFF1A1A1A),
                  size: 16,
                ),
              ),
            ),
            const SizedBox(width: 14),
            // Room name
            Expanded(
              child: Text(
                room.name,
                style: TextStyle(
                  color: isEnabled
                      ? CelestialColors.textPrimary
                      : CelestialColors.textPrimary.withValues(alpha: 0.4),
                  fontSize: 16,
                  fontWeight: FontWeight.w400,
                ),
              ),
            ),
            // Device summary or light count
            Builder(builder: (context) {
              final summary = context
                  .read<ServerSyncProvider>()
                  .deviceSummaryForRoom(room.id);
              final label = summary.isNotEmpty
                  ? summary
                  : () {
                      final count = context
                          .read<ServerSyncProvider>()
                          .lightCountForRoom(room.id);
                      return count > 0
                          ? '$count ${count == 1 ? 'light' : 'lights'}'
                          : '${room.deviceIds.length} lights';
                    }();
              return Flexible(
                child: Text(
                  label,
                  style: TextStyle(
                    color: CelestialColors.textSecondary
                        .withValues(alpha: isEnabled ? 0.6 : 0.3),
                    fontSize: 13,
                  ),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
              );
            }),
            const SizedBox(width: 12),
            // Status indicator
            AnimatedContainer(
              duration: const Duration(milliseconds: 200),
              width: 10,
              height: 10,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: isEnabled
                    ? const Color(0xFF4CAF50)
                    : CelestialColors.textSecondary.withValues(alpha: 0.3),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
