import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../providers/server_sync_provider.dart';
import '../../services/analytics_service.dart';
import '../../services/device_pairing_code.dart';
import '../../services/server_identity.dart';
import '../../services/settings_service.dart';
import '../../widgets/solar_orbit.dart';

typedef LocalBlePairingRequest = Future<Map<String, dynamic>?> Function({
  required String hubType,
  required Map<String, dynamic> params,
  required Duration receiveTimeout,
  required String sessionId,
});

typedef LocalBlePairingStatusRequest = Future<RhythmPairingResultStatus?>
    Function(String sessionId);
typedef LocalBlePairingAcknowledgementRequest = Future<bool> Function(
  String sessionId,
);

@visibleForTesting
({bool matches, String? latchedInstanceScope})
    evaluatePinnedLocalBleServerScope({
  required String routeScope,
  required bool routeUsesFallbackIdentity,
  required String currentFallbackScope,
  required String currentCanonicalScope,
  required bool currentHasInstanceIdentity,
  String? latchedInstanceScope,
}) {
  if (!routeUsesFallbackIdentity) {
    return (
      matches: currentCanonicalScope == routeScope,
      latchedInstanceScope: latchedInstanceScope,
    );
  }
  if (currentFallbackScope != routeScope) {
    return (
      matches: false,
      latchedInstanceScope: latchedInstanceScope,
    );
  }
  if (!currentHasInstanceIdentity) {
    return (
      matches: false,
      latchedInstanceScope: latchedInstanceScope,
    );
  }
  final latched = latchedInstanceScope ?? currentCanonicalScope;
  return (
    matches: currentCanonicalScope == latched,
    latchedInstanceScope: latched,
  );
}

abstract interface class LocalBlePendingPairingStore {
  Future<PendingLocalBlePairing?> load();
  Future<bool> save(PendingLocalBlePairing pairing);
  Future<bool> clear({String? sessionId});
}

typedef LocalBlePendingPairingStoreFactory = LocalBlePendingPairingStore
    Function(String serverScope);

class SettingsLocalBlePendingPairingStore
    implements LocalBlePendingPairingStore {
  const SettingsLocalBlePendingPairingStore(this.serverScope);

  final String serverScope;

  @override
  Future<PendingLocalBlePairing?> load() =>
      SettingsService.instance.loadPendingLocalBlePairing(serverScope);

  @override
  Future<bool> save(PendingLocalBlePairing pairing) {
    if (pairing.serverScope != serverScope) return Future.value(false);
    return SettingsService.instance.savePendingLocalBlePairing(pairing);
  }

  @override
  Future<bool> clear({String? sessionId}) =>
      SettingsService.instance.clearPendingLocalBlePairing(
        serverScope: serverScope,
        sessionId: sessionId,
      );
}

class _NoopLocalBlePendingPairingStore implements LocalBlePendingPairingStore {
  const _NoopLocalBlePendingPairingStore();

  @override
  Future<PendingLocalBlePairing?> load() async => null;

  @override
  Future<bool> save(PendingLocalBlePairing pairing) async => true;

  @override
  Future<bool> clear({String? sessionId}) async => true;
}

/// Validates the global pointer collection without assigning any record to an
/// unidentified route. This lets malformed storage fail closed before hello
/// while keeping Hub-ID fallback scopes entirely out of persistence.
class _ValidateOnlyLocalBlePendingPairingStore
    implements LocalBlePendingPairingStore {
  const _ValidateOnlyLocalBlePendingPairingStore();

  @override
  Future<PendingLocalBlePairing?> load() async {
    await SettingsService.instance.loadPendingLocalBlePairings();
    return null;
  }

  @override
  Future<bool> save(PendingLocalBlePairing pairing) async => false;

  @override
  Future<bool> clear({String? sessionId}) async => false;
}

/// Pairs one device through a server-advertised local-BLE profile.
///
/// The parsed setup fields are sent instead of the original QR payload. Once
/// pairing starts, this route stays open until a correlated HTTP or SSE
/// terminal result arrives so a successful appliance-side association cannot
/// become an invisible "ghost" pairing.
class LocalBleDeviceAddScreen extends StatefulWidget {
  const LocalBleDeviceAddScreen({
    super.key,
    required this.setup,
    this.pairingProfileId,
    required this.inputMethod,
    this.analyticsSource = 'unknown',
    required this.journeyId,
    @visibleForTesting this.pairingRequest,
    @visibleForTesting this.pairingStatusRequest,
    @visibleForTesting this.pairingAcknowledgementRequest,
    @visibleForTesting this.progressEvents,
    @visibleForTesting this.pendingPairingStore,
    @visibleForTesting this.pendingPairingStoreFactory,
    @visibleForTesting this.serverScope,
    @visibleForTesting this.statusPollInterval = const Duration(seconds: 2),
    // Rust owns a 120-second absolute completion budget. Keep 15 seconds for
    // the correlated HTTP/SSE terminal result to reach and reconcile here.
    @visibleForTesting this.pairingDeadline = const Duration(seconds: 135),
  });

  final LocalBleSetup setup;

  /// Server-advertised current ID resolved from [setup]'s parser ID.
  /// Tests that exercise the screen in isolation may omit it when both match.
  final String? pairingProfileId;
  final String inputMethod;
  final String analyticsSource;
  final String journeyId;
  final LocalBlePairingRequest? pairingRequest;
  final LocalBlePairingStatusRequest? pairingStatusRequest;
  final LocalBlePairingAcknowledgementRequest? pairingAcknowledgementRequest;
  final Stream<RhythmPairingProgress>? progressEvents;
  final LocalBlePendingPairingStore? pendingPairingStore;
  final LocalBlePendingPairingStoreFactory? pendingPairingStoreFactory;
  final String? serverScope;
  final Duration statusPollInterval;
  final Duration pairingDeadline;

  static Future<RhythmPairedDevice?> show(
    BuildContext context, {
    required LocalBleSetup setup,
    required String pairingProfileId,
    required String inputMethod,
    required String analyticsSource,
    required String journeyId,
  }) {
    return Navigator.of(context).push<RhythmPairedDevice>(
      MaterialPageRoute(
        builder: (_) => LocalBleDeviceAddScreen(
          setup: setup,
          pairingProfileId: pairingProfileId,
          inputMethod: inputMethod,
          analyticsSource: analyticsSource,
          journeyId: journeyId,
        ),
      ),
    );
  }

