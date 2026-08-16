import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:uuid/uuid.dart';

import '../../providers/server_sync_provider.dart';
import '../../services/analytics_service.dart';
import '../../widgets/solar_orbit.dart';

/// Explicit review shown after Hue pairing and from the reconnect path before
/// Rhythm may run unattended behavior in Hue-backed rooms.
class HueAuthorityScreen extends StatefulWidget {
  const HueAuthorityScreen({
    super.key,
    required this.bridge,
    this.source = 'settings',
    this.topologySyncSupported = false,
  });

  final RhythmHueBridgeAuthority bridge;
  final String source;
  final bool topologySyncSupported;

  static Future<bool?> show(
    BuildContext context,
    RhythmHueBridgeAuthority bridge, {
    required String source,
  }) {
    final topologySyncSupported =
        context.read<ServerSyncProvider>().hueRoomTopologySyncSupported;
    return Navigator.of(context).push<bool>(
      MaterialPageRoute(
        builder: (_) => HueAuthorityScreen(
          bridge: bridge,
          source: source,
          topologySyncSupported: topologySyncSupported,
        ),
      ),
    );
  }

  @override
  State<HueAuthorityScreen> createState() => _HueAuthorityScreenState();
}

class _HueAuthorityScreenState extends State<HueAuthorityScreen> {
  late final Map<String, RhythmHueRoomAuthorityOwner> _owners;
  bool _saving = false;
  String? _error;
  late final String _journeyId;
  late bool _topologySyncEnabled;

  @override
  void initState() {
    super.initState();
    _journeyId = 'hue-authority-${const Uuid().v4()}';
    _topologySyncEnabled = widget.bridge.topologySyncEnabled;
    _owners = {
      for (final room in widget.bridge.rooms)
        room.roomId: room.owner == RhythmHueRoomAuthorityOwner.rhythm
            ? RhythmHueRoomAuthorityOwner.rhythm
            : RhythmHueRoomAuthorityOwner.hue,
    };
    AnalyticsService().logHueAuthorityReviewOpened(
      journeyId: _journeyId,
      source: widget.source,
      roomCount: widget.bridge.rooms.length,
      hadPriorReview: widget.bridge.rooms.every(
        (room) => room.owner != RhythmHueRoomAuthorityOwner.unreviewed,
      ),
    );
  }

  bool get _allRhythm =>
      _owners.isNotEmpty &&
      _owners.values.every(
        (owner) => owner == RhythmHueRoomAuthorityOwner.rhythm,
      );

  bool get _someRhythm => _owners.values.any(
        (owner) => owner == RhythmHueRoomAuthorityOwner.rhythm,
      );

  String get _topologySyncPreview {
    final rooms = widget.bridge.topologySyncRoomCount;
    final lights = widget.bridge.topologySyncLightCount;
    if (rooms == 0 || lights == 0) return '';
    return 'Preview: $lights Hue ${lights == 1 ? 'light' : 'lights'} across '
        '$rooms Rhythm ${rooms == 1 ? 'room' : 'rooms'}. ';
  }

