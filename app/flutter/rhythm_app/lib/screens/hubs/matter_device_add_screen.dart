import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show RhythmConnection, RhythmRoom;

import '../../providers/server_sync_provider.dart';
import '../../widgets/solar_orbit.dart';

/// Phases of the Matter device commissioning flow.
enum _PairingPhase { input, commissioning, roomAssignment }

/// Full-screen modal for commissioning a Matter device.
///
/// Three phases:
///   1. Setup code entry
///   2. Commissioning progress
///   3. Room assignment
class MatterDeviceAddScreen extends StatefulWidget {
  const MatterDeviceAddScreen({super.key});

  static Future<void> show(BuildContext context) {
    return Navigator.of(context).push(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return const MatterDeviceAddScreen();
        },
        transitionsBuilder: (context, animation, secondaryAnimation, child) {
          final curve = CurvedAnimation(
            parent: animation,
            curve: Curves.easeOutCubic,
            reverseCurve: Curves.easeInCubic,
          );
          return SlideTransition(
            position: Tween<Offset>(
              begin: const Offset(0, 1),
              end: Offset.zero,
            ).animate(curve),
            child: child,
          );
        },
        transitionDuration: const Duration(milliseconds: 350),
        reverseTransitionDuration: const Duration(milliseconds: 300),
      ),
    );
  }

  @override
  State<MatterDeviceAddScreen> createState() => _MatterDeviceAddScreenState();
}

