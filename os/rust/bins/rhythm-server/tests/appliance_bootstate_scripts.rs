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
    bootstate_command(root, action, data_dir)
        .env("RHYTHM_BOOTSTATE_REBOOT_DRY_RUN", "1")
        .output()
        .unwrap()
}

/// Runs without the dry-run switch. Reboots go to a recording stand-in that
/// returns, which is what the script sees when a reboot request fails.
fn run_bootstate_with_fake_reboot(root: &Path, action: &str) -> Output {
    let reboot = root.join("fake-reboot");
    write_executable(
        &reboot,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RHYTHM_TEST_REBOOT_LOG"
"#,
    );
    // Both filesystems appear mounted, and `mount` is a recording stand-in.
    let bin = fake_bin_dir(root);
    write_executable(
        &bin.join("mount"),
        r#"#!/bin/sh
printf 'mount %s\n' "$*" >> "$RHYTHM_TEST_REBOOT_LOG"
"#,
    );
    fs::write(
        root.join("proc_mounts"),
        format!(
            "/dev/mmcblk0p1 {} vfat rw 0 0\n/dev/mmcblk0p4 {} ext4 rw 0 0\n",
            root.join("boot").display(),
            root.join("data").display()
        ),
    )
    .unwrap();
    bootstate_command(root, action, root.join("data"))
        .env("PATH", path_with(&bin))
        .env("RHYTHM_BOOTSTATE_REBOOT_BIN", reboot)
        .env("RHYTHM_TEST_REBOOT_LOG", root.join("reboot.log"))
        .output()
        .unwrap()
}

fn fake_bin_dir(root: &Path) -> PathBuf {
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    bin
}

fn path_with(bin: &Path) -> String {
    format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

fn bootstate_command(root: &Path, action: &str, data_dir: PathBuf) -> Command {
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

    let mut command = Command::new("sh");
    command
        .arg(bootstate_script())
        .arg(action)
        .env("RHYTHM_BOOT_MOUNT", root.join("boot"))
        .env("RHYTHM_BOARD_MODEL_FILE", root.join("board-model"))
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
        // Never the host's mount table: the script remounts what it lists.
        .env("RHYTHM_PROC_MOUNTS", root.join("proc_mounts"));
    command
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

const LEGACY_BLUETOOTH_CONFIG: &str =
    "kernel=kernel.img\ndtoverlay=miniuart-bt\ncore_freq=250\nenable_uart=1\ndtoverlay=ramoops\n# custom setting\ndisable_splash=1\n";

fn bluetooth_uart_boot_fixture(name: &str, status: &str) -> PathBuf {
    let root = unique_dir(name);
    write_fake_server(&root, "0.4.2");
    fs::write(root.join("board-model"), "Raspberry Pi Zero W Rev 1.1\0").unwrap();
    fs::write(root.join("boot/config.txt"), LEGACY_BLUETOOTH_CONFIG).unwrap();
    let cmdline = "console=tty1 root=/dev/mmcblk0p3 rootwait rw\n";
    fs::write(root.join("boot/cmdline.txt"), cmdline).unwrap();
    fs::write(root.join("proc_cmdline"), cmdline).unwrap();
    write_bootstate(&root, &pending_bootstate_body(status));
    root
}

#[test]
fn bluetooth_uart_migration_reboots_before_probation_and_preserves_device_state() {
    let root = bluetooth_uart_boot_fixture("uart-upgrade", "pending");
    let pending = read_bootstate(&root);
    for path in [
        "bluetooth/adapter/bond/info",
        "local_ble/devices.json",
        "topology.json",
    ] {
        let path = root.join("data").join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "keep").unwrap();
    }

    let output = run_bootstate(&root, "start");
    assert!(String::from_utf8_lossy(&output.stdout).contains("rebooting before OTA probation"));
    assert_success(output);
    assert_eq!(read_bootstate(&root), pending);
    let migrated = LEGACY_BLUETOOTH_CONFIG.replace("dtoverlay=miniuart-bt\n", "");
    assert_eq!(
        fs::read_to_string(root.join("boot/config.txt")).unwrap(),
        migrated
    );
    assert_eq!(
        fs::read_to_string(root.join("boot/config.txt.pre-bluetooth-uart")).unwrap(),
        LEGACY_BLUETOOTH_CONFIG
    );

    // Also models a power loss after the atomic replacement but before reboot:
    // no marker can get out of sync and no second migration reboot is needed.
    let output = run_bootstate(&root, "start");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("rebooting"));
    assert_success(output);
    assert_eq!(
        bootstate_value(&read_bootstate(&root), "RHYTHM_BOOT_STATUS"),
        Some("booting")
    );
    assert_success(run_bootstate(&root, "success"));
    let healthy = read_bootstate(&root);
    assert_eq!(
        bootstate_value(&healthy, "RHYTHM_BOOT_STATUS"),
        Some("idle")
    );
    assert_eq!(
        bootstate_value(&healthy, "RHYTHM_LAST_GOOD_SLOT"),
        Some("b")
    );
    for path in [
        "bluetooth/adapter/bond/info",
        "local_ble/devices.json",
        "topology.json",
    ] {
        assert_eq!(
            fs::read_to_string(root.join("data").join(path)).unwrap(),
            "keep"
        );
    }
    assert!(fs::read_to_string(root.join("boot/cmdline.txt"))
        .unwrap()
        .contains("root=/dev/mmcblk0p3"));
}

