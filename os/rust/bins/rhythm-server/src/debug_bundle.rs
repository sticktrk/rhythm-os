//! On-demand debug bundle generation for native server/appliance builds.
//!
//! Builds a `tar.gz` bundle in memory so clients can immediately download
//! recent logs, persisted topology/canonical state, and runtime-derived
//! diagnostics without needing filesystem access to the appliance.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::MutexGuard;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;

use rhythm_os::canonical::identity::IntegrationEndpoint;
use rhythm_os::canonical::registry::CanonicalRegistry;
use rhythm_os::canonical::triage::TriageEntry;
use rhythm_os::commands;
use rhythm_os::state::{AppState, SharedState};
use rhythm_os::topology::{
    DevicePlacement, HubRoomBinding, RoomBindingRecord, RoomTopologyStore, TopologyDeviceNode,
};

const DEBUG_BUNDLE_SCHEMA_VERSION: u32 = 2;
const LOG_BASENAMES: &[&str] = &[
    "rhythm-server.log",
    "rhythm-server.err.log",
    "rhythm-matter.log",
    "wifi.log",
    "bluetooth.log",
];
const EXACT_PERSISTED_FILES: &[&str] = &["topology.json", "canonical_registry.json", "rooms.json"];
const PERSISTED_HUB_REGISTRY_GLOB: &str = "hub_registry_*.json";

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

#[derive(Clone)]
struct StateDebugSnapshot {
    topology: RoomTopologyStore,
    canonical_registry: CanonicalRegistry,
}

#[derive(Clone, Debug)]
struct FileArtifact {
    source_path: PathBuf,
    archive_path: String,
}

#[derive(Clone, Debug, Default, Serialize)]
struct HostMetadata {
    os: String,
    os_family: String,
    arch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    boot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    system_uptime_secs: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize)]
