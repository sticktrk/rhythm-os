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

DevicePairingCode classifyDevicePairingCode(String value) {
  final normalized = value.trim();
  if (detectMatterSetupPayloadKind(normalized) == MatterSetupPayloadKind.qr) {
    return DevicePairingCode(
      kind: DevicePairingCodeKind.matter,
      payload: normalized,
    );
  }

  final upper = normalized.toUpperCase();
  if (upper.startsWith('X-HM://')) {
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
            'Rhythm can’t pair a bulb with its Apple Home code. Look for a '
            'Matter QR code or Matter setup code instead.',
      ),
    DevicePairingCodeKind.hue => const DevicePairingGuidance(
        title: 'Pair this bulb in the Hue app',
        message: 'Use the Philips Hue app to add this bulb. Then return to '
            'Add & Review and use Re-Sync to bring it into Rhythm.',
      ),
    DevicePairingCodeKind.unknown => const DevicePairingGuidance(
        title: 'Unknown QR code',
        message:
            'Rhythm doesn’t recognize this pairing code. Try the Matter QR '
            'code or enter the Matter setup code printed beside it.',
      ),
    DevicePairingCodeKind.matter => const DevicePairingGuidance(
        title: 'Matter code',
        message: 'This Matter setup code is ready to pair.',
      ),
  };
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
