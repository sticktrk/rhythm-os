import 'package:flutter/material.dart';

List<String> lightingSettingsWarnings(Object? value) => (value is List ? value : const [])
    .whereType<String>()
    .map((code) => switch (code) {
          'native_scenes_excluded' =>
            'Scenes owned by another integration need to be recreated in Home Assistant.',
          'scene_extensions_excluded' =>
            'Integration-specific scene options were left out.',
          'native_scene_bindings_excluded' =>
            'Some room scene selections need to be chosen again in Home Assistant.',
          'unmapped_scene_entries_excluded' =>
            'Scene settings for unmatched rooms and lights were skipped.',
          'unmapped_mode_defaults_excluded' =>
            'Mode defaults for unmatched rooms were skipped.',
          'newer_target_enablement_preserved' =>
            'A newer manual pause or resume was kept for some lights.',
          _ => 'Some installation-specific settings need review after transfer.',
        })
    .toSet()
    .toList();

String _nodeLabel(Map node, List<Map> nodes) {
  String name(Map item) => item['name'] is String && (item['name'] as String).isNotEmpty
      ? item['name'] as String
      : item['kind'] == 'room' ? 'Unnamed room' : 'Unnamed light';
  String label(Map item) {
    final parent = nodes.where((candidate) => candidate['id'] == item['parent_id']).firstOrNull;
    return parent == null ? name(item) : '${name(item)} · ${name(parent)}';
  }
  final base = label(node);
  if (nodes.where((candidate) => candidate['kind'] == node['kind'] && label(candidate) == base).length < 2) {
    return base;
  }
  final id = node['id'] as String;
  return '$base · ${id.substring(id.length > 8 ? id.length - 8 : 0)}';
}

/// Names help an owner review a match, but are never treated as identity proof.
class LightingSettingsMappingDialog extends StatefulWidget {
  const LightingSettingsMappingDialog({
    super.key,
    required this.source,
    required this.target,
  });

  final Map<String, dynamic> source;
  final Map<String, dynamic> target;

  @override
  State<LightingSettingsMappingDialog> createState() =>
      _LightingSettingsMappingDialogState();
}

class _LightingSettingsMappingDialogState
    extends State<LightingSettingsMappingDialog> {
  final _mappings = <String, String>{};

  @override
  Widget build(BuildContext context) {
    final sources = (widget.source['nodes'] as List? ?? const [])
        .whereType<Map>()
        .where((node) => node['id'] is String)
        .toList();
    final targets = (widget.target['nodes'] as List? ?? const [])
        .whereType<Map>()
        .where((node) => node['id'] is String)
        .toList();
    final warnings = lightingSettingsWarnings(widget.source['warnings']);
    return AlertDialog(
      title: const Text('Match your rooms and lights'),
      content: SizedBox(
        width: 520,
        child: SingleChildScrollView(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              const Text(
                'Restore profiles, schedules, modes and scenes. Match each saved room or light to its Home Assistant destination to transfer its brightness, color and other preferences. Skipped settings stay in the original backup, so you can transfer them after setup.',
              ),
              const SizedBox(height: 12),
              const Text(
                'Saved settings replace matching settings. Rhythm pauses for review. Device pairing, Matter fabrics and integration credentials stay with their original controller.',
              ),
              for (final warning in warnings) ...[
                const SizedBox(height: 8),
                Text(warning),
              ],
              for (final source in sources) ...[
                const SizedBox(height: 16),
                DropdownButtonFormField<String>(
                  key: ValueKey('mapping-${source['id']}'),
                  initialValue: _mappings[source['id']] ?? '',
                  isExpanded: true,
                  decoration: InputDecoration(
                    labelText:
                        '${_nodeLabel(source, sources)} (${source['kind'] == 'room' ? 'room' : 'light'})',
                  ),
                  items: [
                    const DropdownMenuItem(
                      value: '',
                      child: Text('Skip for now'),
                    ),
                    for (final target in targets)
                      if (target['kind'] == source['kind'] &&
                          (!_mappings.containsValue(target['id']) ||
                              _mappings[source['id']] == target['id']))
                        DropdownMenuItem(
                          value: target['id'] as String,
                          child: Text(
                            _nodeLabel(target, targets),
                            overflow: TextOverflow.ellipsis,
                          ),
                        ),
                  ],
                  onChanged: (value) => setState(() {
                    if (value == null || value.isEmpty) {
                      _mappings.remove(source['id']);
                    } else {
                      _mappings[source['id'] as String] = value;
                    }
                  }),
                ),
              ],
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: () =>
              Navigator.of(context).pop(Map<String, String>.from(_mappings)),
          child: const Text('Restore settings'),
        ),
      ],
    );
  }
}
