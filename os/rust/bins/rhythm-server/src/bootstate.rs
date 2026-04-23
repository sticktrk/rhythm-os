//! Durable boot-state file handling for appliance OTA + factory reset.
//!
//! The bootstate file lives on the VFAT /boot partition and is read by init
//! scripts that `source` it as a shell fragment. A power loss on VFAT can
//! silently corrupt a single file, so we:
//!
//! 1. Atomically write the primary file (write-tmp + fsync + rename + fsync
//!    parent directory).
//! 2. Keep a sibling backup copy that's written *first* so a mid-primary-write
//!    power loss still leaves a recoverable file.
//! 3. Embed a SHA-256 hash of the body as a shell-comment header line, so
//!    readers can detect silent corruption. Shell `source` ignores `#` lines,
//!    so existing init scripts keep working without change.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Shell-comment prefix for the hash line. Always the first line of a composed
/// bootstate file.
pub const HASH_HEADER_PREFIX: &str = "# RHYTHM_BOOTSTATE_HASH=";

/// Compose a bootstate body with a leading SHA-256 hash header.
pub fn compose(body: &str) -> Vec<u8> {
    let hash = hex_encode(&Sha256::digest(body.as_bytes()));
    let mut out = Vec::with_capacity(HASH_HEADER_PREFIX.len() + hash.len() + 1 + body.len());
    out.extend_from_slice(HASH_HEADER_PREFIX.as_bytes());
    out.extend_from_slice(hash.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(body.as_bytes());
    out
}

/// Validate the hash header of a composed bootstate file and return the body
/// (without the header). Returns `None` if the header is missing or the hash
/// doesn't match.
pub fn verify_and_extract(contents: &str) -> Option<String> {
    let mut parts = contents.splitn(2, '\n');
    let header = parts.next()?;
    let body = parts.next().unwrap_or("");
    let expected = header.strip_prefix(HASH_HEADER_PREFIX)?.trim();
    let actual = hex_encode(&Sha256::digest(body.as_bytes()));
    if actual.eq_ignore_ascii_case(expected) {
        Some(body.to_string())
    } else {
        None
    }
}

/// Write `body` to `primary` with a sibling backup at `backup`. Both writes
/// go through [`atomic_write_with_sync`] so a power loss at any point leaves
/// at least one valid copy on disk.
pub fn write_with_backup(primary: &Path, backup: &Path, body: &str) -> Result<(), String> {
    let composed = compose(body);
    // Write the backup first; if the primary write fails mid-flight the
    // backup is already good.
    atomic_write_with_sync(backup, &composed)?;
    atomic_write_with_sync(primary, &composed)?;
    Ok(())
}

/// Read a bootstate body, validating the hash. Falls back to the backup if
/// the primary is missing or its hash doesn't match. Returns the body (no
/// header) or `None` if both copies fail.
pub fn read_with_backup(primary: &Path, backup: &Path) -> Option<String> {
    if let Ok(raw) = fs::read_to_string(primary) {
        if let Some(body) = verify_and_extract(&raw) {
            return Some(body);
        }
    }
    if let Ok(raw) = fs::read_to_string(backup) {
        if let Some(body) = verify_and_extract(&raw) {
            return Some(body);
        }
    }
    None
}

/// Write `contents` to `path` durably: write to a sibling `.tmp`, fsync it,
/// atomically rename into place, then fsync the parent directory so the
/// rename itself survives a power loss. On VFAT the parent fsync is
/// best-effort but still valuable; on ext4 it's required.
pub fn atomic_write_with_sync(path: &Path, contents: &[u8]) -> Result<(), String> {
    let tmp = tmp_sibling_path(path);

    {
        let mut file =
            File::create(&tmp).map_err(|e| format!("Failed to create {}: {}", tmp.display(), e))?;
        file.write_all(contents)
            .map_err(|e| format!("Failed to write {}: {}", tmp.display(), e))?;
        file.sync_all()
            .map_err(|e| format!("Failed to sync {}: {}", tmp.display(), e))?;
    }

    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(format!(
            "Failed to rename {} -> {}: {}",
            tmp.display(),
            path.display(),
            e
        ));
    }

    if let Some(parent) = path.parent() {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }

    Ok(())
}

