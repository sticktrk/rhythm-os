import 'dart:async';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:rhythm_core/rhythm_core.dart' show DirectHubAccess;
import 'package:url_launcher/url_launcher.dart';
import 'package:uuid/uuid.dart';

import '../../providers/server_sync_provider.dart';
import '../../providers/home_provider.dart';
import '../../services/analytics_service.dart';
import '../../services/device_commissioning_flow.dart';
import '../../services/ha_matter_pairing_journal.dart';
import '../../services/matter_setup_payload.dart';
import '../../services/phone_matter_commissioner.dart';
import '../../widgets/matter_setup_code_dialog.dart';
import 'device_pairing_scanner_screen.dart';

/// HA is the only device authority in this screen. No direct hub fallbacks.
class HaMatterManagementScreen extends StatefulWidget {
  const HaMatterManagementScreen(
      {super.key,
      required this.api,
      required this.journal,
      required this.isCurrentTarget,
      this.phoneCommissioner = const PhoneMatterCommissioner()});
  final RhythmHaMatterApi api;
  final HaMatterPairingJournal journal;
  final bool Function() isCurrentTarget;
  final PhoneMatterCommissioner phoneCommissioner;

  static Future<void> show(BuildContext context) async {
    final sync = context.read<ServerSyncProvider>();
    final identity = sync.connectedServerInstanceId;
    final hub = sync.connectedServerHub;
    final homes = context.read<HomeProvider>();
    final selectedHubId = homes.activeServerHub?.id;
    final selectedHomeId = homes.currentHome?.id;
    final selection = DirectHubAccess.capture();
    if (!sync.canManageHaMatter ||
        !sync.connection.connected ||
        identity == null) {
      return;
    }
    final homeId = hub?.homeId ?? 'ingress';
    await Navigator.of(context).push<void>(MaterialPageRoute(
        builder: (_) => HaMatterManagementScreen(
            api: sync.api.haMatter,
            journal: HaMatterPairingJournal('$homeId:$identity'),
            isCurrentTarget: () =>
                selection.isSameSelection &&
                homes.activeServerHub?.id == selectedHubId &&
                homes.currentHome?.id == selectedHomeId &&
                sync.canManageHaMatter &&
                sync.connection.connected &&
                sync.connectedServerInstanceId == identity &&
                sync.connectedServerHub?.homeId == hub?.homeId)));
  }

  @override
  State<HaMatterManagementScreen> createState() =>
      _HaMatterManagementScreenState();
}

class _HaMatterManagementScreenState extends State<HaMatterManagementScreen> {
  final _code = TextEditingController();
  late final DeviceCommissioningFlow<HaMatterPairingReceipt> _flow;
  HaMatterCatalog? _catalog;
  HaMatterPairingReceipt? _receipt;
  HaMatterCodeSource _source = HaMatterCodeSource.originalLabel;
  String? _sessionId;
  String? _message;
  bool _busy = true;
  bool _phoneSupported = false;
  bool _journalReady = false;

  bool get _active => mounted && widget.isCurrentTarget();

  @override
  void initState() {
    super.initState();
    _flow = DeviceCommissioningFlow(readReceipt: (id) async {
      if (!_active) return null;
      final receipt = await widget.api.getPairing(id);
      if (!_active) return null;
      return switch (receipt.state) {
        HaMatterPairingState.completed => CommissioningReceipt(
            CommissioningReceiptState.complete,
            result: receipt),
        HaMatterPairingState.failed => CommissioningReceipt(
            CommissioningReceiptState.failed,
            result: receipt),
        HaMatterPairingState.pending => CommissioningReceipt(
            CommissioningReceiptState.pending,
            result: receipt),
        HaMatterPairingState.unknown => null,
      };
    });
    unawaited(_initialize());
  }

