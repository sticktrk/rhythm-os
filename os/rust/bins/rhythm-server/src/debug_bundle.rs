//! On-demand debug bundle generation for native server/appliance builds.
//!
//! Builds a `tar.gz` bundle in memory so clients can immediately download
//! recent logs plus redacted runtime/configuration snapshots.

use std::collections::BTreeSet;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::MutexGuard;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;

use rhythm_os::commands;
use rhythm_os::state::{AppState, SharedState};

const DEBUG_BUNDLE_SCHEMA_VERSION: u32 = 1;
const LOG_BASENAMES: &[&str] = &[
    "rhythm-server.log",
    "rhythm-server.err.log",
    "rhythm-matter.log",
    "wifi.log",
    "bluetooth.log",
];

pub struct DebugBundle {
    pub file_name: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
struct RuntimeSnapshot {
    firmware_version: String,
    platform_type: String,
    platform_context: String,
    data_dir: String,
}

#[derive(Clone, Debug)]
struct LogArtifact {
    source_path: PathBuf,
    archive_path: String,
}

#[derive(Debug, Serialize)]
struct DebugBundleManifest {
    schema_version: u32,
    kind: &'static str,
    created_at: String,
    firmware_version: String,
    platform_type: String,
    platform_context: String,
    data_dir: String,
    searched_log_dirs: Vec<String>,
    captured_logs: Vec<CapturedLogEntry>,
    generated_files: Vec<GeneratedFileEntry>,
    notes: Vec<String>,
}

#[derive(Debug, Serialize)]
struct CapturedLogEntry {
    archive_path: String,
    source_path: String,
    bytes: usize,
}

#[derive(Debug, Serialize)]
struct GeneratedFileEntry {
    archive_path: String,
    bytes: usize,
}

pub fn build_debug_bundle(state: &SharedState) -> Result<DebugBundle> {
    let runtime = snapshot_runtime(state)?;
    let created_at = Utc::now();

    let state_json = commands::build_state_snapshot(state).context("building state snapshot")?;
    let configuration_json =
        commands::build_configuration_bundle(state).context("building configuration bundle")?;

    let searched_log_dirs = discover_log_dirs(&runtime);
    let log_artifacts = discover_log_artifacts(&searched_log_dirs)?;

    let encoder = flate2::write::GzEncoder::new(Vec::<u8>::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);

    append_bytes(&mut builder, "state.json", state_json.as_bytes(), 0o644)?;
    append_bytes(
        &mut builder,
        "configuration.json",
        configuration_json.as_bytes(),
        0o644,
    )?;

    let mut captured_logs = Vec::<CapturedLogEntry>::new();
    for artifact in &log_artifacts {
        let bytes = fs::read(&artifact.source_path)
            .with_context(|| format!("reading {}", artifact.source_path.display()))?;
        append_bytes(&mut builder, &artifact.archive_path, &bytes, 0o644)?;
        captured_logs.push(CapturedLogEntry {
            archive_path: artifact.archive_path.clone(),
            source_path: artifact.source_path.display().to_string(),
            bytes: bytes.len(),
        });
    }

    let mut notes = Vec::<String>::new();
    if captured_logs.is_empty() {
        notes.push("No matching log files were found in the searched directories.".to_string());
    }

    let manifest = DebugBundleManifest {
        schema_version: DEBUG_BUNDLE_SCHEMA_VERSION,
        kind: "debug_bundle",
        created_at: created_at.to_rfc3339(),
        firmware_version: runtime.firmware_version.clone(),
        platform_type: runtime.platform_type.clone(),
        platform_context: runtime.platform_context.clone(),
        data_dir: runtime.data_dir.clone(),
        searched_log_dirs: searched_log_dirs
            .iter()
            .map(|dir| dir.display().to_string())
            .collect(),
        captured_logs,
        generated_files: vec![
            GeneratedFileEntry {
                archive_path: "state.json".to_string(),
                bytes: state_json.len(),
            },
            GeneratedFileEntry {
                archive_path: "configuration.json".to_string(),
                bytes: configuration_json.len(),
            },
        ],
        notes,
    };
    let manifest_json =
        serde_json::to_string_pretty(&manifest).context("serializing debug bundle manifest")?;
    append_bytes(
        &mut builder,
        "manifest.json",
        manifest_json.as_bytes(),
        0o644,
    )?;

    builder
        .finish()
        .context("finalizing debug bundle tar stream")?;
    let encoder = builder
        .into_inner()
        .context("extracting debug bundle gzip encoder")?;
    let bytes = encoder
        .finish()
        .context("finishing debug bundle gzip stream")?;

    Ok(DebugBundle {
        file_name: bundle_file_name(&runtime, created_at),
        bytes,
    })
}

fn snapshot_runtime(state: &SharedState) -> Result<RuntimeSnapshot> {
    let guard = lock_state(state)?;
    Ok(RuntimeSnapshot {
        firmware_version: guard.firmware_version.to_string(),
        platform_type: guard.platform_type.to_string(),
        platform_context: guard.platform_context.to_string(),
        data_dir: guard.data_dir.clone(),
    })
}

fn lock_state(state: &SharedState) -> Result<MutexGuard<'_, AppState>> {
    state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))
}