  @override
  State<LocalBleDeviceAddScreen> createState() =>
      _LocalBleDeviceAddScreenState();
}

class _LocalBleDeviceAddScreenState extends State<LocalBleDeviceAddScreen> {
  static const _accent = Color(0xFF26A69A);

  bool _pairing = false;
  bool _completed = false;
  String? _error;
  String? _progressMessage;
  int _attemptNumber = 0;
  int _requestGeneration = 0;
  String _activeSessionId = '';
  StreamSubscription<RhythmPairingProgress>? _progressSubscription;
  Timer? _deadlineTimer;
  Timer? _statusPollTimer;
  bool _statusPollInFlight = false;
  bool _restoringPending = true;
  bool _pendingPairingStoreUnreadable = false;
  bool _reconciliationUnresolved = false;
  bool _checkingUnresolved = false;
  String? _reconciliationMessage;
  String _activeAnalyticsJourneyId = '';
  String _activeProfileId = '';
  String _activePointerServerScope = '';
  int _activeStartedAtEpochMs = 0;
  RhythmPairedDevice? _successfulDevice;
  List<String> _successWarnings = const [];
  bool _terminalFailureNeedsAcknowledgement = false;
  String? _terminalFailureMessage;
  bool _terminalCacheInFlight = false;
  String? _ownedServerScope;
  String? _ownedSessionId;

  late final LocalBlePendingPairingStore _routePendingPairingStore =
      widget.pendingPairingStore ??
          (widget.pairingRequest != null
              ? const _NoopLocalBlePendingPairingStore()
              : _routeUsesFallbackIdentity
                  ? const _ValidateOnlyLocalBlePendingPairingStore()
                  : _createPersistentPendingPairingStore(_routeServerScope));
  LocalBlePendingPairingStore? _activePendingPairingStore;

  LocalBlePendingPairingStore get _pendingPairingStore =>
      _activePendingPairingStore ?? _routePendingPairingStore;

  late final String _routeServerScope;
  late final bool _routeUsesFallbackIdentity;
  String? _latchedServerInstanceScope;

  ({String canonical, bool usesFallbackIdentity}) _resolveServerScope() {
    final injected = widget.serverScope?.trim();
    if (injected != null && injected.isNotEmpty) {
      return (
        canonical: injected,
        usesFallbackIdentity: false,
      );
    }
    if (widget.pairingRequest != null) {
      return (
        canonical: 'injected-server',
        usesFallbackIdentity: false,
      );
    }
    final sync = context.read<ServerSyncProvider?>();
    final hub = sync?.connectedServerHub;
    if (hub == null) {
      return (canonical: '', usesFallbackIdentity: false);
    }
    final durableScope = localBlePairingConnectedServerScope(
      hub,
      sync?.connectedServerInstanceId,
    );
    final connectedIdentityIsDurable =
        serverIdentityKind(sync?.connectedServerInstanceId) ==
            ServerIdentityKind.durable;
    final hasDurableIdentity = connectedIdentityIsDurable ||
        serverIdentityKind(hub.serverInstanceId) == ServerIdentityKind.durable;
    if (durableScope == null && hasDurableIdentity) {
      // Persisted and freshly authenticated identities disagree.
      return (canonical: '', usesFallbackIdentity: false);
    }
    return (
      canonical: durableScope ?? localBlePairingFallbackServerScope(hub),
      usesFallbackIdentity: !hasDurableIdentity,
    );
  }

  LocalBlePendingPairingStore _createPersistentPendingPairingStore(
    String serverScope,
  ) =>
      widget.pendingPairingStoreFactory?.call(serverScope) ??
      SettingsLocalBlePendingPairingStore(serverScope);

  String get _pairingProfileId =>
      widget.pairingProfileId ?? widget.setup.profileId;

  Map<String, dynamic> get _pairingParams => {
        ...widget.setup.pairingParams,
        'profile_id': _pairingProfileId,
      };

  @override
  void initState() {
    super.initState();
    // Resolve once before provider/widget updates can change the appliance
    // identity used by this route's durable store and ownership fence.
    final serverScope = _resolveServerScope();
    _routeServerScope = serverScope.canonical;
    _routeUsesFallbackIdentity = serverScope.usesFallbackIdentity;
    if (_routeServerScope.isEmpty) {
      _error = 'Connect to a Rhythm Box before starting Bluetooth pairing.';
    }
    AnalyticsService().logScreenView('local_ble_device_add');
    unawaited(_restorePendingPairing());
  }

