use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rhythm_server::bootstate;

const BOOT_MOUNT: &str = "/boot";
const PROC_CMDLINE: &str = "/proc/cmdline";
const BOOTSTATE_FILE: &str = "rhythm-bootstate.env";
const BOOTSTATE_BACKUP_FILE: &str = "rhythm-bootstate.env.bak";
const CMDLINE_BACKUP_FILE: &str = "cmdline.txt.bak";
const OTA_DIR: &str = "ota";
const LOG_DIR: &str = "log";
const BLUETOOTH_DIR: &str = "bluetooth";
const BLUETOOTH_QUARANTINE_DIR: &str = ".bluetooth_factory_reset_quarantine";
const HUE_BLE_DIR: &str = "hue_ble";
#[cfg(target_os = "linux")]
const BLUETOOTHD_STOP_TIMEOUT: Duration = Duration::from_secs(3);
#[cfg(target_os = "linux")]
const BOND_ROTATION_RESERVE: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ApplianceBootSlot {
    A,
    B,
}

impl ApplianceBootSlot {
    fn as_str(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
        }
    }
}

#[derive(Clone, Debug)]
struct FactoryResetPaths {
    data_dir: PathBuf,
    log_dir: PathBuf,
    boot_mount: PathBuf,
}

pub fn prepare_appliance_factory_reset_state(data_dir: &str, current_version: &str) -> Result<()> {
    let data_dir = PathBuf::from(data_dir);
    let log_dir = std::env::var_os("RHYTHM_LOG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join(LOG_DIR));
    let paths = FactoryResetPaths {
        data_dir,
        log_dir,
        boot_mount: PathBuf::from(BOOT_MOUNT),
    };
    let slot = detect_current_slot(&paths.boot_mount);
    prepare_paths(&paths, slot, current_version)
}

/// Perform every unrelated fallible cleanup while BlueZ and all Hue keys are
/// still available. A failure here leaves the current bulb installation
/// retryable and never consumes the final handoff window.
fn prepare_paths(
    paths: &FactoryResetPaths,
    slot: ApplianceBootSlot,
    current_version: &str,
) -> Result<()> {
    install_hue_ble_factory_reset_block(&paths.data_dir)?;
    prepare_bluetooth_quarantine(&paths.data_dir)?;
    remove_dir_if_exists(&paths.log_dir)?;
    remove_dir_if_exists(&paths.data_dir.join(OTA_DIR))?;
    fs::create_dir_all(paths.data_dir.join(OTA_DIR))
        .with_context(|| format!("creating {}", paths.data_dir.join(OTA_DIR).display()))?;
    remove_file_if_exists(&paths.boot_mount.join(CMDLINE_BACKUP_FILE))?;
    write_bootstate(
        &paths.boot_mount.join(BOOTSTATE_FILE),
        &paths.boot_mount.join(BOOTSTATE_BACKUP_FILE),
        slot,
        current_version,
    )?;
    Ok(())
}

pub fn commit_hue_ble_factory_reset_state(
    data_dir: &str,
    handoff_valid_until: Option<Instant>,
) -> Result<()> {
    let data_dir = PathBuf::from(data_dir);
    commit_paths(&data_dir, handoff_valid_until)
}

#[cfg(test)]
fn scrub_paths(
    paths: &FactoryResetPaths,
    slot: ApplianceBootSlot,
    current_version: &str,
) -> Result<()> {
    prepare_paths(paths, slot, current_version)?;
    commit_paths(&paths.data_dir, None)
}

/// Atomically detach every active BlueZ adapter tree while the vendor handoff
/// is fresh, then delete the detached data without holding that time-critical
/// window open for recursive filesystem work.
fn commit_paths(data_dir: &Path, handoff_valid_until: Option<Instant>) -> Result<()> {
    ensure_handoff_fresh(handoff_valid_until)?;
    rotate_bluetooth_entries_into_quarantine(data_dir, handoff_valid_until)?;

    // All adapter keys are now outside BlueZ's configured storage path.
    // Recursive deletion may take arbitrary time without allowing BlueZ to
    // observe a partially scrubbed active key database.
    remove_dir_if_exists(&data_dir.join(HUE_BLE_DIR))?;
    remove_dir_if_exists(&data_dir.join(BLUETOOTH_QUARANTINE_DIR))?;
    sync_directory(data_dir)?;

    // The plan and global adoption block are the final commit records. Keep
    // both through every earlier cleanup step so an interrupted rotation or
    // detached-key cleanup remains fail-closed and auditable after reboot.
    rhythm_hue::ble::HueBleDeviceStore::clear_factory_reset_plan(data_dir)?;
    rhythm_hue::ble::HueBleDeviceStore::clear_paired_orphan_adoption_block(data_dir)?;
    Ok(())
}

fn prepare_bluetooth_quarantine(data_dir: &Path) -> Result<()> {
    fs::create_dir_all(data_dir).with_context(|| format!("creating {}", data_dir.display()))?;
    let quarantine_dir = data_dir.join(BLUETOOTH_QUARANTINE_DIR);
    remove_dir_if_exists(&quarantine_dir)?;
    fs::create_dir(&quarantine_dir)
        .with_context(|| format!("creating {}", quarantine_dir.display()))?;
    sync_directory(&quarantine_dir)?;
    sync_directory(data_dir)
}

fn rotate_bluetooth_entries_into_quarantine(
    data_dir: &Path,
    handoff_valid_until: Option<Instant>,
) -> Result<()> {
    let bluetooth_dir = data_dir.join(BLUETOOTH_DIR);
    let quarantine_dir = data_dir.join(BLUETOOTH_QUARANTINE_DIR);

    let quarantine_metadata = fs::symlink_metadata(&quarantine_dir)
        .with_context(|| format!("inspecting {}", quarantine_dir.display()))?;
    if !quarantine_metadata.file_type().is_dir() {
        anyhow::bail!(
            "{} is not the prepared Bluetooth key quarantine directory",
            quarantine_dir.display()
        );
    }
    if fs::read_dir(&quarantine_dir)
        .with_context(|| format!("reading {}", quarantine_dir.display()))?
        .next()
        .transpose()?
        .is_some()
    {
        anyhow::bail!(
            "{} is not empty before Bluetooth key rotation",
            quarantine_dir.display()
        );
    }

    let entries = match fs::read_dir(&bluetooth_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ensure_handoff_fresh(handoff_valid_until)?;
            return Ok(());
        }
        Err(error) => {
            return Err(anyhow::anyhow!(error))
                .with_context(|| format!("reading {}", bluetooth_dir.display()));
        }
    };
    let mut entries = entries
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("enumerating {}", bluetooth_dir.display()))?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        ensure_handoff_fresh(handoff_valid_until)?;
        let destination = quarantine_dir.join(entry.file_name());
        fs::rename(entry.path(), &destination).with_context(|| {
            format!(
                "rotating {} into {}",
                entry.path().display(),
                destination.display()
            )
        })?;
        ensure_handoff_fresh(handoff_valid_until)?;
    }

    ensure_handoff_fresh(handoff_valid_until)?;
    if fs::read_dir(&bluetooth_dir)
        .with_context(|| format!("verifying {}", bluetooth_dir.display()))?
        .next()
        .transpose()?
        .is_some()
    {
        anyhow::bail!(
            "{} received new entries during Bluetooth key rotation",
            bluetooth_dir.display()
        );
    }
    sync_directory(&bluetooth_dir)?;
    ensure_handoff_fresh(handoff_valid_until)?;
    sync_directory(&quarantine_dir)?;
    ensure_handoff_fresh(handoff_valid_until)?;
    sync_directory(data_dir)?;
    ensure_handoff_fresh(handoff_valid_until)
}