fn bundle_file_name(runtime: &RuntimeSnapshot, created_at: DateTime<Utc>) -> String {
    let context = sanitize_filename_component(&runtime.platform_context);
    format!(
        "rhythm-debug-bundle-{}-{}.tar.gz",
        if context.is_empty() {
            "server"
        } else {
            &context
        },
        created_at.format("%Y%m%dT%H%M%SZ")
    )
}

fn sanitize_filename_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn discover_log_dirs(runtime: &RuntimeSnapshot) -> Vec<PathBuf> {
    let mut dirs = Vec::<PathBuf>::new();

    if let Some(log_dir) = std::env::var_os("RHYTHM_LOG_DIR") {
        dirs.push(PathBuf::from(log_dir));
    }

    if !runtime.data_dir.is_empty() {
        dirs.push(Path::new(&runtime.data_dir).join("log"));
    }

    if runtime.platform_context == "server" && cfg!(target_os = "macos") {
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(
                PathBuf::from(home)
                    .join("Library")
                    .join("Logs")
                    .join("Rhythm"),
            );
        }
    }

    let mut dedup = BTreeSet::<PathBuf>::new();
    dirs.into_iter()
        .filter(|dir| dedup.insert(dir.clone()))
        .collect()
}

fn discover_log_artifacts(log_dirs: &[PathBuf]) -> Result<Vec<LogArtifact>> {
    let mut discovered = Vec::<LogArtifact>::new();
    let mut seen_archive_paths = BTreeSet::<String>::new();

    for dir in log_dirs {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(err).with_context(|| format!("reading {}", dir.display()));
            }
        };

        for entry in entries {
            let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !matches_log_name(file_name) {
                continue;
            }

            let archive_path = format!("logs/{}", file_name);
            if !seen_archive_paths.insert(archive_path.clone()) {
                continue;
            }

            discovered.push(LogArtifact {
                source_path: path,
                archive_path,
            });
        }
    }

    discovered.sort_by(|left, right| left.archive_path.cmp(&right.archive_path));
    Ok(discovered)
}

fn matches_log_name(file_name: &str) -> bool {
    LOG_BASENAMES.iter().any(|base_name| {
        file_name == *base_name
            || file_name
                .strip_prefix(base_name)
                .is_some_and(|suffix| suffix.starts_with('.'))
    })
}

fn append_bytes<W: Write>(
    builder: &mut tar::Builder<W>,
    archive_path: &str,
    bytes: &[u8],
    mode: u32,
) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_path(archive_path)?;
    header.set_size(bytes.len() as u64);
    header.set_mode(mode);
    header.set_cksum();
    builder
        .append_data(&mut header, archive_path, Cursor::new(bytes))
        .with_context(|| format!("adding {} to debug bundle", archive_path))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::io::Read;

    fn unique_test_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rhythm-debug-bundle-{}-{}", name, nanos));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn unpack_bundle(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
        let decoder = GzDecoder::new(Cursor::new(bytes));
        let mut archive = tar::Archive::new(decoder);
        let mut files = BTreeMap::<String, Vec<u8>>::new();

        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().to_string_lossy().to_string();
            let mut body = Vec::<u8>::new();
            entry.read_to_end(&mut body).unwrap();
            files.insert(path, body);
        }

        files
    }

    #[test]
    fn build_debug_bundle_includes_logs_and_redacted_snapshots() {
        let root = unique_test_dir("bundle");
        let data_dir = root.join("data");
        let log_dir = data_dir.join("log");
        fs::create_dir_all(&log_dir).unwrap();
        fs::write(log_dir.join("rhythm-server.log"), b"server-log").unwrap();
        fs::write(log_dir.join("rhythm-matter.log.1"), b"matter-log").unwrap();

        let state: SharedState =
            std::sync::Arc::new(std::sync::Mutex::new(rhythm_os::state::AppState::default()));
        {
            let mut guard = state.lock().unwrap();
            guard.firmware_version = "1.2.3";
            guard.platform_type = "appliance";
            guard.platform_context = "rpiz";
            guard.data_dir = data_dir.display().to_string();
        }

        let bundle = build_debug_bundle(&state).unwrap();
        assert!(bundle.file_name.ends_with(".tar.gz"));

        let files = unpack_bundle(&bundle.bytes);
        assert_eq!(
            files.get("logs/rhythm-server.log").map(Vec::as_slice),
            Some(b"server-log".as_slice())
        );
        assert_eq!(
            files.get("logs/rhythm-matter.log.1").map(Vec::as_slice),
            Some(b"matter-log".as_slice())
        );
        assert!(files.contains_key("state.json"));
        assert!(files.contains_key("configuration.json"));
        assert!(files.contains_key("manifest.json"));

        let manifest: Value = serde_json::from_slice(files.get("manifest.json").unwrap()).unwrap();
        assert_eq!(manifest["kind"], "debug_bundle");
        assert_eq!(manifest["platform_context"], "rpiz");
        assert_eq!(manifest["captured_logs"].as_array().unwrap().len(), 2);

        let state_json: Value = serde_json::from_slice(files.get("state.json").unwrap()).unwrap();
        assert_eq!(state_json["context"], "rpiz");

        let _ = fs::remove_dir_all(root);
    }
}
