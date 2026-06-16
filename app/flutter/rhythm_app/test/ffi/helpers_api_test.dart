/// Tests for the Rust FFI helper functions.
///
/// Helper functions provide utilities for device address normalization,
/// Hue device detection, and other common operations.
///
/// Note: These tests require FFI initialization which may not be
/// available in all test environments.
@TestOn('!js')
library;

import 'package:flutter_test/flutter_test.dart';

void main() {
  // All helper API tests require FFI initialization
  // which is not available in standard unit test environments.

  group('IEEE Address Helpers (requires FFI)', () {
    test('normalizeIeee formats addresses consistently', () {
      // normalizeIeee() requires FFI
    }, skip: 'Requires FFI initialization');

    test('normalizeIeee handles lowercase and uppercase', () {
      // Requires FFI
    }, skip: 'Requires FFI initialization');

    test('isHueIeee detects Hue OUI prefix', () {
      // isHueIeee() requires FFI
    }, skip: 'Requires FFI initialization');

    test('areaIdsMatch compares normalized addresses', () {
      // areaIdsMatch() requires FFI
    }, skip: 'Requires FFI initialization');
  });

  group('Group Naming Helpers (requires FFI)', () {
    test('groupNameForArea generates correct names', () {
      // groupNameForArea() requires FFI
    }, skip: 'Requires FFI initialization');

    test('endpointForManufacturer returns correct endpoints', () {
      // endpointForManufacturer() requires FFI
    }, skip: 'Requires FFI initialization');
  });
}