struct ProcessMetadata {
    pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    uptime_secs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_ticks: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
struct FileErrorEntry {
    operation: String,
    path: String,
    error: String,
}

#[derive(Clone, Debug, Serialize)]
struct FdSnapshotEntry {
    fd: String,
    target: String,
}

#[derive(Clone, Debug, Serialize)]
struct ProcessResourceSnapshot {
    schema_version: u32,
    generated_at: String,
    target_os: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    open_fd_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    open_fds: Vec<FdSnapshotEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limits: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sockstat: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sockstat6: Option<String>,
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
    captured_logs: Vec<CapturedFileEntry>,
    captured_persisted_files: Vec<CapturedFileEntry>,
    missing_persisted_files: Vec<String>,
    generated_files: Vec<GeneratedFileEntry>,
    host: HostMetadata,
    process: ProcessMetadata,
    file_errors: Vec<FileErrorEntry>,
    notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct CapturedFileEntry {
    archive_path: String,
    source_path: String,
    bytes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    modified_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct GeneratedFileEntry {
    archive_path: String,
    bytes: usize,
}

#[derive(Clone, Debug, Serialize)]
struct BundleDiagnostics {
    schema_version: u32,
    kind: &'static str,
    started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    searched_log_dirs: Vec<String>,
    persisted_data_dir: String,
    missing_persisted_files: Vec<String>,
    warnings: Vec<String>,
    file_errors: Vec<FileErrorEntry>,
    host: HostMetadata,
    process: ProcessMetadata,
}

impl BundleDiagnostics {
    fn new(
        started_at: DateTime<Utc>,
        persisted_data_dir: String,
        searched_log_dirs: Vec<String>,
    ) -> Self {
        Self {
            schema_version: DEBUG_BUNDLE_SCHEMA_VERSION,
            kind: "debug_bundle_diagnostics",
            started_at: started_at.to_rfc3339(),
            completed_at: None,
            duration_ms: None,
            searched_log_dirs,
            persisted_data_dir,
            missing_persisted_files: Vec::new(),
            warnings: Vec::new(),
            file_errors: Vec::new(),
            host: HostMetadata::default(),
            process: ProcessMetadata::default(),
        }
    }

    fn add_warning(&mut self, warning: impl Into<String>) {
        self.warnings.push(warning.into());
    }

    fn record_file_error(
        &mut self,
        operation: impl Into<String>,
        path: impl Into<String>,
        error: impl std::fmt::Display,
    ) {
        self.file_errors.push(FileErrorEntry {
            operation: operation.into(),
            path: path.into(),
            error: error.to_string(),
        });
    }

    fn finish(
        &mut self,
        completed_at: DateTime<Utc>,
        host: HostMetadata,
        process: ProcessMetadata,
    ) {
        self.completed_at = Some(completed_at.to_rfc3339());
        self.duration_ms = Some(
            (completed_at - parse_rfc3339(&self.started_at))
                .num_milliseconds()
                .max(0) as u64,
        );
        self.missing_persisted_files.sort();
        self.missing_persisted_files.dedup();
        self.warnings.sort();
        self.warnings.dedup();
        self.file_errors.sort_by(|left, right| {
            left.operation
                .cmp(&right.operation)
                .then_with(|| left.path.cmp(&right.path))
                .then_with(|| left.error.cmp(&right.error))
        });
        self.host = host;
        self.process = process;
    }
}

#[derive(Debug, Serialize)]
struct TopologyDebugSnapshot {
    schema_version: u32,
    generated_at: String,
    approved_room_bindings: Vec<RoomBindingRecord>,
    rooms: Vec<TopologyDebugRoom>,
    standalone_devices: Vec<TopologyDebugDevice>,
    missing_canonical_device_ids: Vec<String>,
    mismatched_room_assignments: Vec<RoomAssignmentMismatch>,
}

#[derive(Debug, Serialize)]
struct TopologyDebugRoom {
    room_id: String,
    name: String,
    user_customized: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bootstrap_name: Option<String>,
    hub_room_bindings: Vec<HubRoomBinding>,
    child_device_ids: Vec<String>,
    child_devices: Vec<TopologyDebugDevice>,
}

#[derive(Debug, Serialize)]
struct TopologyDebugDevice {
    topology_node_id: String,
    canonical_device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_type: Option<String>,
    placement: DevicePlacement,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    topology_parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    canonical_room_id: Option<String>,
    endpoints: Vec<TopologyDebugEndpoint>,
}

#[derive(Debug, Serialize)]
struct TopologyDebugEndpoint {
    hub_key: String,
    native_id: String,
    preferred: bool,
    active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_room_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct RoomAssignmentMismatch {
    canonical_device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    topology_parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    canonical_room_id: Option<String>,
    reason: &'static str,
}

#[derive(Debug, Serialize)]
struct TriageQueueSnapshot {
    schema_version: u32,
    generated_at: String,
    total_entries: usize,
    pending_entries: usize,
    pending_device_merges: usize,
    pending_room_bindings: usize,
    pending_unassigned_devices: usize,
    entries: Vec<TriageEntry>,
}

pub fn build_debug_bundle(state: &SharedState) -> Result<DebugBundle> {
    let created_at = Utc::now();
    let runtime = snapshot_runtime(state)?;
    let debug_state = snapshot_debug_state(state)?;

    let state_json = commands::build_state_snapshot(state).context("building state snapshot")?;
    let profile_bundle_json =
        commands::build_profile_bundle(state).context("building profile bundle")?;
    let topology_debug_json = build_topology_debug_json(&debug_state, created_at)
        .context("building topology debug snapshot")?;
    let triage_queue_json = build_triage_queue_json(&debug_state, created_at)
        .context("building triage queue snapshot")?;

    let searched_log_dirs = discover_log_dirs(&runtime);
    let mut diagnostics = BundleDiagnostics::new(
        created_at,
        runtime.data_dir.clone(),
        searched_log_dirs
            .iter()
            .map(|dir| dir.display().to_string())
            .collect(),
    );
    let (host, process) = snapshot_host_and_process_metadata(created_at, &mut diagnostics);
    let process_resources_json = build_process_resources_json(created_at, &mut diagnostics)
        .context("building process resource snapshot")?;

    let log_artifacts = discover_log_artifacts(&searched_log_dirs, &mut diagnostics);
    let persisted_artifacts = discover_persisted_artifacts(&runtime, &mut diagnostics);

    let encoder = flate2::write::GzEncoder::new(Vec::<u8>::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    let mut generated_files = Vec::<GeneratedFileEntry>::new();

    append_generated_file(
        &mut builder,
        &mut generated_files,
        "state.json",
        state_json.as_bytes(),
    )?;
    append_generated_file(
        &mut builder,
        &mut generated_files,
        "profile_bundle.json",
        profile_bundle_json.as_bytes(),
    )?;
    append_generated_file(
        &mut builder,
        &mut generated_files,
        "topology_debug.json",
        topology_debug_json.as_bytes(),
    )?;
    append_generated_file(
        &mut builder,
        &mut generated_files,
        "triage_queue.json",
        triage_queue_json.as_bytes(),
    )?;
    append_generated_file(
        &mut builder,
        &mut generated_files,
        "process_resources.json",
        process_resources_json.as_bytes(),
    )?;

    let mut captured_logs = Vec::<CapturedFileEntry>::new();
    for artifact in &log_artifacts {
        if let Some((captured, bytes)) = capture_artifact(artifact, &mut diagnostics) {
            append_bytes(&mut builder, &artifact.archive_path, &bytes, 0o644)?;
            captured_logs.push(captured);
        }
    }

    let mut captured_persisted_files = Vec::<CapturedFileEntry>::new();
    for artifact in &persisted_artifacts {
        if let Some((captured, bytes)) = capture_artifact(artifact, &mut diagnostics) {
            append_bytes(&mut builder, &artifact.archive_path, &bytes, 0o644)?;
            captured_persisted_files.push(captured);
        }
    }

    let completed_at = Utc::now();
    diagnostics.finish(completed_at, host.clone(), process.clone());
    let diagnostics_json =
        serde_json::to_string_pretty(&diagnostics).context("serializing bundle diagnostics")?;
    append_generated_file(
        &mut builder,
        &mut generated_files,
        "bundle_diagnostics.json",
        diagnostics_json.as_bytes(),
    )?;

    let mut notes = diagnostics.warnings.clone();
    if captured_logs.is_empty() {
        notes.push("No matching log files were captured.".to_string());
    }
    if captured_persisted_files.is_empty() {
        notes.push("No persisted topology/canonical files were captured.".to_string());
    }
    if !diagnostics.file_errors.is_empty() {
        notes.push(format!(
            "{} file capture errors were recorded in bundle_diagnostics.json.",
            diagnostics.file_errors.len()
        ));
    }
    notes.sort();
    notes.dedup();

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
        captured_persisted_files,
        missing_persisted_files: diagnostics.missing_persisted_files.clone(),
        generated_files,
        host,
        process,
        file_errors: diagnostics.file_errors.clone(),
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

fn snapshot_debug_state(state: &SharedState) -> Result<StateDebugSnapshot> {
    let guard = lock_state(state)?;
    Ok(StateDebugSnapshot {
        topology: guard.topology.clone(),
        canonical_registry: guard.canonical_registry.clone(),
    })
}

fn lock_state(state: &SharedState) -> Result<MutexGuard<'_, AppState>> {
    state.lock().map_err(|_| anyhow!("state lock poisoned"))
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

fn discover_log_artifacts(
    log_dirs: &[PathBuf],
    diagnostics: &mut BundleDiagnostics,
) -> Vec<FileArtifact> {
    let mut discovered = Vec::<FileArtifact>::new();
    let mut seen_archive_paths = BTreeSet::<String>::new();

    for dir in log_dirs {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                diagnostics.record_file_error("read_dir", dir.display().to_string(), &err);
                diagnostics.add_warning(format!("Failed to scan log directory {}", dir.display()));
                continue;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    diagnostics.record_file_error(
                        "read_dir_entry",
                        dir.display().to_string(),
                        &err,
                    );
                    continue;
                }
            };
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

            discovered.push(FileArtifact {
                source_path: path,
                archive_path,
            });
        }
    }

    discovered.sort_by(|left, right| left.archive_path.cmp(&right.archive_path));
    discovered
}

fn discover_persisted_artifacts(
    runtime: &RuntimeSnapshot,
    diagnostics: &mut BundleDiagnostics,
) -> Vec<FileArtifact> {
    let mut discovered = Vec::<FileArtifact>::new();
    if runtime.data_dir.is_empty() {
        diagnostics.add_warning("No data_dir configured on runtime snapshot.");
        diagnostics
            .missing_persisted_files
            .extend(EXACT_PERSISTED_FILES.iter().map(|name| (*name).to_string()));
        diagnostics
            .missing_persisted_files
            .push(PERSISTED_HUB_REGISTRY_GLOB.to_string());
        return discovered;
    }

    let data_dir = PathBuf::from(&runtime.data_dir);
    for file_name in EXACT_PERSISTED_FILES {
        let source_path = data_dir.join(file_name);
        if source_path.is_file() {
            discovered.push(FileArtifact {
                source_path,
                archive_path: format!("persisted/{file_name}"),
            });
        } else {
            diagnostics
                .missing_persisted_files
                .push((*file_name).to_string());
        }
    }

    let entries = match fs::read_dir(&data_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            diagnostics.add_warning(format!(
                "Persisted data directory {} does not exist.",
                data_dir.display()
            ));
            diagnostics
                .missing_persisted_files
                .push(PERSISTED_HUB_REGISTRY_GLOB.to_string());
            return discovered;
        }
        Err(err) => {
            diagnostics.record_file_error("read_dir", data_dir.display().to_string(), &err);
            diagnostics.add_warning(format!(
                "Failed to scan persisted data directory {}.",
                data_dir.display()
            ));
            diagnostics
                .missing_persisted_files
                .push(PERSISTED_HUB_REGISTRY_GLOB.to_string());
            return discovered;
        }
    };

    let mut found_hub_registry = false;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                diagnostics.record_file_error(
                    "read_dir_entry",
                    data_dir.display().to_string(),
                    &err,
                );
                continue;
            }
        };

        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let file_name = file_name.to_string();
        if !(file_name.starts_with("hub_registry_") && file_name.ends_with(".json")) {
            continue;
        }