/// Derive the sibling `.tmp` path used by [`atomic_write_with_sync`].
pub fn tmp_sibling_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("rhythm");
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!("{}.tmp", file_name))
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{:02x}", byte);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rhythm-bootstate-{}-{}", name, nanos));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn compose_then_verify_roundtrips_body() {
        let body = "RHYTHM_ACTIVE_SLOT=a\nRHYTHM_LAST_GOOD_SLOT=a\n";
        let composed = compose(body);
        let contents = String::from_utf8(composed).unwrap();
        assert!(contents.starts_with(HASH_HEADER_PREFIX));
        assert_eq!(verify_and_extract(&contents).as_deref(), Some(body));
    }

    #[test]
    fn verify_rejects_tampered_body() {
        let body = "RHYTHM_ACTIVE_SLOT=a\n";
        let composed = String::from_utf8(compose(body)).unwrap();
        let tampered = composed.replace("SLOT=a", "SLOT=b");
        assert_ne!(composed, tampered);
        assert_eq!(verify_and_extract(&tampered), None);
    }

    #[test]
    fn verify_rejects_missing_header() {
        let body = "RHYTHM_ACTIVE_SLOT=a\n";
        assert_eq!(verify_and_extract(body), None);
    }

    #[test]
    fn verify_rejects_truncated_hash() {
        let body = "RHYTHM_ACTIVE_SLOT=a\n";
        let composed = String::from_utf8(compose(body)).unwrap();
        let (head, tail) = composed.split_once('\n').unwrap();
        let truncated = format!("{}cut\n{}", &head[..head.len() - 4], tail);
        assert_eq!(verify_and_extract(&truncated), None);
    }

    #[test]
    fn write_with_backup_creates_both_files() {
        let dir = unique_dir("write-with-backup");
        let primary = dir.join("rhythm-bootstate.env");
        let backup = dir.join("rhythm-bootstate.env.bak");
        let body = "RHYTHM_ACTIVE_SLOT=b\nRHYTHM_ACTIVE_VERSION=1.2.3\n";

        write_with_backup(&primary, &backup, body).unwrap();

        assert_eq!(read_with_backup(&primary, &backup).as_deref(), Some(body));
        let primary_raw = fs::read_to_string(&primary).unwrap();
        let backup_raw = fs::read_to_string(&backup).unwrap();
        assert_eq!(primary_raw, backup_raw);
        assert!(primary_raw.starts_with(HASH_HEADER_PREFIX));
    }

    #[test]
    fn read_with_backup_falls_back_on_corrupted_primary() {
        let dir = unique_dir("fallback");
        let primary = dir.join("rhythm-bootstate.env");
        let backup = dir.join("rhythm-bootstate.env.bak");
        let body = "RHYTHM_ACTIVE_SLOT=a\n";
        write_with_backup(&primary, &backup, body).unwrap();

        // Simulate silent VFAT corruption of the primary: truncate mid-line.
        fs::write(
            &primary,
            "# RHYTHM_BOOTSTATE_HASH=deadbeef\nRHYTHM_ACTIVE_SLOT=a\n",
        )
        .unwrap();

        assert_eq!(read_with_backup(&primary, &backup).as_deref(), Some(body));
    }

    #[test]
    fn read_with_backup_falls_back_when_primary_missing() {
        let dir = unique_dir("missing-primary");
        let primary = dir.join("rhythm-bootstate.env");
        let backup = dir.join("rhythm-bootstate.env.bak");
        let body = "RHYTHM_ACTIVE_SLOT=a\n";
        write_with_backup(&primary, &backup, body).unwrap();
        fs::remove_file(&primary).unwrap();

        assert_eq!(read_with_backup(&primary, &backup).as_deref(), Some(body));
    }

    #[test]
    fn read_with_backup_returns_none_when_both_corrupt() {
        let dir = unique_dir("both-corrupt");
        let primary = dir.join("rhythm-bootstate.env");
        let backup = dir.join("rhythm-bootstate.env.bak");
        fs::write(&primary, "garbage without header").unwrap();
        fs::write(&backup, "also garbage").unwrap();

        assert_eq!(read_with_backup(&primary, &backup), None);
    }

    #[test]
    fn atomic_write_cleans_up_tmp_on_success() {
        let dir = unique_dir("atomic-cleanup");
        let target = dir.join("bootstate.env");
        atomic_write_with_sync(&target, b"hello").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "hello");
        assert!(!tmp_sibling_path(&target).exists());
    }

    #[test]
    fn tmp_sibling_path_stays_in_parent_directory() {
        let tmp = tmp_sibling_path(Path::new("/boot/rhythm-bootstate.env"));
        assert_eq!(tmp.parent(), Some(Path::new("/boot")));
        assert_eq!(
            tmp.file_name().and_then(|n| n.to_str()),
            Some("rhythm-bootstate.env.tmp")
        );
    }
}
