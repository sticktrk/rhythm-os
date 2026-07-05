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

fn pending_bootstate_body(status: &str) -> String {
    format!(
        "RHYTHM_ACTIVE_SLOT=a\nRHYTHM_LAST_GOOD_SLOT=a\nRHYTHM_PENDING_SLOT=b\nRHYTHM_PENDING_VERSION=0.4.2\nRHYTHM_ACTIVE_VERSION=0.4.1\nRHYTHM_BOOT_STATUS={status}\nRHYTHM_LAST_UPDATE_EPOCH_MS=1000\nRHYTHM_LAST_ROLLBACK_SLOT=\nRHYTHM_LAST_ROLLBACK_VERSION=\nRHYTHM_LAST_ROLLBACK_EPOCH_MS=\n"
    )
}

fn run_bootstate(root: &Path, action: &str) -> Output {
    Command::new("sh")
        .arg(bootstate_script())
        .arg(action)
        .env("RHYTHM_BOOT_MOUNT", root.join("boot"))
        .env("RHYTHM_BOOTSTATE_DATA_DIR", root.join("data/ota"))
        .env("RHYTHM_SERVER_BIN", root.join("rhythm-server"))
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
    // Stale pidfiles must never leave an orphaned connector running: stop()
    // sweeps by binary name, not just recorded pids.
    assert!(body.contains("kill_stray_connectors"));
}