/// Install a marker outside the integration directory cleared by shared reset
/// logic. A restart while this marker exists blocks adoption of every unknown
/// paired BlueZ object.
pub fn install_hue_ble_factory_reset_block(data_dir: impl AsRef<Path>) -> Result<()> {
    rhythm_hue::ble::HueBleDeviceStore::install_paired_orphan_adoption_block(data_dir)
}

/// Stop BlueZ immediately before platform cleanup removes adapter-bound link
/// keys. Shared hub shutdown has already completed, so a failed stop leaves
/// the durable safety marker in place and never races a live daemon against
/// the on-disk scrub.
pub fn stop_bluetoothd_for_factory_reset(handoff_valid_until: Option<Instant>) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        ensure_handoff_fresh(handoff_valid_until)?;
        let command_deadline = bounded_stop_deadline(handoff_valid_until)?;
        let mut stop_command = std::process::Command::new("/etc/init.d/S40bluetoothd");
        stop_command.arg("stop");
        let stop_result =
            run_command_until(&mut stop_command, command_deadline, "stopping bluetoothd");
        match stop_result {
            Ok(status) if status.success() => {}
            Ok(status) => log::warn!(
                target: "sys",
                "bluetoothd init script returned {status} during factory reset"
            ),
            Err(error) => log::warn!(
                target: "sys",
                "Failed to stop bluetoothd before factory reset: {error}"
            ),
        }

        if bluetoothd_is_running()? {
            log::warn!(
                target: "sys",
                "bluetoothd remained alive after its init script; terminating it before clearing link keys"
            );
            let mut term_command = std::process::Command::new("killall");
            term_command.args(["-TERM", "bluetoothd"]);
            let term_result = run_command_until(
                &mut term_command,
                command_deadline,
                "terminating bluetoothd",
            );
            if !term_result.as_ref().is_ok_and(|status| status.success()) {
                log::warn!(
                    target: "sys",
                    "Failed to terminate bluetoothd cleanly: {term_result:?}"
                );
            }
            sleep_until_or_deadline(Duration::from_millis(200), command_deadline)?;
        }
        if bluetoothd_is_running()? {
            let mut kill_command = std::process::Command::new("killall");
            kill_command.args(["-KILL", "bluetoothd"]);
            let kill_result =
                run_command_until(&mut kill_command, command_deadline, "killing bluetoothd");
            if !kill_result.as_ref().is_ok_and(|status| status.success()) {
                log::warn!(
                    target: "sys",
                    "Failed to kill bluetoothd before clearing link keys: {kill_result:?}"
                );
            }
        }
        if bluetoothd_is_running()? {
            anyhow::bail!("bluetoothd is still running after TERM/KILL attempts");
        }
        ensure_handoff_fresh(handoff_valid_until)?;
    }
    #[cfg(not(target_os = "linux"))]
    ensure_handoff_fresh(handoff_valid_until)?;
    Ok(())
}

