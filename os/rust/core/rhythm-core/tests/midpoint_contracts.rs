use rhythm_core::{MidpointValue, SunTimes};

#[test]
fn midpoint_dynamic_and_fixed_contracts_cover_user_config_forms() {
    let sun = SunTimes {
        sunrise: 6.25,
        sunset: 19.75,
        day_length: 13.5,
    };

    let fixed = MidpointValue::fixed(7.125);
    assert_eq!(fixed.resolve(&sun), 7.125);
    assert_eq!(fixed.resolve_or(None, 5.0), 7.125);
    assert!(fixed.is_fixed());
    assert!(!fixed.is_dynamic());
    assert_eq!(format!("{fixed:.2}"), "7.12");

    assert_eq!(MidpointValue::Sunrise.resolve(&sun), 6.25);
    assert_eq!(MidpointValue::Sunset.resolve(&sun), 7.75);
    assert_eq!(MidpointValue::Sunrise.resolve_or(None, 6.0), 6.0);
    assert_eq!(MidpointValue::Sunset.to_string(), "sunset");

    let from_integer: MidpointValue = serde_json::from_str("8").unwrap();
    let from_string: MidpointValue = serde_json::from_str("\"SunRise\"").unwrap();
    assert_eq!(from_integer, MidpointValue::Fixed(8.0));
    assert_eq!(from_string, MidpointValue::Sunrise);
}