#[test]
fn bluetooth_uart_migration_does_not_mask_a_failed_candidate_or_change_rollback_policy() {
    for status in ["booting", "rollback_required"] {
        let root = bluetooth_uart_boot_fixture(&format!("uart-rollback-{status}"), status);
        assert_success(run_bootstate(&root, "start"));
        assert_eq!(
            fs::read_to_string(root.join("boot/config.txt")).unwrap(),
            LEGACY_BLUETOOTH_CONFIG
        );
        assert!(!root.join("boot/config.txt.pre-bluetooth-uart").exists());
        assert!(fs::read_to_string(root.join("boot/cmdline.txt"))
            .unwrap()
            .contains("root=/dev/mmcblk0p2"));
    }

    // The migration belongs to its candidate: when that candidate fails
    // probation, the previous image gets back the exact config.txt it was
    // qualified with, and the next update attempt migrates again.
    let root = bluetooth_uart_boot_fixture("uart-rollback-after-migration", "pending");
    assert_success(run_bootstate(&root, "start"));
    assert_eq!(
        fs::read_to_string(root.join("boot/rhythm-bluetooth-uart.pending")).unwrap(),
        "b:0.4.2\n"
    );
    assert_success(run_bootstate(&root, "start"));
    let output = run_bootstate(&root, "start");
    assert!(String::from_utf8_lossy(&output.stdout)
        .contains("restored the previous Bluetooth UART routing"));
    assert_success(output);
    assert_eq!(
        fs::read_to_string(root.join("boot/config.txt")).unwrap(),
        LEGACY_BLUETOOTH_CONFIG
    );
    assert!(!root.join("boot/rhythm-bluetooth-uart.pending").exists());
    assert!(fs::read_to_string(root.join("boot/cmdline.txt"))
        .unwrap()
        .contains("root=/dev/mmcblk0p2"));
    assert_eq!(
        bootstate_value(&read_bootstate(&root), "RHYTHM_LAST_ROLLBACK_SLOT"),
        Some("b")
    );
}

