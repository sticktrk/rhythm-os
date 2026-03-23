import 'package:flutter_test/flutter_test.dart';

/// ConfigModel tests - These tests require FFI initialization.
///
/// ConfigModel's constructor calls RawConfig.defaults() which internally
/// uses FFI to get default values from the Rust library.
///
/// To run these tests, FFI must be initialized first by calling
/// RustLib.init() before creating a ConfigModel instance.
///
/// For now, these tests are skipped and will be enabled when
/// FFI integration tests are run.
void main() {
  group('ConfigModel (requires FFI)', () {
    test('all tests require FFI initialization', () {
      // This test group requires FFI to be initialized before
      // ConfigModel can be constructed. ConfigModel() calls
      // RawConfig.defaults() which uses FFI internally.
    }, skip: 'ConfigModel requires FFI initialization');

    // Original tests moved here for documentation:
    //
    // group('initial state', () {
    //   test('has correct default values')
    //   test('selectedHour is around current time')
    //   test('activeHalf defaults to morning')
    //   test('calloutAutoFollow defaults to true')
    //   test('stepCount has positive default')
    //   test('showSteps defaults to true')
    //   test('showSolarContext defaults to true')
    //   test('use12Hour defaults to true')
    //   test('month defaults to current month')
    // });
    //
    // group('setSelectedHour', () {
    //   test('updates selectedHour')
    //   test('clamps to 0-24 range')
    //   test('disables calloutAutoFollow')
    //   test('updates activeHalf based on solarNoon')
    //   test('activeHalf unchanged when solarNoon not provided')
    //   test('notifies listeners')
    // });
    //
    // group('setActiveHalf', () {
    //   test('updates activeHalf')
    //   test('notifies listeners')
    // });
    //
    // etc... See ffi/ tests for full coverage
  });
}
