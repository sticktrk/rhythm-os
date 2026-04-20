//! Scenario: Brightness slider & step controls — "Room Card → Brightness Controls".
//!
//! Tests the brightness slider (absolute set) and step_up/step_down/dim_up/dim_down
//! controls which are the most-used room card interactions. Offset accumulation
//! and interaction between time and brightness offsets had zero test coverage.
//!
//! ## API journey
//!
//! 1. User taps room card → step up/down buttons on card
//! 2. Drags brightness slider → PUT /api/nodes/brightness
//! 3. Multiple taps accumulate offsets along the curve
//! 4. Reset returns to current adaptive position (zeroes all offsets)
//!
//! ## Curve context
//!
//! The test harness runs at 2PM on June 21 (summer solstice) — near the
//! peak of the bell curve. At this time:
//! - step_up (brighten) is at the boundary → returns 0 offset (no-op)
//! - step_down (dim) moves toward evening → produces a positive time offset
//!
//! Tests use step_down for offset accumulation since it always produces change.

mod harness;

use harness::{room, TestHarness};

// ============================================================================
// Scenario: step_down accumulates time offset
// ============================================================================

/// Multiple step_down actions should change time_offset cumulatively,
/// moving along the bell curve without affecting brightness_offset.
#[test]
fn step_down_accumulates_time_offset() {
    let harness = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    harness.action("kitchen", "on").unwrap();

    // -- Action: step_down twice --
    harness.action("kitchen", "step_down").unwrap();
    let t1 = harness.snapshot("kitchen").unwrap().time_offset_minutes;

    harness.action("kitchen", "step_down").unwrap();
    let t2 = harness.snapshot("kitchen").unwrap().time_offset_minutes;

    // -- Assert: cumulative offset (direction depends on curve position) --
    assert!(
        t1 != 0.0,
        "first step_down should create non-zero time offset, got {}",
        t1
    );
    assert!(
        t2.abs() > t1.abs(),
        "second step_down should increase offset magnitude: |{}| > |{}|",
        t2,
        t1
    );
    assert_eq!(
        harness.snapshot("kitchen").unwrap().brightness_offset,
        0.0,
        "step_down should not affect brightness_offset"
    );
}

// ============================================================================
// Scenario: step_up at peak is a safe boundary no-op
// ============================================================================

/// At 2PM on summer solstice, the curve is at peak brightness. step_up
/// (brighten) should be a safe no-op (boundary detection), returning 0 offset.
#[test]
fn step_up_at_peak_is_boundary_noop() {
    let harness = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    harness.action("kitchen", "on").unwrap();

    // -- Action: step_up at peak --
    harness.action("kitchen", "step_up").unwrap();

    // -- Assert: no time offset change (at boundary) --
    let snap = harness.snapshot("kitchen").unwrap();
    assert_eq!(
        snap.time_offset_minutes, 0.0,
        "step_up at peak should be no-op"
    );
    assert_eq!(
        snap.brightness_offset, 0.0,
        "brightness offset should be unchanged"
    );
}

// ============================================================================
// Scenario: Brightness set overrides brightness offset
// ============================================================================

/// Setting absolute brightness via the slider should recalculate
/// brightness_offset so effective brightness matches the target.
#[test]
fn brightness_set_overrides_offset() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    // Turn on and create a brightness offset via dim_up
    harness.action("kitchen", "on").unwrap();
    harness.action("kitchen", "dim_up").unwrap();
    let before = harness.snapshot("kitchen").unwrap();
    assert!(
        before.brightness_offset > 0.0,
        "dim_up should create positive offset"
    );

    // Also step_down to create a time offset
    harness.action("kitchen", "step_down").unwrap();
    let time_before = harness.snapshot("kitchen").unwrap().time_offset_minutes;
    assert!(time_before != 0.0, "step_down should create time offset");

    // -- Action: set absolute brightness --
    spy.reset();
    harness.set_brightness("kitchen", 50);

    // -- Assert: brightness matches target, time offset preserved --
    let on_calls = spy.turn_on_calls();
    assert!(
        !on_calls.is_empty(),
        "set_brightness should dispatch turn_on"
    );
    assert_eq!(
        on_calls[0].1.brightness, 50,
        "brightness should be exactly 50"
    );

    let after = harness.snapshot("kitchen").unwrap();
    assert_eq!(
        time_before, after.time_offset_minutes,
        "time offset should be preserved"
    );
}

// ============================================================================
// Scenario: Reset clears all offsets
// ============================================================================