        found_hub_registry = true;
        discovered.push(FileArtifact {
            source_path: path,
            archive_path: format!("persisted/{file_name}"),
        });
    }

    if !found_hub_registry {
        diagnostics
            .missing_persisted_files
            .push(PERSISTED_HUB_REGISTRY_GLOB.to_string());
    }

    discovered.sort_by(|left, right| left.archive_path.cmp(&right.archive_path));
    discovered
}

fn matches_log_name(file_name: &str) -> bool {
    LOG_BASENAMES.iter().any(|base_name| {
        file_name == *base_name
            || file_name
                .strip_prefix(base_name)
                .is_some_and(|suffix| suffix.starts_with('.'))
    })
}

fn capture_artifact(
    artifact: &FileArtifact,
    diagnostics: &mut BundleDiagnostics,
) -> Option<(CapturedFileEntry, Vec<u8>)> {
    let bytes = match fs::read(&artifact.source_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            diagnostics.record_file_error("read", artifact.source_path.display().to_string(), &err);
            diagnostics.add_warning(format!("Failed to read {}", artifact.source_path.display()));
            return None;
        }
    };

    let modified_at = path_modified_rfc3339(&artifact.source_path, diagnostics);
    Some((
        CapturedFileEntry {
            archive_path: artifact.archive_path.clone(),
            source_path: artifact.source_path.display().to_string(),
            bytes: bytes.len(),
            modified_at,
        },
        bytes,
    ))
}

