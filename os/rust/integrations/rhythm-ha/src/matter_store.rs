//! Integration-owned original label codes. Never included in portable exports or diagnostics.

use crate::matter::{validate_setup_code, MatterDevice};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::Path,
    sync::{Mutex, OnceLock},
};

pub const STORE_NAME: &str = "ha-setup-payloads.json";
const MAX_BYTES: u64 = 8 * 1024 * 1024;
const PENDING_TTL: i64 = 7 * 24 * 3600;
static LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Clone, Serialize, Deserialize)]
pub struct ConfirmedSession {
    pub fingerprint: String,
    pub expires_at: i64,
    /// Present only after HA acknowledged commissioning, never a temporary handoff code.
    pub original: Option<String>,
    pub device: Option<MatterDevice>,
}

#[derive(Serialize, Deserialize)]
struct Original {
    device: MatterDevice,
    code: String,
}

#[derive(Serialize, Deserialize)]
struct Store {
    schema_version: u32,
    installation_id: String,
    originals: BTreeMap<String, Original>,
    confirmed: BTreeMap<String, ConfirmedSession>,
}

fn write(dir: &Path, store: &Store) -> Result<()> {
    let temporary = dir.join(format!("{STORE_NAME}.tmp"));
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if temporary
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        anyhow::bail!("Private Matter store is a symbolic link");
    }
    let mut file = options.open(&temporary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(&serde_json::to_vec(store)?)?;
    file.sync_all()?;
    fs::rename(temporary, dir.join(STORE_NAME))?;
    fs::File::open(dir)?.sync_all()?;
    Ok(())
}

fn access<T>(
    dir: &str,
    installation: &str,
    change: impl FnOnce(&mut Store) -> Result<T>,
) -> Result<T> {
    let _lock = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("Private Matter store lock"))?;
    // `matter/` is already removed by current and previous supported factory
    // reset implementations. This is only storage, never a Matter driver.
    let directory = Path::new(dir).join("matter");
    if !directory.exists() {
        fs::create_dir(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        }
    }
    anyhow::ensure!(
        directory.symlink_metadata()?.is_dir(),
        "Invalid private Matter directory"
    );
    let dir = directory.as_path();
    let path = dir.join(STORE_NAME);
    let mut store = match path.symlink_metadata() {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.len() <= MAX_BYTES,
                "Invalid private Matter store"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                anyhow::ensure!(
                    metadata.permissions().mode() & 0o077 == 0,
                    "Private Matter store permissions require repair"
                );
            }
            let store: Store = serde_json::from_slice(&fs::read(&path)?)
                .context("Invalid private Matter store")?;
            anyhow::ensure!(
                store.schema_version == 1 && store.installation_id == installation,
                "Private Matter store identity or schema requires review"
            );
            store
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Store {
            schema_version: 1,
            installation_id: installation.to_owned(),
            originals: BTreeMap::new(),
            confirmed: BTreeMap::new(),
        },
        Err(e) => return Err(e.into()),
    };
    anyhow::ensure!(
        store.originals.len() <= 4096 && store.confirmed.len() <= 100,
        "Private Matter store exceeds limits"
    );
    store
        .confirmed
        .retain(|_, s| s.expires_at > chrono::Utc::now().timestamp());
    let value = change(&mut store)?;
    write(dir, &store)?;
    Ok(value)
}

pub fn confirmed(dir: &str, installation: &str, session: &str) -> Result<Option<ConfirmedSession>> {
    access(dir, installation, |s| Ok(s.confirmed.get(session).cloned()))
}

pub fn remember_success(
    dir: &str,
    installation: &str,
    session: &str,
    fingerprint: &str,
    original: Option<&str>,
) -> Result<()> {
    if let Some(code) = original {
        validate_setup_code(code)?;
    }
    access(dir, installation, |s| {
        if let Some(existing) = s.confirmed.get(session) {
            anyhow::ensure!(
                existing.fingerprint == fingerprint,
                "Pairing session identity changed"
            );
            return Ok(());
        }
        anyhow::ensure!(
            s.confirmed.len() < 100,
            "Private Matter receipt store is full"
        );
        s.confirmed.insert(
            session.to_owned(),
            ConfirmedSession {
                fingerprint: fingerprint.to_owned(),
                expires_at: chrono::Utc::now().timestamp() + PENDING_TTL,
                original: original.map(str::to_owned),
                device: None,
            },
        );
        Ok(())
    })
}

pub fn bind_session(
    dir: &str,
    installation: &str,
    session: &str,
    device: &MatterDevice,
) -> Result<()> {
    access(dir, installation, |s| {
        let record = s
            .confirmed
            .get_mut(session)
            .context("Confirmed original label code is unavailable or expired")?;
        if let Some(bound) = &record.device {
            anyhow::ensure!(
                bound.identity == device.identity,
                "This session is already bound to another device"
            );
            return Ok(());
        }
        let code = record
            .original
            .take()
            .context("This session has no original label code")?;
        anyhow::ensure!(
            s.originals.len() < 4096 || s.originals.contains_key(&device.device_id),
            "Original code store is full"
        );
        s.originals.insert(
            device.device_id.clone(),
            Original {
                device: device.clone(),
                code,
            },
        );
        record.device = Some(device.clone());
        Ok(())
    })
}