  @override
  void dispose() {
    _releaseSessionOwnership();
    _deadlineTimer?.cancel();
    _statusPollTimer?.cancel();
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

  Future<Map<String, dynamic>?> _pair(String sessionId) {
    final injected = widget.pairingRequest;
    if (injected != null) {
      return injected(
        hubType: 'local_ble',
        params: _pairingParams,
        receiveTimeout: widget.pairingDeadline,
        sessionId: sessionId,
      );
    }
    if (!_currentServerMatchesRoute()) {
      return Future.value({
        'request_delivery': 'accepted_or_unknown',
        'error': 'The active Rhythm Box changed before pairing started.',
      });
    }
    return context.read<ServerSyncProvider>().api.pairDevice(
          hubType: 'local_ble',
          params: _pairingParams,
          receiveTimeout: widget.pairingDeadline,
          sessionId: sessionId,
        );
  }

  Future<RhythmPairingResultStatus?> _pairingStatus(String sessionId) {
    final injected = widget.pairingStatusRequest;
    if (injected != null) return injected(sessionId);
    if (widget.pairingRequest != null) return Future.value(null);
    if (!_currentServerMatchesRoute()) return Future.value(null);
    return context.read<ServerSyncProvider>().api.getPairingResult(sessionId);
  }

  Future<bool> _acknowledgeServerResult(String sessionId) {
    final injected = widget.pairingAcknowledgementRequest;
    if (injected != null) return injected(sessionId);
    if (widget.pairingRequest != null) return Future.value(true);
    if (!_currentServerMatchesRoute()) return Future.value(false);
    return context
        .read<ServerSyncProvider>()
        .api
        .acknowledgePairingResult(sessionId);
  }

  bool _currentServerMatchesRoute() {
    final sync = context.read<ServerSyncProvider?>();
    final hub = sync?.connectedServerHub;
    if (hub == null) return false;
    final connectedServerInstanceId = sync?.connectedServerInstanceId;
    final connectedIdentityIsDurable =
        serverIdentityKind(connectedServerInstanceId) ==
            ServerIdentityKind.durable;
    if (!connectedIdentityIsDurable) {
      // A durable persisted ID may scope local storage, but only a fresh hello
      // proves which appliance currently owns this transport.
      return false;
    }
    final currentScope = localBlePairingConnectedServerScope(
      hub,
      connectedServerInstanceId,
    );
    if (currentScope == null) return false;
    final evaluation = evaluatePinnedLocalBleServerScope(
      routeScope: _routeServerScope,
      routeUsesFallbackIdentity: _routeUsesFallbackIdentity,
      currentFallbackScope: localBlePairingFallbackServerScope(hub),
      currentCanonicalScope: currentScope,
      currentHasInstanceIdentity: connectedIdentityIsDurable,
      latchedInstanceScope: _latchedServerInstanceScope,
    );
    _latchedServerInstanceScope = evaluation.latchedInstanceScope;
    return evaluation.matches;
  }

  /// Captures the exact authenticated appliance scope for a new operation.
  ///
  /// A Hub-ID fallback may keep a route stable while its first hello is in
  /// flight, but it is never a persistence or request authority. Once a
  /// durable hello is accepted, this operation keeps one exact store for its
  /// pending pointer, terminal cache, acknowledgement, and local clear.
  String? _bindPendingStoreForNewAdmission() {
    if (widget.pairingRequest != null) {
      _activePendingPairingStore ??= _routePendingPairingStore;
      return _routeServerScope;
    }
    if (!_currentServerMatchesRoute()) return null;
    final durableScope = _routeUsesFallbackIdentity
        ? _latchedServerInstanceScope
        : _routeServerScope;
    if (durableScope == null || durableScope.isEmpty) return null;
    _activePendingPairingStore ??= widget.pendingPairingStore ??
        (_routeUsesFallbackIdentity
            ? _createPersistentPendingPairingStore(durableScope)
            : _routePendingPairingStore);
    return durableScope;
  }

  Future<void> _restorePendingPairing() async {
    try {
      final pending = await _routePendingPairingStore.load();
      if (!mounted || pending == null) return;
      _activePendingPairingStore = _routePendingPairingStore;
      final generation = ++_requestGeneration;
      _attemptNumber = pending.attemptNumber;
      _activeSessionId = pending.sessionId;
      _activeAnalyticsJourneyId = pending.journeyId;
      _activeProfileId = pending.profileId;
      _activePointerServerScope = pending.serverScope;
      _activeStartedAtEpochMs = pending.startedAtEpochMs;
      _claimSessionOwnership(
        pending.sessionId,
        serverScope: pending.serverScope,
      );
      final cachedTerminal = pending.terminalResult;
      if (cachedTerminal != null) {
        if (cachedTerminal.status == PendingLocalBleTerminalResult.complete) {
          _surfaceSuccessfulTerminal(
            RhythmPairedDevice(
              deviceId: cachedTerminal.deviceId!,
              name: cachedTerminal.deviceName!,
              deviceType: cachedTerminal.deviceType!,
              manufacturer: cachedTerminal.manufacturer,
              model: cachedTerminal.model,
            ),
            warnings: cachedTerminal.warnings,
            autoAcknowledge: false,
          );
        } else {
          _surfaceTerminalFailure(
            cachedTerminal.error?.trim().isNotEmpty == true
                ? cachedTerminal.error!.trim()
                : cachedTerminal.status ==
                        PendingLocalBleTerminalResult.notFound
                    ? 'The Rhythm Box has no record of the previous request. '
                        'You can start a new pairing safely.'
                    : 'Bluetooth pairing failed. You can start a new pairing.',
            failureStage:
                cachedTerminal.status == PendingLocalBleTerminalResult.notFound
                    ? 'terminal_status_not_found'
                    : 'terminal_status',
          );
        }
        return;
      }
      setState(() {
        _pairing = false;
        _reconciliationUnresolved = true;
        _checkingUnresolved = true;
        _error = null;
        _progressMessage =
            'Checking whether your previous Bluetooth pairing finished…';
      });
      _subscribeToProgress();
      await _pollPairingStatus(generation, surfaceUnresolved: true);
    } catch (_) {
      if (mounted) {
        setState(() {
          _pendingPairingStoreUnreadable = true;
          _reconciliationUnresolved = true;
          _reconciliationMessage =
              'Rhythm could not read the saved pairing recovery record. No '
              'new Bluetooth pairing will start until that record can be '
              'recovered.';
          _progressMessage = null;
          _error = null;
        });
      }
    } finally {
      if (mounted) {
        setState(() {
          _restoringPending = false;
          _checkingUnresolved = false;
        });
      }
    }
  }

  void _scheduleDeadline(int generation, Duration duration) {
    _deadlineTimer?.cancel();
    _deadlineTimer = Timer(duration, () {
      if (!mounted ||
          _completed ||
          !_pairing ||
          generation != _requestGeneration) {
        return;
      }
      _enterUnresolvedReconciliation(
        'Rhythm still cannot confirm whether pairing finished. You can leave '
        'this screen, but a new Bluetooth pairing is blocked until you check '
        'this session again.',
      );
    });
  }

  void _startStatusPolling(int generation, {bool pollImmediately = false}) {
    _statusPollTimer?.cancel();
    if (pollImmediately) unawaited(_pollPairingStatus(generation));
    _statusPollTimer = Timer.periodic(
      widget.statusPollInterval,
      (_) => unawaited(_pollPairingStatus(generation)),
    );
  }

  Future<void> _pollPairingStatus(
    int generation, {
    bool surfaceUnresolved = false,
  }) async {
    if (_statusPollInFlight ||
        !mounted ||
        _completed ||
        (!_pairing && !_reconciliationUnresolved) ||
        generation != _requestGeneration) {
      return;
    }
    _statusPollInFlight = true;
    try {
      final status = await _pairingStatus(_activeSessionId);
      if (!mounted ||
          _completed ||
          (!_pairing && !_reconciliationUnresolved) ||
          generation != _requestGeneration) {
        return;
      }
      if (status == null) {
        if (surfaceUnresolved) {
          _enterUnresolvedReconciliation(
            'Rhythm could not reach the Box to check this pairing. No new '
            'Bluetooth pairing will start until its status is known.',
          );
        }
        return;
      }
      if (status.sessionId != _activeSessionId ||
          (status.state != RhythmPairingResultState.notFound &&
              status.hubType != 'local_ble')) {
        _enterUnresolvedReconciliation(
          'The Rhythm Box returned a status for a different pairing. The '
          'saved session was kept so it can be checked again safely.',
        );
        return;
      }
      if (status.state == RhythmPairingResultState.notFound) {
        await _resolvePairingNotFound();
        return;
      }
      if (status.state == RhythmPairingResultState.pending) {
        if (surfaceUnresolved) {
          _enterUnresolvedReconciliation(
            'The Rhythm Box still reports this pairing as pending. Check '
            'again before starting another Bluetooth pairing.',
          );
        }
        return;
      }
      final result = status.result;
      if (result == null || result.hubType != 'local_ble') {
        _enterUnresolvedReconciliation(
          'The Rhythm Box returned an invalid Bluetooth pairing result. The '
          'saved session was kept so it can be checked again safely.',
        );
        return;
      }
      if (result.status == RhythmPairingStatus.failed) {
        _fail(
          result.error?.trim().isNotEmpty == true
              ? result.error!.trim()
              : 'Bluetooth pairing failed.',
          failureStage: 'terminal_status',
          serverTerminal: true,
        );
        return;
      }
      final devices = result.completedDevices;
      if (devices.isEmpty) {
        _fail(
          'The Rhythm Box completed pairing but did not return a device.',
          failureStage: 'terminal_status_missing_device',
          serverTerminal: true,
        );
        return;
      }
      _completeSuccessfully(devices.first, warnings: result.warnings);
    } catch (_) {
      if (surfaceUnresolved && mounted) {
        _enterUnresolvedReconciliation(
          'Rhythm could not reach the Box to check this pairing. No new '
          'Bluetooth pairing will start until its status is known.',
        );
      }
    } finally {
      _statusPollInFlight = false;
      if (surfaceUnresolved && mounted) {
        setState(() => _checkingUnresolved = false);
      }
    }
  }

  void _enterUnresolvedReconciliation(String message) {
    if (!mounted || _completed || _activeSessionId.isEmpty) return;
    _deadlineTimer?.cancel();
    _deadlineTimer = null;
    _statusPollTimer?.cancel();
    _statusPollTimer = null;
    setState(() {
      _pairing = false;
      _reconciliationUnresolved = true;
      _checkingUnresolved = false;
      _reconciliationMessage = message;
      _progressMessage = null;
      _error = null;
    });
  }

  Future<void> _checkUnresolvedStatus() async {
    if (_pendingPairingStoreUnreadable ||
        _checkingUnresolved ||
        !_reconciliationUnresolved ||
        _completed) {
      return;
    }
    final generation = _requestGeneration;
    setState(() {
      _checkingUnresolved = true;
      _reconciliationMessage = 'Checking the saved pairing session…';
    });
    try {
      await _pollPairingStatus(generation, surfaceUnresolved: true);
    } finally {
      if (mounted && generation == _requestGeneration) {
        setState(() => _checkingUnresolved = false);
      }
    }
  }

  Future<void> _resolvePairingNotFound() async {
    await _cacheAndSurfaceTerminalFailure(
      'The Rhythm Box has no record of the previous request. You can start a '
      'new pairing safely.',
      failureStage: 'terminal_status_not_found',
      terminalStatus: PendingLocalBleTerminalResult.notFound,
    );
  }

  void _subscribeToProgress() {
    _progressSubscription?.cancel();
    final sessionId = _activeSessionId;
    _progressSubscription = _pairingProgressStream?.listen((event) {
      if (!mounted ||
          _completed ||
          (!_pairing && !_reconciliationUnresolved) ||
          event.hubType != 'local_ble') {
        return;
      }
      if (event.sessionId != sessionId) return;

      if (event.status == RhythmPairingStatus.complete ||
          event.stage == RhythmPairingStage.complete) {
        final devices = event.completedDevices;
        if (devices.isNotEmpty) {
          _completeSuccessfully(devices.first, warnings: event.warnings);
        } else {
          _fail(
            'The Rhythm Box completed pairing but did not return a device.',
            failureStage: 'terminal_event_missing_device',
            serverTerminal: true,
          );
        }
        return;
      }
      if (event.status == RhythmPairingStatus.failed ||
          event.stage == RhythmPairingStage.failed) {
        _fail(
          event.error?.trim().isNotEmpty == true
              ? event.error!.trim()
              : event.message.trim().isNotEmpty
                  ? event.message.trim()
                  : 'Bluetooth pairing failed.',
          failureStage: 'terminal_event',
          serverTerminal: true,
        );
        return;
      }

      final message = event.message.trim();
      if (message.isNotEmpty) {
        setState(() => _progressMessage = message);
      }
    });
  }

  Future<void> _startPairing() async {
    if (_pairing ||
        _completed ||
        _restoringPending ||
        _reconciliationUnresolved ||
        _terminalFailureNeedsAcknowledgement ||
        _routeServerScope.isEmpty) {
      return;
    }
    final admissionServerScope = _bindPendingStoreForNewAdmission();
    if (admissionServerScope == null) {
      setState(() {
        _error = 'Wait for the Rhythm Box connection to finish before '
            'starting Bluetooth pairing.';
      });
      return;
    }
    final attempt = ++_attemptNumber;
    final generation = ++_requestGeneration;
    _activeSessionId = attempt == 1
        ? widget.journeyId
        : '${widget.journeyId}-attempt-$attempt';
    _activeAnalyticsJourneyId = widget.journeyId;
    _activeProfileId = _pairingProfileId;
    _activePointerServerScope = admissionServerScope;
    _activeStartedAtEpochMs = DateTime.now().millisecondsSinceEpoch;
    _claimSessionOwnership(
      _activeSessionId,
      serverScope: admissionServerScope,
    );
    setState(() {
      _pairing = true;
      _reconciliationUnresolved = false;
      _reconciliationMessage = null;
      _error = null;
      _progressMessage = 'Looking for the device near your Rhythm Box…';
    });
    _scheduleDeadline(generation, widget.pairingDeadline);
    _subscribeToProgress();
    HapticFeedback.mediumImpact();
    AnalyticsService().logLocalBlePairingAttempted(
      journeyId: widget.journeyId,
      profileId: _pairingProfileId,
      source: widget.analyticsSource,
      inputMethod: widget.inputMethod,
      attemptNumber: attempt,
    );

    final pendingSaved = await _pendingPairingStore.save(
      PendingLocalBlePairing(
        sessionId: _activeSessionId,
        journeyId: _activeAnalyticsJourneyId,
        attemptNumber: attempt,
        profileId: _pairingProfileId,
        serverScope: admissionServerScope,
        startedAtEpochMs: _activeStartedAtEpochMs,
      ),
    );
    if (!mounted || _completed || generation != _requestGeneration) return;
    if (!pendingSaved) {
      _failWithoutDurableSession(
        'Rhythm could not save a safe pairing recovery record. Please try '
        'again before putting the device in pairing mode.',
        failureStage: 'recovery_state_unavailable',
      );
      return;
    }

    try {
      final response = await _pair(_activeSessionId);
      if (!mounted || _completed || generation != _requestGeneration) return;
      final status = response?['status']?.toString().toLowerCase();
      final httpStatus = _httpStatus(response?['http_status']);
      final requestDelivery = response?['request_delivery']?.toString();
      if (requestDelivery == 'not_sent') {
        _fail(
          'The pairing request did not reach the Rhythm Box. Check the '
          'connection and try again.',
          failureStage: 'request_not_sent',
        );
        return;
      }
      if (_isDefinitiveHttpRejection(httpStatus)) {
        _fail(
          'The Rhythm Box rejected the pairing request. Put the device back '
          'in pairing mode and try again.',
          failureStage: 'server_rejected',
        );
        return;
      }
      final successfulResponse = (httpStatus == null || httpStatus == 200) &&
          (requestDelivery == null || requestDelivery == 'delivered');
      if (successfulResponse && (status == 'complete' || status == 'failed')) {
        try {
          final terminal = RhythmPairingSessionResult.fromJson(
            Map<String, dynamic>.from(response!),
          );
          if (terminal.hubType != 'local_ble') {
            _waitForTerminalConfirmation(generation);
            return;
          }
          if (terminal.status == RhythmPairingStatus.failed) {
            _fail(
              terminal.error?.trim().isNotEmpty == true
                  ? terminal.error!.trim()
                  : 'The Rhythm Box could not pair this device. Put it back '
                      'in pairing mode and try again.',
              failureStage: 'pairing',
              serverTerminal: true,
            );
            return;
          }
          _completeSuccessfully(terminal.completedDevices.first,
              warnings: terminal.warnings);
        } catch (_) {
          _waitForTerminalConfirmation(generation);
        }
        return;
      }
      _waitForTerminalConfirmation(generation);
    } catch (_) {
      if (!mounted || _completed || generation != _requestGeneration) return;
      _waitForTerminalConfirmation(generation);
    }
  }

  int? _httpStatus(Object? value) => switch (value) {
        int status => status,
        num status => status.toInt(),
        String status => int.tryParse(status),
        _ => null,
      };

  bool _isDefinitiveHttpRejection(int? status) {
    // 408 and proxy-style 499 describe the request channel ending; neither
    // proves the appliance stopped work that it may already have accepted.
    if (status == null || status == 408 || status == 499) return false;
    return status >= 400 && status < 500;
  }

  void _waitForTerminalConfirmation(int generation) {
    if (!mounted ||
        _completed ||
        !_pairing ||
        generation != _requestGeneration) {
      return;
    }
    setState(() {
      _progressMessage =
          'Request sent. Waiting for final confirmation from your Rhythm Box…';
    });
    _startStatusPolling(generation, pollImmediately: true);
  }

  void _completeSuccessfully(
    RhythmPairedDevice device, {
    List<String> warnings = const [],
  }) {
    if (!mounted ||
        _completed ||
        _terminalCacheInFlight ||
        (!_pairing && !_reconciliationUnresolved)) {
      return;
    }
    if (!_validTerminalDevice(device)) {
      _fail(
        'The Rhythm Box completed pairing but returned an invalid device.',
        failureStage: 'invalid_response',
        serverTerminal: true,
      );
      return;
    }
    final visibleWarnings = _sanitizeTerminalWarnings(warnings);
    _terminalCacheInFlight = true;
    _requestGeneration += 1;
    _pairing = false;
    _reconciliationUnresolved = false;
    _reconciliationMessage = null;
    _stopActiveReconciliation();
    setState(() {
      _checkingUnresolved = true;
      _progressMessage = 'Saving the completed pairing safely…';
      _error = null;
    });
    unawaited(_cacheAndSurfaceSuccess(device, visibleWarnings));
  }

  Future<void> _cacheAndSurfaceSuccess(
    RhythmPairedDevice device,
    List<String> warnings,
  ) async {
    final cached = await _cacheTerminalResult(
      PendingLocalBleTerminalResult(
        status: PendingLocalBleTerminalResult.complete,
        deviceId: device.deviceId,
        deviceName: device.name,
        deviceType: device.deviceType,
        manufacturer: device.manufacturer,
        model: device.model,
        warnings: warnings,
      ),
    );
    if (!mounted) return;
    _terminalCacheInFlight = false;
    if (!cached) {
      _enterUnresolvedReconciliation(
        'The Rhythm Box completed pairing, but Rhythm could not save the '
        'result safely. The saved session was kept so it can be checked '
        'again before starting another pairing.',
      );
      return;
    }
    _surfaceSuccessfulTerminal(
      device,
      warnings: warnings,
      autoAcknowledge: warnings.isEmpty,
      haptic: true,
    );
  }

  void _surfaceSuccessfulTerminal(
    RhythmPairedDevice device, {
    required List<String> warnings,
    required bool autoAcknowledge,
    bool haptic = false,
  }) {
    if (!mounted || _completed) return;
    _completed = true;
    _pairing = false;
    _reconciliationUnresolved = false;
    _reconciliationMessage = null;
    _requestGeneration += 1;
    _stopActiveReconciliation();
    AnalyticsService().logLocalBlePairingCompleted(
      journeyId: _activeAnalyticsJourneyId.isNotEmpty
          ? _activeAnalyticsJourneyId
          : _activeSessionId,
      profileId:
          _activeProfileId.isNotEmpty ? _activeProfileId : _pairingProfileId,
      source: widget.analyticsSource,
      inputMethod: widget.inputMethod,
      attemptNumber: _attemptNumber,
      outcome: 'succeeded',
      deduplicationId: 'local_ble_pairing_completed:$_activeSessionId',
    );
    if (haptic) HapticFeedback.heavyImpact();
    if (autoAcknowledge) {
      _checkingUnresolved = true;
      unawaited(_clearPointerAndPop(device));
      return;
    }
    setState(() {
      _checkingUnresolved = false;
      _successfulDevice = device;
      _successWarnings = warnings;
      _progressMessage = null;
      _error = null;
    });
  }

  Future<bool> _cacheTerminalResult(
    PendingLocalBleTerminalResult result,
  ) async {
    final sessionId = _activeSessionId;
    if (sessionId.isEmpty) return false;
    try {
      return await _pendingPairingStore.save(
        PendingLocalBlePairing(
          sessionId: sessionId,
          journeyId: _activeAnalyticsJourneyId.isNotEmpty
              ? _activeAnalyticsJourneyId
              : sessionId,
          attemptNumber: _attemptNumber < 1 ? 1 : _attemptNumber,
          profileId: _activeProfileId.isNotEmpty
              ? _activeProfileId
              : _pairingProfileId,
          serverScope: _activePointerServerScope.isNotEmpty
              ? _activePointerServerScope
              : _routeServerScope,
          startedAtEpochMs: _activeStartedAtEpochMs,
          terminalResult: result,
        ),
      );
    } catch (_) {
      return false;
    }
  }

  bool _validTerminalDevice(RhythmPairedDevice device) =>
      _validTerminalString(device.deviceId, 256) &&
      _validTerminalString(device.name, 256) &&
      _validTerminalString(device.deviceType, 64) &&
      _validOptionalTerminalString(device.manufacturer, 128) &&
      _validOptionalTerminalString(device.model, 128);

  bool _validTerminalString(String value, int maxLength) =>
      value.trim().isNotEmpty && value.length <= maxLength;

  bool _validOptionalTerminalString(String? value, int maxLength) =>
      value == null || value.length <= maxLength;

  List<String> _sanitizeTerminalWarnings(List<String> warnings) => warnings
      .map((warning) => warning.trim())
      .where((warning) => warning.isNotEmpty)
      .take(32)
      .map((warning) =>
          warning.length <= 512 ? warning : warning.substring(0, 512))
      .toList(growable: false);

  Future<void> _clearPointerAndPop(RhythmPairedDevice device) async {
    final serverAcknowledged = await _acknowledgeServerResult(_activeSessionId);
    if (!mounted) return;
    if (!serverAcknowledged) {
      setState(() {
        _checkingUnresolved = false;
        _successfulDevice = device;
        _error = 'The device was added, but the Rhythm Box could not save '
            'your acknowledgement. Tap Continue to try again.';
      });
      return;
    }
    final cleared =
        await _pendingPairingStore.clear(sessionId: _activeSessionId);
    if (!mounted) return;
    if (!cleared) {
      setState(() {
        _checkingUnresolved = false;
        _successfulDevice = device;
        _error = 'The device was added, but Rhythm could not save your '
            'acknowledgement. Tap Continue to try again.';
      });
      return;
    }
    setState(() {
      _checkingUnresolved = false;
      _error = null;
    });
    Navigator.of(context).pop(device);
  }

  Future<void> _acknowledgeSuccessfulPairing() async {
    final device = _successfulDevice;
    if (device == null || _checkingUnresolved) return;
    setState(() => _checkingUnresolved = true);
    final serverAcknowledged = await _acknowledgeServerResult(_activeSessionId);
    if (!mounted) return;
    if (!serverAcknowledged) {
      setState(() {
        _checkingUnresolved = false;
        _error = 'The Rhythm Box could not save your acknowledgement. The '
            'pairing result is still safe; tap Continue to try again.';
      });
      return;
    }
    final cleared =
        await _pendingPairingStore.clear(sessionId: _activeSessionId);
    if (!mounted) return;
    if (!cleared) {
      setState(() {
        _checkingUnresolved = false;
        _error = 'Rhythm could not save your acknowledgement. The pairing '
            'result is still safe; tap Continue to try again.';
      });
      return;
    }
    setState(() {
      _checkingUnresolved = false;
      _successfulDevice = null;
      _error = null;
    });
    Navigator.of(context).pop(device);
  }

  void _fail(
    String message, {
    required String failureStage,
    bool serverTerminal = false,
  }) {
    if (!mounted ||
        _completed ||
        _terminalCacheInFlight ||
        (!_pairing && !_reconciliationUnresolved)) {
      return;
    }
    if (serverTerminal) {
      unawaited(
        _cacheAndSurfaceTerminalFailure(
          message,
          failureStage: failureStage,
          terminalStatus: PendingLocalBleTerminalResult.failed,
        ),
      );
      return;
    }
    _requestGeneration += 1;
    _pairing = false;
    _reconciliationUnresolved = false;
    _reconciliationMessage = null;
    _stopActiveReconciliation();
    setState(() {
      _checkingUnresolved = true;
      _error = null;
      _progressMessage = null;
    });
    unawaited(_finishFailure(message, failureStage: failureStage));
  }

  Future<void> _cacheAndSurfaceTerminalFailure(
    String message, {
    required String failureStage,
    required String terminalStatus,
  }) async {
    if (!mounted || _completed || _terminalCacheInFlight) return;
    final safeMessage = message.trim().isEmpty
        ? 'Bluetooth pairing failed.'
        : message.trim().length <= 512
            ? message.trim()
            : message.trim().substring(0, 512);
    _terminalCacheInFlight = true;
    _requestGeneration += 1;
    _pairing = false;
    _reconciliationUnresolved = false;
    _reconciliationMessage = null;
    _stopActiveReconciliation();
    setState(() {
      _checkingUnresolved = true;
      _error = null;
      _progressMessage = 'Saving the pairing result safely…';
    });
    final cached = await _cacheTerminalResult(
      PendingLocalBleTerminalResult(
        status: terminalStatus,
        error: safeMessage,
      ),
    );
    if (!mounted) return;
    _terminalCacheInFlight = false;
    if (!cached) {
      _enterUnresolvedReconciliation(
        'Pairing finished, but Rhythm could not save the result safely. The '
        'saved session was kept so it can be checked again before starting '
        'another pairing.',
      );
      return;
    }
    _surfaceTerminalFailure(safeMessage, failureStage: failureStage);
  }

  void _surfaceTerminalFailure(
    String message, {
    required String failureStage,
  }) {
    if (!mounted || _completed) return;
    _requestGeneration += 1;
    _pairing = false;
    _reconciliationUnresolved = false;
    _reconciliationMessage = null;
    _stopActiveReconciliation();
    _terminalFailureNeedsAcknowledgement = true;
    _terminalFailureMessage = message;
    AnalyticsService().logLocalBlePairingCompleted(
      journeyId: _activeAnalyticsJourneyId.isNotEmpty
          ? _activeAnalyticsJourneyId
          : _activeSessionId,
      profileId:
          _activeProfileId.isNotEmpty ? _activeProfileId : _pairingProfileId,
      source: widget.analyticsSource,
      inputMethod: widget.inputMethod,
      attemptNumber: _attemptNumber,
      outcome: 'failed',
      failureStage: failureStage,
      deduplicationId: 'local_ble_pairing_completed:$_activeSessionId',
    );
    setState(() {
      _checkingUnresolved = false;
      _error = message;
      _progressMessage = null;
    });
  }

  Future<void> _acknowledgeTerminalFailure() async {
    if (!_terminalFailureNeedsAcknowledgement || _checkingUnresolved) return;
    final sessionId = _activeSessionId;
    setState(() => _checkingUnresolved = true);
    final serverAcknowledged = await _acknowledgeServerResult(sessionId);
    if (!mounted || _activeSessionId != sessionId) return;
    if (!serverAcknowledged) {
      setState(() {
        _checkingUnresolved = false;
        _error = 'The Rhythm Box could not save your acknowledgement. The '
            'pairing result is still safe; tap Continue to try again.';
      });
      return;
    }
    final cleared = await _pendingPairingStore.clear(sessionId: sessionId);
    if (!mounted || _activeSessionId != sessionId) return;
    if (!cleared) {
      setState(() {
        _checkingUnresolved = false;
        _error = 'The Rhythm Box saved your acknowledgement, but the app '
            'could not. The result is still safe; tap Continue to try again.';
      });
      return;
    }
    _releaseSessionOwnership();
    setState(() {
      _checkingUnresolved = false;
      _terminalFailureNeedsAcknowledgement = false;
      _error = _terminalFailureMessage;
      _terminalFailureMessage = null;
      _activeSessionId = '';
      _activeAnalyticsJourneyId = '';
      _activeProfileId = '';
      _activePointerServerScope = '';
      _activeStartedAtEpochMs = 0;
    });
  }

  Future<void> _finishFailure(
    String message, {
    required String failureStage,
  }) async {
    final sessionId = _activeSessionId;
    final cleared = await _pendingPairingStore.clear(sessionId: sessionId);
    if (!mounted || _activeSessionId != sessionId) return;
    if (!cleared) {
      setState(() {
        _checkingUnresolved = false;
        _reconciliationUnresolved = true;
        _reconciliationMessage = 'Pairing finished, but Rhythm could not '
            'save the result acknowledgement. Check again before starting '
            'another Bluetooth pairing.';
      });
      return;
    }
    _releaseSessionOwnership();
    AnalyticsService().logLocalBlePairingCompleted(
      journeyId: _activeAnalyticsJourneyId.isNotEmpty
          ? _activeAnalyticsJourneyId
          : _activeSessionId,
      profileId: _pairingProfileId,
      source: widget.analyticsSource,
      inputMethod: widget.inputMethod,
      attemptNumber: _attemptNumber,
      outcome: 'failed',
      failureStage: failureStage,
      deduplicationId: 'local_ble_pairing_completed:$_activeSessionId',
    );
    setState(() {
      _checkingUnresolved = false;
      _reconciliationUnresolved = false;
      _error = message;
      _progressMessage = null;
    });
  }

  void _failWithoutDurableSession(
    String message, {
    required String failureStage,
  }) {
    _requestGeneration += 1;
    _pairing = false;
    _reconciliationUnresolved = false;
    _reconciliationMessage = null;
    _stopActiveReconciliation();
    _releaseSessionOwnership();
    AnalyticsService().logLocalBlePairingCompleted(
      journeyId: _activeAnalyticsJourneyId.isNotEmpty
          ? _activeAnalyticsJourneyId
          : _activeSessionId,
      profileId: _pairingProfileId,
      source: widget.analyticsSource,
      inputMethod: widget.inputMethod,
      attemptNumber: _attemptNumber,
      outcome: 'failed',
      failureStage: failureStage,
      deduplicationId: 'local_ble_pairing_completed:$_activeSessionId',
    );
    setState(() {
      _error = message;
      _progressMessage = null;
    });
  }

  void _stopActiveReconciliation() {
    _deadlineTimer?.cancel();
    _deadlineTimer = null;
    _statusPollTimer?.cancel();
    _statusPollTimer = null;
    _progressSubscription?.cancel();
    _progressSubscription = null;
  }

  void _claimSessionOwnership(
    String sessionId, {
    String? serverScope,
  }) {
    final resolvedScope = serverScope ?? _routeServerScope;
    if (_ownedServerScope == resolvedScope && _ownedSessionId == sessionId) {
      return;
    }
    _releaseSessionOwnership();
    _ownedServerScope = resolvedScope;
    _ownedSessionId = sessionId;
    LocalBlePairingRouteOwnership.claim(resolvedScope, sessionId);
  }

  void _releaseSessionOwnership() {
    final serverScope = _ownedServerScope;
    final sessionId = _ownedSessionId;
    if (serverScope != null && sessionId != null) {
      LocalBlePairingRouteOwnership.release(serverScope, sessionId);
    }
    _ownedServerScope = null;
    _ownedSessionId = null;
  }

  @override
  Widget build(BuildContext context) {
    return PopScope(
      canPop: !_pairing &&
          _successfulDevice == null &&
          !_terminalFailureNeedsAcknowledgement &&
          !_checkingUnresolved,
      onPopInvokedWithResult: (didPop, _) {
        if (didPop) return;
        ScaffoldMessenger.of(context)
          ..hideCurrentSnackBar()
          ..showSnackBar(
            SnackBar(
              content: Text(
                _successfulDevice != null
                    ? 'Review the pairing note and tap Continue.'
                    : _terminalFailureNeedsAcknowledgement
                        ? 'Review the final pairing result and tap Continue.'
                        : 'Pairing status is still being checked. Please wait.',
              ),
            ),
          );
      },
      child: Scaffold(
        key: const ValueKey('local-ble-pairing-screen'),
        backgroundColor: CelestialColors.backgroundDark,
        appBar: AppBar(
          backgroundColor: CelestialColors.backgroundDark,
          foregroundColor: CelestialColors.textPrimary,
          title: const Text('Add Bluetooth Device'),
        ),
        body: SafeArea(
          top: false,
          child: ListView(
            padding: const EdgeInsets.fromLTRB(22, 20, 22, 32),
            children: [
              Icon(
                _successfulDevice == null
                    ? Icons.bluetooth_rounded
                    : Icons.check_circle_rounded,
                color: _accent,
                size: 58,
              ),
              const SizedBox(height: 20),
              Text(
                _successfulDevice == null
                    ? 'Put the device in pairing mode'
                    : 'Device added',
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 24,
                  fontWeight: FontWeight.w700,
                ),
              ),
              const SizedBox(height: 12),
              Text(
                _successfulDevice == null
                    ? 'Follow the device’s pairing-mode instructions, then '
                        'keep it close to the Rhythm Box. Rhythm will verify '
                        'that it matches the setup code you scanned.'
                    : '${_successfulDevice!.name} is connected. Review the '
                        'note below before continuing.',
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.86),
                  fontSize: 15,
                  height: 1.45,
                ),
              ),
              if (_progressMessage != null) ...[
                const SizedBox(height: 24),
                Text(
                  _progressMessage!,
                  key: const ValueKey('local-ble-pairing-progress'),
                  textAlign: TextAlign.center,
                  style: const TextStyle(color: CelestialColors.textSecondary),
                ),
              ],
              if (_error != null) ...[
                const SizedBox(height: 24),
                Container(
                  key: const ValueKey('local-ble-pairing-error'),
                  padding: const EdgeInsets.all(14),
                  decoration: BoxDecoration(
                    color: Colors.red.withValues(alpha: 0.1),
                    borderRadius: BorderRadius.circular(12),
                    border: Border.all(
                      color: Colors.red.withValues(alpha: 0.35),
                    ),
                  ),
                  child: Text(
                    _error!,
                    style: const TextStyle(color: CelestialColors.textPrimary),
                  ),
                ),
              ],
              if (_reconciliationUnresolved) ...[
                const SizedBox(height: 24),
                Container(
                  key: const ValueKey('local-ble-pairing-unresolved'),
                  padding: const EdgeInsets.all(14),
                  decoration: BoxDecoration(
                    color: Colors.amber.withValues(alpha: 0.1),
                    borderRadius: BorderRadius.circular(12),
                    border: Border.all(
                      color: Colors.amber.withValues(alpha: 0.35),
                    ),
                  ),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      const Text(
                        'Previous pairing needs confirmation',
                        style: TextStyle(
                          color: CelestialColors.textPrimary,
                          fontWeight: FontWeight.w700,
                        ),
                      ),
                      const SizedBox(height: 8),
                      Text(
                        _reconciliationMessage ??
                            'Check the saved session before starting another '
                                'Bluetooth pairing.',
                        style: const TextStyle(
                          color: CelestialColors.textSecondary,
                        ),
                      ),
                    ],
                  ),
                ),
              ],
              if (_successWarnings.isNotEmpty) ...[
                const SizedBox(height: 24),
                Container(
                  key: const ValueKey('local-ble-pairing-warnings'),
                  padding: const EdgeInsets.all(14),
                  decoration: BoxDecoration(
                    color: Colors.amber.withValues(alpha: 0.1),
                    borderRadius: BorderRadius.circular(12),
                    border: Border.all(
                      color: Colors.amber.withValues(alpha: 0.35),
                    ),
                  ),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      const Text(
                        'Pairing completed with a note',
                        style: TextStyle(
                          color: CelestialColors.textPrimary,
                          fontWeight: FontWeight.w700,
                        ),
                      ),
                      const SizedBox(height: 8),
                      for (final warning in _successWarnings)
                        Text(
                          '• $warning',
                          style: const TextStyle(
                            color: CelestialColors.textSecondary,
                          ),
                        ),
                    ],
                  ),
                ),
              ],
              const SizedBox(height: 30),
              SizedBox(
                height: 54,
                child: FilledButton.icon(
                  key: ValueKey(
                    _terminalFailureNeedsAcknowledgement
                        ? 'acknowledge-local-ble-pairing-failure'
                        : _successfulDevice != null
                            ? 'continue-local-ble-pairing'
                            : _reconciliationUnresolved
                                ? 'check-local-ble-pairing-status'
                                : 'find-local-ble-device',
                  ),
                  onPressed: _terminalFailureNeedsAcknowledgement
                      ? _checkingUnresolved
                          ? null
                          : _acknowledgeTerminalFailure
                      : _successfulDevice != null
                          ? _checkingUnresolved
                              ? null
                              : _acknowledgeSuccessfulPairing
                          : _reconciliationUnresolved
                              ? _pendingPairingStoreUnreadable ||
                                      _checkingUnresolved
                                  ? null
                                  : _checkUnresolvedStatus
                              : _pairing || _restoringPending
                                  ? null
                                  : _routeServerScope.isEmpty
                                      ? null
                                      : _startPairing,
                  style: FilledButton.styleFrom(backgroundColor: _accent),
                  icon: _pairing || _restoringPending || _checkingUnresolved
                      ? const SizedBox(
                          width: 20,
                          height: 20,
                          child: CircularProgressIndicator(
                            strokeWidth: 2,
                            color: Colors.white,
                          ),
                        )
                      : Icon(
                          _successfulDevice != null
                              ? Icons.arrow_forward_rounded
                              : _terminalFailureNeedsAcknowledgement
                                  ? Icons.arrow_forward_rounded
                                  : _reconciliationUnresolved
                                      ? _pendingPairingStoreUnreadable
                                          ? Icons.error_outline_rounded
                                          : Icons.refresh_rounded
                                      : Icons.bluetooth_searching_rounded,
                        ),
                  label: Text(_terminalFailureNeedsAcknowledgement
                      ? _checkingUnresolved
                          ? 'Saving Confirmation…'
                          : 'Continue'
                      : _successfulDevice != null
                          ? _checkingUnresolved
                              ? 'Saving Confirmation…'
                              : 'Continue'
                          : _reconciliationUnresolved
                              ? _pendingPairingStoreUnreadable
                                  ? 'Recovery Record Unavailable'
                                  : _checkingUnresolved
                                      ? 'Checking Status…'
                                      : 'Check Status Again'
                              : _restoringPending
                                  ? 'Checking Previous Pairing…'
                                  : _pairing
                                      ? 'Looking for Device…'
                                      : 'Find Device'),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
