//! Scenario: Absorb time offset — "Light Tuning Screen".
//!
//! Tests that absorbing a time offset into the curve config adjusts the
//! width parameters and resets all room offsets to zero.
//!
//! ## API journey
//!
//! 1. User opens Light Tuning screen
//! 2. Drags gradient slider to preview lighting at a different time
//! 3. Taps "Absorb" button
//! 4. POST /api/config/absorb-offset adjusts curve widths
//! 5. All room time offsets reset to 0
//! 6. Lighting stays the same at the current time

mod harness;

use harness::{room, TestHarness};

// ============================================================================
// Scenario: Absorb resets room offsets to zero
// ============================================================================

/// After absorb, every room's time_offset_minutes should be 0.
#[test]
fn absorb_resets_all_room_offsets() {
    let harness = TestHarness::new().with_discovery(
        vec![room("kitchen", "Kitchen"), room("bedroom", "Bedroom")],
        vec![],
    );
    harness.sync();

    // Turn rooms on and set offsets via the slider path
    harness.action("kitchen", "on").unwrap();
    harness.action("bedroom", "on").unwrap();
    harness.set_room_offset("kitchen", 30.0);
    harness.set_room_offset("bedroom", 30.0);

    // Verify offsets are set
    let k_before = harness.snapshot("kitchen").unwrap();
    let b_before = harness.snapshot("bedroom").unwrap();
    assert!((k_before.time_offset_minutes - 30.0).abs() < 0.1);
    assert!((b_before.time_offset_minutes - 30.0).abs() < 0.1);

    // -- Action: absorb the offset --
    harness.absorb_offset(30.0);

    // -- Assert: all room offsets are now 0 --
    let k_after = harness.snapshot("kitchen").unwrap();
    let b_after = harness.snapshot("bedroom").unwrap();
    assert!(
        k_after.time_offset_minutes.abs() < 0.1,
        "kitchen offset should be 0 after absorb, got {}",
        k_after.time_offset_minutes
    );
    assert!(
        b_after.time_offset_minutes.abs() < 0.1,
        "bedroom offset should be 0 after absorb, got {}",
        b_after.time_offset_minutes
    );
}

// ============================================================================
// Scenario: Absorb modifies curve width parameters
// ============================================================================

/// Absorbing a positive offset on the morning side should decrease width_left
/// (faster ramp = brighter earlier). The harness runs at 14:00 (2 PM) which
/// is past solar noon, so with default sunrise=6/sunset=20 we're on the
/// evening side. A positive offset moves further from noon → higher width_right.
#[test]
fn absorb_adjusts_width_for_current_side() {
    let harness = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();
    harness.action("kitchen", "on").unwrap();

    let before = harness.config();

    // Harness runs at 14:00 (evening side, mu ≈ 13.0 for sunrise=6/sunset=20).
    // +60min offset moves to 15:00, further from mu → width_right increases.
    harness.absorb_offset(60.0);

    let after = harness.config();

    // Evening side should change (the harness time is 14:00, which is past mu)
    // Morning side should be unchanged
    assert_eq!(
        after.width_left_bri, before.width_left_bri,
        "morning width should be unchanged"
    );
    assert_eq!(
        after.width_left_cct, before.width_left_cct,
        "morning cct width should be unchanged"
    );

    // Evening width should increase (further from noon = steeper ramp)
    assert!(
        after.width_right_bri > before.width_right_bri,
        "evening width_right_bri should increase: {} -> {}",
        before.width_right_bri,
        after.width_right_bri
    );
    assert!(
        after.width_right_cct > before.width_right_cct,
        "evening width_right_cct should increase: {} -> {}",
        before.width_right_cct,
        after.width_right_cct
    );
}

// ============================================================================
// Scenario: Absorb preserves other config fields
// ============================================================================

/// Absorbing should only change width params for the current side;
/// min/max brightness, min/max color temp, and shape_p should be unchanged.
#[test]
fn absorb_preserves_other_config_fields() {
    let harness = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();
    harness.action("kitchen", "on").unwrap();

    let before = harness.config();
    harness.absorb_offset(60.0);
    let after = harness.config();

    assert_eq!(after.min_brightness, before.min_brightness);
    assert_eq!(after.max_brightness, before.max_brightness);
    assert_eq!(after.min_color_temp, before.min_color_temp);
    assert_eq!(after.max_color_temp, before.max_color_temp);
    assert_eq!(after.shape_p, before.shape_p);
    assert_eq!(after.max_dim_steps, before.max_dim_steps);
}

// ============================================================================
// Scenario: Absorb with zero offset is a no-op on config
// ============================================================================

/// Zero offset → factor = 1.0, no width change; rooms still reset.
#[test]
fn absorb_zero_offset_no_config_change() {
    let harness = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();
    harness.action("kitchen", "on").unwrap();
    harness.set_room_offset("kitchen", 15.0);

    let before = harness.config();
    harness.absorb_offset(0.0);
    let after = harness.config();

    // Config should be identical (factor = 1.0)
    assert_eq!(after.width_left_bri, before.width_left_bri);
    assert_eq!(after.width_right_bri, before.width_right_bri);
    assert_eq!(after.width_left_cct, before.width_left_cct);
    assert_eq!(after.width_right_cct, before.width_right_cct);

    // But room offset should still be reset
    let snap = harness.snapshot("kitchen").unwrap();
    assert!(
        snap.time_offset_minutes.abs() < 0.1,
        "offset should reset even with zero absorb, got {}",
        snap.time_offset_minutes
    );
}

// ============================================================================
// Scenario: Successive absorbs compound
// ============================================================================

/// Two absorbs of +30min should produce a larger width change than one +30min.
#[test]
fn successive_absorbs_compound() {
    let harness = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();
    harness.action("kitchen", "on").unwrap();

    let original = harness.config();

    // First absorb
    harness.absorb_offset(30.0);
    let after_one = harness.config();

    // Second absorb
    harness.absorb_offset(30.0);
    let after_two = harness.config();

    // Both should change the same side (evening at 14:00)
    // After two absorbs, width_right should be further from original
    let delta_one = (after_one.width_right_bri - original.width_right_bri).abs();
    let delta_two = (after_two.width_right_bri - original.width_right_bri).abs();
    assert!(
        delta_two > delta_one,
        "two absorbs should compound: delta_one={}, delta_two={}",
        delta_one,
        delta_two
    );
}
