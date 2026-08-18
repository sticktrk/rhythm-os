use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use rhythm_server::bootstate;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

fn bootstate_script() -> PathBuf {
    repo_root()
        .join("install/rpiz/buildroot/board/rhythm/rpiz/rootfs-overlay/etc/init.d/S41bootstate")
}

fn launch_script() -> PathBuf {
    repo_root()
        .join("install/rpiz/buildroot/board/rhythm/rpiz/rootfs-overlay/usr/bin/rhythm-launch")
}

fn init_script_dir() -> PathBuf {
    repo_root().join("install/rpiz/buildroot/board/rhythm/rpiz/rootfs-overlay/etc/init.d")
}

fn cloudflared_script() -> PathBuf {
    init_script_dir().join("rhythm-cloudflared")
}

fn unique_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("rhythm-appliance-script-{name}-{nanos}"));
    fs::create_dir_all(&dir).unwrap();
    fs::create_dir_all(dir.join("boot")).unwrap();
    fs::create_dir_all(dir.join("data")).unwrap();
    dir
}

fn write_executable(path: &Path, body: &str) {
    let mut file = File::create(path).unwrap();
    writeln!(file, "{body}").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn write_fake_server(root: &Path, version: &str) -> PathBuf {
    let server = root.join("rhythm-server");
    write_executable(
        &server,
        &format!(
            r#"#!/bin/sh
if [ "${{1:-}}" = "--version" ]; then
    echo "rhythm-server {version}"
    exit 0
fi
exit 0
"#
        ),
    );
    server
}

fn bootstate_paths(root: &Path) -> (PathBuf, PathBuf) {
    let primary = root.join("boot/rhythm-bootstate.env");
    let backup = root.join("boot/rhythm-bootstate.env.bak");
    (primary, backup)
}

fn rollback_required_latch(root: &Path) -> PathBuf {
    root.join("run/rhythm-rollback-required")
}

fn write_bootstate(root: &Path, body: &str) {
    let (primary, backup) = bootstate_paths(root);
    bootstate::write_with_backup(&primary, &backup, body).unwrap();
}

fn read_bootstate(root: &Path) -> String {
    let (primary, backup) = bootstate_paths(root);
    bootstate::read_with_backup(&primary, &backup).unwrap()
}

fn bootstate_value<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    body.lines()
        .filter_map(|line| line.split_once('='))
        .find_map(|(k, v)| (k == key).then_some(v))
}

fn assert_pending_rollback_required(root: &Path) -> String {
    let cmdline = fs::read_to_string(root.join("boot/cmdline.txt")).unwrap();
    assert!(cmdline.contains("root=/dev/mmcblk0p3"));
    assert!(!cmdline.contains("root=/dev/mmcblk0p2"));
    let body = read_bootstate(root);
    assert_eq!(bootstate_value(&body, "RHYTHM_PENDING_SLOT"), Some("b"));
    assert_eq!(
        bootstate_value(&body, "RHYTHM_PENDING_VERSION"),
        Some("0.4.2")
    );
    assert_eq!(
        bootstate_value(&body, "RHYTHM_BOOT_STATUS"),
        Some("rollback_required")
    );
    assert!(rollback_required_latch(root).exists());
    body
}

fn pending_bootstate_body(status: &str) -> String {
    format!(
        "RHYTHM_ACTIVE_SLOT=a\nRHYTHM_LAST_GOOD_SLOT=a\nRHYTHM_PENDING_SLOT=b\nRHYTHM_PENDING_VERSION=0.4.2\nRHYTHM_ACTIVE_VERSION=0.4.1\nRHYTHM_BOOT_STATUS={status}\nRHYTHM_LAST_UPDATE_EPOCH_MS=1000\nRHYTHM_LAST_ROLLBACK_SLOT=\nRHYTHM_LAST_ROLLBACK_VERSION=\nRHYTHM_LAST_ROLLBACK_EPOCH_MS=\n"
    )
}

fn run_bootstate(root: &Path, action: &str) -> Output {
    run_bootstate_with_data_dir(root, action, root.join("data"))
}

