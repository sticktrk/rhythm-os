import 'dart:async';
import 'package:flutter/material.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:uuid/uuid.dart';
import '../../services/analytics_service.dart';
import '../../services/cloud_backed_server_api.dart';
import 'saved_wifi_screen.dart';

class MatterWifiChangeScreen extends StatefulWidget {
  const MatterWifiChangeScreen(
      {super.key, required this.api, required this.deviceId});
  final CloudBackedServerApi api;
  final String deviceId;
  @override
  State<MatterWifiChangeScreen> createState() => _MatterWifiChangeScreenState();
}

class _MatterWifiChangeScreenState extends State<MatterWifiChangeScreen> {
  RhythmWifiChangeReceipt? _receipt;
  String? _operation;
  String? _error;
  bool _busy = false;
  bool _loaded = false;
  Timer? _poll;
  String? _reported;
  String? _entryJourney = const Uuid().v4();
  @override
  void initState() {
    super.initState();
    AnalyticsService().logWifiAction(
        networkChange: true,
        journeyId: _entryJourney!,
        action: 'entry',
        outcome: 'opened');
    _refresh();
  }

  @override
  void dispose() {
    _poll?.cancel();
    super.dispose();
  }

  void _accept(RhythmWifiChangeReceipt? receipt) {
    if (!mounted) return;
    setState(() {
      _receipt = receipt;
      _operation = receipt?.operationId ?? _operation;
      _error = null;
      _loaded = true;
    });
    _poll?.cancel();
    if (receipt?.isPending == true || (receipt?.retryAfterMs ?? 0) > 0) {
      _poll = Timer(const Duration(seconds: 3), _refresh);
    }
    if (receipt != null &&
        !receipt.isPending &&
        _reported != receipt.operationId) {
      _reported = receipt.operationId;
      AnalyticsService().logWifiAction(
          networkChange: true,
          journeyId: receipt.operationId,
          action: 'change',
          outcome: receipt.succeeded
              ? 'succeeded'
              : receipt.code == 'recovery_required'
                  ? 'recovery_required'
                  : 'failed');
    }
  }

  Future<void> _refresh() async {
    if (_busy) return;
    setState(() => _busy = true);
    try {
      final result = _operation == null
          ? await widget.api.getLatestMatterWifiChange(widget.deviceId)
          : await widget.api.getMatterWifiChange(_operation!);
      _accept(result);
    } catch (_) {
      if (mounted) {
        setState(() => _error =
            'The result could not be checked. Keep both networks available and check status again.');
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _start() async {
    final profile = await SavedWifiScreen.select(context, widget.api);
    if (!mounted || profile == null) return;
    final confirm = await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
                title: const Text('Move this bulb to the selected network?'),
                content: const Text(
                    'Keep the bulb powered on and both networks available. The Rhythm Box must be able to reach devices on both networks. Do not move the Box or reset the bulb during the change.'),
                actions: [
                  TextButton(
                      onPressed: () => Navigator.pop(context, false),
                      child: const Text('Cancel')),
                  FilledButton(
                      onPressed: () => Navigator.pop(context, true),
                      child: const Text('Change network'))
                ]));
    if (!mounted || confirm != true) return;
    final operation = _entryJourney ?? const Uuid().v4();
    if (_entryJourney == null) {
      AnalyticsService().logWifiAction(
          networkChange: true,
          journeyId: operation,
          action: 'entry',
          outcome: 'opened');
    }
    _entryJourney = null;
    setState(() {
      _operation = operation;
      _receipt = null;
      _busy = true;
      _error = null;
    });
    AnalyticsService().logWifiAction(
        networkChange: true,
        journeyId: operation,
        action: 'change',
        outcome: 'attempt');
    try {
      _accept(await widget.api.startMatterWifiChange(
          operationId: operation,
          deviceId: widget.deviceId,
          profileId: profile));
    } on RhythmWifiException catch (error) {
      final rejected = {'conflict', 'owner_required', 'not_found', 'rejected'}
          .contains(error.category);
      if (mounted) {
        setState(() {
          if (rejected) _operation = null;
          _error = rejected
              ? 'The request was not accepted. Check owner access, the saved network and any active network change, then check status.'
              : 'The request result is uncertain. Check status before taking another action.';
        });
      }
    } catch (_) {
      // Preserve the ID even when POST delivery is unknown. Check status only;
      // neither a timeout nor a missing receipt authorizes automatic replay.
      if (mounted) {
        setState(() => _error =
            'The request result is uncertain. Check status before taking another action.');
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  String get _message {
    final receipt = _receipt;
    if (receipt == null) {
      return 'Move one reachable Matter Wi-Fi bulb. Thread devices, bridges and offline bulbs cannot use this action.';
    }
    if (receipt.isPending) {
      return 'Changing network and verifying the same bulb. You can leave this screen and check the result later.';
    }
    if (receipt.succeeded) {
      return 'Network change verified. The same bulb responded on the selected network. Its name and room are unchanged.';
    }
    final reason = switch (receipt.code) {
      'unsupported' =>
        'This device does not expose a supported direct Wi-Fi network interface.',
      'offline' =>
        'The bulb could not be reached. Restore its previous network and try again.',
      'network_slots' =>
        'The bulb has no spare network slot. Its working network was preserved. Use a manufacturer-supported network change procedure.',
      'credentials_rejected' =>
        'The bulb rejected the network credentials. Check the saved password.',
      'network_not_found' =>
        'The bulb could not find the selected network. Check its range and supported Wi-Fi band.',
      'fail_safe_busy' =>
        'The bulb is busy with another commissioning operation. Wait before trying again.',
      'rejected' =>
        'The bulb rejected the network configuration. Check network security and compatibility.',
      _ =>
        'The final network state could not be verified. Keep both networks available and check normal bulb controls.',
    };
    return '$reason ${receipt.rollbackVerified ? 'The original network connection was verified.' : 'If it stays offline, restore the old network or follow the manufacturer’s recovery instructions.'}';
  }

  @override
  Widget build(BuildContext context) {
    final canStart = _loaded &&
        !_busy &&
        _error == null &&
        _receipt?.isPending != true &&
        (_receipt?.retryAfterMs ?? 0) == 0 &&
        (_operation == null || _receipt != null);
    return Scaffold(
        appBar: AppBar(title: const Text('Change bulb Wi-Fi')),
        body: ListView(padding: const EdgeInsets.all(24), children: [
          if (_busy || _receipt?.isPending == true)
            const LinearProgressIndicator(),
          const SizedBox(height: 20),
          Text(_message, key: const ValueKey('wifi-change-status')),
          if (_error != null)
            Padding(
                padding: const EdgeInsets.only(top: 16), child: Text(_error!)),
          const SizedBox(height: 20),
          FilledButton(
              key: const ValueKey('wifi-change-start'),
              onPressed: canStart ? _start : null,
              child: const Text('Choose network')),
          TextButton(
              onPressed: _busy ? null : _refresh,
              child: const Text('Check status')),
          if ((_receipt?.retryAfterMs ?? 0) > 0)
            const Text(
                'Waiting for the device fail-safe recovery window before another attempt.'),
        ]));
  }
}