#[test]
fn bluetooth_uart_migration_is_permanent_once_its_candidate_is_proven() {
    let root = bluetooth_uart_boot_fixture("uart-committed", "pending");
    assert_success(run_bootstate(&root, "start"));
    assert_success(run_bootstate(&root, "start"));
    assert!(root.join("boot/rhythm-bluetooth-uart.pending").exists());
    assert_success(run_bootstate(&root, "success"));
    assert!(!root.join("boot/rhythm-bluetooth-uart.pending").exists());
    let migrated = fs::read_to_string(root.join("boot/config.txt")).unwrap();

    // A later candidate on the other slot fails: its rollback must not undo a
    // migration that an earlier image already proved.
    fs::write(
        root.join("proc_cmdline"),
        "console=tty1 root=/dev/mmcblk0p2 rootwait rw\n",
    )
    .unwrap();
    fs::write(
        root.join("boot/cmdline.txt"),
        "console=tty1 root=/dev/mmcblk0p2 rootwait rw\n",
    )
    .unwrap();
    write_bootstate(
        &root,
        "RHYTHM_ACTIVE_SLOT=b\nRHYTHM_LAST_GOOD_SLOT=b\nRHYTHM_PENDING_SLOT=a\nRHYTHM_PENDING_VERSION=0.4.3\nRHYTHM_ACTIVE_VERSION=0.4.2\nRHYTHM_BOOT_STATUS=booting\n",
    );
    // Even a stale record from the decided candidate is ignored.
    fs::write(root.join("boot/rhythm-bluetooth-uart.pending"), "b:0.4.2\n").unwrap();
    assert_success(run_bootstate(&root, "start"));
    assert_eq!(
        fs::read_to_string(root.join("boot/config.txt")).unwrap(),
        migrated
    );
    assert!(!root.join("boot/rhythm-bluetooth-uart.pending").exists());
    assert!(fs::read_to_string(root.join("boot/cmdline.txt"))
        .unwrap()
        .contains("root=/dev/mmcblk0p3"));
}

#[test]
fn bluetooth_uart_migration_reboot_is_forced_once_and_survives_a_returning_reboot() {
    let root = bluetooth_uart_boot_fixture("uart-real-reboot", "pending");
    let output = run_bootstate_with_fake_reboot(&root, "start");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert_success(output);
    // Writable filesystems are frozen before the forced reboot and thawed
    // again because this boot continues.
    let boot = root.join("boot").display().to_string();
    let data = root.join("data").display().to_string();
    assert_eq!(
        fs::read_to_string(root.join("reboot.log")).unwrap(),
        format!(
            "mount -o remount,ro {boot}\nmount -o remount,ro {data}\n-f\nmount -o remount,rw {boot}\nmount -o remount,rw {data}\n"
        )
    );
    // The reboot request returned, so this boot carries on into probation.
    assert!(stderr.contains("Bluetooth UART reboot returned"));
    assert_eq!(
        bootstate_value(&read_bootstate(&root), "RHYTHM_BOOT_STATUS"),
        Some("booting")
    );
    assert!(!fs::read_to_string(root.join("boot/config.txt"))
        .unwrap()
        .contains("miniuart-bt"));
}

#[test]
fn bluetooth_uart_migration_cannot_reboot_forever() {
    let root = bluetooth_uart_boot_fixture("uart-reboot-limit", "idle");
    write_bootstate(
        &root,
        "RHYTHM_ACTIVE_SLOT=b\nRHYTHM_LAST_GOOD_SLOT=b\nRHYTHM_BOOT_STATUS=idle\n",
    );
    // A boot partition that keeps presenting the directive (the rewrite is
    // not persisting) gets two attempts, then the appliance stays up.
    for expect_reboot in [true, true, false, false] {
        fs::write(root.join("boot/config.txt"), LEGACY_BLUETOOTH_CONFIG).unwrap();
        let output = run_bootstate(&root, "start");
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        assert_success(output);
        assert_eq!(
            stdout.contains("rebooting before OTA probation"),
            expect_reboot
        );
        assert_eq!(stderr.contains("reboot limit reached"), !expect_reboot);
    }
    assert_eq!(
        bootstate_value(&read_bootstate(&root), "RHYTHM_BOOT_STATUS"),
        Some("idle")
    );

    // A boot that finds the directive gone re-arms the allowance.
    assert_success(run_bootstate(&root, "start"));
    assert!(!root
        .join("data/ota/bluetooth-uart-migration-reboots")
        .exists());
}