fn path_modified_rfc3339(path: &Path, diagnostics: &mut BundleDiagnostics) -> Option<String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) => {
            diagnostics.record_file_error("metadata", path.display().to_string(), &err);
            return None;
        }
    };

    match metadata.modified() {
        Ok(modified_at) => Some(DateTime::<Utc>::from(modified_at).to_rfc3339()),
        Err(err) => {
            diagnostics.record_file_error("modified", path.display().to_string(), &err);
            None
        }
    }
}

fn snapshot_host_and_process_metadata(
    now: DateTime<Utc>,
    diagnostics: &mut BundleDiagnostics,
) -> (HostMetadata, ProcessMetadata) {
    #[cfg(not(target_os = "linux"))]
    let _ = (&now, &diagnostics);

    #[allow(unused_mut)]
    let mut host = HostMetadata {
        os: std::env::consts::OS.to_string(),
        os_family: std::env::consts::FAMILY.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        boot_id: None,
        system_uptime_secs: None,
    };
    #[allow(unused_mut)]
    let mut process = ProcessMetadata {
        pid: std::process::id(),
        started_at: None,
        uptime_secs: None,
        start_ticks: None,
    };

    #[cfg(target_os = "linux")]
    {
        host.boot_id = linux_boot_id(diagnostics);
        host.system_uptime_secs = linux_system_uptime_secs(diagnostics);
        process.start_ticks = linux_process_start_ticks(diagnostics);

        let ticks_per_second = linux_clock_ticks_per_second(diagnostics);
        if let (Some(system_uptime_secs), Some(start_ticks), Some(ticks_per_second)) = (
            host.system_uptime_secs,
            process.start_ticks,
            ticks_per_second,
        ) {
            let started_after_boot_secs = start_ticks as f64 / ticks_per_second as f64;
            let uptime_secs = (system_uptime_secs - started_after_boot_secs).max(0.0);
            process.uptime_secs = Some(uptime_secs);
            process.started_at = Some(
                (now - chrono::Duration::milliseconds((uptime_secs * 1000.0).round() as i64))
                    .to_rfc3339(),
            );
        }
    }

    (host, process)
}

