import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../providers/server_sync_provider.dart';
import '../../services/analytics_service.dart';
import '../../services/monster_cloud_service.dart';
import '../../widgets/solar_orbit.dart';
import '../../widgets/stage_timeline.dart';

class MonsterPairedDevice {
  const MonsterPairedDevice({
    required this.nativeDeviceId,
    required this.name,
    required this.deviceType,
    this.manufacturer,
    this.model,
  });

  final String nativeDeviceId;
  final String name;
  final String deviceType;
  final String? manufacturer;
  final String? model;
}

class MonsterDevicePairingResult {
  const MonsterDevicePairingResult({
    required this.device,
    this.warnings = const [],
  });

  final MonsterPairedDevice device;
  final List<String> warnings;
}

class MonsterDiscoveredCandidate {
  const MonsterDiscoveredCandidate({required this.dsn, required this.address});

  final String dsn;
  final String address;

  String get dsnSuffix => dsn.length <= 4
      ? dsn.toUpperCase()
      : dsn.substring(dsn.length - 4).toUpperCase();
}

typedef MonsterPairingRequest = Future<Map<String, dynamic>?> Function({
  required String hubType,
  required Map<String, dynamic> params,
  required Duration receiveTimeout,
  required String sessionId,
});

/// Stage names shared with the appliance's `rhythm-monster` hub.
abstract final class MonsterPairingStage {
  static const discover = 'discover';
  static const provision = 'provision';
  static const adopt = 'adopt';
}

/// Drives the staged Monster onboarding: the Rhythm Box finds the strip and
/// joins it to Wi-Fi over Bluetooth, the app brokers the cloud registration,
/// and the Box adopts the LAN key only after a signed readback.
class MonsterDeviceAddScreen extends StatefulWidget {
  const MonsterDeviceAddScreen({
    super.key,
    this.analyticsSource = 'unknown',
    this.journeyId,
    this.inputMethod = 'nearby_sheet',
    @visibleForTesting this.pairingRequest,
    @visibleForTesting this.cloudService,
    @visibleForTesting this.progressEvents,
    @visibleForTesting this.completeRetryDelay = const Duration(seconds: 5),
    @visibleForTesting this.completeRetryLimit = 12,
  });

  final String analyticsSource;
  final String? journeyId;
  final String inputMethod;
  final MonsterPairingRequest? pairingRequest;
  final MonsterCloudService? cloudService;
  final Stream<RhythmPairingProgress>? progressEvents;
  final Duration completeRetryDelay;
  final int completeRetryLimit;

  static Future<MonsterDevicePairingResult?> show(
    BuildContext context, {
    String analyticsSource = 'unknown',
    String? journeyId,
    String inputMethod = 'nearby_sheet',
  }) {
    return Navigator.of(context).push<MonsterDevicePairingResult>(
      MaterialPageRoute(
        builder: (_) => MonsterDeviceAddScreen(
          analyticsSource: analyticsSource,
          journeyId: journeyId,
          inputMethod: inputMethod,
        ),
      ),
    );
  }

  @override
  State<MonsterDeviceAddScreen> createState() => _MonsterDeviceAddScreenState();
}

class _MonsterDeviceAddScreenState extends State<MonsterDeviceAddScreen> {
  static const _accent = Color(0xFF7C4DFF);
  static const _danger = Color(0xFFEF5350);
  static const _discoverTimeout = Duration(seconds: 45);
  static const _provisionTimeout = Duration(minutes: 3);
  static const _adoptTimeout = Duration(seconds: 75);
  static const _stages = [
    StageTimelineItem(label: 'Find the strip', icon: Icons.radar_rounded),
    StageTimelineItem(label: 'Join Wi-Fi', icon: Icons.wifi_rounded),
    StageTimelineItem(
        label: 'Register with Monster', icon: Icons.cloud_rounded),
    StageTimelineItem(label: 'Add to Rhythm', icon: Icons.light_mode_rounded),
  ];

  late final String _journeyId = widget.journeyId ?? 'monster-pair';
  StreamSubscription<RhythmPairingProgress>? _progressSubscription;
  String? _activeSessionId;
  int _attemptNumber = 0;
  int _stageIndex = 0;
  String? _message;
  bool _running = false;
  bool _failed = false;
  bool _flowCompleted = false;
  String? _error;

