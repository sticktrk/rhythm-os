#![cfg(unix)]

/// Exercise the exact native commissioning admission scope in the portable Cargo suite,
/// without requiring a CHIP checkout or a physical device.
#[test]
fn native_bluez_commissioning_scope() {
    use std::process::Command;
    let directory = std::env::temp_dir().join(format!(
        "rhythm-bluez-scope-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let executable = directory.join("bluez-scope-test");
    let compile = Command::new("c++")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args([
            "-std=c++17",
            "-pthread",
            "native/tests/bluez_commissioning_scope_test.cc",
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
