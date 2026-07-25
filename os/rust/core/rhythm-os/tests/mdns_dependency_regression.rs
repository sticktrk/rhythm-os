const WORKSPACE_LOCK: &str = include_str!("../../../../../Cargo.lock");
const FIRST_NSEC_BOUNDS_FIXED_VERSION: (u64, u64, u64) = (0, 13, 2);

fn locked_package_version(lock: &str, package: &str) -> Option<(u64, u64, u64)> {
    lock.split("[[package]]").skip(1).find_map(|block| {
        let expected_name = format!("name = \"{package}\"");
        if !block.lines().any(|line| line.trim() == expected_name) {
            return None;
        }

        let version = block
            .lines()
            .find_map(|line| line.trim().strip_prefix("version = \""))?
            .strip_suffix('"')?;
        let mut parts = version.split('.');
        Some((
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        ))
    })
}

#[test]
fn mdns_dependency_rejects_truncated_nsec_bitmaps_instead_of_panicking() {
    let locked_version =
        locked_package_version(WORKSPACE_LOCK, "mdns-sd").expect("mdns-sd must be locked");

    assert!(
        locked_version >= FIRST_NSEC_BOUNDS_FIXED_VERSION,
        "mdns-sd {locked_version:?} predates the NSEC bitmap bounds check in 0.13.2"
    );
}