#[cfg(target_os = "linux")]
fn linux_boot_id(diagnostics: &mut BundleDiagnostics) -> Option<String> {
    match fs::read_to_string("/proc/sys/kernel/random/boot_id") {
        Ok(contents) => {
            let trimmed = contents.trim().to_string();
            (!trimmed.is_empty()).then_some(trimmed)
        }
        Err(err) => {
            diagnostics.record_file_error("read", "/proc/sys/kernel/random/boot_id", &err);
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_system_uptime_secs(diagnostics: &mut BundleDiagnostics) -> Option<f64> {
    let contents = match fs::read_to_string("/proc/uptime") {
        Ok(contents) => contents,
        Err(err) => {
            diagnostics.record_file_error("read", "/proc/uptime", &err);
            return None;
        }
    };
    contents
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<f64>().ok())
        .or_else(|| {
            diagnostics.add_warning("Failed to parse /proc/uptime.");
            None
        })
}

#[cfg(target_os = "linux")]
fn linux_process_start_ticks(diagnostics: &mut BundleDiagnostics) -> Option<u64> {
    let stat = match fs::read_to_string("/proc/self/stat") {
        Ok(stat) => stat,
        Err(err) => {
            diagnostics.record_file_error("read", "/proc/self/stat", &err);
            return None;
        }
    };

    let close_paren = stat.rfind(')')?;
    let remainder = stat.get(close_paren + 1..)?.trim();
    let fields: Vec<&str> = remainder.split_whitespace().collect();
    fields
        .get(19)
        .and_then(|value| value.parse::<u64>().ok())
        .or_else(|| {
            diagnostics.add_warning("Failed to parse process start ticks from /proc/self/stat.");
            None
        })
}

#[cfg(target_os = "linux")]
fn linux_clock_ticks_per_second(diagnostics: &mut BundleDiagnostics) -> Option<u64> {
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks <= 0 {
        diagnostics.add_warning("libc::sysconf(_SC_CLK_TCK) returned an invalid value.");
        None
    } else {
        Some(ticks as u64)
    }
}

fn build_process_resources_json(
    generated_at: DateTime<Utc>,
    diagnostics: &mut BundleDiagnostics,
) -> Result<String> {
    #[cfg(target_os = "linux")]
    let snapshot = {
        let mut snapshot = ProcessResourceSnapshot {
            schema_version: DEBUG_BUNDLE_SCHEMA_VERSION,
            generated_at: generated_at.to_rfc3339(),
            target_os: std::env::consts::OS.to_string(),
            open_fd_count: None,
            open_fds: Vec::new(),
            limits: None,
            sockstat: None,
            sockstat6: None,
        };

        snapshot.open_fds = linux_open_fd_snapshot(diagnostics);
        snapshot.open_fd_count = Some(snapshot.open_fds.len());
        snapshot.limits = linux_read_optional_proc_file("/proc/self/limits", diagnostics);
        snapshot.sockstat = linux_read_optional_proc_file("/proc/net/sockstat", diagnostics);
        snapshot.sockstat6 = linux_read_optional_proc_file("/proc/net/sockstat6", diagnostics);
        snapshot
    };

    #[cfg(not(target_os = "linux"))]
    let snapshot = {
        let _ = diagnostics;
        ProcessResourceSnapshot {
            schema_version: DEBUG_BUNDLE_SCHEMA_VERSION,
            generated_at: generated_at.to_rfc3339(),
            target_os: std::env::consts::OS.to_string(),
            open_fd_count: None,
            open_fds: Vec::new(),
            limits: None,
            sockstat: None,
            sockstat6: None,
        }
    };

    serde_json::to_string_pretty(&snapshot).context("serializing process resource snapshot")
}

#[cfg(target_os = "linux")]
fn linux_read_optional_proc_file(
    path: &str,
    diagnostics: &mut BundleDiagnostics,
) -> Option<String> {
    match fs::read_to_string(path) {
        Ok(contents) => Some(contents),
        Err(err) => {
            diagnostics.record_file_error("read", path, &err);
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_open_fd_snapshot(diagnostics: &mut BundleDiagnostics) -> Vec<FdSnapshotEntry> {
    let entries = match fs::read_dir("/proc/self/fd") {
        Ok(entries) => entries,
        Err(err) => {
            diagnostics.record_file_error("read_dir", "/proc/self/fd", &err);
            return Vec::new();
        }
    };

    let mut fds = Vec::<FdSnapshotEntry>::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                diagnostics.record_file_error("read_dir_entry", "/proc/self/fd", &err);
                continue;
            }
        };
        let path = entry.path();
        let fd = entry.file_name().to_string_lossy().to_string();
        let target = match fs::read_link(&path) {
            Ok(target) => target.display().to_string(),
            Err(err) => {
                diagnostics.record_file_error("read_link", path.display().to_string(), &err);
                "<unreadable>".to_string()
            }
        };
        fds.push(FdSnapshotEntry { fd, target });
    }

    fds.sort_by(|left, right| {
        let left_fd = left.fd.parse::<u64>().ok();
        let right_fd = right.fd.parse::<u64>().ok();
        left_fd
            .cmp(&right_fd)
            .then_with(|| left.fd.cmp(&right.fd))
            .then_with(|| left.target.cmp(&right.target))
    });
    fds
}

fn build_topology_debug_json(
    debug_state: &StateDebugSnapshot,
    generated_at: DateTime<Utc>,
) -> Result<String> {
    let topology = &debug_state.topology;
    let canonical_registry = &debug_state.canonical_registry;

    let mut approved_room_bindings = topology.approved_bindings().to_vec();
    approved_room_bindings.sort_by(|left, right| {
        left.rhythm_room_id
            .cmp(&right.rhythm_room_id)
            .then_with(|| left.hub_key.to_string().cmp(&right.hub_key.to_string()))
            .then_with(|| left.hub_room_id.cmp(&right.hub_room_id))
    });

    let mut rooms: Vec<_> = topology.rooms().collect();
    rooms.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.id.cmp(&right.id))
    });

    let room_ids: HashSet<String> = rooms.iter().map(|room| room.id.clone()).collect();

    let rooms = rooms
        .into_iter()
        .map(|room| {
            let mut hub_room_bindings = room.hub_room_bindings.clone();
            hub_room_bindings.sort_by(|left, right| {
                left.hub_key
                    .to_string()
                    .cmp(&right.hub_key.to_string())
                    .then_with(|| left.hub_room_id.cmp(&right.hub_room_id))
            });

            let mut room_devices = room.devices.clone();
            room_devices.sort_by(|left, right| left.device_id.cmp(&right.device_id));
            let child_device_ids = room_devices
                .iter()
                .map(|device| device.device_id.clone())
                .collect::<Vec<_>>();
            let child_devices = room_devices
                .iter()
                .map(|device| {
                    let node = topology.get_device_node(&device.device_id);
                    topology_debug_device_from_assignment(
                        device.device_id.clone(),
                        node,
                        canonical_registry,
                        Some(room.id.as_str()),
                        device.placement.clone(),
                    )
                })
                .collect();

            TopologyDebugRoom {
                room_id: room.id.clone(),
                name: room.name.clone(),
                user_customized: room.user_customized,
                bootstrap_name: room.bootstrap_name.clone(),
                hub_room_bindings,
                child_device_ids,
                child_devices,
            }
        })
        .collect::<Vec<_>>();

    let mut standalone_nodes: Vec<_> = topology
        .device_nodes()
        .filter(|node| node.parent_id.is_none())
        .collect();
    standalone_nodes.sort_by(|left, right| left.id.cmp(&right.id));
    let standalone_devices = standalone_nodes
        .into_iter()
        .map(|node| {
            topology_debug_device_from_assignment(
                node.canonical_device_id.clone(),
                Some(node),
                canonical_registry,
                None,
                node.placement.clone(),
            )
        })
        .collect::<Vec<_>>();

    let mut missing_canonical_device_ids: Vec<_> = topology
        .device_nodes()
        .filter_map(|node| {
            canonical_registry
                .get(&node.canonical_device_id)
                .is_none()
                .then(|| node.canonical_device_id.clone())
        })
        .collect();
    missing_canonical_device_ids.sort();
    missing_canonical_device_ids.dedup();

    let mut mismatched_room_assignments =
        collect_room_assignment_mismatches(topology, canonical_registry, &room_ids);
    mismatched_room_assignments.sort_by(|left, right| {
        left.canonical_device_id
            .cmp(&right.canonical_device_id)
            .then_with(|| left.reason.cmp(right.reason))
            .then_with(|| left.topology_parent_id.cmp(&right.topology_parent_id))
            .then_with(|| left.canonical_room_id.cmp(&right.canonical_room_id))
    });
    mismatched_room_assignments.dedup();

    serde_json::to_string_pretty(&TopologyDebugSnapshot {
        schema_version: DEBUG_BUNDLE_SCHEMA_VERSION,
        generated_at: generated_at.to_rfc3339(),
        approved_room_bindings,
        rooms,
        standalone_devices,
        missing_canonical_device_ids,
        mismatched_room_assignments,
    })
    .context("serializing topology debug snapshot")
}

