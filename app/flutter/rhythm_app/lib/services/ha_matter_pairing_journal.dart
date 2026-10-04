import '../data/local_data_source.dart';

/// Only an operation pointer is persisted; setup codes stay on the owner addon.
class HaMatterPairingJournal {
  HaMatterPairingJournal(this.scope, {LocalDataSource? storage})
      : _storage = storage ?? LocalDataSource();
  final String scope;
  final LocalDataSource _storage;
  String get _key => 'ha_matter_pairing_v1:$scope';

  String? read() {
    final value = _storage.getSettingsValue(_key);
    if (value == null) return null;
    if (value is! String ||
        !RegExp(r'^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$')
            .hasMatch(value)) {
      throw const FormatException('Invalid Matter pairing pointer');
    }
    return value;
  }

  Future<void> save(String sessionId) async {
    final existing = read();
    if (existing != null && existing != sessionId) {
      throw StateError('An earlier pairing still needs reconciliation');
    }
    await _storage.saveSettingsValue(_key, sessionId);
  }

  Future<void> clear(String sessionId) async {
    if (read() == sessionId) await _storage.deleteSettingsValue(_key);
  }
}
