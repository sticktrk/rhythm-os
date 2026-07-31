import 'matter_setup_payload.dart';

enum DevicePairingCodeKind { matter, homeKit, hue, unknown }

class DevicePairingCode {
  const DevicePairingCode({
    required this.kind,
    required this.payload,
  });

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
    this.choices = const [],
    this.continuationAllowed = true,
  });

  final DevicePairingCode code;
  final DevicePairingGuidance guidance;
  final List<DevicePairingCode> choices;
  final bool continuationAllowed;

  bool get requiresChoice => choices.length > 1;

  bool get canContinue =>
      continuationAllowed &&
      !requiresChoice &&
      _canContinueWithDevicePairingCode(code);
}

/// Shared intake processor for camera scans and manually entered codes.
///
/// Six-character Hue serials are a Hue Bridge/Zigbee onboarding mechanism.
/// They are actionable only while a connected bridge explicitly advertises
/// serial search. Direct Hue Bluetooth pairing does not use this intake.
DevicePairingCodeDecision? processDevicePairingCodes(
  Iterable<String> values, {
  bool hueBridgeSerialSearchAvailable = false,
  bool hueBridgeOnly = false,
}) {
  final codes = values
      .map(classifyDevicePairingCode)
      .where((code) => code.payload.isNotEmpty)
      .toList();
  if (codes.isEmpty) return null;

  final matterCodes = _distinctActionableCodes(
    codes.where((code) => code.kind == DevicePairingCodeKind.matter),
  );
  final hueSerialCodes = _distinctActionableCodes(
    codes.where(
      (code) =>
          code.kind == DevicePairingCodeKind.hue &&
          normalizeHueBridgeSerial(code.payload) != null,
    ),
  );

  if (hueBridgeOnly) {
    if (hueSerialCodes.isNotEmpty) {
      final code = hueSerialCodes.first;
      return DevicePairingCodeDecision(
        code: code,
        guidance: guidanceForDevicePairingCode(
          code.kind,
          hueBridgeSerialSearchAvailable: hueBridgeSerialSearchAvailable,
        ),
        continuationAllowed: hueBridgeSerialSearchAvailable,
      );
    }

    return DevicePairingCodeDecision(
      code: codes.first,
      guidance: const DevicePairingGuidance(
        title: 'Hue bulb serial not found',
        message: 'Scan the QR code beside the six-character serial printed '
            'on the Hue bulb, or enter that serial manually.',
      ),
      continuationAllowed: false,
    );
  }

  if (matterCodes.isNotEmpty &&
      hueSerialCodes.isNotEmpty &&
      hueBridgeSerialSearchAvailable) {
    final choices = [matterCodes.first, hueSerialCodes.first];
    return DevicePairingCodeDecision(
      code: choices.first,
      guidance: guidanceForDevicePairingCode(choices.first.kind),
      choices: choices,
    );
  }

  if (matterCodes.isNotEmpty) {
    final code = matterCodes.first;
    return DevicePairingCodeDecision(
      code: code,
      guidance: guidanceForDevicePairingCode(code.kind),
    );
  }

  if (hueSerialCodes.isNotEmpty) {
    final code = hueSerialCodes.first;
    return DevicePairingCodeDecision(
      code: code,
      guidance: guidanceForDevicePairingCode(
        code.kind,
        hueBridgeSerialSearchAvailable: hueBridgeSerialSearchAvailable,
      ),
      continuationAllowed: hueBridgeSerialSearchAvailable,
    );
  }

  final code = codes.first;
  return DevicePairingCodeDecision(
    code: code,
    guidance: guidanceForDevicePairingCode(
      code.kind,
      hueBridgeSerialSearchAvailable: hueBridgeSerialSearchAvailable,
    ),
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
      payload: normalizeHueBridgeSerial(normalized) ?? normalized,
    );
  }

  return DevicePairingCode(
    kind: DevicePairingCodeKind.unknown,
    payload: normalized,
  );
}

DevicePairingGuidance guidanceForDevicePairingCode(
  DevicePairingCodeKind kind, {
  bool hueBridgeSerialSearchAvailable = false,
}) {
  return switch (kind) {
    DevicePairingCodeKind.homeKit => const DevicePairingGuidance(
        title: 'HomeKit isn’t supported',
        message:
            'Rhythm can’t add this device with its Apple Home code. Look for a '
            'Matter QR code or Matter setup code instead.',
      ),
    DevicePairingCodeKind.hue when hueBridgeSerialSearchAvailable =>
      const DevicePairingGuidance(
        title: 'Hue Bridge serial',
        message: 'This six-character serial is ready to search for through '
            'your connected Hue Bridge.',
      ),
    DevicePairingCodeKind.hue => const DevicePairingGuidance(
        title: 'Use nearby Bluetooth scan',
        message: 'A six-character Hue serial can add a Zigbee bulb through a '
            'connected Hue Bridge, but that path is not available right now. '
            'To pair directly, choose “Scan for nearby Hue Bluetooth bulbs.”',
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
  if (normalizeHueBridgeSerial(normalized) != null) {
    return true;
  }

  if (_hasHuePrefix(upper)) {
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

bool _hasHuePrefix(String upper) {
  return upper.startsWith('HUE:') ||
      upper.startsWith('HUE://') ||
      upper.startsWith('PHILIPS-HUE:') ||
      upper.startsWith('PHILIPSHUE:');
}

bool _canContinueWithDevicePairingCode(DevicePairingCode code) {
  return code.kind == DevicePairingCodeKind.matter ||
      (code.kind == DevicePairingCodeKind.hue &&
          normalizeHueBridgeSerial(code.payload) != null);
}

List<DevicePairingCode> _distinctActionableCodes(
  Iterable<DevicePairingCode> codes,
) {
  final seen = <String>{};
  return [
    for (final code in codes)
      if (seen.add('${code.kind.name}:${code.payload}')) code,
  ];
}

/// Returns the normalized six-character serial used by Hue Bridge search.
///
/// Separators are accepted for manual entry. A Philips Hue Zigbee setup QR is
/// also accepted when it begins with validated `HUE:Z:{install-token}
/// M:{EUI-64}` fields; product-specific trailing fields are ignored. The final
/// six hexadecimal characters of `M:` are used for the Bridge search. The value
/// is never used to authenticate or identify a direct Hue Bluetooth connection.
String? normalizeHueBridgeSerial(String value) {
  final setupQrSerial = hueBridgeSerialFromSetupQr(value);
  if (setupQrSerial != null) return setupQrSerial;

  var candidate = value.trim().toUpperCase();
  for (final prefix in const [
    'PHILIPS-HUE:',
    'PHILIPSHUE:',
    'HUE://',
    'HUE:',
  ]) {
    if (candidate.startsWith(prefix)) {
      candidate = candidate.substring(prefix.length);
      break;
    }
  }
  candidate = candidate.replaceAll(RegExp(r'[\s-]'), '');
  if (!RegExp(r'^[0-9A-F]{6}$').hasMatch(candidate)) return null;
  return candidate;
}

/// Extracts the Bridge search serial from a Philips Hue Zigbee setup QR.
String? hueBridgeSerialFromSetupQr(String value) {
  final match = RegExp(
    r'^HUE:Z:[A-Z0-9]+ M:([A-Z0-9]{16})(?:\s|$)',
    caseSensitive: false,
  ).firstMatch(value.trim());
  final eui64 = match?.group(1)?.toUpperCase();
  if (eui64 == null) return null;
  final serial = eui64.substring(eui64.length - 6);
  return RegExp(r'^[0-9A-F]{6}$').hasMatch(serial) ? serial : null;
}