  Future<void> _save() async {
    if (_saving) return;
    setState(() {
      _saving = true;
      _error = null;
    });
    final hueRoomCount = _owners.values
        .where((owner) => owner == RhythmHueRoomAuthorityOwner.hue)
        .length;
    final rhythmRoomCount = _owners.length - hueRoomCount;
    AnalyticsService().logHueAuthorityReviewSubmitted(
      journeyId: _journeyId,
      source: widget.source,
      roomCount: _owners.length,
      hueRoomCount: hueRoomCount,
      rhythmRoomCount: rhythmRoomCount,
      bridgeTakeoverRequested: _allRhythm,
    );
    final updated = await context.read<ServerSyncProvider>().updateHueAuthority(
          bridge: widget.bridge,
          owners: _owners,
          correlationId: _journeyId,
          topologySyncEnabled:
              widget.topologySyncSupported ? _topologySyncEnabled : null,
        );
    if (!mounted) return;
    if (updated == null) {
      AnalyticsService().logHueAuthorityReviewCompleted(
        journeyId: _journeyId,
        source: widget.source,
        outcome: 'failed',
        bridgeTakeoverRequested: _allRhythm,
        failureStage: 'server_update',
      );
      setState(() {
        _saving = false;
        _error = 'The room list changed or the bridge could not be updated. '
            'Refresh and try again.';
      });
      return;
    }
    AnalyticsService().logHueAuthorityReviewCompleted(
      journeyId: _journeyId,
      source: widget.source,
      outcome: 'succeeded',
      bridgeTakeoverRequested: _allRhythm,
    );
    Navigator.of(context).pop(true);
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      appBar: AppBar(
        backgroundColor: CelestialColors.backgroundDark,
        foregroundColor: CelestialColors.textPrimary,
        title: const Text('Hue room automation'),
      ),
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.fromLTRB(20, 12, 20, 28),
          children: [
            const Text(
              'Who should automate each room?',
              style: TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 24,
                fontWeight: FontWeight.w700,
              ),
            ),
            const SizedBox(height: 10),
            const Text(
              'Choose Hue to keep Hue schedules, motion behavior, and button '
              'automation in charge. Rhythm can still show the room, use Hue '
              'scenes, and respond to controls you send in the app.',
              style: TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 15,
                height: 1.45,
              ),
            ),
            const SizedBox(height: 20),
            for (final room in widget.bridge.rooms) ...[
              _RoomAuthorityCard(
                room: room,
                owner: _owners[room.roomId]!,
                onChanged: _saving
                    ? null
                    : (owner) => setState(() {
                          _owners[room.roomId] = owner;
                        }),
              ),
              const SizedBox(height: 12),
            ],
            Container(
              padding: const EdgeInsets.all(14),
              decoration: BoxDecoration(
                color: _allRhythm
                    ? const Color(0x33FFB900)
                    : CelestialColors.backgroundCard,
                borderRadius: BorderRadius.circular(12),
                border: Border.all(
                  color: _allRhythm
                      ? const Color(0x99FFB900)
                      : CelestialColors.textSecondary.withValues(alpha: 0.2),
                ),
              ),
              child: Text(
                _allRhythm
                    ? 'All rooms chose Rhythm. Continuing may pause Hue '
                        'automations across this bridge after Rhythm saves a '
                        'recovery record. Returning any room to Hue restores '
                        'only unchanged fields that Rhythm paused.'
                    : _someRhythm
                        ? 'Mixed choices keep Hue automations unchanged. Hue '
                            'automations can span rooms, so Rhythm automation '
                            'stays paused for this bridge in this safety '
                            'release. Manual app controls still work.'
                        : 'Hue bridge changes stay off. Rhythm observes these '
                            'rooms and accepts manual app controls, but does '
                            'not run unattended automation.',
                style: const TextStyle(
                  color: CelestialColors.textSecondary,
                  height: 1.4,
                ),
              ),
            ),
            if (widget.topologySyncSupported) ...[
              const SizedBox(height: 14),
              Material(
                color: CelestialColors.backgroundCard,
                clipBehavior: Clip.antiAlias,
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(12),
                  side: BorderSide(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.2),
                  ),
                ),
                child: SwitchListTile.adaptive(
                  value: _topologySyncEnabled,
                  onChanged: _saving
                      ? null
                      : (enabled) => setState(() {
                            _topologySyncEnabled = enabled;
                          }),
                  activeTrackColor: const Color(0xFFFFB900),
                  title: const Text(
                    'Sync Rhythm rooms to Hue',
                    style: TextStyle(color: CelestialColors.textPrimary),
                  ),
                  subtitle: Text(
                    _topologySyncPreview +
                        (_allRhythm
                            ? 'Creates explicitly owned Hue mirror rooms and moves only Hue lights. '
                                'Hue zones, scenes, accessories, and user rooms are preserved.'
                            : 'You can save this preference now, but syncing waits until every room '
                                'uses Rhythm automation. Lights keep working individually meanwhile.'),
                    style: const TextStyle(
                      color: CelestialColors.textSecondary,
                      height: 1.35,
                    ),
                  ),
                ),
              ),
              if (_topologySyncEnabled &&
                  widget.bridge.topologySyncStatus == 'attention') ...[
                const SizedBox(height: 8),
                const Text(
                  'Hue room sync needs attention. Rhythm is keeping individual bulb commands '
                  'active so lighting still works. Reconnect Hue or save again to retry; '
                  'ambiguous Hue rooms are never selected automatically.',
                  style: TextStyle(color: Colors.orangeAccent),
                ),
              ] else if (_topologySyncEnabled &&
                  widget.bridge.topologySyncStatus == 'pending') ...[
                const SizedBox(height: 8),
                const Text(
                  'Hue verification is pending. Rhythm is using individual bulb commands until '
                  'the bridge reports an exact room match.',
                  style: TextStyle(color: CelestialColors.textSecondary),
                ),
              ],
            ],
            if (_error != null) ...[
              const SizedBox(height: 14),
              Text(_error!, style: const TextStyle(color: Colors.redAccent)),
            ],
            const SizedBox(height: 22),
            FilledButton(
              onPressed: _saving ? null : _save,
              style: FilledButton.styleFrom(
                backgroundColor: const Color(0xFFFFB900),
                foregroundColor: Colors.black,
                padding: const EdgeInsets.symmetric(vertical: 16),
              ),
              child: Text(_saving ? 'Saving…' : 'Save room choices'),
            ),
          ],
        ),
      ),
    );
  }
}

class _RoomAuthorityCard extends StatelessWidget {
  const _RoomAuthorityCard({
    required this.room,
    required this.owner,
    required this.onChanged,
  });

  final RhythmHueRoomAuthority room;
  final RhythmHueRoomAuthorityOwner owner;
  final ValueChanged<RhythmHueRoomAuthorityOwner>? onChanged;

  @override
  Widget build(BuildContext context) {
    return Container(
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(14),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 16, 16, 4),
            child: Text(
              room.name,
              style: const TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 17,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
          _option(
            value: RhythmHueRoomAuthorityOwner.hue,
            title: 'Hue',
            subtitle: 'Keep existing Hue automations',
          ),
          _option(
            value: RhythmHueRoomAuthorityOwner.rhythm,
            title: 'Rhythm',
            subtitle: 'Use Rhythm automation when every bridge room agrees',
          ),
        ],
      ),
    );
  }

  Widget _option({
    required RhythmHueRoomAuthorityOwner value,
    required String title,
    required String subtitle,
  }) {
    final selected = owner == value;
    return Material(
      color: Colors.transparent,
      child: ListTile(
        enabled: onChanged != null,
        onTap: onChanged == null ? null : () => onChanged!(value),
        leading: Icon(
          selected ? Icons.radio_button_checked : Icons.radio_button_unchecked,
          color: selected
              ? const Color(0xFFFFB900)
              : CelestialColors.textSecondary,
        ),
        title: Text(
          title,
          style: const TextStyle(color: CelestialColors.textPrimary),
        ),
        subtitle: Text(
          subtitle,
          style: const TextStyle(color: CelestialColors.textSecondary),
        ),
      ),
    );
  }
}
