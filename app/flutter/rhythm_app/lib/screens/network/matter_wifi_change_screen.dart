import 'dart:async';
import 'package:flutter/material.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:uuid/uuid.dart';
import '../../services/analytics_service.dart';
import '../../services/cloud_backed_server_api.dart';
import '../../widgets/solar_orbit.dart';
import 'network_ui.dart';
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
                backgroundColor: CelestialColors.backgroundCard,
                title: const Text('Move this bulb?',
                    style: TextStyle(color: CelestialColors.textPrimary)),
                content: const Text(
                    'Keep the bulb powered on and both networks available. Do not move the Rhythm Box or reset the bulb until the change finishes.',
                    style: TextStyle(
                        color: CelestialColors.textSecondary, height: 1.4)),
                actions: [
                  TextButton(
                      onPressed: () => Navigator.pop(context, false),
                      child: const Text('Cancel',
                          style:
                              TextStyle(color: CelestialColors.textSecondary))),
                  TextButton(
                      style: TextButton.styleFrom(foregroundColor: networkTeal),
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
      return 'Move this bulb to another saved network. Its name, room and schedules stay the same. Works with Matter Wi-Fi bulbs that are online; Thread and bridged devices cannot be moved.';
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

  ({IconData icon, Color color, String title}) get _status {
    final receipt = _receipt;
    if (receipt == null) {
      return (
        icon: Icons.wifi_rounded,
        color: networkTeal,
        title: 'Move this bulb'
      );
    }
    if (receipt.isPending) {
      return (
        icon: Icons.sync_rounded,
        color: networkTeal,
        title: 'Changing network…'
      );
    }
    if (receipt.succeeded) {
      return (
        icon: Icons.check_rounded,
        color: const Color(0xFF22C55E),
        title: 'Network changed'
      );
    }
    return (
      icon: Icons.priority_high_rounded,
      color: CelestialColors.warning,
      title: receipt.rollbackVerified
          ? 'Network not changed'
          : 'Network change not confirmed'
    );
  }

  @override
  Widget build(BuildContext context) {
    final canStart = _loaded &&
        !_busy &&
        _error == null &&
        _receipt?.isPending != true &&
        (_receipt?.retryAfterMs ?? 0) == 0 &&
        (_operation == null || _receipt != null);
    final status = _status;
    return NetworkScaffold(
        title: 'Change bulb Wi-Fi',
        busy: _busy || _receipt?.isPending == true,
        footer: Column(mainAxisSize: MainAxisSize.min, children: [
          FilledButton(
              key: const ValueKey('wifi-change-start'),
              style: networkPrimaryButtonStyle,
              onPressed: canStart ? _start : null,
              child: const Text('Choose network')),
          const SizedBox(height: 4),
          TextButton(
              onPressed: _busy ? null : _refresh,
              style: TextButton.styleFrom(foregroundColor: networkTeal),
              child: const Text('Check status')),
        ]),
        children: [
          Container(
              padding: const EdgeInsets.all(20),
              decoration: BoxDecoration(
                  color: CelestialColors.backgroundCard,
                  borderRadius: BorderRadius.circular(14),
                  border: Border.all(
                      color: CelestialColors.orbitRing.withValues(alpha: 0.5))),
              child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(children: [
                      Container(
                          width: 40,
                          height: 40,
                          decoration: BoxDecoration(
                              shape: BoxShape.circle,
                              color: status.color.withValues(alpha: 0.16)),
                          child:
                              Icon(status.icon, color: status.color, size: 20)),
                      const SizedBox(width: 14),
                      Expanded(
                          child: Text(status.title,
                              style: const TextStyle(
                                  color: CelestialColors.textPrimary,
                                  fontSize: 16,
                                  fontWeight: FontWeight.w600))),
                    ]),
                    const SizedBox(height: 14),
                    Text(_message,
                        key: const ValueKey('wifi-change-status'),
                        style: const TextStyle(
                            color: CelestialColors.textSecondary,
                            fontSize: 14,
                            height: 1.45)),
                  ])),
          if (_error != null) NetworkNotice(text: _error!),
          if ((_receipt?.retryAfterMs ?? 0) > 0)
            const NetworkNotice(
                text:
                    'Waiting for the bulb\u2019s recovery window to close before another attempt.'),
          if (_receipt == null) ...[
            const NetworkSectionHeader('BEFORE YOU START'),
            for (final (icon, text) in const [
              (Icons.power_rounded, 'Keep the bulb powered on.'),
              (
                Icons.router_rounded,
                'Keep both networks available until the change finishes.'
              ),
              (
                Icons.hub_outlined,
                'The Rhythm Box must be able to reach devices on both networks.'
              ),
            ])
              Padding(
                  padding: const EdgeInsets.fromLTRB(4, 0, 4, 12),
                  child: Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Icon(icon,
                            size: 18,
                            color: CelestialColors.textSecondary
                                .withValues(alpha: 0.8)),
                        const SizedBox(width: 12),
                        Expanded(child: NetworkBodyText(text)),
                      ])),
          ],
        ]);
  }
}