fn ensure_handoff_fresh(valid_until: Option<Instant>) -> Result<()> {
    if valid_until.is_some_and(|deadline| Instant::now() >= deadline) {
        anyhow::bail!(
            "Hue Bluetooth replacement-pairing windows expired before active adapter keys were fully rotated"
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn bounded_stop_deadline(handoff_valid_until: Option<Instant>) -> Result<Instant> {
    let local_deadline = Instant::now() + BLUETOOTHD_STOP_TIMEOUT;
    let deadline = handoff_valid_until
        .and_then(|deadline| deadline.checked_sub(BOND_ROTATION_RESERVE))
        .map_or(local_deadline, |handoff_deadline| {
            local_deadline.min(handoff_deadline)
        });
    if Instant::now() >= deadline {
        anyhow::bail!("Not enough fresh Hue handoff time remains to stop bluetoothd safely");
    }
    Ok(deadline)
}

#[cfg(target_os = "linux")]
fn run_command_until(
    command: &mut std::process::Command,
    deadline: Instant,
    operation: &str,
) -> Result<std::process::ExitStatus> {
    let mut child = command
        .spawn()
        .with_context(|| format!("{operation}: spawning command"))?;
    loop {
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("{operation}: waiting for command"))?
        {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("{operation} exceeded the bounded Hue handoff deadline");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(target_os = "linux")]
fn sleep_until_or_deadline(duration: Duration, deadline: Instant) -> Result<()> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining < duration {
        anyhow::bail!("bluetoothd stop exceeded the bounded Hue handoff deadline");
    }
    std::thread::sleep(duration);
    Ok(())
}

#[cfg(target_os = "linux")]
fn bluetoothd_is_running() -> Result<bool> {
    process_is_running_in(Path::new("/proc"), "bluetoothd")
}

fn process_is_running_in(proc_root: &Path, process_name: &str) -> Result<bool> {
    let entries =
        fs::read_dir(proc_root).with_context(|| format!("reading {}", proc_root.display()))?;
    for entry in entries {
        let entry = entry
            .with_context(|| format!("enumerating process entries in {}", proc_root.display()))?;
        if !entry
            .file_name()
            .to_string_lossy()
            .chars()
            .all(|character| character.is_ascii_digit())
        {
            continue;
        }
        match fs::read_to_string(entry.path().join("comm")) {
            Ok(name) if name.trim() == process_name => return Ok(true),
            Ok(_) => {}
            // Processes may exit between listing /proc and reading comm.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(anyhow::anyhow!(error)).with_context(|| {
                    format!("reading process name for {}", entry.path().display())
                });
            }
        }
    }
    Ok(false)
}

