import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmDevice;
import 'package:uuid/uuid.dart';

import '../../providers/server_sync_provider.dart';
import '../../services/matter_bulb_tester_service.dart';
import '../../widgets/solar_orbit.dart';

class MatterBulbTesterScreen extends StatefulWidget {
  const MatterBulbTesterScreen({
    super.key,
    required this.device,
    required this.nativeDeviceId,
  });

  final RhythmDevice device;
  final String nativeDeviceId;

  @override
  State<MatterBulbTesterScreen> createState() => _MatterBulbTesterScreenState();
}

class _MatterBulbTesterScreenState extends State<MatterBulbTesterScreen> {
  static const _uuid = Uuid();
  static const _lowDimMinBrightnessHint = 10;

  final _notesController = TextEditingController();
  final _throttleController = TextEditingController(text: '250');
  final _steps = const [
    _MatterBulbTestStep(
      id: 'identify',
      title: 'Identify',
      prompt: 'Did this bulb blink?',
      runLabel: 'Blink bulb',
    ),
    _MatterBulbTestStep(
      id: 'brightness_without_on',
      title: 'Brightness Without On',
      prompt: 'Did the bulb turn on after only a brightness command?',
      runLabel: 'Test brightness',
    ),
    _MatterBulbTestStep(
      id: 'brightness_with_on',
      title: 'Explicit On',
      prompt: 'Did explicit on followed by brightness work?',
      runLabel: 'Test explicit on',
    ),
    _MatterBulbTestStep(
      id: 'dim_low',
      title: 'Low Dim',
      prompt: 'Did the bulb stay on at a stable low dim level?',
      runLabel: 'Test low dim',
    ),
    _MatterBulbTestStep(
      id: 'dim_ramp',
      title: 'Dimming Ramp',
      prompt: 'Did the bulb fade down smoothly instead of jumping or failing?',
      runLabel: 'Test fade',
    ),
    _MatterBulbTestStep(
      id: 'color_temperature',
      title: 'Color Temperature',
      prompt: 'Did the bulb move to warm white?',
      runLabel: 'Test warm white',
    ),
    _MatterBulbTestStep(
      id: 'xy_color',
      title: 'XY Color',
      prompt: 'Did the bulb change to a saturated color?',
      runLabel: 'Test color',
    ),
    _MatterBulbTestStep(
      id: 'rapid_commands',
      title: 'Rapid Commands',
      prompt: 'Did the bulb keep up with the command burst?',
      runLabel: 'Test burst',
    ),
  ];

  final Map<String, _StepObservation> _observations = {};
  int _currentStep = 0;
  bool _running = false;
  bool _saving = false;
  String? _status;

  @override
  void dispose() {
    _notesController.dispose();
    _throttleController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final currentIndex = _currentStep.clamp(0, _steps.length - 1).toInt();
    final current = _steps[currentIndex];
    final answeredCount = _observations.values
        .where((observation) => observation.answered)
        .length;
    final inferredQuirks = _inferredQuirks();
    final capabilityHints = _capabilityHints();

    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(context),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.fromLTRB(20, 8, 20, 28),
                children: [
                  _buildDeviceBlock(),
                  const SizedBox(height: 16),
                  _buildCurrentStep(current),
                  const SizedBox(height: 16),
                  _buildStepList(),
                  const SizedBox(height: 16),
                  _buildInferredQuirks(inferredQuirks, capabilityHints),
                  const SizedBox(height: 16),
                  _buildNotes(),
                  const SizedBox(height: 20),
                  _buildSaveButton(answeredCount),
                  if (_status != null) ...[
                    const SizedBox(height: 12),
                    Text(
                      _status!,
                      textAlign: TextAlign.center,
                      style: const TextStyle(
                        color: CelestialColors.textSecondary,
                        fontSize: 13,
                      ),
                    ),
                  ],
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: Row(
        children: [
          GestureDetector(
            onTap: () => Navigator.of(context).pop(),
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: CelestialColors.accentBlue.withValues(alpha: 0.2),
              ),
              child: const Icon(
                Icons.chevron_left,
                color: CelestialColors.accentBlue,
                size: 24,
              ),
            ),
          ),
          const Expanded(
            child: Text(
              'Matter Bulb Tester',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
          const SizedBox(width: 40),
        ],
      ),
    );
  }

