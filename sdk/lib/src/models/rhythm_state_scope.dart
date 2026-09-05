import 'rhythm_room.dart';

/// Optional sections of GET /api/state. The base is always included.
enum RhythmStateInclude { base, controls, nodes, configuration }

/// Describes the complete subset carried by a selected state response.
/// Absence on RhythmHello means the legacy, complete snapshot contract.
class RhythmStateScope {
  final Set<RhythmStateInclude> included;
  final String nodes;

  const RhythmStateScope({required this.included, required this.nodes});

  factory RhythmStateScope.fromJson(Map<String, dynamic> json) {
    if (json['schema_version'] != 1 || json['included'] is! List) {
      throw const FormatException('Unsupported state scope');
    }
    final included = <RhythmStateInclude>{};
    for (final name in json['included'] as List) {
      final values = RhythmStateInclude.values.where((v) => v.name == name);
      if (values.isEmpty) throw const FormatException('Unknown state include');
      included.add(values.single);
    }
    final nodes = json['nodes'];
    final expected = included.contains(RhythmStateInclude.nodes)
        ? 'all'
        : included.contains(RhythmStateInclude.controls)
            ? 'controls'
            : 'none';
    if (!included.contains(RhythmStateInclude.base) ||
        (included.contains(RhythmStateInclude.nodes) &&
            included.contains(RhythmStateInclude.controls)) ||
        nodes != expected) {
      throw const FormatException('Inconsistent state scope');
    }
    return RhythmStateScope(
        included: Set.unmodifiable(included), nodes: expected);
  }

  static bool isControl(RhythmRoom node) =>
      node.kind.isRoom ||
      (node.kind == RhythmNodeKind.lightDevice && node.parentId == null);

  /// Replace only the complete requested subset. Omission is never deletion.
  List<RhythmRoom> mergeNodes(
      List<RhythmRoom> previous, List<RhythmRoom> received) {
    if (nodes == 'none') return List.of(previous);
    if (nodes == 'all') return List.of(received);
    final receivedIds = received.map((node) => node.id).toSet();
    final removedRooms = previous
        .where((node) => node.kind.isRoom && !receivedIds.contains(node.id))
        .map((node) => node.id)
        .toSet();
    return [
      for (final node in previous)
        if (!isControl(node) &&
            !receivedIds.contains(node.id) &&
            !removedRooms.contains(node.parentId))
          node,
      ...received,
    ];
  }
}