fn run_bootstate_with_data_dir(root: &Path, action: &str, data_dir: PathBuf) -> Output {
    let wifi_init = root.join("wifi-init");
    let network_ready_probe = root.join("network-ready-probe");
    let network_ready_file = root.join("network-ready");
    if !wifi_init.exists() {
        write_executable(
            &wifi_init,
            r#"#!/bin/sh
[ "${1:-}" = "start" ] || exit 0
: > "$RHYTHM_TEST_NETWORK_READY_FILE"
"#,
        );
    }
    if !network_ready_probe.exists() {
        write_executable(
            &network_ready_probe,
            r#"#!/bin/sh
[ -e "$RHYTHM_TEST_NETWORK_READY_FILE" ]
"#,
        );
    }

    Command::new("sh")
        .arg(bootstate_script())
        .arg(action)
        .env("RHYTHM_BOOT_MOUNT", root.join("boot"))
        .env("RHYTHM_BOOTSTATE_DATA_DIR", root.join("data/ota"))
        .env(
            "RHYTHM_BOOTSTATE_ROLLBACK_REQUIRED_LATCH",
            root.join("run/rhythm-rollback-required"),
        )
        .env("RHYTHM_DATA_DIR", data_dir)
        .env("RHYTHM_SERVER_BIN", root.join("rhythm-server"))
        .env("RHYTHM_BOOTSTATE_WIFI_INIT", wifi_init)
        .env("RHYTHM_BOOTSTATE_NETWORK_READY_PROBE", network_ready_probe)
        .env("RHYTHM_BOOTSTATE_NETWORK_WAIT_ATTEMPTS", "3")
        .env("RHYTHM_BOOTSTATE_NETWORK_WAIT_SLEEP_SECS", "0")
        .env("RHYTHM_TEST_NETWORK_READY_FILE", network_ready_file)
        .env("RHYTHM_CMDLINE_FILE", root.join("boot/cmdline.txt"))
        .env("RHYTHM_PROC_CMDLINE", root.join("proc_cmdline"))
        .env("RHYTHM_BOOTSTATE_REBOOT_DRY_RUN", "1")
        .output()
        .unwrap()
}

