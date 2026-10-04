import 'package:dio/dio.dart';

/// HA owns the Matter fabric; this client only addresses the authenticated addon.
enum HaMatterCodeSource { originalLabel, sharing }

extension HaMatterCodeSourceWire on HaMatterCodeSource {
  String get wire =>
      this == HaMatterCodeSource.originalLabel ? 'original_label' : 'sharing';
}

class HaMatterDevice {
  const HaMatterDevice(
      {required this.deviceId,
      required this.identity,
      required this.name,
      required this.nodeId,
      this.isBridge = false});
  final String deviceId;
  final String identity;
  final String name;
  final String nodeId;
  final bool isBridge;

  factory HaMatterDevice.fromJson(Map<String, dynamic> json) {
    final id = json['device_id'];
    final identity = json['identity'];
    if (id is! String ||
        id.isEmpty ||
        identity is! String ||
        identity.isEmpty) {
      throw const FormatException('Invalid Home Assistant device identity');
    }
    return HaMatterDevice(
        deviceId: id,
        identity: identity,
        name: json['name'] is String ? json['name'] as String : 'Matter device',
        nodeId: json['node_id']?.toString() ?? '',
        isBridge: json['is_bridge'] == true);
  }
}

class HaMatterCatalog {
  HaMatterCatalog.fromJson(Map<String, dynamic> json)
      : available = json['schema_version'] == 1 && json['available'] == true,
        devices = (json['devices'] as List? ?? const [])
            .whereType<Map>()
            .map((v) => HaMatterDevice.fromJson(Map<String, dynamic>.from(v)))
            .toList(growable: false),
        _capabilities = json['capabilities'] is Map
            ? Map<String, dynamic>.from(json['capabilities'] as Map)
            : const {};
  final bool available;
  final List<HaMatterDevice> devices;
  final Map<String, dynamic> _capabilities;
  bool get pairOnNetwork =>
      available && _capabilities['pair_on_network'] == true;
  bool get phoneCommissioning =>
      available && _capabilities['phone_commissioning'] == true;
  bool get share => available && _capabilities['share'] == true;
  bool get remove => available && _capabilities['remove'] == true;
  bool get originalSetupCode =>
      available && _capabilities['original_setup_code'] == true;
}

enum HaMatterPairingState { pending, completed, failed, unknown }

class HaMatterPairingReceipt {
  HaMatterPairingReceipt.fromJson(Map<String, dynamic> json)
      : sessionId = json['session_id'] as String? ?? '',
        state = switch (json['status']) {
          'pending' => HaMatterPairingState.pending,
          'completed' => HaMatterPairingState.completed,
          'failed' => HaMatterPairingState.failed,
          _ => HaMatterPairingState.unknown,
        },
        needsDeviceConfirmation = json['needs_device_confirmation'] == true,
        canCloseAfterReview =
            json['status'] == 'unknown' && json['can_close_attempt'] == true,
        originalCodeSaved = json['original_code_saved'] == true;
  final String sessionId;
  final HaMatterPairingState state;
  final bool needsDeviceConfirmation;
  final bool canCloseAfterReview;
  final bool originalCodeSaved;
  bool get unresolved =>
      state == HaMatterPairingState.pending ||
      state == HaMatterPairingState.unknown;
}

/// A bounded error never contains request bodies, codes, tokens or server text.
class HaMatterRequestException implements Exception {
  const HaMatterRequestException(this.statusCode);
  final int? statusCode;
  @override
  String toString() => 'Home Assistant Matter request failed';
}

class RhythmHaMatterApi {
  RhythmHaMatterApi(Dio dio) : _dio = dio;
  final Dio _dio;
  static const _root = 'api/addon/matter';
  String get baseUrl => _dio.options.baseUrl.replaceFirst(RegExp(r'/$'), '');