  Widget _buildDeviceBlock() {
    final product = widget.device.productInfo;
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: _panelDecoration(),
      child: Row(
        children: [
          Container(
            width: 46,
            height: 46,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: const Color(0xFFFFB74D).withValues(alpha: 0.16),
              border: Border.all(
                color: const Color(0xFFFFB74D).withValues(alpha: 0.4),
              ),
            ),
            child: const Icon(
              Icons.lightbulb_outline,
              color: Color(0xFFFFB74D),
            ),
          ),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  widget.device.displayName,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 16,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                if (product != null) ...[
                  const SizedBox(height: 3),
                  Text(
                    product,
                    style: const TextStyle(
                      color: CelestialColors.textSecondary,
                      fontSize: 13,
                    ),
                  ),
                ],
                const SizedBox(height: 3),
                Text(
                  widget.nativeDeviceId,
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                    fontSize: 12,
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildCurrentStep(_MatterBulbTestStep step) {
    final observation = _observations[step.id];
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: _panelDecoration(borderColor: CelestialColors.sunWarm),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  step.title,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 17,
                    fontWeight: FontWeight.w700,
                  ),
                ),
              ),
              Text(
                '${_currentStep + 1}/${_steps.length}',
                style: const TextStyle(
                  color: CelestialColors.textSecondary,
                  fontSize: 13,
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          Text(
            step.prompt,
            style: const TextStyle(
              color: CelestialColors.textSecondary,
              fontSize: 14,
              height: 1.3,
            ),
          ),
          const SizedBox(height: 14),
          SizedBox(
            height: 46,
            child: ElevatedButton.icon(
              onPressed: _running ? null : () => _runStep(step),
              icon: _running
                  ? const SizedBox(
                      width: 18,
                      height: 18,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    )
                  : const Icon(Icons.play_arrow_rounded),
              label: Text(_running ? 'Running' : step.runLabel),
              style: ElevatedButton.styleFrom(
                backgroundColor: CelestialColors.sunWarm,
                foregroundColor: CelestialColors.backgroundDark,
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(12),
                ),
              ),
            ),
          ),
          const SizedBox(height: 12),
          Row(
            children: [
              Expanded(
                child: _AnswerButton(
                  label: 'Yes',
                  selected: observation?.worked == true,
                  onTap: () => _recordObservation(step, true),
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: _AnswerButton(
                  label: 'No',
                  selected: observation?.worked == false,
                  onTap: () => _recordObservation(step, false),
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: _AnswerButton(
                  label: 'Unsure',
                  selected: observation?.answered == true &&
                      observation?.worked == null,
                  onTap: () => _recordObservation(step, null),
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _buildStepList() {
    return Column(
      children: [
        for (int i = 0; i < _steps.length; i++) ...[
          _StepSummaryRow(
            step: _steps[i],
            selected: i == _currentStep,
            observation: _observations[_steps[i].id],
            onTap: () => setState(() => _currentStep = i),
          ),
          if (i < _steps.length - 1) const SizedBox(height: 8),
        ],
      ],
    );
  }

  Widget _buildInferredQuirks(
    List<dynamic> inferredQuirks,
    Map<String, dynamic> capabilityHints,
  ) {
    final labels = [
      ...inferredQuirks.map(_quirkLabel),
      ..._capabilityLabels(capabilityHints),
    ];
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: _panelDecoration(),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text(
            'Local profile',
            style: TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 15,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(height: 10),
          if (labels.isEmpty)
            const Text(
              'No local adjustments inferred yet.',
              style: TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 13,
              ),
            )
          else
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                for (final label in labels)
                  Container(
                    padding:
                        const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
                    decoration: BoxDecoration(
                      color: CelestialColors.accentBlue.withValues(alpha: 0.18),
                      borderRadius: BorderRadius.circular(12),
                      border: Border.all(
                        color:
                            CelestialColors.accentBlue.withValues(alpha: 0.35),
                      ),
                    ),
                    child: Text(
                      label,
                      style: const TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 12,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ),
              ],
            ),
          if (_observations['rapid_commands']?.worked == false) ...[
            const SizedBox(height: 12),
            TextField(
              controller: _throttleController,
              keyboardType: TextInputType.number,
              style: const TextStyle(color: CelestialColors.textPrimary),
              decoration: const InputDecoration(
                labelText: 'Command gap ms',
                labelStyle: TextStyle(color: CelestialColors.textSecondary),
                enabledBorder: UnderlineInputBorder(
                  borderSide: BorderSide(color: CelestialColors.orbitRing),
                ),
                focusedBorder: UnderlineInputBorder(
                  borderSide: BorderSide(color: CelestialColors.sunWarm),
                ),
              ),
              onChanged: (_) => setState(() {}),
            ),
          ],
        ],
      ),
    );
  }

  Widget _buildNotes() {
    return TextField(
      controller: _notesController,
      minLines: 2,
      maxLines: 4,
      style: const TextStyle(color: CelestialColors.textPrimary),
      decoration: InputDecoration(
        hintText: 'Notes',
        hintStyle: TextStyle(
          color: CelestialColors.textSecondary.withValues(alpha: 0.7),
        ),
        filled: true,
        fillColor: CelestialColors.backgroundCard.withValues(alpha: 0.7),
        enabledBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(14),
          borderSide: BorderSide(
            color: CelestialColors.orbitRing.withValues(alpha: 0.3),
          ),
        ),
        focusedBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(14),
          borderSide: const BorderSide(color: CelestialColors.sunWarm),
        ),
      ),
    );
  }

  Widget _buildSaveButton(int answeredCount) {
    return SizedBox(
      height: 50,
      child: ElevatedButton.icon(
        onPressed: _saving || answeredCount == 0 ? null : _saveReport,
        icon: _saving
            ? const SizedBox(
                width: 18,
                height: 18,
                child: CircularProgressIndicator(strokeWidth: 2),
              )
            : const Icon(Icons.cloud_upload_outlined),
        label: Text(_saving ? 'Saving' : 'Save Results'),
        style: ElevatedButton.styleFrom(
          backgroundColor: CelestialColors.accentBlue,
          foregroundColor: Colors.white,
          disabledBackgroundColor:
              CelestialColors.orbitRing.withValues(alpha: 0.25),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(14),
          ),
        ),
      ),
    );
  }

  Future<void> _runStep(_MatterBulbTestStep step) async {
    setState(() {
      _running = true;
      _status = null;
    });
    HapticFeedback.mediumImpact();

    final result =
        await context.read<ServerSyncProvider>().api.runMatterBulbTest(
              deviceId: widget.device.id,
              test: step.id,
            );

    if (!mounted) return;
    final existing = _observations[step.id];
    setState(() {
      _observations[step.id] = _StepObservation(
        worked: existing?.worked,
        answered: existing?.answered ?? false,
        serverResult: result,
        recordedAt: existing?.recordedAt ?? DateTime.now(),
      );
      _running = false;
      if (result == null) {
        _status = 'The server did not return a result for ${step.title}.';
      }
    });
  }

  void _recordObservation(_MatterBulbTestStep step, bool? worked) {
    HapticFeedback.selectionClick();
    final existing = _observations[step.id];
    setState(() {
      _observations[step.id] = _StepObservation(
        worked: worked,
        answered: true,
        serverResult: existing?.serverResult,
        recordedAt: DateTime.now(),
      );
      final currentIndex =
          _steps.indexWhere((candidate) => candidate.id == step.id);
      if (currentIndex >= 0 && currentIndex < _steps.length - 1) {
        _currentStep = currentIndex + 1;
      }
      _status = null;
    });
  }

  Future<void> _saveReport() async {
    setState(() {
      _saving = true;
      _status = null;
    });

    final sync = context.read<ServerSyncProvider>();
    final report = _buildReport();
    final service = MatterBulbTesterService.instance;
    await service.saveLocalReport({
      ...report,
      'local_status': 'pending',
    });

    final serverResult = await sync.api.saveMatterBulbTestReport(report);
    final cloudResult = await service.submitCloudReport(
      report: {
        ...report,
        if (serverResult != null) 'server_save_result': serverResult,
      },
      serverVersion: sync.firmwareVersion,
      serverPlatformContext: sync.serverPlatformContext,
    );

    final savedReport = {
      ...report,
      'local_status': 'saved',
      if (serverResult != null) 'server_save_result': serverResult,
      'cloud_uploaded': cloudResult.uploaded,
      if (cloudResult.message != null) 'cloud_error': cloudResult.message,
      if (cloudResult.reportId != null) 'cloud_report_id': cloudResult.reportId,
    };
    await service.saveLocalReport(savedReport);

    if (!mounted) return;
    final serverSaved = serverResult?['status'] == 'saved';
    final appliedLocal = serverResult?['applied_local'] == true;
    setState(() {
      _saving = false;
      _status = switch ((serverSaved, appliedLocal, cloudResult.uploaded)) {
        (true, true, true) =>
          'Saved locally, applied on this server, and sent to Rhythm.',
        (true, true, false) => 'Saved locally and applied on this server.',
        (true, false, true) =>
          'Saved locally, saved on this server, and sent to Rhythm.',
        (true, false, false) => 'Saved locally and on this server.',
        (false, _, true) =>
          'Saved locally and sent to Rhythm. Server save failed.',
        (false, _, false) => 'Saved locally. Server save failed.',
      };
    });
  }

  Map<String, dynamic> _buildReport() {
    final capabilityHints = _capabilityHints();
    return {
      'report_id': _uuid.v4(),
      'device_id': widget.nativeDeviceId,
      'canonical_device_id': widget.device.id,
      'created_at': DateTime.now().toUtc().toIso8601String(),
      'device': {
        'name': widget.device.displayName,
        'manufacturer': widget.device.manufacturer,
        'model': widget.device.model,
        'native_device_id': widget.nativeDeviceId,
      },
      'observations': [
        for (final step in _steps)
          if (_observations[step.id] != null)
            {
              'test': step.id,
              'title': step.title,
              'worked': _observations[step.id]!.worked,
              'answered': _observations[step.id]!.answered,
              'recorded_at':
                  _observations[step.id]!.recordedAt.toUtc().toIso8601String(),
              if (_observations[step.id]!.serverResult != null)
                'server_result': _observations[step.id]!.serverResult,
            },
      ],
      'inferred_quirks': _inferredQuirks(),
      if (capabilityHints.isNotEmpty) 'capability_hints': capabilityHints,
      'dimming': {
        'low_dim_test_percent': 3,
        'ramp_start_percent': 85,
        'ramp_end_percent': 10,
        'ramp_transition_ms': 3000,
        'low_dim_worked': _observations['dim_low']?.worked,
        'ramp_worked': _observations['dim_ramp']?.worked,
      },
      'notes': _notesController.text.trim(),
    };
  }

  List<dynamic> _inferredQuirks() {
    final quirks = <dynamic>[];
    final brightnessWithoutOn = _observations['brightness_without_on']?.worked;
    final brightnessWithOn = _observations['brightness_with_on']?.worked;
    if (brightnessWithoutOn == false && brightnessWithOn != false) {
      quirks.add('needs_explicit_on');
    }

    final colorTemperature = _observations['color_temperature']?.worked;
    final xyColor = _observations['xy_color']?.worked;
    if (colorTemperature == false && xyColor == true) {
      quirks.add('needs_xy_not_ct');
    }

    if (_observations['rapid_commands']?.worked == false) {
      final throttleMs = int.tryParse(_throttleController.text.trim()) ?? 250;
      quirks.add({'command_throttle_ms': throttleMs.clamp(50, 2000)});
    }
    return quirks;
  }

  Map<String, dynamic> _capabilityHints() {
    final hints = <String, dynamic>{};
    if (_observations['dim_low']?.worked == false) {
      hints['min_brightness'] = _lowDimMinBrightnessHint;
    }
    if (_observations['dim_ramp']?.worked == false) {
      hints['supports_transition'] = false;
    }
    return hints;
  }

  String _quirkLabel(dynamic quirk) {
    if (quirk == 'needs_explicit_on') return 'Explicit On';
    if (quirk == 'needs_xy_not_ct') return 'Prefer XY';
    if (quirk is Map && quirk['command_throttle_ms'] != null) {
      return '${quirk['command_throttle_ms']} ms gap';
    }
    return quirk.toString();
  }

  List<String> _capabilityLabels(Map<String, dynamic> capabilityHints) {
    final labels = <String>[];
    final minBrightness = capabilityHints['min_brightness'];
    if (minBrightness is int) {
      labels.add('Min brightness $minBrightness%');
    }
    if (capabilityHints['supports_transition'] == false) {
      labels.add('No fade transitions');
    }
    return labels;
  }

  BoxDecoration _panelDecoration({Color? borderColor}) {
    return BoxDecoration(
      color: CelestialColors.backgroundCard.withValues(alpha: 0.76),
      borderRadius: BorderRadius.circular(16),
      border: Border.all(
        color:
            (borderColor ?? CelestialColors.orbitRing).withValues(alpha: 0.35),
      ),
    );
  }
}

class _MatterBulbTestStep {
  const _MatterBulbTestStep({
    required this.id,
    required this.title,
    required this.prompt,
    required this.runLabel,
  });