#[test]
fn bluetooth_uart_rewrite_that_removes_nothing_never_reboots() {
    let root = bluetooth_uart_boot_fixture("uart-noop-rewrite", "idle");
    write_bootstate(
        &root,
        "RHYTHM_ACTIVE_SLOT=b\nRHYTHM_LAST_GOOD_SLOT=b\nRHYTHM_BOOT_STATUS=idle\n",
    );
    // Stand-in for a sed whose pattern dialect disagrees with grep's: the
    // directive is detected but the rewrite leaves it in place.
    let bin = fake_bin_dir(&root);
    write_executable(
        &bin.join("sed"),
        r#"#!/bin/sh
if [ "$1" = "-E" ]; then
    exec cat "$3"
fi
for dir in /usr/bin /bin; do
    [ -x "$dir/sed" ] && exec "$dir/sed" "$@"
done
exit 127
"#,
    );
    for _ in 0..3 {
        let output = bootstate_command(&root, "start", root.join("data"))
            .env("PATH", path_with(&bin))
            .env("RHYTHM_BOOTSTATE_REBOOT_DRY_RUN", "1")
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&output.stderr).contains("failed verification"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("rebooting"));
        assert_success(output);
        assert_eq!(
            fs::read_to_string(root.join("boot/config.txt")).unwrap(),
            LEGACY_BLUETOOTH_CONFIG
        );
        assert!(!root.join("boot/config.txt.pre-bluetooth-uart").exists());
        assert!(!root.join("boot/rhythm-bluetooth-uart.pending").exists());
    }
}

