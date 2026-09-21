#![cfg(unix)]

/// Exercise the exact native transaction policy in the portable Cargo suite,
/// without requiring a CHIP checkout or a physical device.
#[test]
fn native_wifi_transaction_policy() {
    use std::process::Command;
    let directory = std::env::temp_dir().join(format!(
        "rhythm-wifi-policy-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let executable = directory.join("wifi-change-test");
    let compile = Command::new("c++")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args([
            "-std=c++17",
            "native/tests/wifi_change_transaction_test.cc",
            "-o",
        ])
        .arg(&executable)
        .output()
        .expect("host C++ compiler required for the native policy test");
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let result = Command::new(&executable).output().unwrap();
    std::fs::remove_dir_all(&directory).unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