/// The reset action should zero both time_offset and brightness_offset,
/// returning the room to its current adaptive curve position.
#[test]
fn reset_clears_all_offsets() {
    let harness = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    // Build up both offsets
    harness.action("kitchen", "on").unwrap();
    harness.action("kitchen", "step_down").unwrap();
    harness.action("kitchen", "dim_up").unwrap();

    let before = harness.snapshot("kitchen").unwrap();
    assert!(before.time_offset_minutes != 0.0, "should have time offset");
    assert!(
        before.brightness_offset > 0.0,
        "should have brightness offset"
    );

    // -- Action: reset --
    harness.action("kitchen", "reset").unwrap();

    // -- Assert: all offsets zeroed --
    let after = harness.snapshot("kitchen").unwrap();
    assert_eq!(
        after.time_offset_minutes, 0.0,
        "time offset should be 0 after reset"
    );
    assert_eq!(
        after.brightness_offset, 0.0,
        "brightness offset should be 0 after reset"
    );
    assert!(
        after.rhythm_enabled,
        "rhythm should still be enabled after reset"
    );
}

// ============================================================================
// Scenario: dim_up/dim_down accumulates brightness offset
// ============================================================================

/// dim_up/dim_down should change brightness_offset without affecting
/// time_offset. Multiple calls should accumulate.
#[test]
fn dim_up_down_accumulates() {
    let harness = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    harness.action("kitchen", "on").unwrap();

    // -- Action: dim_up twice --
    harness.action("kitchen", "dim_up").unwrap();
    let b1 = harness.snapshot("kitchen").unwrap().brightness_offset;

    harness.action("kitchen", "dim_up").unwrap();
    let b2 = harness.snapshot("kitchen").unwrap().brightness_offset;

    // -- Assert: cumulative --
    assert!(
        b1 > 0.0,
        "first dim_up should create positive brightness offset, got {}",
        b1
    );
    assert!(
        b2 > b1,
        "second dim_up should increase offset: {} > {}",
        b2,
        b1
    );
    assert_eq!(
        harness.snapshot("kitchen").unwrap().time_offset_minutes,
        0.0,
        "dim should not affect time_offset"
    );

    // -- Action: dim_down --
    harness.action("kitchen", "dim_down").unwrap();
    let b3 = harness.snapshot("kitchen").unwrap().brightness_offset;

    // -- Assert: decreased --
    assert!(b3 < b2, "dim_down should decrease offset: {} < {}", b3, b2);
}

// ============================================================================
// Scenario: step then brightness preserves time offset
// ============================================================================

/// Setting absolute brightness after step_down should recalculate
/// brightness_offset but preserve time_offset (they are independent axes).
#[test]
fn step_then_brightness_preserves_time() {
    let harness = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    harness.action("kitchen", "on").unwrap();

    // Create time offset via step_down
    harness.action("kitchen", "step_down").unwrap();
    let time_after_step = harness.snapshot("kitchen").unwrap().time_offset_minutes;
    assert!(
        time_after_step != 0.0,
        "step_down should create time offset"
    );

    // -- Action: set absolute brightness --
    harness.set_brightness("kitchen", 60);

    // -- Assert: time offset preserved --
    let after = harness.snapshot("kitchen").unwrap();
    assert_eq!(
        time_after_step, after.time_offset_minutes,
        "time offset should be preserved after brightness set"
    );
}

// ============================================================================
// Scenario: Offsets survive rhythm_off and rhythm_on
// ============================================================================

/// rhythm_off/rhythm_on should toggle the rhythm_enabled flag without
/// affecting accumulated offsets.
#[test]
fn offsets_survive_rhythm_off_on() {
    let harness = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    // Build up offsets
    harness.action("kitchen", "on").unwrap();
    harness.action("kitchen", "step_down").unwrap();
    harness.action("kitchen", "dim_up").unwrap();

    let original = harness.snapshot("kitchen").unwrap();
    assert!(
        original.time_offset_minutes != 0.0,
        "should have time offset"
    );
    assert!(
        original.brightness_offset > 0.0,
        "should have brightness offset"
    );

    // -- Action: rhythm_off --
    harness.action("kitchen", "rhythm_off").unwrap();
    let after_off = harness.snapshot("kitchen").unwrap();
    assert!(!after_off.rhythm_enabled, "rhythm should be disabled");
    assert_eq!(
        original.time_offset_minutes, after_off.time_offset_minutes,
        "time offset preserved through rhythm_off"
    );
    assert_eq!(
        original.brightness_offset, after_off.brightness_offset,
        "brightness offset preserved through rhythm_off"
    );

    // -- Action: rhythm_on --
    harness.action("kitchen", "rhythm_on").unwrap();
    let after_on = harness.snapshot("kitchen").unwrap();
    assert!(after_on.rhythm_enabled, "rhythm should be re-enabled");
    assert_eq!(
        original.time_offset_minutes, after_on.time_offset_minutes,
        "time offset preserved through rhythm_on"
    );
    assert_eq!(
        original.brightness_offset, after_on.brightness_offset,
        "brightness offset preserved through rhythm_on"
    );
}