fn detect_current_slot(boot_mount: &Path) -> ApplianceBootSlot {
    match fs::read_to_string(PROC_CMDLINE) {
        Ok(cmdline) => appliance_boot_slot_from_cmdline(&cmdline).unwrap_or_else(|| {
            boot_slot_from_bootstate(boot_mount).unwrap_or(ApplianceBootSlot::A)
        }),
        Err(_) => boot_slot_from_bootstate(boot_mount).unwrap_or(ApplianceBootSlot::A),
    }
}

fn appliance_boot_slot_from_cmdline(cmdline: &str) -> Option<ApplianceBootSlot> {
    for token in cmdline.split_whitespace() {
        match token {
            "root=/dev/mmcblk0p2" => return Some(ApplianceBootSlot::A),
            "root=/dev/mmcblk0p3" => return Some(ApplianceBootSlot::B),
            _ => {}
        }
    }
    None
}

fn boot_slot_from_bootstate(boot_mount: &Path) -> Option<ApplianceBootSlot> {
    let primary = boot_mount.join(BOOTSTATE_FILE);
    let backup = boot_mount.join(BOOTSTATE_BACKUP_FILE);

    if let Some(body) = bootstate::read_with_backup(&primary, &backup) {
        return parse_slot_from_bootstate_body(&body);
    }

    // Legacy fallback for units that haven't written a hashed bootstate yet
    // (first boot after upgrade). A file with a hash header but bad hash is
    // treated as corrupted and not parsed.
    let raw = fs::read_to_string(&primary).ok()?;
    if raw.starts_with(bootstate::HASH_HEADER_PREFIX) {
        return None;
    }
    parse_slot_from_bootstate_body(&raw)
}

fn parse_slot_from_bootstate_body(contents: &str) -> Option<ApplianceBootSlot> {
    for line in contents.lines() {
        if let Some(value) = line
            .strip_prefix("RHYTHM_ACTIVE_SLOT=")
            .or_else(|| line.strip_prefix("RHYTHM_LAST_GOOD_SLOT="))
        {
            return match value.trim() {
                "a" => Some(ApplianceBootSlot::A),
                "b" => Some(ApplianceBootSlot::B),
                _ => None,
            };
        }
    }
    None
}

fn write_bootstate(
    primary: &Path,
    backup: &Path,
    slot: ApplianceBootSlot,
    current_version: &str,
) -> Result<()> {
    let body = format!(
        "RHYTHM_ACTIVE_SLOT={slot}\n\
RHYTHM_LAST_GOOD_SLOT={slot}\n\
RHYTHM_PENDING_SLOT=\n\
RHYTHM_PENDING_VERSION=\n\
RHYTHM_ACTIVE_VERSION={version}\n\
RHYTHM_BOOT_STATUS=idle\n\
RHYTHM_LAST_UPDATE_EPOCH_MS=\n\
RHYTHM_LAST_ROLLBACK_SLOT=\n\
RHYTHM_LAST_ROLLBACK_VERSION=\n\
RHYTHM_LAST_ROLLBACK_EPOCH_MS=\n",
        slot = slot.as_str(),
        version = current_version
    );
    if let Some(parent) = primary.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    bootstate::write_with_backup(primary, backup, &body)
        .map_err(|e| anyhow::anyhow!("writing bootstate: {}", e))
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(anyhow::anyhow!(error)).with_context(|| format!("removing {}", path.display()))
        }
    }
}

