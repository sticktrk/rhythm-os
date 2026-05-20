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
  static const _rapidGapsMs = [50, 100, 200, 500, 1000];

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
      id: 'turn_off',
      title: 'Baseline Off',
      prompt: 'Did the bulb turn fully off?',
      runLabel: 'Turn off bulb',
    ),
    _MatterBulbTestStep(
      id: 'brightness_without_on',
      title: 'Brightness Without On',
      prompt:
          'Starting from off, did the bulb turn on after only a brightness command?',
      runLabel: 'Test brightness',
    ),
    _MatterBulbTestStep(
      id: 'brightness_with_on',
      title: 'Explicit On',
      prompt: 'Did explicit on followed by brightness work?',
      runLabel: 'Test explicit on',
    ),
    _MatterBulbTestStep(
      id: 'level_move_to_level',
      title: 'MoveToLevel',
      prompt:
          'Starting from off, did plain MoveToLevel turn the bulb on or visibly set brightness?',
      runLabel: 'Test MoveToLevel',
    ),
    _MatterBulbTestStep(
      id: 'level_move_to_level_with_onoff',
      title: 'MoveToLevelWithOnOff',
      prompt:
          'Starting from off, did MoveToLevelWithOnOff turn the bulb on and set brightness?',
      runLabel: 'Test MTL OnOff',
    ),
    _MatterBulbTestStep(
      id: 'level_step',
      title: 'Step',
      prompt:
          'It should first reset to neutral white, then run a plain Step down command. Did brightness step down?',
      runLabel: 'Test Step',
    ),
    _MatterBulbTestStep(
      id: 'level_step_with_onoff',
      title: 'StepWithOnOff',
      prompt:
          'Starting from off, did StepWithOnOff turn the bulb on or visibly step brightness?',
      runLabel: 'Test Step OnOff',
    ),
    _MatterBulbTestStep(
      id: 'dim_low',
      title: 'Low Dim',
      prompt:
          'It should first reset to neutral white, then drop low. Did it stay on at a stable low dim level?',
      runLabel: 'Test low dim',
    ),
    _MatterBulbTestStep(
      id: 'dim_ramp',
      title: 'Dimming Ramp',
      prompt:
          'It should first reset to neutral white, then fade down. Did it fade smoothly instead of jumping or failing?',
      runLabel: 'Test fade',
    ),
    _MatterBulbTestStep(
      id: 'brightness_steps',
      title: 'Brightness Range',
      prompt:
          'It should first reset to neutral white, then step low, medium, and full. Were the steps visible?',
      runLabel: 'Test range',
    ),
    _MatterBulbTestStep(
      id: 'color_temperature_warm',
      title: 'Warm White',
      prompt:
          'It should first reset to neutral white, then move warm. Did it become warm white?',
      runLabel: 'Test warm white',
    ),
    _MatterBulbTestStep(
      id: 'color_temperature_cool',
      title: 'Cool White',
      prompt:
          'It should first reset to neutral white, then move cool. Did it become cool white?',
      runLabel: 'Test cool white',
    ),
    _MatterBulbTestStep(
      id: 'xy_red',
      title: 'Red',
      prompt:
          'It should first reset to neutral white, then turn red. Did it turn red?',
      runLabel: 'Test red',
    ),
    _MatterBulbTestStep(
      id: 'xy_green',
      title: 'Green',
      prompt:
          'It should first reset to neutral white, then turn green. Did it turn green?',
      runLabel: 'Test green',
    ),
    _MatterBulbTestStep(
      id: 'xy_blue',
      title: 'Blue',
      prompt:
          'It should first reset to neutral white, then turn blue. Did it turn blue?',
      runLabel: 'Test blue',
    ),
    _MatterBulbTestStep(
      id: 'hue_sat_red',
      title: 'Hue/Sat Red',
      prompt:
          'It should first reset to neutral white, then use Hue/Sat red. Did it turn red?',
      runLabel: 'Test HS red',
    ),
    _MatterBulbTestStep(
      id: 'hue_sat_green',
      title: 'Hue/Sat Green',
      prompt:
          'It should first reset to neutral white, then use Hue/Sat green. Did it turn green?',
      runLabel: 'Test HS green',
    ),
    _MatterBulbTestStep(
      id: 'hue_sat_blue',
      title: 'Hue/Sat Blue',
      prompt:
          'It should first reset to neutral white, then use Hue/Sat blue. Did it turn blue?',
      runLabel: 'Test HS blue',
    ),
    _MatterBulbTestStep(
      id: 'ct_to_xy',
      title: 'CT to XY',
      prompt:
          'It should first reset to neutral white, move warm, then switch to XY red. Did it end red?',
      runLabel: 'Test CT to XY',
    ),
    _MatterBulbTestStep(
      id: 'xy_to_ct',
      title: 'XY to CT',
      prompt:
          'It should first reset to neutral white, move XY blue, then switch to warm white. Did it end warm white?',
      runLabel: 'Test XY to CT',
    ),
    _MatterBulbTestStep(
      id: 'ct_to_hue_sat',
      title: 'CT to Hue/Sat',
      prompt:
          'It should first reset to neutral white, move warm, then switch to Hue/Sat blue. Did it end blue?',
      runLabel: 'Test CT to HS',
    ),
    _MatterBulbTestStep(
      id: 'hue_sat_to_ct',
      title: 'Hue/Sat to CT',
      prompt:
          'It should first reset to neutral white, move Hue/Sat blue, then switch to warm white. Did it end warm white?',
      runLabel: 'Test HS to CT',
    ),
    _MatterBulbTestStep(
      id: 'on_level_restore',
      title: 'On Level Restore',
      prompt:
          'It should first reset to neutral white, dim to 10%, turn off, then turn on. Did it come back at the low level?',
      runLabel: 'Test restore',
    ),
    _MatterBulbTestStep(
      id: 'power_on_behavior',
      title: 'Power-On Restore',
      prompt:
          'After it sets warm white at 50%, physically power-cycle the bulb. Did it come back warm at about 50%?',
      runLabel: 'Set power test',
    ),
    _MatterBulbTestStep(
      id: 'rapid_commands',
      title: 'Rapid Commands',
      prompt:
          'It should first reset to neutral white, then rapidly cycle high-contrast colors, warm/cool white, or brightness. Did it visibly change more than once and land on the final state?',
      runLabel: 'Test burst',
    ),
    _MatterBulbTestStep(
      id: 'rapid_50ms',
      title: 'Rapid 50 ms',
      prompt:
          'It should first reset to neutral white, then rapidly cycle high-contrast changes with 50 ms gaps. Did it visibly change more than once and land on the final state?',
      runLabel: 'Test 50 ms',
    ),
    _MatterBulbTestStep(
      id: 'rapid_100ms',
      title: 'Rapid 100 ms',
      prompt:
          'It should first reset to neutral white, then rapidly cycle high-contrast changes with 100 ms gaps. Did it visibly change more than once and land on the final state?',
      runLabel: 'Test 100 ms',
    ),
    _MatterBulbTestStep(
      id: 'rapid_200ms',
      title: 'Rapid 200 ms',
      prompt:
          'It should first reset to neutral white, then rapidly cycle high-contrast changes with 200 ms gaps. Did it visibly change more than once and land on the final state?',
      runLabel: 'Test 200 ms',
    ),
    _MatterBulbTestStep(
      id: 'rapid_500ms',
      title: 'Rapid 500 ms',
      prompt:
          'It should first reset to neutral white, then rapidly cycle high-contrast changes with 500 ms gaps. Did it visibly change more than once and land on the final state?',
      runLabel: 'Test 500 ms',
    ),
    _MatterBulbTestStep(
      id: 'rapid_1000ms',
      title: 'Rapid 1000 ms',
      prompt:
          'It should first reset to neutral white, then rapidly cycle high-contrast changes with 1000 ms gaps. Did it visibly change more than once and land on the final state?',
      runLabel: 'Test 1000 ms',
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
          if (_needsThrottleInput()) ...[
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
    final claimedCapabilities = _firstServerMap('claimed_capabilities');
    final endpointValidation = _firstServerMap('endpoint_validation');
    final testParameters = _firstServerMap('test_parameters');
    final rawCapabilitySnapshot = _firstServerMap('raw_capability_snapshot');
    final operatorNotes = _notesController.text.trim();
    return {
      'schema_version': 2,
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
      'claimed_capabilities': claimedCapabilities ?? <String, dynamic>{},
      if (rawCapabilitySnapshot != null)
        'raw_capability_snapshot': rawCapabilitySnapshot,
      if (endpointValidation != null) 'endpoint_validation': endpointValidation,
      if (testParameters != null) 'test_parameters': testParameters,
      'tested_capabilities': _testedCapabilities(),
      'command_results': _commandResults(),
      'readback_consistency': _readbackConsistency(),
      'visual_observations': _visualObservations(),
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
      'inferred_behavioral_quirks': _inferredBehavioralQuirks(),
      'recommended_control_strategy': _recommendedControlStrategy(),
      if (capabilityHints.isNotEmpty) 'capability_hints': capabilityHints,
      'dimming': {
        'low_dim_test_percent': 3,
        'ramp_start_percent': 85,
        'ramp_end_percent': 10,
        'ramp_transition_ms': 3000,
        'low_dim_worked': _observations['dim_low']?.worked,
        'ramp_worked': _observations['dim_ramp']?.worked,
      },
      'color_coverage': {
        'warm_white_worked': _observations['color_temperature_warm']?.worked,
        'cool_white_worked': _observations['color_temperature_cool']?.worked,
        'red_worked': _observations['xy_red']?.worked,
        'green_worked': _observations['xy_green']?.worked,
        'blue_worked': _observations['xy_blue']?.worked,
        'hue_sat_red_worked': _observations['hue_sat_red']?.worked,
        'hue_sat_green_worked': _observations['hue_sat_green']?.worked,
        'hue_sat_blue_worked': _observations['hue_sat_blue']?.worked,
        'ct_to_xy_worked': _observations['ct_to_xy']?.worked,
        'xy_to_ct_worked': _observations['xy_to_ct']?.worked,
        'ct_to_hue_sat_worked': _observations['ct_to_hue_sat']?.worked,
        'hue_sat_to_ct_worked': _observations['hue_sat_to_ct']?.worked,
      },
      'operator_notes': operatorNotes,
      'notes': _reportNotes(operatorNotes),
    };
  }

  Map<String, dynamic>? _firstServerMap(String key) {
    for (final step in _steps) {
      final value = _observations[step.id]?.serverResult?[key];
      if (value is Map) {
        return Map<String, dynamic>.from(value);
      }
    }
    return null;
  }

  Map<String, dynamic> _commandResults() {
    return {
      for (final step in _steps)
        if (_observations[step.id]?.serverResult != null)
          step.id: {
            'status': _observations[step.id]!.serverResult?['status'],
            'command_count':
                _observations[step.id]!.serverResult?['command_count'],
            'failed_count':
                _observations[step.id]!.serverResult?['failed_count'],
            'commands': _observations[step.id]!.serverResult?['commands'],
          },
    };
  }

  Map<String, dynamic> _visualObservations() {
    return {
      for (final step in _steps)
        if (_observations[step.id] != null)
          step.id: {
            'title': step.title,
            'answer': _answerLabel(_observations[step.id]!),
            'worked': _observations[step.id]!.worked,
            'answered': _observations[step.id]!.answered,
            'recorded_at':
                _observations[step.id]!.recordedAt.toUtc().toIso8601String(),
          },
    };
  }

  Map<String, dynamic> _testedCapabilities() {
    return {
      'onoff': {
        'identify': _observations['identify']?.worked,
        'turn_off': _observations['turn_off']?.worked,
      },
      'level_control': {
        'move_to_level_with_onoff_without_explicit_on':
            _observations['brightness_without_on']?.worked,
        'move_to_level_with_onoff_after_explicit_on':
            _observations['brightness_with_on']?.worked,
        'move_to_level': _observations['level_move_to_level']?.worked,
        'move_to_level_with_onoff':
            _observations['level_move_to_level_with_onoff']?.worked,
        'step': _observations['level_step']?.worked,
        'step_with_onoff': _observations['level_step_with_onoff']?.worked,
        'brightness_steps': _observations['brightness_steps']?.worked,
        'low_dim': _observations['dim_low']?.worked,
        'on_level_restore': _observations['on_level_restore']?.worked,
      },
      'level_command_variants': {
        'move_to_level_with_onoff': {
          'tested': true,
          'worked_without_explicit_on':
              _observations['brightness_without_on']?.worked,
          'worked_after_explicit_on':
              _observations['brightness_with_on']?.worked,
          'worked_variant_test':
              _observations['level_move_to_level_with_onoff']?.worked,
        },
        'move_to_level': {
          'tested': true,
          'worked': _observations['level_move_to_level']?.worked,
        },
        'step': {
          'tested': true,
          'worked': _observations['level_step']?.worked,
        },
        'step_with_onoff': {
          'tested': true,
          'worked': _observations['level_step_with_onoff']?.worked,
        },
      },
      'color_temperature': {
        'warm': _observations['color_temperature_warm']?.worked,
        'cool': _observations['color_temperature_cool']?.worked,
      },
      'xy_color': {
        'red': _observations['xy_red']?.worked,
        'green': _observations['xy_green']?.worked,
        'blue': _observations['xy_blue']?.worked,
      },
      'hue_saturation': {
        'red': _observations['hue_sat_red']?.worked,
        'green': _observations['hue_sat_green']?.worked,
        'blue': _observations['hue_sat_blue']?.worked,
      },
      'color_mode_switching': {
        'ct_to_xy': _observations['ct_to_xy']?.worked,
        'xy_to_ct': _observations['xy_to_ct']?.worked,
        'ct_to_hue_sat': _observations['ct_to_hue_sat']?.worked,
        'hue_sat_to_ct': _observations['hue_sat_to_ct']?.worked,
      },
      'transitions': {
        'dim_ramp': _observations['dim_ramp']?.worked,
        'transition_behavior': _transitionBehavior(),
      },
      'rapid_commands': {
        'zero_gap': _observations['rapid_commands']?.worked,
        for (final gapMs in _rapidGapsMs)
          '${gapMs}ms': _observations['rapid_${gapMs}ms']?.worked,
      },
      'power_on_behavior': {
        'restored_warm_50_percent': _observations['power_on_behavior']?.worked,
      },
    };
  }

  Map<String, dynamic> _readbackConsistency() {
    return {
      'onoff': 'tracked',
      'level': 'tracked_when_native_bridge_available',
      'color': 'tracked_when_native_bridge_available',
      'by_test': {
        for (final step in _steps)
          if (_observations[step.id] != null)
            step.id: {
              'command_ack': _commandAckStatus(
                _observations[step.id]?.serverResult,
              ),
              'onoff_readback': _onOffReadbackStatus(
                _observations[step.id]?.serverResult,
              ),
              'visible_behavior': _answerLabel(_observations[step.id]!),
            },
      },
    };
  }

  Map<String, dynamic> _recommendedControlStrategy() {
    final minBrightness = _capabilityHints()['min_brightness'];
    final spacingMs = _recommendedCommandSpacingMs();
    return {
      'turn_on_sequence': _turnOnSequence(),
      'brightness_command': _preferredLevelCommand(),
      'preferred_level_command': _preferredLevelCommand(),
      'color_command': _preferredColorCommand(),
      'min_brightness': minBrightness,
      'transition_behavior': _transitionBehavior(),
      'recommended_command_spacing_ms': spacingMs,
      'on_restores_previous_level': _observations['on_level_restore']?.worked,
      'power_on_behavior': _powerOnBehavior(),
    };
  }

  List<dynamic> _inferredBehavioralQuirks() {
    final quirks = <dynamic>[];
    final xyResults = [
      _observations['xy_red']?.worked,
      _observations['xy_green']?.worked,
      _observations['xy_blue']?.worked,
    ];
    final hueSatResults = [
      _observations['hue_sat_red']?.worked,
      _observations['hue_sat_green']?.worked,
      _observations['hue_sat_blue']?.worked,
    ];
    final ctResults = [
      _observations['color_temperature_warm']?.worked,
      _observations['color_temperature_cool']?.worked,
    ];
    final xyWasTested = xyResults.any((worked) => worked != null);
    final xyAllFailed =
        xyWasTested && xyResults.every((worked) => worked == false);
    final hueSatWorks = hueSatResults.any((worked) => worked == true);
    final ctWorks = ctResults.any((worked) => worked == true);

    if (_observations['on_level_restore']?.worked == false) {
      quirks.add('on_does_not_restore_previous_level');
    }
    if (_observations['power_on_behavior']?.worked == true) {
      quirks.add({'power_on_behavior': 'restore_previous'});
    } else if (_observations['power_on_behavior']?.worked == false) {
      quirks.add({'power_on_behavior': 'not_restore_previous'});
    }
    if (xyAllFailed) {
      quirks.add('xy_color_commands_ack_but_no_visible_change');
    }

    final modeSwitchResults = <String, bool?>{
      if (!xyAllFailed) 'ct_to_xy_fails': _observations['ct_to_xy']?.worked,
      if (!xyAllFailed && ctWorks)
        'xy_to_ct_fails': _observations['xy_to_ct']?.worked,
      if (hueSatWorks)
        'ct_to_hue_sat_fails': _observations['ct_to_hue_sat']?.worked,
      if (hueSatWorks && ctWorks)
        'hue_sat_to_ct_fails': _observations['hue_sat_to_ct']?.worked,
    };
    for (final entry in modeSwitchResults.entries) {
      if (entry.value == false) quirks.add(entry.key);
    }
    return quirks;
  }

  List<String> _reportNotes(String operatorNotes) {
    final notes = <String>[];
    if (operatorNotes.isNotEmpty) notes.add(operatorNotes);
    return notes;
  }

  String _answerLabel(_StepObservation observation) {
    if (!observation.answered) return 'unanswered';
    if (observation.worked == true) return 'yes';
    if (observation.worked == false) return 'no';
    return 'unsure';
  }

  String _commandAckStatus(Map<String, dynamic>? serverResult) {
    if (serverResult == null) return 'not_run';
    final failedCount = serverResult['failed_count'];
    final commandCount = serverResult['command_count'];
    if (failedCount is int && failedCount == 0) return 'success';
    if (failedCount is int &&
        commandCount is int &&
        failedCount < commandCount) {
      return 'partial';
    }
    if (failedCount is int) return 'failed';
    return serverResult['status']?.toString() ?? 'unknown';
  }

  String _onOffReadbackStatus(Map<String, dynamic>? serverResult) {
    if (serverResult == null) return 'not_run';
    var sawOk = false;
    var sawError = false;

    void inspectReadback(dynamic value) {
      if (value is! Map) return;
      final onOff = value['onoff'];
      if (onOff is! Map) return;
      if (onOff['ok'] == true) sawOk = true;
      if (onOff['ok'] == false) sawError = true;
    }

    final commands = serverResult['commands'];
    if (commands is List) {
      for (final command in commands) {
        if (command is! Map) continue;
        inspectReadback(command['readback_before']);
        inspectReadback(command['readback_after_immediate']);
        inspectReadback(command['readback_after_500ms']);
        inspectReadback(command['readback_after_1500ms']);
      }
    }

    if (!sawOk && !sawError) return 'not_available';
    if (sawOk && sawError) return 'mixed';
    if (sawOk) return 'available';
    return 'failed';
  }

  String _turnOnSequence() {
    final brightnessWithoutOn = _observations['brightness_without_on']?.worked;
    final brightnessWithOn = _observations['brightness_with_on']?.worked;
    final moveToLevelWithOnOff =
        _observations['level_move_to_level_with_onoff']?.worked;
    if (brightnessWithoutOn == true) return 'level_command_turns_on';
    if (brightnessWithoutOn == false &&
        (brightnessWithOn == true || moveToLevelWithOnOff == true)) {
      return 'explicit_on_then_level';
    }
    if (brightnessWithoutOn == false && brightnessWithOn == false) {
      return 'explicit_on_then_level_unreliable';
    }
    return 'unknown';
  }

  String _preferredLevelCommand() {
    if (_observations['level_move_to_level_with_onoff']?.worked == true) {
      return 'move_to_level_with_onoff';
    }
    if (_observations['brightness_with_on']?.worked == true) {
      return 'move_to_level_with_onoff';
    }
    if (_observations['level_move_to_level']?.worked == true) {
      return 'move_to_level';
    }
    if (_observations['level_step_with_onoff']?.worked == true) {
      return 'step_with_onoff';
    }
    if (_observations['level_step']?.worked == true) {
      return 'step';
    }
    return 'move_to_level_with_onoff';
  }

  String _preferredColorCommand() {
    final hueSatWorked = [
      _observations['hue_sat_red']?.worked,
      _observations['hue_sat_green']?.worked,
      _observations['hue_sat_blue']?.worked,
    ].any((worked) => worked == true);
    if (hueSatWorked) return 'hue_saturation';

    final xyWorked = [
      _observations['xy_red']?.worked,
      _observations['xy_green']?.worked,
      _observations['xy_blue']?.worked,
    ].any((worked) => worked == true);
    if (xyWorked) return 'xy';

    final colorTemperatureWorked = [
      _observations['color_temperature_warm']?.worked,
      _observations['color_temperature_cool']?.worked,
    ].any((worked) => worked == true);
    if (colorTemperatureWorked) return 'color_temperature';

    return 'onoff_or_dimming_only';
  }

  String _transitionBehavior() {
    final worked = _observations['dim_ramp']?.worked;
    if (worked == true) return 'smooth';
    if (worked == false) return 'instant_jump_or_ignored';
    return 'unknown';
  }

  int? _recommendedCommandSpacingMs() {
    for (final gapMs in _rapidGapsMs) {
      if (_observations['rapid_${gapMs}ms']?.worked == true) return gapMs;
    }
    final anyRapidFailure = _observations['rapid_commands']?.worked == false ||
        _rapidGapsMs.any(
          (gapMs) => _observations['rapid_${gapMs}ms']?.worked == false,
        );
    if (_observations['rapid_commands']?.worked == true && !anyRapidFailure) {
      return 50;
    }
    if (anyRapidFailure) {
      final throttleMs = int.tryParse(_throttleController.text.trim()) ?? 250;
      return throttleMs.clamp(50, 2000);
    }
    return null;
  }

  bool _needsThrottleInput() {
    if (_observations['rapid_commands']?.worked == false) return true;
    return _rapidGapsMs.any(
      (gapMs) => _observations['rapid_${gapMs}ms']?.worked == false,
    );
  }

  String _powerOnBehavior() {
    final worked = _observations['power_on_behavior']?.worked;
    if (worked == true) return 'restore_previous';
    if (worked == false) return 'not_restore_previous';
    return 'unknown';
  }

  List<dynamic> _inferredQuirks() {
    final quirks = <dynamic>[];
    final brightnessWithoutOn = _observations['brightness_without_on']?.worked;
    final brightnessWithOn = _observations['brightness_with_on']?.worked;
    if (brightnessWithoutOn == false && brightnessWithOn != false) {
      quirks.add('needs_explicit_on');
    }

    final colorTemperatureResults = [
      _observations['color_temperature_warm']?.worked,
      _observations['color_temperature_cool']?.worked,
    ];
    final xyResults = [
      _observations['xy_red']?.worked,
      _observations['xy_green']?.worked,
      _observations['xy_blue']?.worked,
    ];
    final colorTemperatureFailed =
        colorTemperatureResults.any((worked) => worked == false);
    final xyWorked = xyResults.any((worked) => worked == true);
    if (colorTemperatureFailed && xyWorked) {
      quirks.add('needs_xy_not_ct');
    }

    final spacingMs = _recommendedCommandSpacingMs();
    if (spacingMs != null && spacingMs > 50) {
      quirks.add({'command_throttle_ms': spacingMs});
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