  /// Retained across retries so an already-provisioned strip is never sent a
  /// second Wi-Fi payload; only registration and adoption are repeated.
  String? _retainedDsn;
  String? _retainedTicket;
  bool _provisioned = false;
  bool _uncertainProvision = false;

  MonsterCloudService get _cloud =>
      widget.cloudService ?? MonsterCloudService.instance;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _start();
    });
  }

  @override
  void dispose() {
    _progressSubscription?.cancel();
    super.dispose();
  }

  Stream<RhythmPairingProgress>? get _pairingProgressStream {
    final injected = widget.progressEvents;
    if (injected != null) return injected;
    return context
        .read<ServerSyncProvider?>()
        ?.connection
        .pairingProgressEvents;
  }

  void _subscribeToProgress() {
    _progressSubscription?.cancel();
    _progressSubscription = _pairingProgressStream?.listen((event) {
      if (!mounted || event.hubType != 'monster' || _flowCompleted) return;
      if (event.sessionId != _activeSessionId) return;
      if (event.stage == RhythmPairingStage.complete ||
          event.stage == RhythmPairingStage.failed) {
        return;
      }
      final message = event.message.trim();
      if (message.isEmpty) return;
      setState(() => _message = message);
    });
  }

  Future<Map<String, dynamic>?> _pair(
    String stage,
    Map<String, dynamic> params,
    Duration timeout,
  ) {
    _activeSessionId = '$_journeyId-$stage-$_attemptNumber';
    final request = widget.pairingRequest;
    final body = {'stage': stage, ...params};
    if (request != null) {
      return request(
        hubType: 'monster',
        params: body,
        receiveTimeout: timeout,
        sessionId: _activeSessionId!,
      );
    }
    return context.read<ServerSyncProvider>().api.pairDevice(
          hubType: 'monster',
          params: body,
          receiveTimeout: timeout,
          sessionId: _activeSessionId!,
        );
  }

  Future<void> _start() async {
    if (_running || _flowCompleted) return;
    _running = true;
    _attemptNumber += 1;
    _subscribeToProgress();
    setState(() {
      _failed = false;
      _error = null;
      _uncertainProvision = false;
      _stageIndex = _provisioned ? 2 : 0;
      _message = null;
    });
    HapticFeedback.mediumImpact();
    AnalyticsService().logMonsterPairingAttempted(
      journeyId: _journeyId,
      source: widget.analyticsSource,
      inputMethod: widget.inputMethod,
      attemptNumber: _attemptNumber,
      resumed: _provisioned,
    );
    try {
      if (!_cloud.canUse) {
        _fail(
          'Sign in to your Rhythm account to add a Monster strip.',
          failureStage: 'not_signed_in',
        );
        return;
      }
      if (!_provisioned) {
        final candidate = await _discover();
        if (candidate == null || !mounted) return;
        final begin = await _cloud.begin(candidate.dsn);
        if (!mounted) return;
        if (!begin.ok || begin.setupToken == null || begin.ticket == null) {
          _fail(monsterCloudFailureMessage(begin), failureStage: 'cloud_begin');
          return;
        }
        _retainedDsn = candidate.dsn;
        _retainedTicket = begin.ticket;
        final provisioned = await _provision(candidate, begin.setupToken!);
        if (!provisioned || !mounted) return;
        _provisioned = true;
      }
      final credentials = await _complete();
      if (credentials == null || !mounted) return;
      await _adopt(credentials);
    } catch (_) {
      if (!mounted || _flowCompleted) return;
      _fail(
        'Could not reach the Rhythm Box. Check its connection and try again.',
        failureStage: 'request_exception',
      );
    } finally {
      _running = false;
    }
  }

  Future<MonsterDiscoveredCandidate?> _discover() async {
    setState(() {
      _stageIndex = 0;
      _message = 'Hold the strip\'s button until it blinks, then keep it near '
          'your Rhythm Box.';
    });
    final response = await _pair(
      MonsterPairingStage.discover,
      const {},
      _discoverTimeout,
    );
    if (!mounted) return null;
    final failure = _responseFailure(response);
    if (failure != null) {
      _fail(failure, failureStage: 'discover');
      return null;
    }
    final candidates = _candidatesFromResponse(response!);
    if (candidates.isEmpty) {
      _fail(
        'No Monster strip in setup mode was found. Hold its button until it '
        'blinks and try again.',
        failureStage: 'discover_empty',
      );
      return null;
    }
    if (candidates.length == 1) return candidates.single;
    final chosen = await showDialog<MonsterDiscoveredCandidate>(
      context: context,
      builder: (dialogContext) => SimpleDialog(
        title: const Text('Choose the strip to add'),
        children: [
          for (final candidate in candidates)
            SimpleDialogOption(
              key: ValueKey('monster-candidate-${candidate.dsn}'),
              onPressed: () => Navigator.of(dialogContext).pop(candidate),
              child: ListTile(
                contentPadding: EdgeInsets.zero,
                leading: const Icon(Icons.light_mode_rounded),
                title: Text('Neon Flow …${candidate.dsnSuffix}'),
                subtitle: const Text('Serial ends with these characters'),
              ),
            ),
        ],
      ),
    );
    if (chosen == null && mounted) {
      _fail('Choose a strip to continue.', failureStage: 'candidate_declined');
    }
    return chosen;
  }

  Future<bool> _provision(
    MonsterDiscoveredCandidate candidate,
    String setupToken,
  ) async {
    setState(() {
      _stageIndex = 1;
      _message = 'Sending your Wi-Fi details to the strip. This can take a '
          'minute.';
    });
    final response = await _pair(
      MonsterPairingStage.provision,
      {
        'dsn': candidate.dsn,
        'address': candidate.address,
        'setup_token': setupToken,
      },
      _provisionTimeout,
    );
    if (!mounted) return false;
    final failure = _responseFailure(response);
    if (failure == null) return true;
    final uncertain = _details(response)?['uncertain'] == true;
    _uncertainProvision = uncertain;
    _fail(
      uncertain
          ? '$failure If the strip\'s light shows it joined Wi-Fi, continue '
              'without sending Wi-Fi again.'
          : failure,
      failureStage: uncertain ? 'provision_uncertain' : 'provision',
    );
    return false;
  }

  /// Continue after an uncertain Wi-Fi write: the person confirmed the strip
  /// joined, so registration and adoption run with the retained ticket.
  Future<void> _continueAfterUncertainProvision() async {
    if (_retainedDsn == null || _retainedTicket == null) return;
    _provisioned = true;
    await _start();
  }

  Future<MonsterCloudResponse?> _complete() async {
    final dsn = _retainedDsn;
    final ticket = _retainedTicket;
    if (dsn == null || ticket == null) {
      _fail('This setup session expired. Start over from the scan.',
          failureStage: 'ticket_missing');
      return null;
    }
    setState(() {
      _stageIndex = 2;
      _message = 'Registering the strip with Monster and fetching its key.';
    });
    MonsterCloudResponse response = await _cloud.complete(dsn, ticket);
    var attempts = 0;
    while (!response.ok && response.lanPending && mounted) {
      attempts += 1;
      if (attempts > widget.completeRetryLimit) break;
      setState(() {
        _message = 'Waiting for the strip to appear on your network…';
      });
      await Future<void>.delayed(widget.completeRetryDelay);
      if (!mounted) return null;
      response = await _cloud.complete(dsn, ticket);
    }
    if (!mounted) return null;
    if (!response.ok || !response.hasCredentials) {
      final expired = response.error == 'invalid_ticket';
      if (expired) {
        _provisioned = false;
        _retainedTicket = null;
      }
      _fail(
        monsterCloudFailureMessage(response),
        failureStage:
            response.lanPending ? 'cloud_lan_pending' : 'cloud_complete',
      );
      return null;
    }
    if (response.dsn != dsn) {
      _fail('The cloud returned a different strip. Start over from the scan.',
          failureStage: 'cloud_identity');
      return null;
    }
    return response;
  }

  Future<void> _adopt(MonsterCloudResponse credentials) async {
    setState(() {
      _stageIndex = 3;
      _message = 'Checking the strip on your network.';
    });
    final response = await _pair(
      MonsterPairingStage.adopt,
      credentials.adoptParams(),
      _adoptTimeout,
    );
    if (!mounted) return;
    final failure = _responseFailure(response);
    if (failure != null) {
      _fail(failure, failureStage: 'adopt');
      return;
    }
    final device = _deviceFromResponse(response!);
    if (device == null) {
      _fail('The Rhythm Box finished but did not report the new light.',
          failureStage: 'adopt_missing_device');
      return;
    }
    _flowCompleted = true;
    AnalyticsService().logMonsterPairingCompleted(
      journeyId: _journeyId,
      source: widget.analyticsSource,
      inputMethod: widget.inputMethod,
      attemptNumber: _attemptNumber,
      outcome: 'succeeded',
    );
    HapticFeedback.heavyImpact();
    Navigator.of(context).pop(
      MonsterDevicePairingResult(
        device: device,
        warnings: _responseWarnings(response),
      ),
    );
  }

  void _fail(String message, {required String failureStage}) {
    if (!mounted || _flowCompleted) return;
    AnalyticsService().logMonsterPairingCompleted(
      journeyId: _journeyId,
      source: widget.analyticsSource,
      inputMethod: widget.inputMethod,
      attemptNumber: _attemptNumber,
      outcome: 'failed',
      failureStage: failureStage,
    );
    HapticFeedback.lightImpact();
    setState(() {
      _failed = true;
      _error = message;
    });
  }

  String? _responseFailure(Map<String, dynamic>? response) {
    if (response == null) {
      return 'The Rhythm Box did not answer the pairing request.';
    }
    final httpStatus = response['http_status'] as int?;
    if (httpStatus != null && httpStatus != 200) {
      return _responseError(response) ??
          'The Rhythm Box rejected the pairing request.';
    }
    if (response['status'] == 'failed') {
      return _responseError(response) ?? 'Monster setup failed.';
    }
    if (response['status'] != 'complete') {
      return _responseError(response) ??
          'The Rhythm Box returned an unexpected pairing result.';
    }
    return null;
  }

  Map<String, dynamic>? _details(Map<String, dynamic>? response) =>
      _jsonMap(response?['details']);

  List<MonsterDiscoveredCandidate> _candidatesFromResponse(
    Map<String, dynamic> response,
  ) {
    final raw = _details(response)?['candidates'];
    if (raw is! List) return const [];
    final candidates = <MonsterDiscoveredCandidate>[];
    for (final entry in raw) {
      final map = _jsonMap(entry);
      final dsn = map?['dsn']?.toString().trim() ?? '';
      final address = map?['address']?.toString().trim() ?? '';
      if (dsn.isEmpty || address.isEmpty) continue;
      candidates.add(MonsterDiscoveredCandidate(dsn: dsn, address: address));
    }
    return candidates;
  }

  MonsterPairedDevice? _deviceFromResponse(Map<String, dynamic> response) {
    final devices = response['devices'];
    final raw = devices is List && devices.isNotEmpty
        ? devices.first
        : response['device'];
    final device = _jsonMap(raw);
    if (device == null) return null;
    final id = (device['device_id'] ?? device['native_id'] ?? device['id'])
            ?.toString()
            .trim() ??
        '';
    if (id.isEmpty) return null;
    return MonsterPairedDevice(
      nativeDeviceId: id,
      name: device['name'] as String? ?? 'Monster Neon Flow',
      deviceType: device['device_type'] as String? ?? 'light',
      manufacturer: device['manufacturer'] as String?,
      model: device['model'] as String?,
    );
  }

  String? _responseError(Map<String, dynamic> response) {
    final error = response['error'];
    if (error is String && error.trim().isNotEmpty) return error.trim();
    if (error is Map) {
      final message = error['message'];
      if (message is String && message.trim().isNotEmpty) {
        return message.trim();
      }
    }
    return null;
  }

  List<String> _responseWarnings(Map<String, dynamic> response) {
    return (response['warnings'] as List? ?? const [])
        .whereType<String>()
        .map((warning) => warning.trim())
        .where((warning) => warning.isNotEmpty)
        .toList(growable: false);
  }

  Map<String, dynamic>? _jsonMap(Object? value) {
    if (value is Map<String, dynamic>) return value;
    if (value is Map) return value.cast<String, dynamic>();
    return null;
  }

  @override
  Widget build(BuildContext context) {
    final message = _failed
        ? _error
        : (_message ??
            'Keep the strip powered on and close to your Rhythm Box.');
    return PopScope(
      canPop: !_running,
      onPopInvokedWithResult: (didPop, _) {
        if (didPop || !_running) return;
        ScaffoldMessenger.of(context)
          ..hideCurrentSnackBar()
          ..showSnackBar(
            const SnackBar(
              content: Text(
                'Setup is still in progress. Keep this screen open until it '
                'finishes.',
              ),
            ),
          );
      },
      child: Scaffold(
        key: const ValueKey('monster-pairing-screen'),
        backgroundColor: CelestialColors.backgroundDark,
        appBar: AppBar(
          backgroundColor: CelestialColors.backgroundDark,
          foregroundColor: CelestialColors.textPrimary,
          title: const Text('Add Monster Neon Flow'),
          elevation: 0,
        ),
        body: SafeArea(
          top: false,
          child: ListView(
            padding: const EdgeInsets.fromLTRB(24, 16, 24, 28),
            children: [
              Align(
                alignment: Alignment.centerLeft,
                child: Container(
                  width: 72,
                  height: 72,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: _accent.withValues(alpha: 0.14),
                    border: Border.all(color: _accent.withValues(alpha: 0.42)),
                  ),
                  child: const Icon(
                    Icons.light_mode_rounded,
                    color: _accent,
                    size: 36,
                  ),
                ),
              ),
              const SizedBox(height: 20),
              const Text(
                'Adding a Monster Neon Flow strip',
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 25,
                  height: 1.15,
                  fontWeight: FontWeight.w700,
                  letterSpacing: -0.4,
                ),
              ),
              const SizedBox(height: 9),
              Text(
                'Your Rhythm Box joins the strip to its Wi-Fi network and '
                'registers it with Monster using Rhythm\'s account. No Monster '
                'login is needed.',
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.84),
                  fontSize: 14,
                  height: 1.45,
                ),
              ),
              const SizedBox(height: 26),
              Container(
                padding: const EdgeInsets.all(18),
                decoration: BoxDecoration(
                  color: CelestialColors.backgroundCard,
                  borderRadius: BorderRadius.circular(18),
                  border: Border.all(
                    color:
                        (_failed ? _danger : _accent).withValues(alpha: 0.34),
                  ),
                ),
                child: StageTimeline(
                  stages: _stages,
                  activeIndex: _stageIndex,
                  activeMessage: message,
                  failed: _failed,
                  accent: _accent,
                ),
              ),
              if (_failed && _error != null) ...[
                const SizedBox(height: 12),
                Text(
                  _error!,
                  key: const ValueKey('monster-pairing-error'),
                  style: const TextStyle(
                    color: _danger,
                    fontSize: 13,
                    height: 1.4,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ],
              if (_failed) ...[
                const SizedBox(height: 18),
                if (_uncertainProvision) ...[
                  SizedBox(
                    height: 52,
                    child: FilledButton.icon(
                      key: const ValueKey('monster-continue-registration'),
                      onPressed:
                          _running ? null : _continueAfterUncertainProvision,
                      style: FilledButton.styleFrom(
                        backgroundColor: _accent,
                        foregroundColor: Colors.white,
                      ),
                      icon: const Icon(Icons.cloud_done_rounded),
                      label: const Text(
                        'Strip joined Wi-Fi, continue',
                        style: TextStyle(fontWeight: FontWeight.w700),
                      ),
                    ),
                  ),
                  const SizedBox(height: 10),
                ],
                SizedBox(
                  height: 52,
                  child: FilledButton.icon(
                    key: const ValueKey('monster-retry'),
                    onPressed: _running
                        ? null
                        : () {
                            if (_uncertainProvision) {
                              // A fresh scan means a fresh ticket; never reuse
                              // the token of an uncertain write.
                              _provisioned = false;
                              _retainedTicket = null;
                              _retainedDsn = null;
                            }
                            _start();
                          },
                    style: FilledButton.styleFrom(
                      backgroundColor: _uncertainProvision
                          ? CelestialColors.backgroundCard
                          : _accent,
                      foregroundColor: Colors.white,
                    ),
                    icon: const Icon(Icons.refresh_rounded),
                    label: Text(
                      _provisioned && !_uncertainProvision
                          ? 'Retry Registration'
                          : 'Scan Again',
                      style: const TextStyle(fontWeight: FontWeight.w700),
                    ),
                  ),
                ),
                const SizedBox(height: 10),
                SizedBox(
                  height: 48,
                  child: TextButton(
                    key: const ValueKey('monster-cancel'),
                    onPressed:
                        _running ? null : () => Navigator.of(context).pop(),
                    child: const Text('Cancel'),
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}