pub fn save_original(
    dir: &str,
    installation: &str,
    device: &MatterDevice,
    code: &str,
) -> Result<()> {
    validate_setup_code(code)?;
    access(dir, installation, |s| {
        anyhow::ensure!(
            s.originals.len() < 4096 || s.originals.contains_key(&device.device_id),
            "Original code store is full"
        );
        s.originals.insert(
            device.device_id.clone(),
            Original {
                device: device.clone(),
                code: code.to_owned(),
            },
        );
        Ok(())
    })
}

pub fn original(dir: &str, installation: &str, device: &MatterDevice) -> Result<Option<String>> {
    access(dir, installation, |s| {
        Ok(s.originals
            .get(&device.device_id)
            .filter(|entry| entry.device.identity == device.identity)
            .map(|entry| entry.code.clone()))
    })
}

pub fn forget(dir: &str, installation: &str, device: &MatterDevice) -> Result<()> {
    access(dir, installation, |s| {
        if s.originals
            .get(&device.device_id)
            .is_some_and(|e| e.device.identity == device.identity)
        {
            s.originals.remove(&device.device_id);
        }
        s.confirmed.retain(|_, r| {
            !r.device
                .as_ref()
                .is_some_and(|d| d.identity == device.identity)
        });
        Ok(())
    })
}

/// Only call with a complete, live registry from the loaded HA Matter entry.
/// External HA removals must also retire now-unrecoverable label secrets.
pub fn reconcile_devices(
    dir: &str,
    installation: &str,
    devices: &[MatterDevice],
    registry_ids: &[String],
) -> Result<()> {
    access(dir, installation, |s| {
        let remains = |bound: &MatterDevice| {
            registry_ids.contains(&bound.device_id)
                && devices
                    .iter()
                    .find(|d| d.device_id == bound.device_id)
                    .is_none_or(|current| current.identity == bound.identity)
        };
        s.originals.retain(|_, entry| remains(&entry.device));
        s.confirmed
            .retain(|_, entry| entry.device.as_ref().is_none_or(remains));
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_binding_permissions_identity_and_removal() {
        let dir = std::env::temp_dir().join(format!(
            "rhythm-ha-code-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.to_str().unwrap();
        let device = MatterDevice {
            device_id: "ha-id".into(),
            identity: "proof".into(),
            node_id: 1,
            name: "Light".into(),
            entity_ids: vec![],
            is_bridge: false,
            config_entry_id: "matter".into(),
        };
        remember_success(
            path,
            "installation",
            "session",
            "fingerprint",
            Some("12345678901"),
        )
        .unwrap();
        assert_eq!(
            confirmed(path, "installation", "session")
                .unwrap()
                .unwrap()
                .original
                .as_deref(),
            Some("12345678901")
        );
        bind_session(path, "installation", "session", &device).unwrap();
        assert!(confirmed(path, "installation", "session")
            .unwrap()
            .unwrap()
            .original
            .is_none());
        assert_eq!(
            original(path, "installation", &device).unwrap().as_deref(),
            Some("12345678901")
        );
        let mut replacement = device.clone();
        replacement.identity = "replacement".into();
        assert!(original(path, "installation", &replacement)
            .unwrap()
            .is_none());
        assert!(bind_session(path, "installation", "session", &replacement).is_err());
        reconcile_devices(path, "installation", &[], &[device.device_id.clone()]).unwrap();
        assert!(
            original(path, "installation", &device).unwrap().is_some(),
            "omitted metadata must preserve the code"
        );
        assert!(original(path, "other-installation", &device).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(dir.join("matter").join(STORE_NAME))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        forget(path, "installation", &device).unwrap();
        assert!(original(path, "installation", &device).unwrap().is_none());
        assert!(confirmed(path, "installation", "session")
            .unwrap()
            .is_none());
        remember_success(
            path,
            "installation",
            "expiring",
            "fingerprint",
            Some("12345678901"),
        )
        .unwrap();
        access(path, "installation", |s| {
            s.confirmed.get_mut("expiring").unwrap().expires_at = 0;
            Ok(())
        })
        .unwrap();
        assert!(confirmed(path, "installation", "expiring")
            .unwrap()
            .is_none());
        assert!(!fs::read_to_string(dir.join("matter").join(STORE_NAME))
            .unwrap()
            .contains("12345678901"));
        let future = fs::read_to_string(dir.join("matter").join(STORE_NAME))
            .unwrap()
            .replace("\"schema_version\":1", "\"schema_version\":99");
        fs::write(dir.join("matter").join(STORE_NAME), &future).unwrap();
        assert!(confirmed(path, "installation", "expiring").is_err());
        assert_eq!(
            fs::read_to_string(dir.join("matter").join(STORE_NAME)).unwrap(),
            future
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
