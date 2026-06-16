//! Persisted Matter fabric identity for the local commissioner.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const FABRIC_IDENTITY_FILENAME: &str = "fabric-identity.json";

const FABRIC_IDENTITY_SCHEMA_VERSION: u32 = 1;
const IPK_BYTES: usize = 16;

/// Local commissioner identity that must survive process restarts and backups.
///
/// `label` is Rhythm's human/config credential label. `operational_fabric_id`
/// and `ipk_hex` are the Matter operational identity values used by the native
/// CHIP controller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatterFabricIdentity {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub label: String,
    pub operational_fabric_id: u64,
    pub ipk_hex: String,
}

impl MatterFabricIdentity {
    pub fn load_or_create(data_path: impl AsRef<Path>, label: &str) -> Result<Self> {
        Self::load_or_create_guarded(data_path, label, &[])
    }

    pub fn load_or_create_with_controller_storage(
        data_path: impl AsRef<Path>,
        label: &str,
        controller_storage_path: impl AsRef<Path>,
    ) -> Result<Self> {
        let controller_storage_paths = [controller_storage_path.as_ref().to_path_buf()];
        Self::load_or_create_guarded(data_path, label, &controller_storage_paths)
    }

    pub fn load_or_create_with_controller_storage_paths(
        data_path: impl AsRef<Path>,
        label: &str,
        controller_storage_paths: &[PathBuf],
    ) -> Result<Self> {
        Self::load_or_create_guarded(data_path, label, controller_storage_paths)
    }

    fn load_or_create_guarded(
        data_path: impl AsRef<Path>,
        label: &str,
        controller_storage_paths: &[PathBuf],
    ) -> Result<Self> {
        if label.trim().is_empty() {
            anyhow::bail!("Matter fabric label must not be empty");
        }

        let path = identity_path(data_path.as_ref());
        match fs::read_to_string(&path) {
            Ok(json) => {
                let identity: Self = serde_json::from_str(&json)
                    .with_context(|| format!("decoding {}", path.display()))?;
                identity.validate()?;
                if identity.label != label {
                    anyhow::bail!(
                        "Matter fabric label mismatch: persisted '{}' but configured '{}'",
                        identity.label,
                        label
                    );
                }
                Ok(identity)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Some(storage_path) =
                    controller_storage_paths.iter().find(|path| path.is_file())
                {
                    anyhow::bail!(
                        "Matter controller storage exists at {} but {} is missing; reset Matter controller state or restore the matching fabric identity before starting",
                        storage_path.display(),
                        path.display()
                    );
                }
                let identity = Self::create(label)?;
                write_identity_atomic(&path, &identity)?;
                Ok(identity)
            }
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn identity_path(data_path: impl AsRef<Path>) -> PathBuf {
        identity_path(data_path.as_ref())
    }

    pub fn ipk_bytes(&self) -> Result<[u8; IPK_BYTES]> {
        decode_hex_16(&self.ipk_hex)
    }

    fn create(label: &str) -> Result<Self> {
        let mut fabric_id_bytes = [0u8; 8];
        let operational_fabric_id = loop {
            fill_random(&mut fabric_id_bytes)?;
            let id = u64::from_be_bytes(fabric_id_bytes);
            if id != 0 {
                break id;
            }
        };

        let mut ipk = [0u8; IPK_BYTES];
        fill_random(&mut ipk)?;

        Ok(Self {
            schema_version: FABRIC_IDENTITY_SCHEMA_VERSION,
            label: label.to_string(),
            operational_fabric_id,
            ipk_hex: encode_hex(&ipk),
        })
    }

    fn validate(&self) -> Result<()> {
        if self.schema_version != FABRIC_IDENTITY_SCHEMA_VERSION {
            anyhow::bail!(
                "Unsupported Matter fabric identity schema version {}",
                self.schema_version
            );
        }
        if self.label.trim().is_empty() {
            anyhow::bail!("Matter fabric identity label must not be empty");
        }
        if self.operational_fabric_id == 0 {
            anyhow::bail!("Matter operational fabric id must be non-zero");
        }
        self.ipk_bytes()?;
        Ok(())
    }
}

fn default_schema_version() -> u32 {
    FABRIC_IDENTITY_SCHEMA_VERSION
}

fn identity_path(data_path: &Path) -> PathBuf {
    data_path.join(FABRIC_IDENTITY_FILENAME)
}

fn write_identity_atomic(path: &Path, identity: &MatterFabricIdentity) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }

