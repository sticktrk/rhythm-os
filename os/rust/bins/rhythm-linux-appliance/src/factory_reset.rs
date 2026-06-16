use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rhythm_server::bootstate;

const BOOT_MOUNT: &str = "/boot";
const PROC_CMDLINE: &str = "/proc/cmdline";
const BOOTSTATE_FILE: &str = "rhythm-bootstate.env";
const BOOTSTATE_BACKUP_FILE: &str = "rhythm-bootstate.env.bak";
const CMDLINE_BACKUP_FILE: &str = "cmdline.txt.bak";
const OTA_DIR: &str = "ota";
const LOG_DIR: &str = "log";

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

pub fn scrub_appliance_factory_reset_state(data_dir: &str, current_version: &str) -> Result<()> {
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
    scrub_paths(&paths, slot, current_version)
}

fn scrub_paths(
    paths: &FactoryResetPaths,
    slot: ApplianceBootSlot,
    current_version: &str,
) -> Result<()> {
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
    fn scrub_paths_clears_appliance_artifacts_and_resets_bootstate() {
        let root = temp_root("factory-reset");
        let data_dir = root.join("data");
        let log_dir = data_dir.join("log");
        let ota_dir = data_dir.join("ota");
        let boot_mount = root.join("boot");
        fs::create_dir_all(&log_dir).unwrap();
        fs::create_dir_all(&ota_dir).unwrap();
        fs::create_dir_all(&boot_mount).unwrap();

        fs::write(log_dir.join("rhythm-server.log"), "old log").unwrap();
        fs::write(log_dir.join("wifi.log.1"), "old wifi log").unwrap();
        fs::write(ota_dir.join("rootfs.ext2.gz.download"), "ota").unwrap();
        fs::write(ota_dir.join("bootstate.env"), "stale").unwrap();
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