fn assert_success(output: Output) {
    assert!(
        output.status.success(),
        "script failed: status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn bootstate_start_recovers_from_hashed_backup_when_primary_is_corrupt() {
    let root = unique_dir("backup-recovery");
    write_fake_server(&root, "0.4.2");
    fs::write(
        root.join("boot/cmdline.txt"),
        "console=tty1 root=/dev/mmcblk0p3 rootwait rw\n",
    )
    .unwrap();
    fs::write(
        root.join("proc_cmdline"),
        "console=tty1 root=/dev/mmcblk0p3 rootwait rw\n",
    )
    .unwrap();

    let (primary, backup) = bootstate_paths(&root);
    bootstate::write_with_backup(&primary, &backup, &pending_bootstate_body("pending")).unwrap();
    fs::write(
        &primary,
        "# RHYTHM_BOOTSTATE_HASH=deadbeef\nRHYTHM_PENDING_SLOT=a\n",
    )
    .unwrap();

    assert_success(run_bootstate(&root, "start"));

    let body = read_bootstate(&root);
    assert_eq!(bootstate_value(&body, "RHYTHM_ACTIVE_SLOT"), Some("b"));
    assert_eq!(bootstate_value(&body, "RHYTHM_LAST_GOOD_SLOT"), Some("a"));
    assert_eq!(bootstate_value(&body, "RHYTHM_PENDING_SLOT"), Some("b"));
    assert_eq!(
        bootstate_value(&body, "RHYTHM_BOOT_STATUS"),
        Some("booting")
    );
    assert_eq!(
        bootstate_value(&body, "RHYTHM_ACTIVE_VERSION"),
        Some("0.4.2")
    );

    let mirror = fs::read_to_string(root.join("data/ota/bootstate.env")).unwrap();
    assert_eq!(
        bootstate::verify_and_extract(&mirror).as_deref(),
        Some(body.as_str())
    );
}

#[test]
fn failed_second_boot_rollback_cannot_later_be_marked_successful() {
    let root = unique_dir("second-boot-rollback-latch");
    write_fake_server(&root, "0.4.2");
    fs::write(
        root.join("boot/cmdline.txt"),
        "console=tty1 root=/dev/mmcblk0p3 rootwait rw\n",
    )
    .unwrap();
    fs::write(
        root.join("proc_cmdline"),
        "console=tty1 root=/dev/mmcblk0p3 rootwait rw\n",
    )
    .unwrap();
    write_bootstate(&root, &pending_bootstate_body("booting"));
    fs::create_dir_all(root.join("data/local_ble")).unwrap();
    fs::write(root.join("data/local_ble/devices.json"), "sensitive").unwrap();

    let start = run_bootstate_with_data_dir(&root, "start", PathBuf::from("relative-data"));
    assert!(!start.status.success());
    let rollback_required_body = assert_pending_rollback_required(&root);

    let success = run_bootstate(&root, "success");
    assert!(!success.status.success());
    assert!(String::from_utf8_lossy(&success.stderr).contains("rollback is required"));
    assert_eq!(read_bootstate(&root), rollback_required_body);
    assert_pending_rollback_required(&root);
    assert!(root.join("data/local_ble/devices.json").exists());

    assert_success(run_bootstate(&root, "start"));
    let cmdline = fs::read_to_string(root.join("boot/cmdline.txt")).unwrap();
    assert!(cmdline.contains("root=/dev/mmcblk0p2"));
    assert!(!cmdline.contains("root=/dev/mmcblk0p3"));
    let body = read_bootstate(&root);
    assert_eq!(bootstate_value(&body, "RHYTHM_PENDING_SLOT"), Some(""));
    assert_eq!(bootstate_value(&body, "RHYTHM_BOOT_STATUS"), Some("idle"));
    assert!(!root.join("data/local_ble").exists());
}

#[test]
fn bootstate_success_marks_pending_slot_last_good_and_writes_hashed_backup() {
    let root = unique_dir("success");
    write_fake_server(&root, "0.4.2");
    fs::write(
        root.join("proc_cmdline"),
        "console=tty1 root=/dev/mmcblk0p3 rootwait rw\n",
    )
    .unwrap();
    write_bootstate(&root, &pending_bootstate_body("booting"));

    assert_success(run_bootstate(&root, "success"));

    let body = read_bootstate(&root);
    assert_eq!(bootstate_value(&body, "RHYTHM_ACTIVE_SLOT"), Some("b"));
    assert_eq!(bootstate_value(&body, "RHYTHM_LAST_GOOD_SLOT"), Some("b"));
    assert_eq!(bootstate_value(&body, "RHYTHM_PENDING_SLOT"), Some(""));
    assert_eq!(bootstate_value(&body, "RHYTHM_PENDING_VERSION"), Some(""));
    assert_eq!(bootstate_value(&body, "RHYTHM_BOOT_STATUS"), Some("idle"));

    let (primary, backup) = bootstate_paths(&root);
    let primary_raw = fs::read_to_string(primary).unwrap();
    let backup_raw = fs::read_to_string(backup).unwrap();
    assert!(primary_raw.starts_with(bootstate::HASH_HEADER_PREFIX));
    assert_eq!(
        bootstate::verify_and_extract(&primary_raw).as_deref(),
        Some(body.as_str())
    );
    assert_eq!(
        bootstate::verify_and_extract(&backup_raw).as_deref(),
        Some(body.as_str())
    );
}

#[test]
fn bootstate_success_version_mismatch_leaves_pending_rollback_armed() {
    let root = unique_dir("success-version-mismatch");
    write_fake_server(&root, "0.4.1");
    fs::write(
        root.join("boot/cmdline.txt"),
        "console=tty1 root=/dev/mmcblk0p3 rootwait rw\n",
    )
    .unwrap();
    fs::write(
        root.join("proc_cmdline"),
        "console=tty1 root=/dev/mmcblk0p3 rootwait rw\n",
    )
    .unwrap();
    let original_body = pending_bootstate_body("booting");
    write_bootstate(&root, &original_body);

    let output = run_bootstate(&root, "success");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("leaving rollback armed"));

    let cmdline = fs::read_to_string(root.join("boot/cmdline.txt")).unwrap();
    assert!(cmdline.contains("root=/dev/mmcblk0p3"));
    assert!(!cmdline.contains("root=/dev/mmcblk0p2"));

    let body = read_bootstate(&root);
    assert_eq!(body, original_body);
    assert!(!root.join("data/ota/bootstate.env").exists());
}