fn remove_dir_if_exists(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(anyhow::anyhow!(error)).with_context(|| format!("removing {}", path.display()))
        }
    }
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(path)
        .with_context(|| format!("opening {} for sync", path.display()))?
        .sync_all()
        .with_context(|| format!("syncing {}", path.display()))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let unique = std::env::temp_dir().join(format!(
            "rhythm-linux-appliance-{name}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&unique).unwrap();
        unique
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn process_scan_detects_named_processes() {
        let root = temp_root("process-scan");
        fs::create_dir_all(root.join("101")).unwrap();
        fs::create_dir_all(root.join("202")).unwrap();
        fs::create_dir_all(root.join("not-a-pid")).unwrap();
        fs::write(root.join("101").join("comm"), "other-daemon\n").unwrap();
        fs::write(root.join("202").join("comm"), "bluetoothd\n").unwrap();
        fs::write(root.join("not-a-pid").join("comm"), "bluetoothd\n").unwrap();

        assert!(process_is_running_in(&root, "bluetoothd").unwrap());
        assert!(!process_is_running_in(&root, "missing-daemon").unwrap());
        cleanup(&root);
    }

    #[test]
    fn process_scan_errors_instead_of_assuming_the_daemon_stopped() {
        let root = temp_root("process-scan-error");
        let not_a_proc_dir = root.join("proc");
        fs::write(&not_a_proc_dir, "not a directory").unwrap();

        let error = process_is_running_in(&not_a_proc_dir, "bluetoothd")
            .expect_err("an unreadable process table must fail closed");

        assert!(format!("{error:#}").contains("reading"));
        cleanup(&root);
    }

    #[test]
    fn scrub_paths_clears_appliance_artifacts_and_resets_bootstate() {
        let root = temp_root("factory-reset");
        let data_dir = root.join("data");
        let log_dir = data_dir.join("log");
        let ota_dir = data_dir.join("ota");
        let bluetooth_dir = data_dir.join(BLUETOOTH_DIR);
        let boot_mount = root.join("boot");
        fs::create_dir_all(&log_dir).unwrap();
        fs::create_dir_all(&ota_dir).unwrap();
        fs::create_dir_all(&bluetooth_dir).unwrap();
        fs::create_dir_all(&boot_mount).unwrap();

        fs::write(log_dir.join("rhythm-server.log"), "old log").unwrap();
        fs::write(log_dir.join("wifi.log.1"), "old wifi log").unwrap();
        fs::write(ota_dir.join("rootfs.ext2.gz.download"), "ota").unwrap();
        fs::write(ota_dir.join("bootstate.env"), "stale").unwrap();
        let adapter_dir = bluetooth_dir.join("AA:BB:CC:DD:EE:FF");
        let device_dir = adapter_dir.join("11:22:33:44:55:66");
        fs::create_dir_all(&device_dir).unwrap();
        fs::create_dir_all(adapter_dir.join("cache")).unwrap();
        fs::write(adapter_dir.join("settings"), "adapter settings").unwrap();
        fs::write(device_dir.join("info"), "link key = secret").unwrap();
        fs::write(
            adapter_dir.join("cache").join("11:22:33:44:55:66"),
            "cached device",
        )
        .unwrap();
        fs::write(bluetooth_dir.join("top-level-marker"), "also secret").unwrap();
        rhythm_hue::ble::HueBleDeviceStore::persist_factory_reset_plan(
            &data_dir,
            &rhythm_hue::ble::HueBleFactoryResetPlan::default(),
        )
        .unwrap();
        fs::write(boot_mount.join(CMDLINE_BACKUP_FILE), "backup").unwrap();
        fs::write(
            boot_mount.join(BOOTSTATE_FILE),
            "RHYTHM_ACTIVE_SLOT=a\nRHYTHM_LAST_GOOD_SLOT=a\nRHYTHM_LAST_UPDATE_EPOCH_MS=123\n",
        )
        .unwrap();

        let paths = FactoryResetPaths {
            data_dir: data_dir.clone(),
            log_dir: log_dir.clone(),
            boot_mount: boot_mount.clone(),
        };
        scrub_paths(&paths, ApplianceBootSlot::B, "1.2.3").unwrap();

        assert!(
            !log_dir.exists(),
            "factory reset should remove appliance logs"
        );
        assert!(
            ota_dir.exists(),
            "factory reset should recreate the OTA dir"
        );
        assert!(
            fs::read_dir(&ota_dir).unwrap().next().is_none(),
            "factory reset should clear OTA staging contents"
        );
        assert!(
            bluetooth_dir.exists() && fs::read_dir(&bluetooth_dir).unwrap().next().is_none(),
            "factory reset should atomically detach every persistent Bluetooth adapter tree"
        );
        assert!(
            !data_dir.join(BLUETOOTH_QUARANTINE_DIR).exists(),
            "detached Bluetooth keys should be deleted after the active tree is empty"
        );
        assert!(
            !data_dir.join(HUE_BLE_DIR).exists(),
            "factory reset should clear Hue BLE metadata after bonds are gone"
        );
        assert!(
            !data_dir.join(".hue_ble_factory_reset_plan.json").exists(),
            "factory reset should clear the completed durable Hue BLE release plan"
        );
        assert!(
            !rhythm_hue::ble::HueBleDeviceStore::load(&data_dir)
                .unwrap()
                .blocks_paired_orphan_adoption(),
            "a confirmed bond scrub should clear the global reset block"
        );
        assert!(
            !boot_mount.join(CMDLINE_BACKUP_FILE).exists(),
            "factory reset should remove OTA cmdline backups"
        );
        let expected_body = concat!(
            "RHYTHM_ACTIVE_SLOT=b\n",
            "RHYTHM_LAST_GOOD_SLOT=b\n",
            "RHYTHM_PENDING_SLOT=\n",
            "RHYTHM_PENDING_VERSION=\n",
            "RHYTHM_ACTIVE_VERSION=1.2.3\n",
            "RHYTHM_BOOT_STATUS=idle\n",
            "RHYTHM_LAST_UPDATE_EPOCH_MS=\n",
            "RHYTHM_LAST_ROLLBACK_SLOT=\n",
            "RHYTHM_LAST_ROLLBACK_VERSION=\n",
            "RHYTHM_LAST_ROLLBACK_EPOCH_MS=\n",
        );

        let primary = fs::read_to_string(boot_mount.join(BOOTSTATE_FILE)).unwrap();
        let backup = fs::read_to_string(boot_mount.join(BOOTSTATE_BACKUP_FILE)).unwrap();
        assert_eq!(
            bootstate::verify_and_extract(&primary).as_deref(),
            Some(expected_body)
        );
        assert_eq!(
            bootstate::verify_and_extract(&backup).as_deref(),
            Some(expected_body),
            "factory reset must sync the backup copy alongside the primary"
        );

        cleanup(&root);
    }

    #[test]
    fn failed_bluetooth_scrub_retains_the_global_orphan_block() {
        let root = temp_root("factory-reset-bluetooth-failure");
        let data_dir = root.join("data");
        let boot_mount = root.join("boot");
        fs::create_dir_all(&data_dir).unwrap();
        fs::create_dir_all(&boot_mount).unwrap();
        fs::write(data_dir.join(BLUETOOTH_DIR), "not a directory").unwrap();
        let paths = FactoryResetPaths {
            data_dir: data_dir.clone(),
            log_dir: data_dir.join(LOG_DIR),
            boot_mount,
        };

        let error = scrub_paths(&paths, ApplianceBootSlot::A, "1.2.3")
            .expect_err("a bond database that cannot be read must fail reset cleanup");

        assert!(format!("{error:#}").contains(BLUETOOTH_DIR));
        assert!(
            rhythm_hue::ble::HueBleDeviceStore::load(&data_dir)
                .unwrap()
                .blocks_paired_orphan_adoption(),
            "the durable block must survive a failed bond scrub and process restart"
        );
        cleanup(&root);
    }

    #[test]
    fn detached_key_cleanup_failure_retains_reset_plan_and_orphan_block() {
        let root = temp_root("factory-reset-detached-cleanup-failure");
        let data_dir = root.join("data");
        let bluetooth_dir = data_dir.join(BLUETOOTH_DIR);
        let adapter_dir = bluetooth_dir.join("AA:BB:CC:DD:EE:FF");
        let boot_mount = root.join("boot");
        fs::create_dir_all(&adapter_dir).unwrap();
        fs::create_dir_all(&boot_mount).unwrap();
        fs::write(adapter_dir.join("settings"), "link key = secret").unwrap();
        fs::write(data_dir.join(HUE_BLE_DIR), "not a directory").unwrap();
        rhythm_hue::ble::HueBleDeviceStore::persist_factory_reset_plan(
            &data_dir,
            &rhythm_hue::ble::HueBleFactoryResetPlan::default(),
        )
        .unwrap();
        let paths = FactoryResetPaths {
            data_dir: data_dir.clone(),
            log_dir: data_dir.join(LOG_DIR),
            boot_mount,
        };

        prepare_paths(&paths, ApplianceBootSlot::A, "1.2.3").unwrap();
        let error = commit_paths(&data_dir, None)
            .expect_err("failed detached-key cleanup must not commit reset metadata");

        assert!(format!("{error:#}").contains(HUE_BLE_DIR));
        assert!(
            fs::read_dir(&bluetooth_dir).unwrap().next().is_none(),
            "active BlueZ storage must already be empty after atomic rotation"
        );
        assert_eq!(
            fs::read_to_string(
                data_dir
                    .join(BLUETOOTH_QUARANTINE_DIR)
                    .join("AA:BB:CC:DD:EE:FF")
                    .join("settings")
            )
            .unwrap(),
            "link key = secret",
            "a failed recursive cleanup may retain keys only in the inactive quarantine"
        );
        assert!(
            data_dir.join(".hue_ble_factory_reset_plan.json").exists(),
            "the durable release plan must survive until detached keys are deleted"
        );
        assert!(
            data_dir.join(".hue_ble_factory_reset_pending").exists(),
            "the global orphan-adoption block must survive until detached keys are deleted"
        );
        cleanup(&root);
    }

    #[test]
    fn non_bluetooth_prepare_failure_leaves_every_bond_key_intact() {
        let root = temp_root("factory-reset-prepare-failure");
        let data_dir = root.join("data");
        let bluetooth_dir = data_dir.join(BLUETOOTH_DIR);
        let boot_mount = root.join("boot-as-file");
        fs::create_dir_all(&bluetooth_dir).unwrap();
        fs::write(bluetooth_dir.join("link-key"), "secret").unwrap();
        fs::write(&boot_mount, "not a directory").unwrap();
        let paths = FactoryResetPaths {
            data_dir: data_dir.clone(),
            log_dir: data_dir.join(LOG_DIR),
            boot_mount,
        };

        prepare_paths(&paths, ApplianceBootSlot::A, "1.2.3")
            .expect_err("unwritable boot state must stop before the key boundary");

        assert_eq!(
            fs::read_to_string(bluetooth_dir.join("link-key")).unwrap(),
            "secret"
        );
        assert!(
            rhythm_hue::ble::HueBleDeviceStore::load(&data_dir)
                .unwrap()
                .blocks_paired_orphan_adoption(),
            "the durable reset block remains fail-closed for retry"
        );
        cleanup(&root);
    }

    #[test]
    fn expired_handoff_deadline_never_starts_bond_rotation() {
        let root = temp_root("factory-reset-expired-handoff");
        let data_dir = root.join("data");
        let bluetooth_dir = data_dir.join(BLUETOOTH_DIR);
        let key_path = bluetooth_dir
            .join("AA:BB:CC:DD:EE:FF")
            .join("11:22:33:44:55:66")
            .join("info");
        fs::create_dir_all(key_path.parent().unwrap()).unwrap();
        fs::write(&key_path, "link key = secret").unwrap();
        prepare_bluetooth_quarantine(&data_dir).unwrap();

        let error = commit_hue_ble_factory_reset_state(
            data_dir.to_str().unwrap(),
            Some(Instant::now() - Duration::from_millis(1)),
        )
        .expect_err("an expired vendor handoff must fail before key deletion");

        assert!(format!("{error:#}").contains("expired"));
        assert_eq!(fs::read_to_string(&key_path).unwrap(), "link key = secret");
        assert!(
            fs::read_dir(data_dir.join(BLUETOOTH_QUARANTINE_DIR))
                .unwrap()
                .next()
                .is_none(),
            "an expired handoff must leave even the prepared quarantine unused"
        );
        cleanup(&root);
    }

    #[test]
    fn boot_slot_from_bootstate_falls_back_to_backup_on_corruption() {
        let root = temp_root("bootstate-fallback");
        let boot_mount = root.join("boot");
        fs::create_dir_all(&boot_mount).unwrap();

        write_bootstate(
            &boot_mount.join(BOOTSTATE_FILE),
            &boot_mount.join(BOOTSTATE_BACKUP_FILE),
            ApplianceBootSlot::B,
            "1.2.3",
        )
        .unwrap();

        // Corrupt the primary: a flipped byte breaks the hash but keeps the
        // header line intact, mimicking silent VFAT corruption.
        let primary = boot_mount.join(BOOTSTATE_FILE);
        let raw = fs::read_to_string(&primary).unwrap();
        let mut bytes = raw.into_bytes();
        let body_start = bytes.iter().position(|&b| b == b'\n').unwrap() + 1;
        bytes[body_start] ^= 0x01;
        fs::write(&primary, bytes).unwrap();

        assert_eq!(
            boot_slot_from_bootstate(&boot_mount),
            Some(ApplianceBootSlot::B)
        );

        cleanup(&root);
    }

    #[test]
    fn scrub_paths_is_idempotent() {
        let root = temp_root("factory-reset-idempotent");
        let data_dir = root.join("data");
        let log_dir = data_dir.join("log");
        let boot_mount = root.join("boot");
        fs::create_dir_all(&log_dir).unwrap();
        fs::create_dir_all(&boot_mount).unwrap();

        let paths = FactoryResetPaths {
            data_dir: data_dir.clone(),
            log_dir: log_dir.clone(),
            boot_mount: boot_mount.clone(),
        };

        // First reset against a populated state.
        fs::write(log_dir.join("rhythm-server.log"), "old").unwrap();
        scrub_paths(&paths, ApplianceBootSlot::A, "1.2.3").unwrap();
        let first_primary = fs::read_to_string(boot_mount.join(BOOTSTATE_FILE)).unwrap();
        let first_backup = fs::read_to_string(boot_mount.join(BOOTSTATE_BACKUP_FILE)).unwrap();

        // Second reset against the post-reset state should produce the same
        // result (modulo timestamps the format doesn't include) without any
        // partial / orphan files left behind.
        scrub_paths(&paths, ApplianceBootSlot::A, "1.2.3").unwrap();
        let second_primary = fs::read_to_string(boot_mount.join(BOOTSTATE_FILE)).unwrap();
        let second_backup = fs::read_to_string(boot_mount.join(BOOTSTATE_BACKUP_FILE)).unwrap();

        assert_eq!(first_primary, second_primary);
        assert_eq!(first_backup, second_backup);
        assert!(
            !boot_mount.join("rhythm-bootstate.env.tmp").exists(),
            "no stale .tmp file on primary after repeat reset"
        );
        assert!(
            !boot_mount.join("rhythm-bootstate.env.bak.tmp").exists(),
            "no stale .tmp file on backup after repeat reset"
        );
        let ota_dir = data_dir.join(OTA_DIR);
        assert!(ota_dir.exists());
        assert!(fs::read_dir(&ota_dir).unwrap().next().is_none());

        cleanup(&root);
    }

    #[test]
    fn boot_slot_from_bootstate_rejects_hashed_file_with_bad_hash() {
        let root = temp_root("bootstate-bad-hash");
        let boot_mount = root.join("boot");
        fs::create_dir_all(&boot_mount).unwrap();

        // Write a header-formatted file with a body that doesn't match.
        fs::write(
            boot_mount.join(BOOTSTATE_FILE),
            "# RHYTHM_BOOTSTATE_HASH=deadbeef\nRHYTHM_ACTIVE_SLOT=a\n",
        )
        .unwrap();
        // No backup. Result should be None — not a silent legacy-fallback read.
        assert_eq!(boot_slot_from_bootstate(&boot_mount), None);

        cleanup(&root);
    }

    #[test]
    fn appliance_boot_slot_from_cmdline_detects_slot() {
        assert_eq!(
            appliance_boot_slot_from_cmdline("console=tty1 root=/dev/mmcblk0p2 rw"),
            Some(ApplianceBootSlot::A)
        );
        assert_eq!(
            appliance_boot_slot_from_cmdline("console=tty1 root=/dev/mmcblk0p3 rw"),
            Some(ApplianceBootSlot::B)
        );
        assert_eq!(appliance_boot_slot_from_cmdline("console=tty1 rw"), None);
    }

    #[test]
    fn boot_slot_from_bootstate_uses_active_slot_fallback() {
        let root = temp_root("bootstate-slot");
        let boot_mount = root.join("boot");
        fs::create_dir_all(&boot_mount).unwrap();
        fs::write(
            boot_mount.join(BOOTSTATE_FILE),
            "RHYTHM_ACTIVE_SLOT=b\nRHYTHM_LAST_GOOD_SLOT=a\n",
        )
        .unwrap();

        assert_eq!(
            boot_slot_from_bootstate(&boot_mount),
            Some(ApplianceBootSlot::B)
        );

        cleanup(&root);
    }
}