  /// Used only by the native commissioning handoff to this same addon.
  String? get handoffAuthToken {
    final header = _dio.options.headers['Authorization'] ??
        _dio.options.headers['authorization'];
    return header is String && header.startsWith('Bearer ')
        ? header.substring(7)
        : null;
  }

  Future<Map<String, dynamic>> _request(String path, String method,
      {Map<String, dynamic>? data, Map<String, dynamic>? query}) async {
    try {
      final response = await _dio.request<Object?>(path,
          data: data,
          queryParameters: query,
          options: Options(
              method: method,
              receiveTimeout: const Duration(seconds: 250),
              validateStatus: (_) => true,
              headers: {'Cache-Control': 'no-store'}));
      if ((response.statusCode ?? 500) >= 300 || response.data is! Map) {
        throw HaMatterRequestException(response.statusCode);
      }
      return Map<String, dynamic>.from(response.data as Map);
    } on DioException catch (e) {
      throw HaMatterRequestException(e.response?.statusCode);
    }
  }

  Future<HaMatterCatalog> getCatalog() async =>
      HaMatterCatalog.fromJson(await _request(_root, 'GET'));
  Future<HaMatterPairingReceipt> pair(
          {required String sessionId,
          required String setupCode,
          required HaMatterCodeSource codeSource}) async =>
      _receipt(
          await _request('$_root/pair', 'POST', data: {
            'session_id': sessionId,
            'setup_code': setupCode,
            'code_source': codeSource.wire,
            'rendezvous': 'on_network',
          }),
          sessionId);
  Future<HaMatterPairingReceipt> getPairing(String sessionId) async => _receipt(
      await _request('$_root/pairing/${Uri.encodeComponent(sessionId)}', 'GET'),
      sessionId);
  Future<HaMatterPairingReceipt> confirmDevice(
          String sessionId, HaMatterDevice device) async =>
      _receipt(
          await _request(
              '$_root/pairing/${Uri.encodeComponent(sessionId)}/device', 'POST',
              data: {
                'device_id': device.deviceId,
                'identity': device.identity,
              }),
          sessionId);
  HaMatterPairingReceipt _receipt(Map<String, dynamic> json, String expected) {
    final receipt = HaMatterPairingReceipt.fromJson(json);
    if (receipt.sessionId != expected)
      throw const FormatException('Mismatched pairing receipt');
    return receipt;
  }

  Future<String?> getOriginalCode(HaMatterDevice device) async {
    final data = await _request(
        '$_root/setup-code/${Uri.encodeComponent(device.deviceId)}', 'GET',
        query: {'identity': device.identity});
    if (data['device_id'] != device.deviceId ||
        data['identity'] != device.identity ||
        data['code_source'] != 'original_label') {
      throw const FormatException('Mismatched setup code identity');
    }
    final code = data['setup_code'];
    return data['available'] == true && code is String && code.isNotEmpty
        ? code
        : null;
  }

  Future<void> saveOriginalCode(HaMatterDevice device, String code) async {
    await _request(
        '$_root/setup-code/${Uri.encodeComponent(device.deviceId)}', 'PUT',
        data: {
          'identity': device.identity,
          'setup_code': code,
          'code_source': 'original_label',
        });
  }

  Future<({String code, int expiresIn})> shareDevice(
      HaMatterDevice device) async {
    final data = await _request(
        '$_root/share/${Uri.encodeComponent(device.deviceId)}', 'POST',
        data: {'identity': device.identity});
    final code = data['manual_code'] ?? data['setup_code'];
    if (code is! String || code.isEmpty)
      throw const FormatException('Missing sharing code');
    return (
      code: code,
      expiresIn: (data['expires_in'] as num?)?.toInt() ?? 300
    );
  }

  Future<void> removeDevice(HaMatterDevice device) async {
    await _request(
        '$_root/devices/${Uri.encodeComponent(device.deviceId)}', 'DELETE',
        data: {'identity': device.identity});
  }
}