#[test]
fn bootstate_fail_rolls_cmdline_back_and_records_failed_version() {
    let root = unique_dir("fail");
    write_fake_server(&root, "0.4.2");
    fs::write(
        root.join("boot/cmdline.txt"),
        "console=tty1 root=/dev/mmcblk0p3 rootwait rw\n",
    )
    .unwrap();
    fs::write(
        root.join("proc_cmdline"),
        "console=tty1 root=/dev/mmcblk0p3 rootwait rw\n",
    )
    .unwrap();
    write_bootstate(&root, &pending_bootstate_body("booting"));
    for directory in ["local_ble", "aidot_ble"] {
        fs::create_dir_all(root.join("data").join(directory)).unwrap();
        fs::write(
            root.join("data").join(directory).join("devices.json"),
            "sensitive",
        )
        .unwrap();
    }
    for file in [
        "pairing_history.json",
        "pairing_history.json.tmp",
        "pairing_metadata.json",
        "pairing_metadata.json.tmp",
        "server_metadata.json.tmp",
    ] {
        fs::write(root.join("data").join(file), "sensitive").unwrap();
    }
    fs::write(
        root.join("data/server_metadata.json"),
        r#"{"server_instance_id":"srv-stable"}"#,
    )
    .unwrap();
    fs::write(root.join("data/settings.json"), "keep").unwrap();

    assert_success(run_bootstate(&root, "fail"));

    let cmdline = fs::read_to_string(root.join("boot/cmdline.txt")).unwrap();
    assert!(cmdline.contains("root=/dev/mmcblk0p2"));
    assert!(!cmdline.contains("root=/dev/mmcblk0p3"));

    let body = read_bootstate(&root);
    assert_eq!(bootstate_value(&body, "RHYTHM_ACTIVE_SLOT"), Some("b"));
    assert_eq!(bootstate_value(&body, "RHYTHM_PENDING_SLOT"), Some(""));
    assert_eq!(bootstate_value(&body, "RHYTHM_BOOT_STATUS"), Some("idle"));
    assert_eq!(
        bootstate_value(&body, "RHYTHM_LAST_ROLLBACK_SLOT"),
        Some("b")
    );
    assert_eq!(
        bootstate_value(&body, "RHYTHM_LAST_ROLLBACK_VERSION"),
        Some("0.4.2")
    );
    for path in [
        "local_ble",
        "aidot_ble",
        "pairing_history.json",
        "pairing_history.json.tmp",
        "pairing_metadata.json",
        "pairing_metadata.json.tmp",
        "server_metadata.json.tmp",
    ] {
        assert!(
            !root.join("data").join(path).exists(),
            "{path} should be scrubbed"
        );
    }
    assert!(root.join("data/server_metadata.json").exists());
    assert_eq!(
        fs::read_to_string(root.join("data/settings.json")).unwrap(),
        "keep"
    );
}

#[test]
fn bootstate_scrub_failure_does_not_switch_slots_or_clear_pending_state() {
    let root = unique_dir("fail-closed-scrub");
    write_fake_server(&root, "0.4.2");
    fs::write(
        root.join("boot/cmdline.txt"),
        "console=tty1 root=/dev/mmcblk0p3 rootwait rw\n",
    )
    .unwrap();
    fs::write(
        root.join("proc_cmdline"),
        "console=tty1 root=/dev/mmcblk0p3 rootwait rw\n",
    )
    .unwrap();
    write_bootstate(&root, &pending_bootstate_body("booting"));
    fs::create_dir_all(root.join("data/local_ble")).unwrap();
    fs::write(root.join("data/local_ble/devices.json"), "sensitive").unwrap();

    let output = run_bootstate_with_data_dir(&root, "fail", PathBuf::from("relative-data"));
    assert!(!output.status.success());
    let rollback_required_body = assert_pending_rollback_required(&root);
    assert!(root.join("data/local_ble/devices.json").exists());

    let success = run_bootstate(&root, "success");
    assert!(!success.status.success());
    assert_eq!(read_bootstate(&root), rollback_required_body);
    assert_pending_rollback_required(&root);

    // Persistent rollback_required is independently authoritative if the
    // same-boot latch is lost or cleared unexpectedly.
    fs::remove_file(rollback_required_latch(&root)).unwrap();
    let success_without_latch = run_bootstate(&root, "success");
    assert!(!success_without_latch.status.success());
    assert_eq!(read_bootstate(&root), rollback_required_body);

    // A later fail invocation retries from rollback_required, recreates the
    // latch, completes the scrub, and only then switches slots.
    assert_success(run_bootstate(&root, "fail"));
    let cmdline = fs::read_to_string(root.join("boot/cmdline.txt")).unwrap();
    assert!(cmdline.contains("root=/dev/mmcblk0p2"));
    assert!(!cmdline.contains("root=/dev/mmcblk0p3"));
    let body = read_bootstate(&root);
    assert_eq!(bootstate_value(&body, "RHYTHM_PENDING_SLOT"), Some(""));
    assert_eq!(bootstate_value(&body, "RHYTHM_BOOT_STATUS"), Some("idle"));
    assert!(!root.join("data/local_ble").exists());
    assert!(rollback_required_latch(&root).exists());
}

