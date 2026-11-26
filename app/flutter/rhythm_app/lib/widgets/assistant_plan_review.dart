import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import 'assistant_chat_widgets.dart';
import 'solar_orbit.dart';

class AssistantPlanReview extends StatelessWidget {
  const AssistantPlanReview(
      {super.key,
      required this.approval,
      required this.enabled,
      required this.onDecide,
      this.initiallyExpanded = false});

  final RhythmAssistantApproval approval;
  final bool enabled, initiallyExpanded;
  final ValueChanged<bool> onDecide;

  String _reviewLabel(String key) {
    final words = key.replaceAll('_', ' ').trim();
    return words.isEmpty ? words : words[0].toUpperCase() + words.substring(1);
  }

  @override
  Widget build(BuildContext context) {
    const gap = SizedBox(height: 12);
    return Card(
        color: CelestialColors.backgroundCard,
        margin: EdgeInsets.zero,
        elevation: 0,
        clipBehavior: Clip.antiAlias,
        shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(16),
            side: const BorderSide(color: CelestialColors.orbitRing)),
        // Read the page's theme, not the app's above it.
        child: Builder(
            builder: (context) => Theme(
                data: Theme.of(context)
                    .copyWith(dividerColor: Colors.transparent),
                child: ExpansionTile(
                    key: ValueKey(approval.planId),
                    initiallyExpanded: initiallyExpanded,
                    title: Text(_reviewLabel(approval.operation
                        .replaceFirst(RegExp(r'^plan_'), ''))),
                    subtitle: Text('Requested by ${approval.credentialId}'),
                    childrenPadding: const EdgeInsets.fromLTRB(16, 0, 16, 16),
                    expandedCrossAxisAlignment: CrossAxisAlignment.start,
                    expandedAlignment: Alignment.centerLeft,
                    children: [
                      for (final field
                          in approval.review.entries.where((entry) => !const {
                                'plan_id',
                                'apply_arguments',
                                'operation',
                                'correlation_id',
                                'server_instance_id'
                              }.contains(entry.key))) ...[
                        AssistantNote(_reviewLabel(field.key)),
                        SelectableText(field.value is String
                            ? field.value as String
                            : const JsonEncoder.withIndent('  ')
                                .convert(field.value)),
                        gap,
                      ],
                      Wrap(spacing: 8, children: [
                        FilledButton(
                            onPressed: enabled ? () => onDecide(true) : null,
                            child: const Text('Approve')),
                        TextButton(
                            onPressed: enabled ? () => onDecide(false) : null,
                            child: const Text('Decline')),
                      ]),
                    ]))));
  }
}