fn build_triage_queue_json(
    debug_state: &StateDebugSnapshot,
    generated_at: DateTime<Utc>,
) -> Result<String> {
    let triage = debug_state.canonical_registry.triage();
    let mut entries = triage.all().to_vec();
    entries.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });

    serde_json::to_string_pretty(&TriageQueueSnapshot {
        schema_version: DEBUG_BUNDLE_SCHEMA_VERSION,
        generated_at: generated_at.to_rfc3339(),
        total_entries: triage.len(),
        pending_entries: triage.pending_count(),
        pending_device_merges: triage.pending_device_count(),
        pending_room_bindings: triage.pending_room_count(),
        pending_unassigned_devices: triage.pending_unassigned_count(),
        entries,
    })
    .context("serializing triage queue snapshot")
}

fn topology_debug_device_from_assignment(
    canonical_device_id: String,
    node: Option<&TopologyDeviceNode>,
    canonical_registry: &CanonicalRegistry,
    fallback_parent_id: Option<&str>,
    fallback_placement: DevicePlacement,
) -> TopologyDebugDevice {
    let canonical_device = canonical_registry.get(&canonical_device_id);
    let mut endpoints = canonical_device
        .map(|device| {
            let mut endpoints = device
                .endpoints
                .iter()
                .map(topology_debug_endpoint_from_integration)
                .collect::<Vec<_>>();
            endpoints.sort_by(|left, right| {
                left.hub_key
                    .cmp(&right.hub_key)
                    .then_with(|| left.native_id.cmp(&right.native_id))
            });
            endpoints
        })
        .unwrap_or_default();
    endpoints.sort_by(|left, right| {
        left.hub_key
            .cmp(&right.hub_key)
            .then_with(|| left.native_id.cmp(&right.native_id))
    });

    TopologyDebugDevice {
        topology_node_id: node
            .map(|node| node.id.clone())
            .unwrap_or_else(|| canonical_device_id.clone()),
        canonical_device_id,
        device_name: canonical_device.map(|device| device.name.clone()),
        device_type: canonical_device.and_then(|device| serialize_device_type(&device.device_type)),
        placement: node
            .map(|node| node.placement.clone())
            .unwrap_or(fallback_placement),
        topology_parent_id: node
            .and_then(|node| node.parent_id.clone())
            .or_else(|| fallback_parent_id.map(str::to_string)),
        canonical_room_id: canonical_device.and_then(|device| device.room_id.clone()),
        endpoints,
    }
}

