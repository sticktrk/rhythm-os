import 'matter_setup_payload.dart';

enum DevicePairingCodeKind { matter, homeKit, hue, unknown }

class DevicePairingCode {
  const DevicePairingCode({required this.kind, required this.payload});

  final DevicePairingCodeKind kind;
  final String payload;
}

class DevicePairingGuidance {
  const DevicePairingGuidance({required this.title, required this.message});

  final String title;
  final String message;
}

class DevicePairingCodeDecision {
  const DevicePairingCodeDecision({
    required this.code,
    required this.guidance,
  });

  final DevicePairingCode code;
  final DevicePairingGuidance guidance;

  bool get canContinue => code.kind == DevicePairingCodeKind.matter;
}

/// Shared intake processor for camera scans and manually entered codes.
///
/// Matter wins when a camera reports multiple codes in one frame. Everything
/// else returns ecosystem-specific guidance without entering commissioning.
DevicePairingCodeDecision? processDevicePairingCodes(Iterable<String> values) {
  final codes = values
      .map(classifyDevicePairingCode)
      .where((code) => code.payload.isNotEmpty)
      .toList();
  if (codes.isEmpty) return null;

  final code = codes.cast<DevicePairingCode?>().firstWhere(
            (candidate) => candidate?.kind == DevicePairingCodeKind.matter,
            orElse: () => null,
          ) ??
      codes.first;
  return DevicePairingCodeDecision(
    code: code,
    guidance: guidanceForDevicePairingCode(code.kind),
  );
}

DevicePairingCode classifyDevicePairingCode(String value) {
  final normalized = value.trim();
  if (isLikelyMatterSetupPayload(normalized)) {
    return DevicePairingCode(
      kind: DevicePairingCodeKind.matter,
      payload: normalized,
    );
  }

  final upper = normalized.toUpperCase();
  if (upper.startsWith('X-HM://') || _isManualHomeKitCode(normalized)) {
    return DevicePairingCode(
      kind: DevicePairingCodeKind.homeKit,
      payload: normalized,
    );
  }

  if (_isHuePairingCode(normalized, upper)) {
    return DevicePairingCode(
      kind: DevicePairingCodeKind.hue,
      payload: normalized,
    );
  }

  return DevicePairingCode(
    kind: DevicePairingCodeKind.unknown,
    payload: normalized,
  );
}

DevicePairingGuidance guidanceForDevicePairingCode(DevicePairingCodeKind kind) {
  return switch (kind) {
    DevicePairingCodeKind.homeKit => const DevicePairingGuidance(
        title: 'HomeKit isn’t supported',
        message:
            'Rhythm can’t add this device with its Apple Home code. Look for a '
            'Matter QR code or Matter setup code instead.',
      ),
    DevicePairingCodeKind.hue => const DevicePairingGuidance(
        title: 'Pair this device in the Hue app',
        message: 'Use the Philips Hue app to add this device. Then return to '
            'Add & Review and use Sync Devices to bring it into Rhythm.',
      ),
    DevicePairingCodeKind.unknown => const DevicePairingGuidance(
        title: 'Code not recognized',
        message: 'Rhythm couldn’t identify this code. Check that you entered '
            'the complete setup code, or try scanning the device QR code.',
      ),
    DevicePairingCodeKind.matter => const DevicePairingGuidance(
        title: 'Matter code',
        message: 'This Matter setup code is ready to pair.',
      ),
  };
}

bool _isManualHomeKitCode(String value) {
  final compact = value.replaceAll(RegExp(r'[\s-]'), '');
  return compact.length == 8 && RegExp(r'^\d+$').hasMatch(compact);
}

bool _isHuePairingCode(String normalized, String upper) {
  if (upper.startsWith('HUE:') ||
      upper.startsWith('HUE://') ||
      upper.startsWith('PHILIPS-HUE:') ||
      upper.startsWith('PHILIPSHUE:')) {
    return true;
  }

  final uri = Uri.tryParse(normalized);
  if (uri == null || (uri.scheme != 'http' && uri.scheme != 'https')) {
    return false;
  }

  final host = uri.host.toLowerCase();
  return host == 'philips-hue.com' ||
      host.endsWith('.philips-hue.com') ||
      host == 'meethue.com' ||
      host.endsWith('.meethue.com');
}
