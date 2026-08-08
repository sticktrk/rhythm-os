import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmDeviceProfileId;

import 'matter_setup_payload.dart';

enum DevicePairingCodeKind { matter, homeKit, hue, localBle, unknown }

/// A parsed, bounded setup request for a server-advertised local BLE profile.
///
/// Profile-specific QR grammar stays behind [parseLocalBleSetupCode]. Shared
/// intake and pairing surfaces only carry the opaque, validated setup map.
class LocalBleSetup {
  LocalBleSetup({
    required this.profileId,
    required Map<String, String> setupFields,
  }) : setupFields = Map.unmodifiable(Map<String, String>.from(setupFields));

  final String profileId;
  final Map<String, String> setupFields;

  Map<String, dynamic> get pairingParams => {
        'profile_id': profileId,
        'setup': setupFields,
      };

  String get deduplicationKey {
    final entries = setupFields.entries.toList(growable: false)
      ..sort((left, right) => left.key.compareTo(right.key));
    final buffer = StringBuffer('${profileId.length}:$profileId');
    for (final entry in entries) {
      buffer
        ..write('|${entry.key.length}:')
        ..write(entry.key)
        ..write('${entry.value.length}:')
        ..write(entry.value);
    }
    return buffer.toString();
  }
}

abstract interface class _LocalBleSetupParser {
  String get profileId;

  LocalBleSetup? tryParse(String value);
}

/// Orein/AiDot OC02001 QR grammar and field translation.
///
/// Keeping this implementation private prevents its field names and payload
/// shape from becoming part of the shared app pairing contract.
final class _OreinOc02001SetupParser implements _LocalBleSetupParser {
  const _OreinOc02001SetupParser();

  @override
  String get profileId => RhythmDeviceProfileId.oreinOc02001Button;

  @override
  LocalBleSetup? tryParse(String value) {
    final match = RegExp(
      r'^B:([0-9A-Fa-f]{12})%G\$S:([0-9A-Za-z_-]{1,64})\$M:([0-9A-Za-z_-]{1,64})$',
    ).firstMatch(value);
    if (match == null) return null;
    return LocalBleSetup(
      profileId: profileId,
      setupFields: {
        'ble_identity': match.group(1)!.toUpperCase(),
        'serial_metadata': match.group(2)!,
        'model_metadata': match.group(3)!,
      },
    );
  }
}

const List<_LocalBleSetupParser> _localBleSetupParsers = [
  _OreinOc02001SetupParser(),
];

/// Profile IDs for which this app build can parse local-BLE QR setup input.
///
/// Server-advertised profiles are intersected with this set before the app
/// exposes QR intake, so a newer appliance cannot make an older app claim
/// support for a grammar it does not understand.
final Set<String> locallyRegisteredLocalBleProfileIds = Set.unmodifiable(
  _localBleSetupParsers.map((parser) => parser.profileId),
);

LocalBleSetup? parseLocalBleSetupCode(String value) {
  for (final parser in _localBleSetupParsers) {
    final setup = parser.tryParse(value);
    if (setup != null) return setup;
  }
  return null;
}

class DevicePairingCode {
  const DevicePairingCode({
    required this.kind,
    required this.payload,
    this.localBleSetup,
  });

  final DevicePairingCodeKind kind;
  final String payload;
  final LocalBleSetup? localBleSetup;
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
  Set<String> supportedLocalBleProfileIds = const {},
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
  final localBleCodes = _distinctActionableCodes(
    codes.where((code) => code.kind == DevicePairingCodeKind.localBle),
  );
  final supportedLocalBleCodes = localBleCodes
      .where(
        (code) => supportedLocalBleProfileIds.contains(
          code.localBleSetup?.profileId,
        ),
      )
      .toList(growable: false);
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

  final choices = <DevicePairingCode>[
    if (matterCodes.isNotEmpty) matterCodes.first,
    if (hueSerialCodes.isNotEmpty && hueBridgeSerialSearchAvailable)
      hueSerialCodes.first,
    if (supportedLocalBleCodes.isNotEmpty) supportedLocalBleCodes.first,
  ];
  if (choices.length > 1) {
    return DevicePairingCodeDecision(
      code: choices.first,
      guidance: guidanceForDevicePairingCode(choices.first.kind),
      choices: choices,
    );
  }

  if (choices.length == 1) {
    final code = choices.single;
    return DevicePairingCodeDecision(
      code: code,
      guidance: guidanceForDevicePairingCode(
        code.kind,
        hueBridgeSerialSearchAvailable: hueBridgeSerialSearchAvailable,
        localBleProfileAvailable: code.localBleSetup != null,
      ),
    );
  }

  if (localBleCodes.isNotEmpty) {
    final code = localBleCodes.first;
    final profileId = code.localBleSetup?.profileId;
    final profileAvailable =
        profileId != null && supportedLocalBleProfileIds.contains(profileId);
    return DevicePairingCodeDecision(
      code: code,
      guidance: guidanceForDevicePairingCode(
        code.kind,
        localBleProfileAvailable: profileAvailable,
      ),
      continuationAllowed: profileAvailable,
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
      localBleProfileAvailable: code.localBleSetup != null &&
          supportedLocalBleProfileIds.contains(code.localBleSetup!.profileId),
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

  final localBleSetup = parseLocalBleSetupCode(normalized);
  if (localBleSetup != null) {
    return DevicePairingCode(
      kind: DevicePairingCodeKind.localBle,
      payload: localBleSetup.profileId,
      localBleSetup: localBleSetup,
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
  bool localBleProfileAvailable = false,
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
        title: 'Use Add Device',
        message: 'A six-character Hue serial can add a Zigbee bulb through a '
            'connected Hue Bridge, but that path is not available right now. '
            'To pair directly, open Add Device and keep the bulb powered on '
            'nearby. Rhythm will offer it when it appears.',
      ),
    DevicePairingCodeKind.localBle when localBleProfileAvailable =>
      const DevicePairingGuidance(
        title: 'Bluetooth device code',
        message: 'This device code is ready to pair with your Rhythm Box.',
      ),
    DevicePairingCodeKind.localBle => const DevicePairingGuidance(
        title: 'Rhythm Box update required',
        message: 'Update your Rhythm Box before adding this Bluetooth device.',
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
      (code.kind == DevicePairingCodeKind.localBle &&
          code.localBleSetup != null) ||
      (code.kind == DevicePairingCodeKind.hue &&
          normalizeHueBridgeSerial(code.payload) != null);
}

List<DevicePairingCode> _distinctActionableCodes(
  Iterable<DevicePairingCode> codes,
) {
  final seen = <String>{};
  return [
    for (final code in codes)
      if (seen.add(
        code.localBleSetup?.deduplicationKey ??
            '${code.kind.name}:${code.payload}',
      ))
        code,
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
