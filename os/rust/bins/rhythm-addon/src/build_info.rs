//! Public, immutable image provenance. Unpackaged builds report unknown image
//! fields rather than mistaking the product version for an HA catalog version.

use serde_json::{json, Value};

pub fn current() -> Value {
    from_values(
        option_env!("RHYTHM_IMAGE_VERSION"),
        option_env!("RHYTHM_PRODUCT_REVISION"),
        option_env!("RHYTHM_PACKAGING_REVISION"),
        option_env!("RHYTHM_BUILD_INPUTS_SHA256"),
    )
}

fn from_values(
    image: Option<&str>,
    product: Option<&str>,
    packaging: Option<&str>,
    inputs: Option<&str>,
) -> Value {
    json!({
        "schema_version": 1,
        "product_version": env!("CARGO_PKG_VERSION"),
        "image_version": image.filter(|value| !value.is_empty() && value.len() <= 64
            && value.bytes().all(|c| c.is_ascii_alphanumeric() || b".-+".contains(&c))),
        "product_revision": hex_value(product, 40),
        "packaging_revision": hex_value(packaging, 40),
        "build_inputs_sha256": hex_value(inputs, 64),
    })
}

fn hex_value(value: Option<&str>, len: usize) -> Option<&str> {
    value.filter(|value| {
        value.len() == len
            && value
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_image_identity_is_distinct_from_the_product_version() {
        let build = from_values(
            Some("1.2.0-dev.4"),
            Some(&"a".repeat(40)),
            Some(&"b".repeat(40)),
            Some(&"c".repeat(64)),
        );
        assert_eq!(build["image_version"], "1.2.0-dev.4");
        assert_eq!(build["product_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(build["product_revision"], "a".repeat(40));
        assert_eq!(build["packaging_revision"], "b".repeat(40));
        assert_eq!(build["build_inputs_sha256"], "c".repeat(64));
    }

    #[test]
    fn absent_or_invalid_metadata_does_not_claim_an_image_identity() {
        for build in [
            from_values(None, None, None, None),
            from_values(
                Some(""),
                Some("unknown"),
                Some("../private"),
                Some("not-a-digest"),
            ),
        ] {
            for field in [
                "image_version",
                "product_revision",
                "packaging_revision",
                "build_inputs_sha256",
            ] {
                assert!(build[field].is_null(), "{field}");
            }
            assert_eq!(build["product_version"], env!("CARGO_PKG_VERSION"));
        }
    }
}