  Future<void> _initialize() async {
    try {
      _sessionId = widget.journal.read();
      _journalReady = true;
      if (_sessionId != null) _flow.expectReceipt(_sessionId);
      final phoneSupported = await widget.phoneCommissioner.isSupported();
      if (!_active) return;
      _phoneSupported = phoneSupported;
      await _refresh();
    } catch (_) {
      if (mounted) {
        setState(() => _message =
            'Could not restore the previous pairing. Reconnect to this Home Assistant before adding another device.');
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _refresh() async {
    if (!_active) return;
    final catalog = await widget.api.getCatalog();
    if (!_active) return;
    _catalog = catalog;
    if (_sessionId != null) {
      final receipt = await widget.api.getPairing(_sessionId!);
      if (!_active) return;
      await _accept(receipt);
    }
  }

  Future<void> _accept(HaMatterPairingReceipt receipt) async {
    if (!_active || receipt.sessionId != _sessionId) return;
    _receipt = receipt;
    _message = switch (receipt.state) {
      HaMatterPairingState.completed => receipt.needsDeviceConfirmation
          ? 'Added to Home Assistant. Review the correct device below and save its original label code. Then review which lights Rhythm may control in the addon.'
          : 'Added to Home Assistant. Assign its area in Home Assistant, then review which lights Rhythm may control in the addon.',
      HaMatterPairingState.failed =>
        'Home Assistant could not finish pairing. Check the device and its pairing window before trying again.',
      HaMatterPairingState.pending =>
        'Home Assistant is still finishing this attempt. Check its result before starting another.',
      HaMatterPairingState.unknown =>
        'The outcome is not known. Keep the device powered on and check this attempt again. Do not pair it again yet.',
    };
    if (!receipt.unresolved) {
      _flow.acceptTerminalResponse();
      if (receipt.originalCodeSaved) {
        _message =
            'Original label code saved. Review which lights Rhythm may control in the Home Assistant addon.';
      }
      if (!receipt.needsDeviceConfirmation) {
        await widget.journal.clear(receipt.sessionId);
        _sessionId = null;
      }
    }
    unawaited(AnalyticsService().logHaMatterAction(
        action: 'check',
        outcome: receipt.state.name,
        sessionId: receipt.sessionId));
  }

  Future<void> _run(String action, Future<void> Function() operation) async {
    if (_busy || !_active) return;
    setState(() => _busy = true);
    try {
      await operation();
    } catch (_) {
      if (mounted) {
        _message = _sessionId == null
            ? 'The request could not be completed. Reconnect to Home Assistant and try again.'
            : 'The pairing result could not be confirmed. Check the existing attempt before trying again.';
      }
      unawaited(AnalyticsService().logHaMatterAction(
          action: action, outcome: 'unknown', sessionId: _sessionId));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _pair({required bool phone}) => _run('pair', () async {
        if (!_journalReady || !isLikelyMatterSetupPayload(_code.text)) return;
        final started = await _flow.begin();
        try {
          if (!_active) return;
          if (started.state == CommissioningStartState.recovered) {
            await _accept(started.result!);
            return;
          }
          if (started.state != CommissioningStartState.ready) {
            _message =
                'The previous attempt needs to be checked before pairing again.';
            return;
          }
          final id = const Uuid().v4();
          // The HA API accepts UUIDs; the shared lifecycle owns reconciliation.
          _flow.sessionId = id;
          await widget.journal.save(id);
          if (!_active) return;
          _sessionId = id;
          _flow.expectReceipt(id);
          _receipt = null;
          unawaited(AnalyticsService().logHaMatterAction(
              action: 'pair', outcome: 'attempt', sessionId: id));
          final code = normalizeMatterSetupPayload(_code.text);
          if (phone) {
            try {
              await widget.phoneCommissioner.commission(
                  baseUrl: widget.api.baseUrl,
                  authToken: widget.api.handoffAuthToken,
                  originalSetupPayload: code,
                  sessionId: id,
                  backend: PhoneMatterBackend.haAddon,
                  codeSource: _source,
                  isCurrentTarget: () => _active);
            } catch (_) {
              // A lost or cancelled callback never proves the server did not pair.
            }
            if (!_active) return;
            await _accept(await widget.api.getPairing(id));
          } else {
            await _accept(await widget.api
                .pair(sessionId: id, setupCode: code, codeSource: _source));
          }
          _code.clear();
          if (_active) {
            _catalog = await widget.api.getCatalog();
          }
        } finally {
          _flow.end();
        }
      });

  Future<void> _scan() async {
    final result = await DevicePairingScannerScreen.show(context,
        showEnterCodeAction: false, analyticsSource: 'ha_matter');
    if (!_active || result?.action != DevicePairingScannerAction.matter) return;
    setState(() => _code.text = result!.payload!);
  }

  Future<void> _showCode(String code,
      {bool sharing = false, int? expiresIn}) async {
    if (!_active) return;
    await showDialog<void>(
        context: context,
        builder: (_) => MatterSetupCodeDialog(
            title: sharing
                ? 'Temporary sharing code'
                : 'Original Matter label code',
            description: sharing
                ? 'Use this in another Matter app within ${((expiresIn ?? 300) / 60).ceil()} minutes. This is not the original label code.'
                : 'Saved by the owner from the device label. Home Assistant cannot recover a missing original code. Keep this private.',
            secret: RhythmPairingRecoverySecret(
                payloadKind: code.startsWith('MT:')
                    ? RhythmPairingRecoveryPayloadKind.qrCode
                    : RhythmPairingRecoveryPayloadKind.manualCode,
                setupPayload: code,
                capturedAt: DateTime.now())));
  }

  Future<bool> _confirm(String title, String message) async =>
      await showDialog<bool>(
          context: context,
          builder: (context) =>
              AlertDialog(title: Text(title), content: Text(message), actions: [
                TextButton(
                    onPressed: () => Navigator.pop(context, false),
                    child: const Text('Cancel')),
                FilledButton(
                    onPressed: () => Navigator.pop(context, true),
                    child: const Text('Confirm')),
              ])) ==
      true;

  Future<void> _closeReviewedAttempt() async {
    final id = _sessionId;
    if (id == null || _receipt?.canCloseAfterReview != true || !_active) return;
    if (!await _confirm('Close this reviewed attempt?',
        'Confirm you checked Home Assistant for the device and reviewed its actual pairing state. This closes the unresolved attempt without pairing again. Keep the original physical label; save it for the correct device once identified.')) {
      return;
    }
    if (!_active) return;
    await _run('check', () async {
      // Re-read before closing: never abandon work that is still pending.
      final receipt = await widget.api.getPairing(id);
      if (!_active || !receipt.canCloseAfterReview) return;
      await widget.journal.clear(id);
      _flow.acceptTerminalResponse();
      _sessionId = null;
      _receipt = null;
      _code.clear();
      _message = 'Reviewed attempt closed. No new pairing was started.';
    });
  }

  Future<void> _saveCode(HaMatterDevice device) async {
    var enteredCode = '';
    final code = await showDialog<String>(
        context: context,
        builder: (context) => StatefulBuilder(
            builder: (context, setDialogState) => AlertDialog(
                    title: Text('Save original label code for ${device.name}'),
                    content: Column(mainAxisSize: MainAxisSize.min, children: [
                      const Text(
                          'Enter the original code printed on this device or its packaging. Do not use a temporary sharing code.'),
                      TextField(
                          obscureText: true,
                          autocorrect: false,
                          enableSuggestions: false,
                          onChanged: (value) =>
                              setDialogState(() => enteredCode = value),
                          decoration: const InputDecoration(
                              labelText: 'Original Matter code')),
                    ]),
                    actions: [
                      TextButton(
                          onPressed: () => Navigator.pop(context),
                          child: const Text('Cancel')),
                      FilledButton(
                          onPressed: isLikelyMatterSetupPayload(enteredCode)
                              ? () => Navigator.pop(context,
                                  normalizeMatterSetupPayload(enteredCode))
                              : null,
                          child: const Text('Save')),
                    ])));
    if (code == null || !_active) return;
    await _run('save_code', () async {
      await widget.api.saveOriginalCode(device, code);
      if (_active) _message = 'Original label code saved for ${device.name}.';
      unawaited(AnalyticsService()
          .logHaMatterAction(action: 'save_code', outcome: 'completed'));
    });
  }

  Future<void> _deviceAction(HaMatterDevice device, String action) async {
    if (!_active || _busy) return;
    if (action == 'save_code') {
      return _saveCode(device);
    }
    if (action == 'remove' &&
        !await _confirm(
            'Remove ${device.name} from Home Assistant?',
            device.isBridge
                ? 'This bridge and its attached devices will stop being available through Home Assistant. Other Matter fabrics are unaffected.'
                : 'Rhythm and Home Assistant will lose access through this fabric. Other Matter fabrics are unaffected.')) {
      return;
    }
    if (action == 'confirm_device' &&
        !await _confirm('Save pairing code for ${device.name}?',
            'Confirm that this is the physical device you just paired. Its original label code will be attached to this device.')) {
      return;
    }
    if (!_active) return;
    await _run(action, () async {
      switch (action) {
        case 'read_code':
          final code = await widget.api.getOriginalCode(device);
          if (code == null) {
            _message =
                'No original code is saved. Use “Save original label code” with the code printed on this device. It cannot be retrieved from Home Assistant.';
          } else {
            await _showCode(code);
          }
        case 'share':
          final shared = await widget.api.shareDevice(device);
          await _showCode(shared.code,
              sharing: true, expiresIn: shared.expiresIn);
        case 'remove':
          await widget.api.removeDevice(device);
          if (_active) {
            _catalog = await widget.api.getCatalog();
            _message = 'Removed from Home Assistant.';
          }
        case 'confirm_device':
          final id = _sessionId;
          if (id != null) {
            await _accept(await widget.api.confirmDevice(id, device));
          }
      }
      unawaited(AnalyticsService()
          .logHaMatterAction(action: action, outcome: 'completed'));
    });
  }

  @override
  void dispose() {
    if (_flow.isRunning && _sessionId != null) {
      unawaited(widget.phoneCommissioner.cancel(_sessionId!));
    }
    _flow.dispose();
    _code.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final catalog = _catalog;
    final current = widget.isCurrentTarget();
    final ready = !_busy && current && _journalReady && _sessionId == null;
    return Scaffold(
        appBar: AppBar(title: const Text('Home Assistant Matter')),
        body: ListView(padding: const EdgeInsets.all(20), children: [
          const Text(
              'Devices pair to Home Assistant. Rhythm controls only the lights approved in the addon.'),
          if (!current)
            const Padding(
                padding: EdgeInsets.symmetric(vertical: 12),
                child:
                    Text('Reconnect to the same Home Assistant to continue.')),
          if (_busy) const LinearProgressIndicator(),
          if (_message != null)
            Padding(
                padding: const EdgeInsets.symmetric(vertical: 12),
                child: Text(_message!)),
          OutlinedButton(
              onPressed:
                  _busy || !current ? null : () => _run('check', _refresh),
              child: Text(_sessionId == null
                  ? 'Refresh devices'
                  : 'Check pairing result')),
          if (_receipt?.canCloseAfterReview == true)
            TextButton(
                onPressed: _busy || !current ? null : _closeReviewedAttempt,
                child: const Text(
                    'I reviewed Home Assistant; close this attempt')),
          if (catalog?.available != true && !_busy)
            const Text(
                'Home Assistant Matter management is unavailable. Check that its Matter integration is connected and you have owner access.'),
          if (catalog?.pairOnNetwork == true ||
              catalog?.phoneCommissioning == true) ...[
            const SizedBox(height: 16),
            DropdownButtonFormField<HaMatterCodeSource>(
                isExpanded: true,
                initialValue: _source,
                decoration: const InputDecoration(labelText: 'Code source'),
                items: const [
                  DropdownMenuItem(
                      value: HaMatterCodeSource.originalLabel,
                      child: Text('Original device label')),
                  DropdownMenuItem(
                      value: HaMatterCodeSource.sharing,
                      child: Text('Temporary sharing code'))
                ],
                onChanged: ready ? (v) => setState(() => _source = v!) : null),
            TextField(
                controller: _code,
                enabled: ready,
                obscureText: true,
                autocorrect: false,
                enableSuggestions: false,
                decoration: const InputDecoration(
                    labelText: 'Matter QR payload or setup code'),
                onChanged: (_) => setState(() {})),
            if (supportsDevicePairingCamera)
              TextButton(
                  onPressed: ready ? _scan : null,
                  child: const Text('Scan Matter QR code')),
            if (catalog?.pairOnNetwork == true)
              FilledButton(
                  onPressed: ready && isLikelyMatterSetupPayload(_code.text)
                      ? () => _pair(phone: false)
                      : null,
                  child: const Text('Add device already on the network')),
            if (catalog?.phoneCommissioning == true && _phoneSupported)
              OutlinedButton(
                  onPressed: ready && isLikelyMatterSetupPayload(_code.text)
                      ? () => _pair(phone: true)
                      : null,
                  child: const Text('Set up new device with this phone')),
            const Text(
                'For a device in another Matter app, open a new sharing window there. New devices need phone network setup first.'),
          ],
          const SizedBox(height: 20),
          OutlinedButton(
              onPressed: () => launchUrl(
                  Uri.parse(
                      'https://my.home-assistant.io/redirect/integrations/'),
                  mode: LaunchMode.externalApplication),
              child: const Text('Review devices and areas in Home Assistant')),
          const Text(
              'After pairing, open Rhythm inside Home Assistant to review and approve the lights it may control.'),
          for (final device in catalog?.devices ?? <HaMatterDevice>[])
            ListTile(
                title: Text(device.name),
                subtitle:
                    Text(device.isBridge ? 'Matter bridge' : 'Matter device'),
                trailing: PopupMenuButton<String>(
                    enabled: !_busy && current,
                    onSelected: (action) => _deviceAction(device, action),
                    itemBuilder: (_) => [
                          if (_receipt?.needsDeviceConfirmation == true &&
                              _sessionId != null)
                            const PopupMenuItem(
                                value: 'confirm_device',
                                child:
                                    Text('Save pairing code for this device')),
                          if (catalog!.originalSetupCode) ...[
                            const PopupMenuItem(
                                value: 'read_code',
                                child: Text('Show original label code')),
                            const PopupMenuItem(
                                value: 'save_code',
                                child: Text('Save original label code')),
                          ],
                          if (catalog.share)
                            const PopupMenuItem(
                                value: 'share',
                                child: Text('Share with another Matter app')),
                          if (catalog.remove)
                            const PopupMenuItem(
                                value: 'remove',
                                child: Text('Remove from Home Assistant')),
                        ])),
        ]));
  }
}