#[test]
fn bootstate_hue_restore_failure_does_not_switch_slots() {
    let root = unique_dir("fail-closed-hue-restore");
    let server = root.join("rhythm-server");
    write_executable(
        &server,
        r#"#!/bin/sh
if [ "${1:-}" = "--version" ]; then
    echo "rhythm-server 0.4.2"
    exit 0
fi
for argument in "$@"; do
    if [ "$argument" = "--restore-hue-before-rollback" ]; then
        exit 9
    fi
done
exit 0
"#,
    );
    fs::write(
        root.join("boot/cmdline.txt"),
        "console=tty1 root=/dev/mmcblk0p3 rootwait rw\n",
    )
    .unwrap();
    fs::write(
        root.join("proc_cmdline"),
        "console=tty1 root=/dev/mmcblk0p3 rootwait rw\n",
    )
    .unwrap();
    write_bootstate(&root, &pending_bootstate_body("booting"));

    let output = run_bootstate(&root, "fail");
    assert!(!output.status.success());
    assert_pending_rollback_required(&root);
    let cmdline = fs::read_to_string(root.join("boot/cmdline.txt")).unwrap();
    assert!(cmdline.contains("root=/dev/mmcblk0p3"));
    assert!(!cmdline.contains("root=/dev/mmcblk0p2"));
}

#[test]
fn bootstate_brings_network_up_and_waits_before_hue_restore() {
    let root = unique_dir("hue-restore-network-order");
    let events = root.join("rollback-events");
    let first_probe = root.join("first-network-probe");
    write_executable(
        &root.join("wifi-init"),
        &format!(
            r#"#!/bin/sh
[ "${{1:-}}" = "start" ] || exit 1
printf 'wifi-start\n' >> '{}'
"#,
            events.display()
        ),
    );
    write_executable(
        &root.join("network-ready-probe"),
        &format!(
            r#"#!/bin/sh
if [ ! -e '{}' ]; then
    : > '{}'
    printf 'network-wait\n' >> '{}'
    exit 1
fi
printf 'network-ready\n' >> '{}'
"#,
            first_probe.display(),
            first_probe.display(),
            events.display(),
            events.display()
        ),
    );
    write_executable(
        &root.join("rhythm-server"),
        &format!(
            r#"#!/bin/sh
if [ "${{1:-}}" = "--version" ]; then
    echo "rhythm-server 0.4.2"
    exit 0
fi
for argument in "$@"; do
    if [ "$argument" = "--restore-hue-before-rollback" ]; then
        printf 'hue-restore\n' >> '{}'
        exit 0
    fi
done
exit 1
"#,
            events.display()
        ),
    );
    fs::write(
        root.join("boot/cmdline.txt"),
        "console=tty1 root=/dev/mmcblk0p3 rootwait rw\n",
    )
    .unwrap();
    fs::write(
        root.join("proc_cmdline"),
        "console=tty1 root=/dev/mmcblk0p3 rootwait rw\n",
    )
    .unwrap();
    write_bootstate(&root, &pending_bootstate_body("booting"));

    assert_success(run_bootstate(&root, "fail"));
    assert_eq!(
        fs::read_to_string(events).unwrap(),
        "wifi-start\nnetwork-wait\nnetwork-ready\nhue-restore\n"
    );
    let cmdline = fs::read_to_string(root.join("boot/cmdline.txt")).unwrap();
    assert!(cmdline.contains("root=/dev/mmcblk0p2"));
}