  final String id;
  final String title;
  final String prompt;
  final String runLabel;
}

class _StepObservation {
  const _StepObservation({
    required this.worked,
    required this.answered,
    required this.serverResult,
    required this.recordedAt,
  });

  final bool? worked;
  final bool answered;
  final Map<String, dynamic>? serverResult;
  final DateTime recordedAt;
}

class _AnswerButton extends StatelessWidget {
  const _AnswerButton({
    required this.label,
    required this.selected,
    required this.onTap,
  });

  final String label;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 150),
        height: 42,
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: selected
              ? CelestialColors.accentBlue
              : CelestialColors.backgroundDark.withValues(alpha: 0.35),
          borderRadius: BorderRadius.circular(12),
          border: Border.all(
            color: selected
                ? CelestialColors.accentBlue
                : CelestialColors.orbitRing.withValues(alpha: 0.35),
          ),
        ),
        child: Text(
          label,
          style: TextStyle(
            color: selected ? Colors.white : CelestialColors.textPrimary,
            fontWeight: FontWeight.w600,
          ),
        ),
      ),
    );
  }
}

class _StepSummaryRow extends StatelessWidget {
  const _StepSummaryRow({
    required this.step,
    required this.selected,
    required this.observation,
    required this.onTap,
  });

  final _MatterBulbTestStep step;
  final bool selected;
  final _StepObservation? observation;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final icon = switch ((observation?.answered, observation?.worked)) {
      (true, true) => Icons.check_circle,
      (true, false) => Icons.cancel,
      (true, null) => Icons.help,
      (false, _) when observation?.serverResult != null => Icons.terminal,
      _ => Icons.radio_button_unchecked,
    };
    final color = switch ((observation?.answered, observation?.worked)) {
      (true, true) => const Color(0xFF81C784),
      (true, false) => const Color(0xFFE57373),
      (true, null) => const Color(0xFFFFD54F),
      (false, _) when observation?.serverResult != null =>
        CelestialColors.accentBlue,
      _ => CelestialColors.textSecondary,
    };

    return GestureDetector(
      onTap: onTap,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
        decoration: BoxDecoration(
          color: selected
              ? CelestialColors.backgroundCard
              : CelestialColors.backgroundCard.withValues(alpha: 0.5),
          borderRadius: BorderRadius.circular(14),
          border: Border.all(
            color: selected
                ? CelestialColors.sunWarm.withValues(alpha: 0.45)
                : CelestialColors.orbitRing.withValues(alpha: 0.25),
          ),
        ),
        child: Row(
          children: [
            Icon(icon, color: color, size: 20),
            const SizedBox(width: 12),
            Expanded(
              child: Text(
                step.title,
                style: const TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 14,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
            if (observation?.serverResult != null)
              Icon(
                Icons.terminal_rounded,
                color: CelestialColors.textSecondary.withValues(alpha: 0.75),
                size: 18,
              ),
          ],
        ),
      ),
    );
  }
}