    let tmp = path.with_file_name(format!(
        "{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(FABRIC_IDENTITY_FILENAME)
    ));
    let json = serde_json::to_string_pretty(identity)?;

    {
        let mut file = File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        file.write_all(json.as_bytes())
            .with_context(|| format!("writing {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("fsync {}", tmp.display()))?;
    }

    fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    if let Some(parent) = path.parent() {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

fn fill_random(bytes: &mut [u8]) -> Result<()> {
    let mut random = File::open("/dev/urandom").context("opening /dev/urandom")?;
    random
        .read_exact(bytes)
        .context("reading random bytes from /dev/urandom")
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn decode_hex_16(hex: &str) -> Result<[u8; IPK_BYTES]> {
    if hex.len() != IPK_BYTES * 2 {
        anyhow::bail!("Matter IPK must be {} hex characters", IPK_BYTES * 2);
    }

    let mut out = [0u8; IPK_BYTES];
    for (index, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
        let high = decode_hex_nibble(chunk[0])?;
        let low = decode_hex_nibble(chunk[1])?;
        out[index] = (high << 4) | low;
    }
    Ok(out)
}

fn decode_hex_nibble(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => anyhow::bail!("Matter IPK must be hex encoded"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(name: &str) -> PathBuf {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "rhythm_matter_fabric_{}_{}_{}",
            name,
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn load_or_create_persists_identity() {
        let dir = temp_dir("persist");
        let created = MatterFabricIdentity::load_or_create(&dir, "default").unwrap();
        assert_eq!(created.schema_version, FABRIC_IDENTITY_SCHEMA_VERSION);
        assert_eq!(created.label, "default");
        assert_ne!(created.operational_fabric_id, 0);
        assert_eq!(created.ipk_bytes().unwrap().len(), IPK_BYTES);

        let loaded = MatterFabricIdentity::load_or_create(&dir, "default").unwrap();
        assert_eq!(loaded, created);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_or_create_rejects_label_mismatch() {
        let dir = temp_dir("mismatch");
        MatterFabricIdentity::load_or_create(&dir, "default").unwrap();
        let err = MatterFabricIdentity::load_or_create(&dir, "other").unwrap_err();
        assert!(err.to_string().contains("label mismatch"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_or_create_rejects_invalid_ipk() {
        let dir = temp_dir("invalid-ipk");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            identity_path(&dir),
            r#"{"schema_version":1,"label":"default","operational_fabric_id":5,"ipk_hex":"abc"}"#,
        )
        .unwrap();

        let err = MatterFabricIdentity::load_or_create(&dir, "default").unwrap_err();
        assert!(err.to_string().contains("IPK"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_or_create_rejects_legacy_controller_storage_without_identity() {
        let dir = temp_dir("legacy-controller-storage");
        let controller_storage = dir.join("chip").join("controller-storage.json");
        fs::create_dir_all(controller_storage.parent().unwrap()).unwrap();
        fs::write(&controller_storage, "{}").unwrap();

        let err = MatterFabricIdentity::load_or_create_with_controller_storage(
            &dir,
            "default",
            &controller_storage,
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("controller storage exists"));
        assert!(message.contains("fabric-identity.json is missing"));
        assert!(!identity_path(&dir).exists());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_or_create_rejects_chip_ini_storage_without_identity() {
        let dir = temp_dir("chip-ini-controller-storage");
        let json_storage = dir.join("chip").join("controller-storage.json");
        let ini_storage = dir
            .join("chip")
            .join("chip_tool_config.controller-storage.ini");
        fs::create_dir_all(ini_storage.parent().unwrap()).unwrap();
        fs::write(&ini_storage, "chip storage").unwrap();

        let err = MatterFabricIdentity::load_or_create_with_controller_storage_paths(
            &dir,
            "default",
            &[json_storage, ini_storage.clone()],
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("controller storage exists"));
        assert!(message.contains(&ini_storage.display().to_string()));
        assert!(!identity_path(&dir).exists());

        let _ = fs::remove_dir_all(dir);
    }
}
