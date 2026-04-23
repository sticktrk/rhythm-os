use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const BOOT_MOUNT: &str = "/boot";
const PROC_CMDLINE: &str = "/proc/cmdline";
const BOOTSTATE_FILE: &str = "rhythm-bootstate.env";
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
    let path = boot_mount.join(BOOTSTATE_FILE);
    let contents = fs::read_to_string(path).ok()?;
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

fn write_bootstate(path: &Path, slot: ApplianceBootSlot, current_version: &str) -> Result<()> {
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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, body).with_context(|| format!("writing {}", path.display()))
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
        assert_eq!(
            fs::read_to_string(boot_mount.join(BOOTSTATE_FILE)).unwrap(),
            concat!(
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
            )
        );

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
