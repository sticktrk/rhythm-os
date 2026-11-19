import 'dart:convert';
import 'package:flutter/material.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

/// Render discovered tools and keep the arguments for a reviewed apply tied
/// to the hub and plan that returned them. No user-entered paths are followed.
class AssistantToolsScreen extends StatefulWidget {
  const AssistantToolsScreen(
      {super.key,
      required this.contract,
      required this.call,
      required this.isCurrentTarget,
      required this.serverId,
      required this.targetChanges});
  final RhythmAssistantContract contract;
  final Future<Map<String, dynamic>> Function(String, Map<String, dynamic>)
      call;
  final bool Function() isCurrentTarget;
  final String serverId;
  final Listenable targetChanges;
  @override
  State<AssistantToolsScreen> createState() => _AssistantToolsState();
}

class _AssistantToolsState extends State<AssistantToolsScreen> {
  RhythmAssistantOperation? _selected;
  Map<String, String> _fields = {};
  Map<String, dynamic>? _result;
  String? _error;
  bool _busy = false, _reviewed = false;

  void _select(RhythmAssistantOperation? op) => setState(() {
        _selected = op;
        _fields = {};
        _result = null;
        _error = null;
        _reviewed = false;
      });

  Iterable<String> _types(Map schema) sync* {
    final alternatives = schema['anyOf'] ?? schema['oneOf'];
    if (alternatives is List) {
      for (final alternative in alternatives.whereType<Map>()) {
        yield* _types(alternative);
      }
    }
    final type = schema['type'];
    if (type is String) yield type;
    if (type is List) yield* type.whereType<String>();
  }

  dynamic _parseArgument(String key, Map schema, String input) {
    final options = schema['enum'];
    if (options is List) {
      return options.firstWhere((v) => v.toString() == input,
          orElse: () =>
              throw FormatException('Choose an advertised value for $key.'));
    }
    final types = _types(schema).toSet();
    if (types.isEmpty || types.length == 1 && types.contains('string')) {
      return input;
    }
    Object? parsed;
    try {
      parsed = jsonDecode(input);
    } on FormatException {
      if (types.contains('string')) return input;
      throw FormatException('Enter ${types.join(' or ')} for $key.');
    }
    final matches = switch (parsed) {
      null => types.contains('null'),
      bool() => types.contains('boolean'),
      int() => types.contains('integer') || types.contains('number'),
      double() => types.contains('number'),
      List() => types.contains('array'),
      Map() => types.contains('object'),
      _ => false,
    };
    if (matches) return parsed;
    if (types.contains('string')) return input;
    throw FormatException('Enter ${types.join(' or ')} for $key.');
  }

  Future<void> _run({bool apply = false}) async {
    if (_busy || !widget.isCurrentTarget() || _selected == null) return;
    final selected = _selected!;
    final plan = _result;
    setState(() {
      _error = null;
      _busy = true;
      _result = null;
      _reviewed = false;
    });
    try {
      String id = selected.id;
      final arguments = <String, dynamic>{};
      if (apply) {
        if (plan == null ||
            plan['server_instance_id'] != widget.serverId ||
            plan['apply_arguments'] is! Map) {
          throw const FormatException('Refresh and review a new plan.');
        }
        id = plan['operation'] as String;
        if (widget.contract.operation(id)?.tier !=
            RhythmAssistantTier.persistentChange) {
          throw const FormatException('The apply operation is unavailable.');
        }
        arguments
            .addAll(Map<String, dynamic>.from(plan['apply_arguments'] as Map));
        arguments['confirmed'] = true;
      } else {
        final required = selected.inputSchema['required'] as List? ?? [];
        final props = selected.inputSchema['properties'] as Map? ?? {};
        for (final key in props.keys.cast<String>()) {
          final schema = props[key] as Map;
          final text = _fields[key]?.trim() ?? '';
          if (text.isEmpty) {
            if (required.contains(key)) {
              throw FormatException('Enter ${key.replaceAll('_', ' ')}.');
            }
            continue;
          }
          arguments[key] = _parseArgument(key, schema, text);
        }
      }
      final result = await widget.call(id, arguments);
      if (!mounted || !widget.isCurrentTarget()) return;
      setState(() {
        _result = result;
        _reviewed = !apply &&
            selected.tier == RhythmAssistantTier.read &&
            result['server_instance_id'] == widget.serverId &&
            result['apply_arguments'] is Map &&
            widget.contract
                    .operation(result['operation'] as String? ?? '')
                    ?.tier ==
                RhythmAssistantTier.persistentChange;
      });
    } on FormatException catch (error) {
      if (mounted) setState(() => _error = error.message);
    } catch (_) {
      if (mounted) {
        setState(() => _error =
            'The hub could not verify the result. Read the current state before retrying.');
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
      listenable: widget.targetChanges,
      builder: (_, __) {
        final op = _selected;
        final enabled = widget.isCurrentTarget() && !_busy;
        final props = op?.inputSchema['properties'] as Map? ?? {};
        return Scaffold(
            appBar: AppBar(title: const Text('Hub tools')),
            body: SafeArea(
                child: ListView(padding: const EdgeInsets.all(16), children: [
              DropdownButtonFormField<RhythmAssistantOperation>(
                  isExpanded: true,
                  decoration: const InputDecoration(labelText: 'Choose a tool'),
                  items: [
                    for (final item in widget.contract.operations.where((o) =>
                        o.tier == RhythmAssistantTier.read ||
                        o.tier == RhythmAssistantTier.temporaryLightOutput))
                      DropdownMenuItem(
                          value: item,
                          child: Text(item.id.replaceAll('_', ' ')))
                  ],
                  onChanged: enabled ? _select : null),
              if (op != null) ...[
                Padding(
                    padding: const EdgeInsets.symmetric(vertical: 12),
                    child: Text(op.description)),
                for (final key in props.keys.cast<String>())
                  TextFormField(
                      key: ValueKey('${op.id}/$key'),
                      enabled: enabled,
                      decoration: InputDecoration(
                          labelText: key.replaceAll('_', ' '),
                          helperText:
                              (props[key]['enum'] as List?)?.join(' / ') ??
                                  props[key]['description'] as String? ??
                                  props[key]['type']?.toString(),
                          helperMaxLines: 3),
                      onChanged: (text) => setState(() {
                            _fields[key] = text;
                            _reviewed = false;
                            _result = null;
                          })),
                FilledButton(
                    onPressed: enabled ? _run : null,
                    child: Text(op.tier == RhythmAssistantTier.read
                        ? 'Inspect or preview'
                        : 'Run on this hub')),
              ],
              if (_busy) const LinearProgressIndicator(),
              if (_error != null) Text(_error!),
              if (!widget.isCurrentTarget())
                const Text('Reconnect to the same hub to continue.'),
              if (_result != null && widget.isCurrentTarget()) ...[
                SelectableText(
                    const JsonEncoder.withIndent('  ').convert(_result)),
                if (_reviewed) ...[
                  FilledButton(
                      onPressed: enabled ? () => _run(apply: true) : null,
                      child: const Text('Confirm reviewed change')),
                  TextButton(
                      onPressed: enabled
                          ? () => setState(() {
                                _reviewed = false;
                                _result = null;
                              })
                          : null,
                      child: const Text('Cancel')),
                ],
              ],
            ])));
      });
}