fn serialize_device_type(device_type: &impl Serialize) -> Option<String> {
    serde_json::to_value(device_type)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
}

fn topology_debug_endpoint_from_integration(
    endpoint: &IntegrationEndpoint,
) -> TopologyDebugEndpoint {
    TopologyDebugEndpoint {
        hub_key: endpoint.hub_key.to_string(),
        native_id: endpoint.native_id.clone(),
        preferred: endpoint.preferred,
        active: endpoint.active,
        source_room_name: endpoint.source_room_name.clone(),
    }
}

fn collect_room_assignment_mismatches(
    topology: &RoomTopologyStore,
    canonical_registry: &CanonicalRegistry,
    room_ids: &HashSet<String>,
) -> Vec<RoomAssignmentMismatch> {
    let mut mismatches = Vec::<RoomAssignmentMismatch>::new();

    for node in topology.device_nodes() {
        let Some(canonical_device) = canonical_registry.get(&node.canonical_device_id) else {
            continue;
        };

        if node.parent_id != canonical_device.room_id {
            mismatches.push(RoomAssignmentMismatch {
                canonical_device_id: canonical_device.id.clone(),
                device_name: Some(canonical_device.name.clone()),
                topology_parent_id: node.parent_id.clone(),
                canonical_room_id: canonical_device.room_id.clone(),
                reason: match (node.parent_id.is_some(), canonical_device.room_id.is_some()) {
                    (false, true) => "topology_standalone_but_canonical_assigned",
                    (true, false) => "canonical_room_missing",
                    _ => "topology_parent_differs_from_canonical_room",
                },
            });
        }

        if let Some(topology_parent_id) = node.parent_id.as_deref() {
            if !room_ids.contains(topology_parent_id) {
                mismatches.push(RoomAssignmentMismatch {
                    canonical_device_id: canonical_device.id.clone(),
                    device_name: Some(canonical_device.name.clone()),
                    topology_parent_id: Some(topology_parent_id.to_string()),
                    canonical_room_id: canonical_device.room_id.clone(),
                    reason: "topology_parent_room_missing",
                });
            }
        }

        if let Some(canonical_room_id) = canonical_device.room_id.as_deref() {
            if !room_ids.contains(canonical_room_id) {
                mismatches.push(RoomAssignmentMismatch {
                    canonical_device_id: canonical_device.id.clone(),
                    device_name: Some(canonical_device.name.clone()),
                    topology_parent_id: node.parent_id.clone(),
                    canonical_room_id: Some(canonical_room_id.to_string()),
                    reason: "canonical_room_id_missing_from_topology",
                });
            }
        }
    }

    for canonical_device in canonical_registry.devices() {
        if canonical_device.room_id.is_some()
            && topology.get_device_node(&canonical_device.id).is_none()
        {
            mismatches.push(RoomAssignmentMismatch {
                canonical_device_id: canonical_device.id.clone(),
                device_name: Some(canonical_device.name.clone()),
                topology_parent_id: None,
                canonical_room_id: canonical_device.room_id.clone(),
                reason: "canonical_assigned_without_topology_node",
            });
        }
    }

    mismatches
}

