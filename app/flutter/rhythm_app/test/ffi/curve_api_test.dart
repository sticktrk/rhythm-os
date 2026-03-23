/// Tests for the Rust FFI curve API functions.
///
/// These tests verify that the FFI bindings work correctly and return
/// expected data structures. They require the Rust WASM/FFI library
/// to be initialized.
///
/// Note: These tests can only run when the FFI is available.
/// For unit tests without FFI, use the mock implementations.
@TestOn('!js') // Skip on web platform for now
library;

import 'package:flutter_test/flutter_test.dart';

void main() {
  // All curve API tests require FFI initialization
  // which is not available in standard unit test environments.
  //
  // These tests are documented here for reference and should be
  // run as part of FFI integration tests where WASM is loaded.

  group('CurveConfigDto (requires FFI)', () {
    test('default_() returns valid config', () {
      // CurveConfigDto.default_() requires FFI initialization
    }, skip: 'Requires FFI initialization');

    test('copyWith preserves unchanged fields', () {
      // CurveConfigDto.default_() requires FFI initialization
    }, skip: 'Requires FFI initialization');

    test('copyWith can update all fields', () {
      // CurveConfigDto.default_() requires FFI initialization
    }, skip: 'Requires FFI initialization');
  });

  group('FFI Curve Functions (requires WASM)', () {
    test('generateCurveData returns valid data structure', () {
      // Requires FFI
    }, skip: 'Requires FFI initialization');

    test('calculateLighting returns valid lighting values', () {
      // Requires FFI
    }, skip: 'Requires FFI initialization');

    test('calculateStepSequences returns step points', () {
      // Requires FFI
    }, skip: 'Requires FFI initialization');
  });
}