#[test]
fn bootstate_network_timeout_attempts_hue_restore_after_bound_and_stays_fail_closed() {
    let root = unique_dir("hue-restore-network-timeout");
    let rollback_events = root.join("rollback-events");
    write_executable(
        &root.join("wifi-init"),
        r#"#!/bin/sh
[ "${1:-}" = "start" ]
"#,
    );
    write_executable(
        &root.join("network-ready-probe"),
        &format!(
            r#"#!/bin/sh
printf 'probe\n' >> '{}'
exit 1
"#,
            rollback_events.display()
        ),
    );
    write_executable(
        &root.join("rhythm-server"),
        &format!(
            r#"#!/bin/sh
if [ "${{1:-}}" = "--version" ]; then
    echo "rhythm-server 0.4.2"
    exit 0
fi
for argument in "$@"; do
    if [ "$argument" = "--restore-hue-before-rollback" ]; then
        printf 'hue-restore\n' >> '{}'
        exit 9
    fi
done
exit 1
"#,
            rollback_events.display()
        ),
    );
    fs::write(
        root.join("boot/cmdline.txt"),
        "console=tty1 root=/dev/mmcblk0p3 rootwait rw\n",
    )
    .unwrap();
    fs::write(
        root.join("proc_cmdline"),
        "console=tty1 root=/dev/mmcblk0p3 rootwait rw\n",
    )
    .unwrap();
    write_bootstate(&root, &pending_bootstate_body("booting"));

    let output = run_bootstate(&root, "fail");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("network did not become ready"));
    assert_eq!(
        fs::read_to_string(rollback_events).unwrap(),
        "probe\nprobe\nprobe\nhue-restore\n"
    );
    assert_pending_rollback_required(&root);
}

#[test]
fn rhythm_launch_calls_bootstate_fail_when_server_exits() {
    let root = unique_dir("launch");
    fs::create_dir_all(root.join("log")).unwrap();
    let server = root.join("server-exits-42");
    write_executable(
        &server,
        r#"#!/bin/sh
echo "server starting"
exit 42
"#,
    );
    let bootstate_log = root.join("bootstate.log");
    let bootstate = root.join("bootstate");
    write_executable(
        &bootstate,
        r#"#!/bin/sh
printf '%s\n' "$1" >> "$RHYTHM_BOOTSTATE_LOG"
exit 0
"#,
    );

    let output = Command::new("sh")
        .arg(launch_script())
        .env("RHYTHM_DATA_DIR", root.join("data"))
        .env("RHYTHM_LOG_DIR", root.join("log"))
        .env("RHYTHM_DEFAULTS", root.join("missing-defaults"))
        .env("RHYTHM_DEV_DEFAULTS", root.join("missing-dev-defaults"))
        .env("RHYTHM_SERVER_BIN", &server)
        .env("RHYTHM_BOOTSTATE_SCRIPT", &bootstate)
        .env("RHYTHM_BOOTSTATE_LOG", &bootstate_log)
        .env("RHYTHM_LOG_PRUNE_BIN", root.join("missing-prune"))
        .env("RHYTHM_CONSOLE", root.join("missing-console"))
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(42),
        "launcher should preserve server exit status; stdout={}; stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(bootstate_log).unwrap(), "fail\n");
}

#[test]
fn cloudflared_service_is_manual_and_resource_guarded() {
    assert!(
        !init_script_dir().join("S44cloudflared").exists(),
        "cloudflared must not auto-start from rcS"
    );

    let script = cloudflared_script();
    let metadata = fs::metadata(&script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(
            metadata.permissions().mode() & 0o111,
            0,
            "cloudflared service script must be executable"
        );
    }

    let body = fs::read_to_string(script).unwrap();
    assert!(body.contains("RHYTHM_CLOUDFLARED_MAX_RESTARTS"));
    assert!(body.contains("RHYTHM_CLOUDFLARED_VMEM_LIMIT_KB"));
    assert!(body.contains("start_cloudflared_child"));
    assert!(body.contains("connector restart limit reached"));
    assert!(body.contains("RHYTHM_CLOUDFLARED_MAX_RESTARTS:-0"));
    assert!(body.contains("RHYTHM_CLOUDFLARED_EDGE_IP_VERSION:-4"));
    assert!(body.contains("--edge-ip-version \"$EDGE_IP_VERSION\""));
    assert!(body.contains("run --token-file \"$TOKEN_FILE\""));
    assert!(!body.contains("run --token \"$token\""));
    // Stale pidfiles must never leave an orphaned connector running: stop()
    // sweeps by binary name, not just recorded pids.
    assert!(body.contains("kill_stray_connectors"));
}
