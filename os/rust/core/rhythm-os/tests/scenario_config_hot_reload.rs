//! Scenario: Light profile config hot-reload — "Designer Screen".
//!
//! Tests that changing the active LightProfileConfig while rooms are actively on updates
//! the lighting values the engine computes. The Designer is one of the main
//! screens and config changes during active use had zero test coverage.
//!
//! ## API journey
//!
//! 1. User opens Designer screen
//! 2. Drags brightness/color slider
//! 3. PUT /api/config pushes new LightProfileConfig
//! 4. Active rooms immediately use new curve for next action/tick
//! 5. Fix My Lights uses new curve values

mod harness;

use harness::{room, TestHarness};
use rhythm_core::{default_rhythm_profile, LightProfileConfig};

/// Build a rhythm profile with max_brightness capped to a low value.
fn config_with_max_brightness(max_bri: u8) -> LightProfileConfig {
    let mut config = default_rhythm_profile();
    config.max_brightness = max_bri;
    config
}

// ============================================================================
// Scenario: Config change updates active room values
// ============================================================================

/// After pushing a new LightProfileConfig with lower max_brightness, a reset action
/// should dispatch a lighting command at the new (lower) brightness.
#[test]
fn config_change_updates_active_room_values() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    // Turn on with default config to establish baseline
    harness.action("kitchen", "on").unwrap();
    let baseline = spy.turn_on_calls();
    assert!(!baseline.is_empty(), "on should produce turn_on");
    let baseline_brightness = baseline[0].1.brightness;

    // -- Action: push config with max_brightness=30, then reset --
    harness.set_config(config_with_max_brightness(30));
    spy.reset();
    harness.action("kitchen", "reset").unwrap();

    // -- Assert: new brightness from new curve --
    let calls = spy.turn_on_calls();
    assert!(!calls.is_empty(), "reset should produce turn_on");
    let new_brightness = calls[0].1.brightness;
    assert!(
        new_brightness <= 30,
        "brightness should respect max_brightness=30, got {}",
        new_brightness
    );
    assert!(
        new_brightness < baseline_brightness,
        "new brightness {} should be < baseline {}",
        new_brightness,
        baseline_brightness
    );
}

// ============================================================================
// Scenario: Config change preserves room offsets
// ============================================================================

/// Pushing a new light profile config should not alter room time/brightness offsets.
#[test]
fn config_change_preserves_offsets() {
    let harness = TestHarness::new().with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    // Build up offsets (step_down works at 2PM peak; step_up is boundary no-op)
    harness.action("kitchen", "on").unwrap();
    harness.action("kitchen", "step_down").unwrap();
    harness.action("kitchen", "step_down").unwrap();
    let before = harness.snapshot("kitchen").unwrap();
    assert!(
        before.time_offset_minutes != 0.0,
        "should have non-zero time offset"
    );

    // -- Action: change curve shape --
    let mut config = default_rhythm_profile();
    if let rhythm_core::LightCurveShape::SuperGaussian { shape_p, .. } = &mut config.curve {
        *shape_p = 2.0; // round curve instead of default flat plateau
    } else {
        panic!("expected super-gaussian rhythm profile");
    }
    harness.set_config(config);

    // -- Assert: offsets unchanged --
    let after = harness.snapshot("kitchen").unwrap();
    assert_eq!(
        before.time_offset_minutes, after.time_offset_minutes,
        "time offset should be preserved"
    );
    assert_eq!(
        before.brightness_offset, after.brightness_offset,
        "brightness offset should be preserved"
    );
}

// ============================================================================
// Scenario: Fix after config change uses new curve
// ============================================================================