#[test]
fn bluetooth_uart_migration_rebuilds_a_config_lost_during_replacement() {
    let root = bluetooth_uart_boot_fixture("uart-lost-config", "pending");
    fs::rename(
        root.join("boot/config.txt"),
        root.join("boot/config.txt.pre-bluetooth-uart"),
    )
    .unwrap();
    let output = run_bootstate(&root, "start");
    assert!(String::from_utf8_lossy(&output.stderr).contains("config.txt is missing"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("rebooting before OTA probation"));
    assert_success(output);
    assert_eq!(
        fs::read_to_string(root.join("boot/config.txt")).unwrap(),
        LEGACY_BLUETOOTH_CONFIG.replace("dtoverlay=miniuart-bt\n", "")
    );
    assert_eq!(
        fs::read_to_string(root.join("boot/config.txt.pre-bluetooth-uart")).unwrap(),
        LEGACY_BLUETOOTH_CONFIG
    );

    // Without our backup there is nothing trustworthy to rebuild from.
    let root = bluetooth_uart_boot_fixture("uart-lost-config-no-backup", "pending");
    fs::remove_file(root.join("boot/config.txt")).unwrap();
    assert_success(run_bootstate(&root, "start"));
    assert!(!root.join("boot/config.txt").exists());
}

#[test]
fn bluetooth_uart_repeat_migration_keeps_the_original_backup() {
    let root = bluetooth_uart_boot_fixture("uart-repeat", "idle");
    write_bootstate(
        &root,
        "RHYTHM_ACTIVE_SLOT=b\nRHYTHM_LAST_GOOD_SLOT=b\nRHYTHM_BOOT_STATUS=idle\n",
    );
    assert_success(run_bootstate(&root, "start"));
    assert_success(run_bootstate(&root, "start"));

    let edited = format!("{LEGACY_BLUETOOTH_CONFIG}gpu_mem=16\n");
    for config in [edited.clone(), format!("{edited}hdmi_blanking=1\n")] {
        fs::write(root.join("boot/config.txt"), &config).unwrap();
        assert_success(run_bootstate(&root, "start"));
        assert_eq!(
            fs::read_to_string(root.join("boot/config.txt.pre-bluetooth-uart")).unwrap(),
            config
        );
        assert_eq!(
            fs::read_to_string(root.join("boot/config.txt.pre-bluetooth-uart.orig")).unwrap(),
            LEGACY_BLUETOOTH_CONFIG
        );
        assert_success(run_bootstate(&root, "start"));
    }
}

#[test]
fn bluetooth_uart_migration_handles_directive_only_and_unterminated_configs() {
    for (name, config, migrated) in [
        ("only", "dtoverlay=miniuart-bt\n", ""),
        (
            "unterminated",
            "kernel=kernel.img\ndtoverlay=miniuart-bt",
            "kernel=kernel.img\n",
        ),
        (
            "repeated",
            "[all]\ndtoverlay=miniuart-bt\r\ncore_freq=250\ndtoverlay=miniuart-bt\n",
            "[all]\ncore_freq=250\n",
        ),
    ] {
        let root = bluetooth_uart_boot_fixture(&format!("uart-shape-{name}"), "pending");
        fs::write(root.join("boot/config.txt"), config).unwrap();
        let output = run_bootstate(&root, "start");
        assert!(String::from_utf8_lossy(&output.stdout).contains("rebooting before OTA probation"));
        assert_success(output);
        assert_eq!(
            fs::read_to_string(root.join("boot/config.txt")).unwrap(),
            migrated
        );
    }
}

#[test]
fn bluetooth_uart_backup_failure_keeps_original_config_and_arms_normal_probation() {
    let root = bluetooth_uart_boot_fixture("uart-backup-failure", "pending");
    fs::create_dir(root.join("boot/config.txt.pre-bluetooth-uart")).unwrap();
    let output = run_bootstate(&root, "start");
    assert!(String::from_utf8_lossy(&output.stderr).contains("refusing non-regular"));
    assert_success(output);
    assert_eq!(
        fs::read_to_string(root.join("boot/config.txt")).unwrap(),
        LEGACY_BLUETOOTH_CONFIG
    );
    assert_eq!(
        bootstate_value(&read_bootstate(&root), "RHYTHM_BOOT_STATUS"),
        Some("booting")
    );
}

#[test]
fn bluetooth_uart_migration_leaves_other_boards_and_custom_overlay_parameters_alone() {
    let fresh =
        fs::read_to_string(repo_root().join("install/rpiz/buildroot/board/rhythm/rpiz/config.txt"))
            .unwrap();
    assert!(!fresh
        .lines()
        .any(|line| line.trim().starts_with("dtoverlay=miniuart-bt")));
    for (name, model, config) in [
        (
            "other-board",
            "Raspberry Pi 4 Model B\0",
            LEGACY_BLUETOOTH_CONFIG,
        ),
        ("unknown-board", "", LEGACY_BLUETOOTH_CONFIG),
        ("fresh-image", "Raspberry Pi Zero W\0", fresh.as_str()),
        (
            "custom-overlay",
            "Raspberry Pi Zero W\0",
            "dtoverlay=miniuart-bt,krnbt=off\n# dtoverlay=miniuart-bt\n",
        ),
    ] {
        let root = bluetooth_uart_boot_fixture(name, "pending");
        fs::write(root.join("board-model"), model).unwrap();
        fs::write(root.join("boot/config.txt"), config).unwrap();
        let output = run_bootstate(&root, "start");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("rebooting"));
        assert_success(output);
        assert_eq!(
            fs::read_to_string(root.join("boot/config.txt")).unwrap(),
            config
        );
        assert!(!root.join("boot/config.txt.pre-bluetooth-uart").exists());
        assert_eq!(
            bootstate_value(&read_bootstate(&root), "RHYTHM_BOOT_STATUS"),
            Some("booting")
        );
    }
}

#[test]
fn bluetooth_uart_migration_handles_whitespace_and_an_idle_legacy_slot_once() {
    let root = bluetooth_uart_boot_fixture("uart-idle", "idle");
    write_bootstate(
        &root,
        "RHYTHM_ACTIVE_SLOT=b\nRHYTHM_LAST_GOOD_SLOT=b\nRHYTHM_BOOT_STATUS=idle\n",
    );
    fs::write(root.join("board-model"), "Raspberry Pi Zero W\0").unwrap();
    fs::write(
        root.join("boot/config.txt"),
        "kernel=kernel.img\n dtoverlay = miniuart-bt  # legacy\r\ncore_freq=250\n",
    )
    .unwrap();
    let output = run_bootstate(&root, "start");
    assert!(String::from_utf8_lossy(&output.stdout).contains("rebooting before OTA probation"));
    assert_success(output);
    assert_eq!(
        fs::read_to_string(root.join("boot/config.txt")).unwrap(),
        "kernel=kernel.img\ncore_freq=250\n"
    );
    let output = run_bootstate(&root, "start");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("rebooting"));
    assert_success(output);
    assert_eq!(
        bootstate_value(&read_bootstate(&root), "RHYTHM_BOOT_STATUS"),
        Some("idle")
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