class _MatterDeviceAddScreenState extends State<MatterDeviceAddScreen>
    with SingleTickerProviderStateMixin {
  static const _teal = Color(0xFF00BCD4);
  static const _tealDeep = Color(0xFF00838F);

  _PairingPhase _phase = _PairingPhase.input;

  // ─── Phase 1: Setup code ───────────────────────────────────
  final _codeControllers = List.generate(3, (_) => TextEditingController());
  final _codeFocuses = List.generate(3, (_) => FocusNode());
  static const _segmentLengths = [4, 3, 4]; // XXXX-XXX-XXXX
  final _prevLengths = [0, 0, 0];
  late final List<VoidCallback> _segmentListeners;

  // ─── Phase 2: Commissioning ────────────────────────────────
  String _statusText = '';
  String? _errorText;
  String? _pairedDeviceId;
  String? _pairedDeviceName;
  String? _pairedManufacturer;
  String? _pairedModel;
  Timer? _pollTimer;
  late AnimationController _pulseController;

  // ─── Phase 3: Room assignment ──────────────────────────────
  String? _selectedRoomId;
  bool _isCreatingRoom = false;
  final _newRoomController = TextEditingController();
  bool _isSaving = false;

  @override
  void initState() {
    super.initState();
    _pulseController = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 2000),
    )..repeat(reverse: true);

    // Store listeners so they can be properly removed/re-added.
    _segmentListeners = List.generate(3, (i) => () => _onSegmentChanged(i));
    for (int i = 0; i < 3; i++) {
      _codeControllers[i].addListener(_segmentListeners[i]);
    }

    // Backspace in empty field → navigate to previous segment.
    for (int i = 1; i < 3; i++) {
      _codeFocuses[i].onKeyEvent = (node, event) {
        if (event is KeyDownEvent &&
            event.logicalKey == LogicalKeyboardKey.backspace &&
            _codeControllers[i].text.isEmpty) {
          _codeFocuses[i - 1].requestFocus();
          final prev = _codeControllers[i - 1];
          if (prev.text.isNotEmpty) {
            prev.text = prev.text.substring(0, prev.text.length - 1);
          }
          return KeyEventResult.handled;
        }
        return KeyEventResult.ignored;
      };
    }
  }

  @override
  void dispose() {
    _pollTimer?.cancel();
    _pulseController.dispose();
    for (final c in _codeControllers) {
      c.dispose();
    }
    for (final f in _codeFocuses) {
      f.dispose();
    }
    _newRoomController.dispose();
    super.dispose();
  }

  void _onSegmentChanged(int segment) {
    final text = _codeControllers[segment].text;
    final prevLen = _prevLengths[segment];
    _prevLengths[segment] = text.length;

    // Paste: multiple chars added at once, exceeding segment capacity.
    if (text.length > _segmentLengths[segment] && text.length - prevLen > 1) {
      _distributeCode(text);
      return;
    }

    // Single char overflow (typed in a full field): truncate.
    if (text.length > _segmentLengths[segment]) {
      _removeSegmentListeners();
      _codeControllers[segment].text =
          text.substring(0, _segmentLengths[segment]);
      _prevLengths[segment] = _segmentLengths[segment];
      _addSegmentListeners();
      if (segment < 2) _codeFocuses[segment + 1].requestFocus();
      setState(() {});
      return;
    }

    // Auto-advance: segment just filled (was shorter, now full).
    if (text.length == _segmentLengths[segment] &&
        prevLen < _segmentLengths[segment] &&
        segment < 2) {
      _codeFocuses[segment + 1].requestFocus();
    }

    // Auto-back: segment just emptied.
    if (text.isEmpty && prevLen > 0 && segment > 0) {
      _codeFocuses[segment - 1].requestFocus();
    }
  }

  void _removeSegmentListeners() {
    for (int i = 0; i < 3; i++) {
      _codeControllers[i].removeListener(_segmentListeners[i]);
    }
  }

  void _addSegmentListeners() {
    for (int i = 0; i < 3; i++) {
      _codeControllers[i].addListener(_segmentListeners[i]);
    }
  }

  /// Distribute a raw digit string (e.g. from paste) across all three segments.
  void _distributeCode(String digits) {
    final clean = digits.replaceAll(RegExp(r'\D'), '');
    _removeSegmentListeners();
    int offset = 0;
    for (int i = 0; i < 3; i++) {
      final end = (offset + _segmentLengths[i]).clamp(0, clean.length);
      _codeControllers[i].text = clean.substring(offset, end);
      _prevLengths[i] = _codeControllers[i].text.length;
      offset = end;
    }
    _addSegmentListeners();
    // Focus the last non-full segment, or the last one.
    for (int i = 0; i < 3; i++) {
      if (_codeControllers[i].text.length < _segmentLengths[i]) {
        _codeFocuses[i].requestFocus();
        setState(() {});
        return;
      }
    }
    _codeFocuses[2].requestFocus();
    setState(() {});
  }

  String get _fullCode =>
      _codeControllers.map((c) => c.text).join();

  bool get _codeComplete => _fullCode.length == 11;

  // ─── Commission ────────────────────────────────────────────

  Future<void> _startCommissioning() async {
    if (!_codeComplete) return;
    HapticFeedback.mediumImpact();
    setState(() {
      _phase = _PairingPhase.commissioning;
      _statusText = 'Searching...';
      _errorText = null;
    });

    final http = context.read<RhythmConnection>();
    final result = await http.api.pairDevice(
      hubType: 'matter',
      params: {'setup_code': _fullCode},
    );

    if (!mounted) return;

    if (result == null) {
      setState(() {
        _statusText = 'Failed';
        _errorText = 'Could not reach the server.';
      });
      return;
    }

    final status = result['status'] as String? ?? 'failed';
    final device = result['device'] as Map<String, dynamic>?;
    final error = result['error'] as String?;

    if (status == 'complete' && device != null) {
      _onPairingComplete(device);
      return;
    }

    if (status == 'failed') {
      setState(() {
        _statusText = 'Failed';
        _errorText = error ?? 'Pairing failed.';
      });
      return;
    }

    // Intermediate state — poll for completion.
    _updateStatusFromString(status);
    if (device != null) _extractDeviceInfo(device);
    _startPolling();
  }

  void _updateStatusFromString(String status) {
    setState(() {
      switch (status) {
        case 'searching':
          _statusText = 'Searching...';
        case 'found':
          _statusText = 'Device found';
        case 'commissioning':
          _statusText = 'Commissioning...';
        default:
          _statusText = status;
      }
    });
  }

  void _extractDeviceInfo(Map<String, dynamic> device) {
    setState(() {
      _pairedDeviceId = device['device_id'] as String?;
      _pairedDeviceName = device['name'] as String?;
      _pairedManufacturer = device['manufacturer'] as String?;
      _pairedModel = device['model'] as String?;
    });
  }

  void _startPolling() {
    int attempts = 0;
    _pollTimer?.cancel();
    _pollTimer = Timer.periodic(const Duration(seconds: 2), (timer) async {
      attempts++;
      if (attempts > 15) {
        // 30s timeout.
        timer.cancel();
        if (mounted) {
          setState(() {
            _statusText = 'Timed out';
            _errorText = 'Commissioning took too long. Please try again.';
          });
        }
        return;
      }

      final http = context.read<RhythmConnection>();
      final devices = await http.api.getCanonicalDevices();
      if (!mounted) {
        timer.cancel();
        return;
      }

      if (devices != null) {
        // Look for a new matter device by checking endpoints for a
        // matter native ID.  The canonical device list uses `id` for the
        // stable UUID; native IDs live inside `endpoints[].native_id`.
        for (final d in devices) {
          final endpoints = d['endpoints'] as List<dynamic>? ?? [];
          final hasMatter = endpoints.any((ep) {
            final nativeId = (ep as Map<String, dynamic>)['native_id'] as String? ?? '';
            return nativeId.startsWith('matter-') &&
                (_pairedDeviceId == null || nativeId == _pairedDeviceId);
          });
          if (hasMatter) {
            timer.cancel();
            _onPairingComplete(d);
            return;
          }
        }
      }
    });
  }

  void _onPairingComplete(Map<String, dynamic> device) {
    _pollTimer?.cancel();
    HapticFeedback.heavyImpact();

    // Canonical devices from the registry have `id` (the stable UUID).
    // PairedDeviceInfo from the pairing response has `device_id` (native).
    // assignDeviceRoom needs the canonical UUID, so prefer `id`.
    final canonicalId = device['id'] as String?;
    final nativeId = device['device_id'] as String?;

    setState(() {
      _pairedDeviceId = canonicalId ?? nativeId;
      _pairedDeviceName =
          device['name'] as String? ?? device['product_name'] as String?;
      _pairedManufacturer =
          device['manufacturer'] as String? ?? device['vendor_name'] as String?;
      _pairedModel = device['model'] as String?;
      _phase = _PairingPhase.roomAssignment;
    });

    // If we only have the native ID (immediate pairing result), resolve the
    // canonical UUID so assignDeviceRoom uses the correct identifier.
    if (canonicalId == null && nativeId != null) {
      _resolveCanonicalId(nativeId);
    }
  }

  /// Fetch canonical devices and resolve the native ID to a canonical UUID.
  Future<void> _resolveCanonicalId(String nativeId) async {
    final http = context.read<RhythmConnection>();
    // Retry a few times — the canonical device may not be registered yet.
    for (int i = 0; i < 5; i++) {
      final devices = await http.api.getCanonicalDevices();
      if (!mounted) return;
      if (devices != null) {
        for (final d in devices) {
          final endpoints = d['endpoints'] as List<dynamic>? ?? [];
          final match = endpoints.any((ep) =>
              (ep as Map<String, dynamic>)['native_id'] == nativeId);
          if (match) {
            setState(() => _pairedDeviceId = d['id'] as String?);
            return;
          }
        }
      }
      await Future.delayed(const Duration(seconds: 1));
    }
  }

  // ─── Room assignment ───────────────────────────────────────

  Future<void> _assignAndFinish() async {
    if (_pairedDeviceId == null) return;
    setState(() => _isSaving = true);

    final http = context.read<RhythmConnection>();

    String? roomId = _selectedRoomId;

    // Create new room if requested.
    if (_isCreatingRoom && _newRoomController.text.trim().isNotEmpty) {
      final result =
          await http.api.createTopologyRoom(_newRoomController.text.trim());
      if (result != null) {
        roomId = result['id'] as String?;
      }
    }

    if (roomId != null) {
      await http.api.assignDeviceRoom(_pairedDeviceId!, roomId);
    }

    // Trigger sync to refresh room/device lists.
    await http.api.triggerSync();

    if (!mounted) return;
    Navigator.of(context).pop();
  }

  void _tryAgain() {
    _pollTimer?.cancel();
    setState(() {
      _phase = _PairingPhase.input;
      _statusText = '';
      _errorText = null;
      _pairedDeviceId = null;
      _pairedDeviceName = null;
      _pairedManufacturer = null;
      _pairedModel = null;
    });
  }

  // ─── Build ─────────────────────────────────────────────────

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.symmetric(horizontal: 24),
                child: switch (_phase) {
                  _PairingPhase.input => _buildInputPhase(),
                  _PairingPhase.commissioning => _buildCommissioningPhase(),
                  _PairingPhase.roomAssignment => _buildRoomAssignmentPhase(),
                },
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader() {
    return Container(
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
                color: _teal.withValues(alpha: 0.15),
                border: Border.all(
                  color: _teal.withValues(alpha: 0.3),
                  width: 1,
                ),
              ),
              child: const Icon(Icons.close, color: _teal, size: 20),
            ),
          ),
          const Expanded(
            child: Text(
              'Add Device',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.3,
              ),
            ),
          ),
          const SizedBox(width: 40),
        ],
      ),
    );
  }

  // ─── Phase 1: Setup code entry ─────────────────────────────

  Widget _buildInputPhase() {
    return Column(
      children: [
        const SizedBox(height: 40),
        // Hero icon with glow.
        AnimatedBuilder(
          animation: _pulseController,
          builder: (context, child) {
            final glow = 0.1 + _pulseController.value * 0.1;
            return Container(
              width: 96,
              height: 96,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                gradient: RadialGradient(
                  colors: [
                    _teal.withValues(alpha: glow),
                    Colors.transparent,
                  ],
                  radius: 1.5,
                ),
              ),
              child: Center(
                child: Container(
                  width: 64,
                  height: 64,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    gradient: LinearGradient(
                      begin: Alignment.topLeft,
                      end: Alignment.bottomRight,
                      colors: [_teal, _tealDeep],
                    ),
                  ),
                  child: const Icon(
                    Icons.developer_board,
                    color: Colors.white,
                    size: 28,
                  ),
                ),
              ),
            );
          },
        ),
        const SizedBox(height: 32),
        Text(
          'Enter Setup Code',
          style: TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 20,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 8),
        Text(
          'Find the 11-digit code on your Matter device',
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.7),
            fontSize: 14,
          ),
        ),
        const SizedBox(height: 40),
        // Segmented code input: XXXX - XXX - XXXX
        Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            _buildCodeSegment(0, 4),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 8),
              child: Text(
                '–',
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.4),
                  fontSize: 24,
                  fontWeight: FontWeight.w300,
                ),
              ),
            ),
            _buildCodeSegment(1, 3),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 8),
              child: Text(
                '–',
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.4),
                  fontSize: 24,
                  fontWeight: FontWeight.w300,
                ),
              ),
            ),
            _buildCodeSegment(2, 4),
          ],
        ),
        const SizedBox(height: 48),
        // Commission button.
        SizedBox(
          width: double.infinity,
          height: 52,
          child: DecoratedBox(
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(14),
              gradient: _codeComplete
                  ? const LinearGradient(colors: [_teal, _tealDeep])
                  : null,
              color: _codeComplete
                  ? null
                  : CelestialColors.textSecondary.withValues(alpha: 0.15),
            ),
            child: MaterialButton(
              onPressed: _codeComplete ? _startCommissioning : null,
              shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(14)),
              child: Text(
                'Commission',
                style: TextStyle(
                  color: _codeComplete
                      ? Colors.white
                      : CelestialColors.textSecondary.withValues(alpha: 0.4),
                  fontSize: 16,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
          ),
        ),
        const SizedBox(height: 40),
      ],
    );
  }

  Widget _buildCodeSegment(int index, int maxLength) {
    return SizedBox(
      width: maxLength == 4 ? 80 : 64,
      child: TextField(
        controller: _codeControllers[index],
        focusNode: _codeFocuses[index],
        keyboardType: TextInputType.number,
        textAlign: TextAlign.center,
        style: const TextStyle(
          color: CelestialColors.textPrimary,
          fontSize: 16,
          fontWeight: FontWeight.w600,
          letterSpacing: 2,
        ),
        inputFormatters: [FilteringTextInputFormatter.digitsOnly],
        decoration: InputDecoration(
          counterText: '',
          contentPadding:
              const EdgeInsets.symmetric(horizontal: 8, vertical: 14),
          filled: true,
          fillColor: CelestialColors.backgroundCard,
          enabledBorder: OutlineInputBorder(
            borderRadius: BorderRadius.circular(12),
            borderSide: BorderSide(
              color: CelestialColors.orbitRing.withValues(alpha: 0.5),
            ),
          ),
          focusedBorder: OutlineInputBorder(
            borderRadius: BorderRadius.circular(12),
            borderSide: const BorderSide(color: _teal, width: 1.5),
          ),
        ),
        onChanged: (_) => setState(() {}),
      ),
    );
  }

  // ─── Phase 2: Commissioning progress ───────────────────────

  Widget _buildCommissioningPhase() {
    final isFailed = _errorText != null;

    return Column(
      children: [
        const SizedBox(height: 60),
        // Pulsing orbital animation.
        AnimatedBuilder(
          animation: _pulseController,
          builder: (context, child) {
            final pulse = _pulseController.value;
            return SizedBox(
              width: 140,
              height: 140,
              child: Stack(
                alignment: Alignment.center,
                children: [
                  // Outer ring.
                  Container(
                    width: 140,
                    height: 140,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      border: Border.all(
                        color: (isFailed ? Colors.red : _teal)
                            .withValues(alpha: 0.1 + pulse * 0.15),
                        width: 1,
                      ),
                    ),
                  ),
                  // Middle ring.
                  Container(
                    width: 100,
                    height: 100,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      border: Border.all(
                        color: (isFailed ? Colors.red : _teal)
                            .withValues(alpha: 0.15 + pulse * 0.2),
                        width: 1.5,
                      ),
                    ),
                  ),
                  // Center icon.
                  Container(
                    width: 64,
                    height: 64,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      gradient: LinearGradient(
                        begin: Alignment.topLeft,
                        end: Alignment.bottomRight,
                        colors: isFailed
                            ? [Colors.red.shade400, Colors.red.shade700]
                            : [_teal, _tealDeep],
                      ),
                      boxShadow: [
                        BoxShadow(
                          color: (isFailed ? Colors.red : _teal)
                              .withValues(alpha: 0.3 + pulse * 0.2),
                          blurRadius: 20 + pulse * 10,
                          spreadRadius: pulse * 4,
                        ),
                      ],
                    ),
                    child: Icon(
                      isFailed ? Icons.close : Icons.bluetooth_searching,
                      color: Colors.white,
                      size: 28,
                    ),
                  ),
                ],
              ),
            );
          },
        ),
        const SizedBox(height: 32),
        Text(
          _statusText,
          style: TextStyle(
            color: isFailed
                ? Colors.red.shade300
                : CelestialColors.textPrimary,
            fontSize: 20,
            fontWeight: FontWeight.w600,
          ),
        ),
        if (_errorText != null) ...[
          const SizedBox(height: 8),
          Text(
            _errorText!,
            textAlign: TextAlign.center,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.7),
              fontSize: 14,
            ),
          ),
        ],
        // Device info card (appears once device found).
        if (_pairedDeviceName != null && !isFailed) ...[
          const SizedBox(height: 32),
          _buildDeviceInfoCard(),
        ],
        if (!isFailed && _errorText == null) ...[
          const SizedBox(height: 32),
          SizedBox(
            width: 20,
            height: 20,
            child: CircularProgressIndicator(
              strokeWidth: 2,
              valueColor:
                  AlwaysStoppedAnimation(_teal.withValues(alpha: 0.6)),
            ),
          ),
        ],
        if (isFailed) ...[
          const SizedBox(height: 32),
          SizedBox(
            width: double.infinity,
            height: 52,
            child: OutlinedButton(
              onPressed: _tryAgain,
              style: OutlinedButton.styleFrom(
                side: BorderSide(color: _teal.withValues(alpha: 0.5)),
                shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(14)),
              ),
              child: const Text(
                'Try Again',
                style: TextStyle(
                  color: _teal,
                  fontSize: 16,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
          ),
        ],
        const SizedBox(height: 40),
      ],
    );
  }

  Widget _buildDeviceInfoCard() {
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.5),
        ),
      ),
      child: Row(
        children: [
          Container(
            width: 40,
            height: 40,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: _teal.withValues(alpha: 0.15),
            ),
            child: Icon(
              Icons.lightbulb_outline,
              color: _teal.withValues(alpha: 0.8),
              size: 20,
            ),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  _pairedDeviceName ?? 'Unknown Device',
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 14,
                    fontWeight: FontWeight.w500,
                  ),
                ),
                if (_pairedManufacturer != null || _pairedModel != null) ...[
                  const SizedBox(height: 2),
                  Text(
                    [_pairedManufacturer, _pairedModel]
                        .where((s) => s != null)
                        .join(' · '),
                    style: TextStyle(
                      color: CelestialColors.textSecondary
                          .withValues(alpha: 0.6),
                      fontSize: 12,
                    ),
                  ),
                ],
              ],
            ),
          ),
        ],
      ),
    );
  }

  // ─── Phase 3: Room assignment ──────────────────────────────

  Widget _buildRoomAssignmentPhase() {
    final syncProvider = context.watch<ServerSyncProvider>();
    final rooms = syncProvider.helloRooms;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const SizedBox(height: 32),
        // Success header.
        Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Container(
              width: 32,
              height: 32,
              decoration: const BoxDecoration(
                shape: BoxShape.circle,
                color: Color(0xFF22C55E),
              ),
              child: const Icon(Icons.check, color: Colors.white, size: 18),
            ),
            const SizedBox(width: 12),
            const Text(
              'Device Added!',
              style: TextStyle(
                color: Color(0xFF22C55E),
                fontSize: 20,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
        const SizedBox(height: 24),
        _buildDeviceInfoCard(),
        const SizedBox(height: 32),
        // Section header.
        Padding(
          padding: const EdgeInsets.only(left: 4, bottom: 10),
          child: Text(
            'ASSIGN TO ROOM',
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.6),
              fontSize: 13,
              fontWeight: FontWeight.w600,
              letterSpacing: 1,
            ),
          ),
        ),
        // Room list.
        Container(
          decoration: BoxDecoration(
            color: CelestialColors.backgroundCard,
            borderRadius: BorderRadius.circular(14),
            border: Border.all(
              color: CelestialColors.orbitRing.withValues(alpha: 0.5),
            ),
          ),
          child: Column(
            children: [
              for (final (index, room) in rooms.indexed) ...[
                if (index > 0)
                  Divider(
                      height: 1,
                      color:
                          CelestialColors.orbitRing.withValues(alpha: 0.3)),
                _buildRoomRow(room),
              ],
              if (rooms.isNotEmpty)
                Divider(
                    height: 1,
                    color: CelestialColors.orbitRing.withValues(alpha: 0.3)),
              _buildCreateRoomRow(),
            ],
          ),
        ),
        const SizedBox(height: 32),
        // Done button.
        SizedBox(
          width: double.infinity,
          height: 52,
          child: DecoratedBox(
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(14),
              gradient: (_selectedRoomId != null ||
                      (_isCreatingRoom &&
                          _newRoomController.text.trim().isNotEmpty))
                  ? const LinearGradient(colors: [_teal, _tealDeep])
                  : null,
              color: (_selectedRoomId == null &&
                      !(_isCreatingRoom &&
                          _newRoomController.text.trim().isNotEmpty))
                  ? CelestialColors.textSecondary.withValues(alpha: 0.15)
                  : null,
            ),
            child: MaterialButton(
              onPressed: (_selectedRoomId != null ||
                          (_isCreatingRoom &&
                              _newRoomController.text.trim().isNotEmpty)) &&
                      !_isSaving
                  ? _assignAndFinish
                  : null,
              shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(14)),
              child: _isSaving
                  ? const SizedBox(
                      width: 20,
                      height: 20,
                      child: CircularProgressIndicator(
                        strokeWidth: 2,
                        valueColor: AlwaysStoppedAnimation(Colors.white),
                      ),
                    )
                  : Text(
                      'Done',
                      style: TextStyle(
                        color: (_selectedRoomId != null ||
                                (_isCreatingRoom &&
                                    _newRoomController.text
                                        .trim()
                                        .isNotEmpty))
                            ? Colors.white
                            : CelestialColors.textSecondary
                                .withValues(alpha: 0.4),
                        fontSize: 16,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
            ),
          ),
        ),
        // Skip option.
        const SizedBox(height: 12),
        Center(
          child: GestureDetector(
            onTap: _isSaving
                ? null
                : () {
                    setState(() {
                      _selectedRoomId = null;
                      _isCreatingRoom = false;
                    });
                    _assignAndFinish();
                  },
            child: Text(
              'Skip for now',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.5),
                fontSize: 14,
              ),
            ),
          ),
        ),
        const SizedBox(height: 40),
      ],
    );
  }

  Widget _buildRoomRow(RhythmRoom room) {
    final isSelected = _selectedRoomId == room.id;
    return GestureDetector(
      onTap: () {
        HapticFeedback.selectionClick();
        setState(() {
          _selectedRoomId = isSelected ? null : room.id;
          _isCreatingRoom = false;
        });
      },
      behavior: HitTestBehavior.opaque,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
        color: isSelected ? _teal.withValues(alpha: 0.08) : null,
        child: Row(
          children: [
            Icon(
              Icons.meeting_room_outlined,
              color: isSelected
                  ? _teal
                  : CelestialColors.textSecondary.withValues(alpha: 0.5),
              size: 20,
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    room.name,
                    style: TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 14,
                      fontWeight:
                          isSelected ? FontWeight.w600 : FontWeight.w500,
                    ),
                  ),
                  if (room.deviceIds.isNotEmpty) ...[
                    const SizedBox(height: 2),
                    Text(
                      '${room.deviceIds.length} device${room.deviceIds.length == 1 ? '' : 's'}',
                      style: TextStyle(
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.5),
                        fontSize: 12,
                      ),
                    ),
                  ],
                ],
              ),
            ),
            if (isSelected)
              Icon(Icons.check_circle, color: _teal, size: 20)
            else
              Icon(
                Icons.circle_outlined,
                color: CelestialColors.orbitRing.withValues(alpha: 0.4),
                size: 20,
              ),
          ],
        ),
      ),
    );
  }

  Widget _buildCreateRoomRow() {
    return GestureDetector(
      onTap: () {
        HapticFeedback.selectionClick();
        setState(() {
          _isCreatingRoom = !_isCreatingRoom;
          _selectedRoomId = null;
        });
      },
      behavior: HitTestBehavior.opaque,
      child: Column(
        children: [
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
            child: Row(
              children: [
                Icon(
                  Icons.add_circle_outline,
                  color: _isCreatingRoom
                      ? _teal
                      : CelestialColors.textSecondary.withValues(alpha: 0.5),
                  size: 20,
                ),
                const SizedBox(width: 12),
                const Expanded(
                  child: Text(
                    'Create New Room',
                    style: TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 14,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                ),
                if (_isCreatingRoom)
                  const Icon(Icons.check_circle, color: _teal, size: 20)
                else
                  Icon(
                    Icons.circle_outlined,
                    color: CelestialColors.orbitRing.withValues(alpha: 0.4),
                    size: 20,
                  ),
              ],
            ),
          ),
          if (_isCreatingRoom) ...[
            Divider(
                height: 1,
                color: CelestialColors.orbitRing.withValues(alpha: 0.3)),
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 8, 16, 12),
              child: TextField(
                controller: _newRoomController,
                autofocus: true,
                style: const TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 14,
                ),
                decoration: InputDecoration(
                  hintText: 'Room name',
                  hintStyle: TextStyle(
                    color:
                        CelestialColors.textSecondary.withValues(alpha: 0.4),
                  ),
                  contentPadding: const EdgeInsets.symmetric(
                      horizontal: 12, vertical: 10),
                  filled: true,
                  fillColor: CelestialColors.backgroundDark,
                  border: OutlineInputBorder(
                    borderRadius: BorderRadius.circular(10),
                    borderSide: BorderSide(
                      color:
                          CelestialColors.orbitRing.withValues(alpha: 0.5),
                    ),
                  ),
                  enabledBorder: OutlineInputBorder(
                    borderRadius: BorderRadius.circular(10),
                    borderSide: BorderSide(
                      color:
                          CelestialColors.orbitRing.withValues(alpha: 0.5),
                    ),
                  ),
                  focusedBorder: OutlineInputBorder(
                    borderRadius: BorderRadius.circular(10),
                    borderSide: const BorderSide(color: _teal),
                  ),
                ),
                onChanged: (_) => setState(() {}),
              ),
            ),
          ],
        ],
      ),
    );
  }
}
