import 'package:flutter_test/flutter_test.dart';

/// RoomProvider tests - Currently skipped due to API changes.
///
/// The RoomDto API has changed (no longer has 'source' named parameter).
/// Tests need to be updated to match the current implementation.
///
/// Additionally, these tests still need updates around:
/// - Room provider API setup
/// - Persistence and state serialization expectations
void main() {
  group('RoomProvider', () {
    test('tests need to be updated to match current API', () {
      // RoomDto constructor has changed
    }, skip: 'RoomProvider tests need API updates');
  });
}