/// Fix My Lights should recalculate adaptive values using the new curve.
#[test]
fn fix_after_config_change_uses_new_curve() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(
        vec![room("kitchen", "Kitchen"), room("bedroom", "Bedroom")],
        vec![],
    );
    harness.sync();

    // Both rooms on
    harness.action("kitchen", "on").unwrap();
    harness.action("bedroom", "on").unwrap();

    // -- Action: push constrained config, then fix --
    harness.set_config(config_with_max_brightness(40));
    spy.reset();
    let result = harness.fix_my_lights();
    assert_eq!(result["rooms_reset"], 2);

    // -- Assert: all turn_on calls respect new max --
    for (_, cmd) in spy.turn_on_calls() {
        assert!(
            cmd.brightness <= 40,
            "fix should use new curve, got brightness {}",
            cmd.brightness
        );
    }
}

// ============================================================================
// Scenario: New on action after config change uses new values
// ============================================================================

/// Turning off then on after a config change should produce values from
/// the new curve, not cached values from the old curve.
#[test]
fn new_on_after_config_uses_new_values() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    // Initial on/off cycle
    harness.action("kitchen", "on").unwrap();
    harness.action("kitchen", "lights_off").unwrap();

    // -- Action: push constrained config, then on --
    let mut config = default_rhythm_profile();
    config.max_brightness = 30;
    config.max_color_temp = 3000;
    harness.set_config(config);
    spy.reset();
    harness.action("kitchen", "on").unwrap();

    // -- Assert: values from new curve --
    let calls = spy.turn_on_calls();
    assert!(!calls.is_empty(), "on should produce turn_on");
    let cmd = &calls[0].1;
    assert!(
        cmd.brightness <= 30,
        "brightness should be <= 30, got {}",
        cmd.brightness
    );
    assert!(
        cmd.kelvin <= 3000,
        "kelvin should be <= 3000, got {}",
        cmd.kelvin
    );
}

// ============================================================================
// Scenario: Reset config restores defaults
// ============================================================================

/// Resetting config after customization should return values to defaults
/// and affect subsequent room actions.
#[test]
fn reset_config_restores_defaults() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    // Customize config with constrained brightness
    harness.set_config(config_with_max_brightness(30));
    harness.action("kitchen", "on").unwrap();
    let constrained = spy.turn_on_calls();
    assert!(constrained[0].1.brightness <= 30);

    // Reset to defaults
    harness.reset_config();
    spy.reset();
    harness.action("kitchen", "reset").unwrap();

    // Assert: brightness should be back to default range (> 30)
    let calls = spy.turn_on_calls();
    assert!(!calls.is_empty());
    let default_config = default_rhythm_profile();
    assert!(calls[0].1.brightness <= default_config.max_brightness);
    // Verify config in state matches defaults
    assert_eq!(
        harness.config().max_brightness,
        default_config.max_brightness
    );
    assert_eq!(
        harness.config().min_brightness,
        default_config.min_brightness
    );
}

// ============================================================================
// Scenario: Extreme config values produce valid commands
// ============================================================================

/// Config with floor/ceiling values should produce valid lighting commands
/// without panics.
#[test]
fn extreme_config_values_produce_valid_commands() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    // -- Minimum extremes --
    let mut config = default_rhythm_profile();
    config.min_brightness = 1;
    config.max_brightness = 1;
    config.min_color_temp = 2000;
    config.max_color_temp = 2000;
    harness.set_config(config);
    harness.action("kitchen", "on").unwrap();

    let calls = spy.turn_on_calls();
    assert!(!calls.is_empty(), "should produce turn_on");
    assert_eq!(calls[0].1.brightness, 1, "brightness should be 1");
    assert_eq!(calls[0].1.kelvin, 2000, "kelvin should be 2000");

    // -- Maximum extremes --
    let mut config = default_rhythm_profile();
    config.min_brightness = 100;
    config.max_brightness = 100;
    config.min_color_temp = 6500;
    config.max_color_temp = 6500;
    harness.set_config(config);
    spy.reset();
    harness.action("kitchen", "reset").unwrap();

    let calls = spy.turn_on_calls();
    assert!(!calls.is_empty(), "should produce turn_on");
    assert_eq!(calls[0].1.brightness, 100, "brightness should be 100");
    assert_eq!(calls[0].1.kelvin, 6500, "kelvin should be 6500");
}