fn append_generated_file<W: Write>(
    builder: &mut tar::Builder<W>,
    generated_files: &mut Vec<GeneratedFileEntry>,
    archive_path: &str,
    bytes: &[u8],
) -> Result<()> {
    append_bytes(builder, archive_path, bytes, 0o644)?;
    generated_files.push(GeneratedFileEntry {
        archive_path: archive_path.to_string(),
        bytes: bytes.len(),
    });
    Ok(())
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

fn parse_rfc3339(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
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
    fn build_debug_bundle_includes_logs_persisted_state_and_diagnostics() {
        let root = unique_test_dir("bundle");
        let data_dir = root.join("data");
        let log_dir = data_dir.join("log");
        fs::create_dir_all(&log_dir).unwrap();
        fs::write(log_dir.join("rhythm-server.log"), b"server-log").unwrap();
        fs::write(log_dir.join("rhythm-matter.log.1"), b"matter-log").unwrap();
        fs::write(data_dir.join("topology.json"), br#"{"rooms":[]}"#).unwrap();
        fs::write(
            data_dir.join("canonical_registry.json"),
            br#"{"devices":{},"triage":{"entries":[]}}"#,
        )
        .unwrap();
        fs::write(
            data_dir.join("hub_registry_mock_local.json"),
            br#"{"rooms":[{"id":"office"}]}"#,
        )
        .unwrap();

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
        assert_eq!(
            files.get("persisted/topology.json").map(Vec::as_slice),
            Some(br#"{"rooms":[]}"#.as_slice())
        );
        assert_eq!(
            files
                .get("persisted/canonical_registry.json")
                .map(Vec::as_slice),
            Some(br#"{"devices":{},"triage":{"entries":[]}}"#.as_slice())
        );
        assert_eq!(
            files
                .get("persisted/hub_registry_mock_local.json")
                .map(Vec::as_slice),
            Some(br#"{"rooms":[{"id":"office"}]}"#.as_slice())
        );
        assert!(files.contains_key("state.json"));
        assert!(files.contains_key("profile_bundle.json"));
        assert!(files.contains_key("topology_debug.json"));
        assert!(files.contains_key("triage_queue.json"));
        assert!(files.contains_key("process_resources.json"));
        assert!(files.contains_key("bundle_diagnostics.json"));
        assert!(files.contains_key("manifest.json"));

        let manifest: Value = serde_json::from_slice(files.get("manifest.json").unwrap()).unwrap();
        assert_eq!(manifest["kind"], "debug_bundle");
        assert_eq!(manifest["platform_context"], "rpiz");
        assert_eq!(manifest["captured_logs"].as_array().unwrap().len(), 2);
        assert_eq!(
            manifest["captured_persisted_files"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert!(manifest["missing_persisted_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "rooms.json"));
        assert!(manifest["generated_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value["archive_path"] == "bundle_diagnostics.json"));
        assert!(manifest["generated_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value["archive_path"] == "process_resources.json"));
        assert!(manifest["process"]["pid"]
            .as_u64()
            .is_some_and(|pid| pid > 0));

        let diagnostics: Value =
            serde_json::from_slice(files.get("bundle_diagnostics.json").unwrap()).unwrap();
        assert_eq!(diagnostics["kind"], "debug_bundle_diagnostics");
        assert_eq!(
            diagnostics["persisted_data_dir"],
            data_dir.display().to_string()
        );
        assert!(diagnostics["missing_persisted_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "rooms.json"));

        let state_json: Value = serde_json::from_slice(files.get("state.json").unwrap()).unwrap();
        assert_eq!(state_json["context"], "rpiz");

        let topology_debug: Value =
            serde_json::from_slice(files.get("topology_debug.json").unwrap()).unwrap();
        assert!(topology_debug["rooms"].is_array());
        assert!(topology_debug["standalone_devices"].is_array());

        let triage_queue: Value =
            serde_json::from_slice(files.get("triage_queue.json").unwrap()).unwrap();
        assert!(triage_queue["entries"].is_array());

        let process_resources: Value =
            serde_json::from_slice(files.get("process_resources.json").unwrap()).unwrap();
        assert_eq!(
            process_resources["schema_version"],
            DEBUG_BUNDLE_SCHEMA_VERSION
        );
        assert_eq!(process_resources["target_os"], std::env::consts::OS);

        let _ = fs::remove_dir_all(root);
    }
}
