//! On-demand debug bundle generation for native server/appliance builds.
//!
//! Builds a `tar.gz` bundle in memory so clients can immediately download
//! recent logs, persisted topology/canonical state, and runtime-derived
//! diagnostics without needing filesystem access to the appliance.

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::fs;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::MutexGuard;
use std::time::SystemTime;

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
    "rhythm-http.log",
    "rhythm-periodic.log",
    "rhythm-sse.log",
    "rhythm-matter.log",
    "rhythm-matter-verbose.log",
    "wifi.log",
    "bluetooth.log",
    "cloudflared.log",
];
const DEBUG_BUNDLE_LOG_CAPTURE_BYTES_LIMIT: u64 = 1024 * 1024;
const DEBUG_BUNDLE_LOG_CAPTURE_ROTATION_LIMIT: u32 = 1;
/// Env override for the per-log-file capture cap. Oversized bundles have
/// timed out the app-side download in the field, so support can dial this
/// down (or up, when investigating with a fast link) without a firmware
/// change.
const LOG_CAPTURE_BYTES_ENV: &str = "RHYTHM_DEBUG_BUNDLE_LOG_CAP_BYTES";
/// Env override for how many rotated files per log family get captured
/// (0 = active file only).
const LOG_CAPTURE_ROTATIONS_ENV: &str = "RHYTHM_DEBUG_BUNDLE_LOG_ROTATIONS";
const EXACT_PERSISTED_FILES: &[&str] = &["topology.json", "canonical_registry.json", "rooms.json"];
/// Included when present, but legitimately absent on devices that have never
/// paired a device or applied an OTA — so never reported as missing.
const OPTIONAL_PERSISTED_FILES: &[&str] = &[
    "pairing_history.json",
    "ota_history.json",
    "activity_history.json",
];
const REMOTE_ACCESS_DEBUG_FILES: &[&str] = &["cloudflared/hostname", "cloudflared/status.env"];
const OTA_DEBUG_FILES: &[&str] = &[crate::auto_update::AUTO_UPDATE_STATE_RELATIVE_PATH];
const BOOT_DIAGNOSTIC_FILES: &[&str] = &[
    crate::boot_diagnostics::CURRENT_BOOT_FILE,
    crate::boot_diagnostics::PREVIOUS_BOOT_FILE,
    crate::boot_diagnostics::RESTART_INTENT_FILE,
    crate::boot_diagnostics::PREVIOUS_RESTART_INTENT_FILE,
    crate::boot_diagnostics::LAST_GASP_FILE,
    crate::boot_diagnostics::PREVIOUS_LAST_GASP_FILE,
    crate::boot_diagnostics::KERNEL_CURRENT_FILE,
    crate::boot_diagnostics::KERNEL_PREVIOUS_FILE,
    crate::boot_diagnostics::HARDWARE_RESET_FILE,
];
const HOST_RECORDER_METADATA_BYTES_LIMIT: u64 = 64 * 1024;
const HOST_RECORDER_SEGMENT_BYTES_LIMIT: u64 = 256 * 1024;
const HOST_RECORDER_PSTORE_BYTES_LIMIT: u64 = 256 * 1024;
const HOST_RECORDER_SEGMENT_FILE_LIMIT: usize = 8;
const HOST_RECORDER_PSTORE_FILE_LIMIT: usize = 8;
const PERSISTED_HUB_REGISTRY_GLOB: &str = "hub_registry_*.json";
#[cfg(target_os = "linux")]
const THREAD_SNAPSHOT_ENTRY_LIMIT: usize = 64;
#[cfg(target_os = "linux")]
const THREAD_WAIT_CHANNEL_BYTES_LIMIT: usize = 128;
#[cfg(target_os = "linux")]
const THREAD_KERNEL_STACK_BYTES_LIMIT: usize = 2 * 1024;
#[cfg(target_os = "linux")]
const THREAD_KERNEL_STACK_CAPTURE_LIMIT: usize = 8;

pub struct DebugBundle {
    pub file_name: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LogSource {
    pub id: String,
    pub file_name: String,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ResolvedLogSource {
    pub source: LogSource,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
pub struct LogTailLine {
    pub source: String,
    pub line_number: usize,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct LogTail {
    pub source: LogSource,
    pub lines: Vec<LogTailLine>,
    pub requested_lines: usize,
    pub returned_lines: usize,
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
    bytes_limit: Option<u64>,
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
struct ThreadSnapshotEntry {
    tid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wait_channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kernel_stack: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kernel_stack_truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kernel_stack_error: Option<String>,
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
    thread_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thread_entries_truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    threads: Vec<ThreadSnapshotEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limits: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    loadavg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    meminfo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sockstat: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sockstat6: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tcp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tcp6: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    udp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    udp6: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    route: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    net_dev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wireless: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    arp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mounts: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    partitions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data_dir_filesystem: Option<DataDirFilesystem>,
}

#[derive(Clone, Debug, Serialize)]
struct DataDirFilesystem {
    path: String,
    block_size_bytes: u64,
    total_bytes: u64,
    available_bytes: u64,
    used_bytes: u64,
    inodes_total: u64,
    inodes_available: u64,
}

#[derive(Debug, Serialize)]
struct RuntimeHealthSnapshot {
    schema_version: u32,
    generated_at: String,
    now_epoch_ms: i64,
    platform: RuntimePlatformHealth,
    mode: RuntimeModeHealth,
    timing: RuntimeTimingHealth,
    logging: RuntimeLoggingHealth,
    queues: RuntimeQueueHealth,
    hubs: Vec<RuntimeHubHealth>,
    topology: RuntimeTopologyHealth,
    motion: RuntimeMotionHealth,
    observed_power: RuntimeObservedPowerHealth,
}

#[derive(Debug, Serialize)]
struct RuntimeLoggingHealth {
    dropped_lines: usize,
}

#[derive(Debug, Serialize)]
struct RuntimePlatformHealth {
    firmware_version: String,
    platform_type: String,
    platform_context: String,
    data_dir: String,
    listen_port: Option<u16>,
    eager_tls_warmup: bool,
    full_device_discovery: bool,
}

#[derive(Debug, Serialize)]
struct RuntimeModeHealth {
    active_mode: String,
    last_change_cause: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_change_transition_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_change_epoch_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_change_age_secs: Option<i64>,
}

#[derive(Debug, Serialize)]
struct RuntimeTimingHealth {
    last_tick_epoch_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_tick_age_secs: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_check_hour: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_check_age_secs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_check_utc_offset_hours: Option<f32>,
    default_motion_timeout_secs: u64,
    default_fade_ms: u32,
    utc_offset_hours: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timezone_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    latitude: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    longitude: Option<f32>,
}

#[derive(Debug, Serialize)]
struct RuntimeQueueHealth {
    work_queue_configured: bool,
    periodic_work_queue_configured: bool,
    pending_periodic_ticks: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    oldest_pending_periodic_tick_node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    oldest_pending_periodic_tick_age_secs: Option<f64>,
    pending_hub_event_receivers: usize,
    pending_motion_clear: usize,
    pending_motion_seed: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    event_broadcast_receiver_count: Option<usize>,
    hub_bootstrap_worker_running: bool,
}

#[derive(Debug, Serialize)]
struct RuntimeHubHealth {
    hub_key: String,
    hub_type: String,
    address: String,
    connected: bool,
    seen_connected_once: bool,
    sync_in_progress: bool,
    runtime_present: bool,
    registry_present: bool,
    discovery_present: bool,
    shutdown_requested: bool,
    credentials_present: bool,
    credentials_can_connect: bool,
    credentials_secrets_redacted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_disconnect_remaining_secs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reconnect_sync_age_secs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    startup_retry: Option<RuntimeHubStartupRetryHealth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hue_sse: Option<RuntimeHueSseHealth>,
}

#[derive(Debug, Serialize)]
struct RuntimeHueSseHealth {
    pending_write_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    oldest_pending_write_age_secs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_connected_epoch_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_sse_activity_epoch_ms: Option<i64>,
    connection_count: u64,
    reconnect_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_reconnect_reason: Option<String>,
}

fn runtime_hue_sse_health(hub: &rhythm_os::hub::ActiveHub) -> Option<RuntimeHueSseHealth> {
    let snapshot = hub
        .data::<rhythm_hue::hub_state::HueHubData>()?
        .sse_liveness
        .snapshot();
    Some(RuntimeHueSseHealth {
        pending_write_count: snapshot.pending_write_count,
        oldest_pending_write_age_secs: snapshot.oldest_pending_write_age_secs,
        last_connected_epoch_ms: snapshot.last_connected_epoch_ms,
        last_sse_activity_epoch_ms: snapshot.last_sse_activity_epoch_ms,
        connection_count: snapshot.connection_count,
        reconnect_count: snapshot.reconnect_count,
        last_reconnect_reason: snapshot.last_reconnect_reason,
    })
}

#[derive(Debug, Serialize)]
struct RuntimeHubStartupRetryHealth {
    attempt_count: u32,
    first_failure_epoch_ms: i64,
    last_failure_epoch_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_retry_epoch_ms: Option<i64>,
    manual_retry_required: bool,
    last_error: String,
}

#[derive(Debug, Serialize)]
struct RuntimeTopologyHealth {
    room_count: usize,
    device_node_count: usize,
    canonical_device_count: usize,
    pending_triage_count: usize,
    room_mode_transition_count: usize,
    active_light_node_count: usize,
}

#[derive(Debug, Serialize)]
struct RuntimeMotionHealth {
    snapshot_count: usize,
    active_count: usize,
    owned_count: usize,
    warning_count: usize,
    snapshots: Vec<RuntimeMotionEntry>,
}

#[derive(Debug, Serialize)]
struct RuntimeMotionEntry {
    node_id: String,
    motion_active: bool,
    motion_owned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    remaining_secs: Option<u64>,
    timeout_secs: u64,
    warning_active: bool,
}

#[derive(Debug, Serialize)]
struct RuntimeObservedPowerHealth {
    entry_count: usize,
    on_count: usize,
    recent_entries: Vec<RuntimeObservedPowerEntry>,
}

#[derive(Debug, Serialize)]
struct RuntimeObservedPowerEntry {
    cache_key: String,
    lights_on: bool,
    observed_at_epoch_ms: u64,
    source: &'static str,
}

#[derive(Debug, Serialize)]
struct LogSummarySnapshot {
    schema_version: u32,
    generated_at: String,
    files: Vec<LogFileSummary>,
    total_lines_scanned: usize,
    warning_count: usize,
    error_count: usize,
    hub_event_channel_full_count: usize,
    sse_line_count: usize,
    periodic_cycle_count: usize,
    launch_line_count: usize,
    recent_warnings: Vec<LogLineEntry>,
    recent_errors: Vec<LogLineEntry>,
    recent_hub_event_channel_full: Vec<LogLineEntry>,
    recent_sse_lines: Vec<LogLineEntry>,
    recent_periodic_cycles: Vec<LogLineEntry>,
    recent_launches: Vec<LogLineEntry>,
    tail: Vec<LogLineEntry>,
}

#[derive(Debug, Serialize)]
struct LogFileSummary {
    archive_path: String,
    source_path: String,
    lines_scanned: usize,
    warning_count: usize,
    error_count: usize,
    hub_event_channel_full_count: usize,
    sse_line_count: usize,
    periodic_cycle_count: usize,
    launch_line_count: usize,
}

#[derive(Clone, Debug, Serialize)]
struct LogLineEntry {
    archive_path: String,
    line_number: usize,
    text: String,
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
    source_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    modified_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct GeneratedFileEntry {
    archive_path: String,
    bytes: usize,
}

#[derive(Clone, Debug, Serialize)]
struct DebugFileMetadata {
    path: String,
    present: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    modified_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct MatterFabricIdentityDebug {
    file: DebugFileMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    schema_version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    operational_fabric_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    operational_fabric_id_hex: Option<String>,
    ipk_hex_present: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parse_error: Option<String>,
}

#[derive(Debug, Serialize)]
struct MatterControllerDebugSnapshot {
    schema_version: u32,
    generated_at: String,
    data_dir: String,
    fabric_identity: MatterFabricIdentityDebug,
    chip_controller_storage: DebugFileMetadata,
    chip_controller_storage_files: Vec<DebugFileMetadata>,
    chip_device_store: DebugFileMetadata,
    /// Parsed contents of the commissioned-device cache (devices.json): node
    /// ids, vendor/product names, endpoints. No credentials live in that file
    /// — fabric secrets stay in the controller storage, which is only
    /// reported as metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    chip_device_store_devices: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chip_device_store_parse_error: Option<String>,
    storage_without_identity: bool,
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

/// App-side log attachment for a device-uploaded bundle. When the device
/// streams the bundle straight to storage (bypassing the app's memory and
/// receive timeout), the app can no longer append its own log afterwards —
/// so it sends the log along with the request and the device embeds it.
pub struct AppLogAttachment {
    pub log_text: String,
    pub metadata: Option<serde_json::Value>,
}

pub fn build_debug_bundle(state: &SharedState) -> Result<DebugBundle> {
    build_debug_bundle_with_app_log(state, None)
}

pub fn build_debug_bundle_with_app_log(
    state: &SharedState,
    app_log: Option<AppLogAttachment>,
) -> Result<DebugBundle> {
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
    let runtime_health_json =
        build_runtime_health_json(state, created_at).context("building runtime health snapshot")?;
    let remote_access_status_json = rhythm_os::remote_access::status_snapshot_json(state);

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
    let process_resources_json =
        build_process_resources_json(created_at, &runtime.data_dir, &mut diagnostics)
            .context("building process resource snapshot")?;
    let matter_controller_json =
        build_matter_controller_debug_json(&runtime, created_at, &mut diagnostics)
            .context("building Matter controller debug snapshot")?;
    let host_flight_recorder_summary_json = if runtime.platform_context == "rpiz" {
        Some(
            rhythm_host_recorder::build_synthesis(Path::new(&runtime.data_dir)).unwrap_or_else(
                |error| {
                    diagnostics.add_warning(format!(
                        "Failed to synthesize host flight recorder evidence: {error}"
                    ));
                    serde_json::json!({
                        "schema_version": 1,
                        "status": "unavailable",
                        "detail": error.to_string(),
                    })
                    .to_string()
                },
            ),
        )
    } else {
        None
    };

    let log_artifacts = discover_log_artifacts(&searched_log_dirs, &mut diagnostics);
    let captured_log_artifacts = log_artifacts_for_bundle_capture(&log_artifacts);
    let log_summary_json =
        build_log_summary_json(&captured_log_artifacts, created_at, &mut diagnostics)
            .context("building log summary snapshot")?;
    let mut persisted_artifacts = discover_persisted_artifacts(&runtime, &mut diagnostics);
    persisted_artifacts.extend(discover_host_recorder_artifacts(&runtime, &mut diagnostics));

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
        "runtime_health.json",
        runtime_health_json.as_bytes(),
    )?;
    append_generated_file(
        &mut builder,
        &mut generated_files,
        "remote_access_status.json",
        remote_access_status_json.as_bytes(),
    )?;
    append_generated_file(
        &mut builder,
        &mut generated_files,
        "log_summary.json",
        log_summary_json.as_bytes(),
    )?;
    append_generated_file(
        &mut builder,
        &mut generated_files,
        "process_resources.json",
        process_resources_json.as_bytes(),
    )?;
    append_generated_file(
        &mut builder,
        &mut generated_files,
        "matter_controller.json",
        matter_controller_json.as_bytes(),
    )?;
    if let Some(summary) = &host_flight_recorder_summary_json {
        append_generated_file(
            &mut builder,
            &mut generated_files,
            "host_flight_recorder_summary.json",
            summary.as_bytes(),
        )?;
    }

    let mut captured_logs = Vec::<CapturedFileEntry>::new();
    for artifact in &captured_log_artifacts {
        if let Some((captured, bytes)) = capture_log_artifact(artifact, &mut diagnostics) {
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

    if let Some(app_log) = app_log {
        append_bytes(
            &mut builder,
            "app/app.log",
            app_log.log_text.as_bytes(),
            0o644,
        )?;
        let metadata = app_log.metadata.unwrap_or_else(|| {
            serde_json::json!({
                "kind": "rhythm_app_log",
                "generated_at": created_at.to_rfc3339(),
                "path": "app/app.log",
            })
        });
        let metadata_json =
            serde_json::to_string_pretty(&metadata).context("serializing app log metadata")?;
        append_bytes(
            &mut builder,
            "app/metadata.json",
            metadata_json.as_bytes(),
            0o644,
        )?;
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

pub fn list_log_sources(state: &SharedState) -> Result<Vec<LogSource>> {
    let runtime = snapshot_runtime(state)?;
    let artifacts = discover_log_artifacts_for_runtime(&runtime);
    let mut sources = Vec::<LogSource>::new();

    for artifact in artifacts {
        if let Some(source) = log_source_from_artifact(&artifact)? {
            sources.push(source);
        }
    }

    sources.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(sources)
}

pub fn resolve_log_source(
    state: &SharedState,
    source_id: &str,
) -> Result<Option<ResolvedLogSource>> {
    let runtime = snapshot_runtime(state)?;
    let requested = normalize_log_source_id(source_id);
    if requested.is_empty() || requested.contains('/') || requested.contains('\\') {
        return Ok(None);
    }

    for artifact in discover_log_artifacts_for_runtime(&runtime) {
        let Some(file_name) = artifact
            .source_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        if file_name != requested {
            continue;
        }
        let Some(source) = log_source_from_artifact(&artifact)? else {
            return Ok(None);
        };
        return Ok(Some(ResolvedLogSource {
            source,
            path: artifact.source_path,
        }));
    }

    Ok(None)
}

pub fn tail_log_source(state: &SharedState, source_id: &str, lines: usize) -> Result<LogTail> {
    let Some(resolved) = resolve_log_source(state, source_id)? else {
        anyhow::bail!("log source not found");
    };
    let requested_lines = clamp_log_tail_lines(lines);
    let tail = read_log_tail_lines(&resolved.path, &resolved.source.id, requested_lines)?;
    Ok(LogTail {
        source: resolved.source,
        returned_lines: tail.len(),
        requested_lines,
        lines: tail,
    })
}

/// Hard per-line byte cap for log scanning. `BufRead::lines()` buffers an
/// entire line into memory before we get to truncate it — a corrupted log
/// (e.g. NUL-padded blocks after power loss, which are valid UTF-8 with no
/// newline) becomes one multi-hundred-MB String and OOM-kills the daemon on
/// a 512 MB appliance. Bytes past the cap are discarded, not buffered.
const MAX_SCANNED_LINE_BYTES: usize = 16 * 1024;

struct BoundedLines<R: std::io::BufRead> {
    reader: R,
}

impl<R: std::io::BufRead> Iterator for BoundedLines<R> {
    type Item = std::io::Result<String>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut line: Vec<u8> = Vec::new();
        let mut discarding = false;
        let mut read_any = false;
        loop {
            let buf = match self.reader.fill_buf() {
                Ok(buf) => buf,
                Err(e) => return Some(Err(e)),
            };
            if buf.is_empty() {
                if read_any {
                    break;
                }
                return None;
            }
            read_any = true;
            if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                if !discarding {
                    let take = pos.min(MAX_SCANNED_LINE_BYTES - line.len());
                    line.extend_from_slice(&buf[..take]);
                }
                self.reader.consume(pos + 1);
                break;
            }
            if !discarding {
                let take = buf.len().min(MAX_SCANNED_LINE_BYTES - line.len());
                line.extend_from_slice(&buf[..take]);
                if line.len() >= MAX_SCANNED_LINE_BYTES {
                    discarding = true;
                }
            }
            let consumed = buf.len();
            self.reader.consume(consumed);
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        Some(Ok(String::from_utf8_lossy(&line).into_owned()))
    }
}

fn bounded_lines(file: fs::File) -> BoundedLines<std::io::BufReader<fs::File>> {
    BoundedLines {
        reader: std::io::BufReader::new(file),
    }
}

pub fn read_log_tail_lines(path: &Path, source_id: &str, lines: usize) -> Result<Vec<LogTailLine>> {
    let requested_lines = clamp_log_tail_lines(lines);
    let file = fs::File::open(path).with_context(|| format!("opening log {}", path.display()))?;
    let mut tail = VecDeque::<LogTailLine>::new();

    for (index, line) in bounded_lines(file).enumerate() {
        let line = line.with_context(|| format!("reading log {}", path.display()))?;
        push_limited(
            &mut tail,
            requested_lines,
            LogTailLine {
                source: source_id.to_string(),
                line_number: index + 1,
                text: truncate_log_line(&line),
            },
        );
    }

    Ok(tail.into())
}

pub fn clamp_log_tail_lines(lines: usize) -> usize {
    lines.clamp(1, 2_000)
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

fn discover_log_artifacts_for_runtime(runtime: &RuntimeSnapshot) -> Vec<FileArtifact> {
    let log_dirs = discover_log_dirs(runtime);
    let mut diagnostics = BundleDiagnostics::new(
        Utc::now(),
        runtime.data_dir.clone(),
        log_dirs
            .iter()
            .map(|dir| dir.display().to_string())
            .collect(),
    );
    discover_log_artifacts(&log_dirs, &mut diagnostics)
}

fn log_source_from_artifact(artifact: &FileArtifact) -> Result<Option<LogSource>> {
    let Some(file_name) = artifact
        .source_path
        .file_name()
        .and_then(|name| name.to_str())
    else {
        return Ok(None);
    };
    if !matches_log_name(file_name) {
        return Ok(None);
    }

    let metadata = fs::metadata(&artifact.source_path)
        .with_context(|| format!("reading log metadata {}", artifact.source_path.display()))?;
    if !metadata.is_file() {
        return Ok(None);
    }

    let modified_at = metadata
        .modified()
        .ok()
        .map(|modified| DateTime::<Utc>::from(modified).to_rfc3339());

    Ok(Some(LogSource {
        id: normalize_log_source_id(file_name),
        file_name: file_name.to_string(),
        bytes: metadata.len(),
        modified_at,
    }))
}

fn normalize_log_source_id(source_id: &str) -> String {
    source_id.trim().to_string()
}

fn discover_log_artifacts(
    log_dirs: &[PathBuf],
    diagnostics: &mut BundleDiagnostics,
) -> Vec<FileArtifact> {
    let mut discovered = Vec::<(FileArtifact, Option<SystemTime>)>::new();
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

            let modified = fs::metadata(&path).and_then(|m| m.modified()).ok();

            discovered.push((
                FileArtifact {
                    source_path: path,
                    archive_path,
                    bytes_limit: None,
                },
                modified,
            ));
        }
    }

    // Iterate oldest -> newest so the sliding `recent_*` deques in
    // build_log_summary_json end up holding the most recent entries
    // (push_limited evicts from the front). Files without mtime sort first
    // so any readable file still surfaces its newest content at the end.
    discovered.sort_by(|left, right| {
        left.1
            .cmp(&right.1)
            .then_with(|| left.0.archive_path.cmp(&right.0.archive_path))
    });
    discovered
        .into_iter()
        .map(|(artifact, _)| artifact)
        .collect()
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
                bytes_limit: None,
            });
        } else {
            diagnostics
                .missing_persisted_files
                .push((*file_name).to_string());
        }
    }

    for file_name in OPTIONAL_PERSISTED_FILES
        .iter()
        .chain(REMOTE_ACCESS_DEBUG_FILES)
    {
        let source_path = data_dir.join(file_name);
        if source_path.is_file() {
            discovered.push(FileArtifact {
                source_path,
                archive_path: format!("persisted/{file_name}"),
                bytes_limit: None,
            });
        }
    }

    for file_name in OTA_DEBUG_FILES {
        let source_path = data_dir.join(file_name);
        if source_path.is_file() {
            discovered.push(FileArtifact {
                source_path,
                archive_path: format!("persisted/{file_name}"),
                bytes_limit: None,
            });
        }
    }

    for file_name in BOOT_DIAGNOSTIC_FILES {
        let source_path = data_dir.join(file_name);
        if source_path.is_file() {
            discovered.push(FileArtifact {
                source_path,
                archive_path: format!("persisted/{file_name}"),
                bytes_limit: None,
            });
        }
    }

    let pstore_dir = data_dir.join(crate::boot_diagnostics::PSTORE_DIR);
    if let Ok(entries) = fs::read_dir(pstore_dir) {
        for entry in entries.flatten().take(8) {
            let source_path = entry.path();
            if !source_path.is_file() {
                continue;
            }
            let Some(file_name) = source_path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            discovered.push(FileArtifact {
                source_path,
                archive_path: format!(
                    "persisted/{}/{}",
                    crate::boot_diagnostics::PSTORE_DIR,
                    file_name
                ),
                bytes_limit: None,
            });
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
            bytes_limit: None,
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

fn discover_host_recorder_artifacts(
    runtime: &RuntimeSnapshot,
    diagnostics: &mut BundleDiagnostics,
) -> Vec<FileArtifact> {
    if runtime.platform_context != "rpiz" || runtime.data_dir.is_empty() {
        return Vec::new();
    }
    let root = PathBuf::from(&runtime.data_dir).join(rhythm_host_recorder::ROOT_RELATIVE_PATH);
    let mut discovered = Vec::new();

    for file_name in [
        rhythm_host_recorder::ring::EARLY_BOOT_FILE,
        rhythm_host_recorder::ring::PREVIOUS_EARLY_BOOT_FILE,
    ] {
        push_host_recorder_artifact(
            &root,
            Path::new(file_name),
            HOST_RECORDER_METADATA_BYTES_LIMIT,
            diagnostics,
            &mut discovered,
        );
    }

    for boot_dir in [
        rhythm_host_recorder::ring::CURRENT_DIR,
        rhythm_host_recorder::ring::PREVIOUS_DIR,
    ] {
        push_host_recorder_artifact(
            &root,
            &Path::new(boot_dir).join("manifest.json"),
            HOST_RECORDER_METADATA_BYTES_LIMIT,
            diagnostics,
            &mut discovered,
        );
        let dir = root.join(boot_dir);
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut names = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_str()?.to_string();
                (name.starts_with("segment-") && name.ends_with(".ndjson")).then_some(name)
            })
            .collect::<Vec<_>>();
        names.sort();
        for name in names.into_iter().take(HOST_RECORDER_SEGMENT_FILE_LIMIT) {
            push_host_recorder_artifact(
                &root,
                &Path::new(boot_dir).join(name),
                HOST_RECORDER_SEGMENT_BYTES_LIMIT,
                diagnostics,
                &mut discovered,
            );
        }
    }

    for pstore_dir in [
        rhythm_host_recorder::ring::PSTORE_CURRENT_DIR,
        rhythm_host_recorder::ring::PSTORE_PREVIOUS_DIR,
    ] {
        let dir = root.join(pstore_dir);
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut names = entries
            .flatten()
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .collect::<Vec<_>>();
        names.sort();
        for name in names.into_iter().take(HOST_RECORDER_PSTORE_FILE_LIMIT) {
            push_host_recorder_artifact(
                &root,
                &Path::new(pstore_dir).join(name),
                HOST_RECORDER_PSTORE_BYTES_LIMIT,
                diagnostics,
                &mut discovered,
            );
        }
    }

    discovered.sort_by(|left, right| left.archive_path.cmp(&right.archive_path));
    discovered
}

fn push_host_recorder_artifact(
    root: &Path,
    relative: &Path,
    bytes_limit: u64,
    diagnostics: &mut BundleDiagnostics,
    discovered: &mut Vec<FileArtifact>,
) {
    let source_path = root.join(relative);
    let metadata = match fs::symlink_metadata(&source_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            diagnostics.record_file_error(
                "host_recorder_metadata",
                source_path.display().to_string(),
                error,
            );
            return;
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        diagnostics.add_warning(format!(
            "Skipped non-regular host recorder artifact {}.",
            source_path.display()
        ));
        return;
    }
    let relative = relative.to_string_lossy().replace('\\', "/");
    discovered.push(FileArtifact {
        source_path,
        archive_path: format!(
            "persisted/{}/{}",
            rhythm_host_recorder::ROOT_RELATIVE_PATH,
            relative
        ),
        bytes_limit: Some(bytes_limit),
    });
}

fn build_matter_controller_debug_json(
    runtime: &RuntimeSnapshot,
    generated_at: DateTime<Utc>,
    diagnostics: &mut BundleDiagnostics,
) -> Result<String> {
    if runtime.data_dir.is_empty() {
        diagnostics.add_warning("No data_dir configured for Matter controller debug snapshot.");
    }

    let matter_dir = PathBuf::from(&runtime.data_dir).join("matter");
    let identity_path = matter_dir.join("fabric-identity.json");
    let controller_storage_paths = matter_controller_storage_paths(&matter_dir);
    let device_store_path = matter_dir.join("chip").join("devices.json");

    let fabric_identity = build_matter_fabric_identity_debug(&identity_path, diagnostics);
    let chip_controller_storage_files = controller_storage_paths
        .iter()
        .map(|path| debug_file_metadata(path, diagnostics))
        .collect::<Vec<_>>();
    let chip_controller_storage = chip_controller_storage_files
        .iter()
        .find(|metadata| metadata.present)
        .cloned()
        .or_else(|| chip_controller_storage_files.first().cloned())
        .unwrap_or_else(|| debug_file_metadata(&matter_dir.join("chip"), diagnostics));
    let chip_device_store = debug_file_metadata(&device_store_path, diagnostics);
    let (chip_device_store_devices, chip_device_store_parse_error) =
        read_chip_device_store_contents(&device_store_path, &chip_device_store, diagnostics);
    let storage_without_identity = chip_controller_storage_files
        .iter()
        .any(|metadata| metadata.present)
        && !fabric_identity.file.present;

    if storage_without_identity {
        diagnostics.add_warning(format!(
            "Matter controller storage exists at {} without {}.",
            chip_controller_storage.path,
            identity_path.display()
        ));
    }

    let snapshot = MatterControllerDebugSnapshot {
        schema_version: DEBUG_BUNDLE_SCHEMA_VERSION,
        generated_at: generated_at.to_rfc3339(),
        data_dir: runtime.data_dir.clone(),
        fabric_identity,
        chip_controller_storage,
        chip_controller_storage_files,
        chip_device_store,
        chip_device_store_devices,
        chip_device_store_parse_error,
        storage_without_identity,
    };
    serde_json::to_string_pretty(&snapshot).context("serializing Matter controller debug snapshot")
}

/// The devices.json cache is small (a few hundred bytes per commissioned
/// node) and credential-free, so surface its parsed contents directly in the
/// bundle instead of just path + byte count — fabric membership questions
/// otherwise require log archaeology.
fn read_chip_device_store_contents(
    path: &Path,
    metadata: &DebugFileMetadata,
    diagnostics: &mut BundleDiagnostics,
) -> (Option<serde_json::Value>, Option<String>) {
    const DEVICE_STORE_EMBED_BYTES_LIMIT: u64 = 256 * 1024;

    if !metadata.present {
        return (None, None);
    }
    if metadata.bytes.unwrap_or(0) > DEVICE_STORE_EMBED_BYTES_LIMIT {
        return (
            None,
            Some(format!(
                "device store larger than {DEVICE_STORE_EMBED_BYTES_LIMIT} bytes; skipped"
            )),
        );
    }

    let json = match fs::read_to_string(path) {
        Ok(json) => json,
        Err(err) => {
            diagnostics.record_file_error("read", path.display().to_string(), &err);
            return (None, Some(err.to_string()));
        }
    };
    match serde_json::from_str::<serde_json::Value>(&json) {
        Ok(value) => (Some(value), None),
        Err(err) => (None, Some(err.to_string())),
    }
}

fn matter_controller_storage_paths(matter_dir: &Path) -> Vec<PathBuf> {
    let chip_dir = matter_dir.join("chip");
    vec![
        chip_dir.join("chip_tool_config.controller-storage.ini"),
        chip_dir.join("controller-storage.json"),
        chip_dir.join("chip_tool_config.ini"),
    ]
}

fn build_matter_fabric_identity_debug(
    path: &Path,
    diagnostics: &mut BundleDiagnostics,
) -> MatterFabricIdentityDebug {
    let file = debug_file_metadata(path, diagnostics);
    let mut snapshot = MatterFabricIdentityDebug {
        file,
        schema_version: None,
        label: None,
        operational_fabric_id: None,
        operational_fabric_id_hex: None,
        ipk_hex_present: false,
        parse_error: None,
    };

    if !snapshot.file.present {
        return snapshot;
    }

    let json = match fs::read_to_string(path) {
        Ok(json) => json,
        Err(err) => {
            diagnostics.record_file_error("read", path.display().to_string(), &err);
            snapshot.parse_error = Some(err.to_string());
            return snapshot;
        }
    };

    let value: serde_json::Value = match serde_json::from_str(&json) {
        Ok(value) => value,
        Err(err) => {
            snapshot.parse_error = Some(err.to_string());
            return snapshot;
        }
    };

    snapshot.schema_version = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64);
    snapshot.label = value
        .get("label")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    snapshot.operational_fabric_id = value
        .get("operational_fabric_id")
        .and_then(serde_json::Value::as_u64);
    snapshot.operational_fabric_id_hex = snapshot
        .operational_fabric_id
        .map(|fabric_id| format!("0x{fabric_id:016X}"));
    snapshot.ipk_hex_present = value
        .get("ipk_hex")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|ipk| !ipk.is_empty());

    snapshot
}

fn debug_file_metadata(path: &Path, diagnostics: &mut BundleDiagnostics) -> DebugFileMetadata {
    let path_string = path.display().to_string();
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return DebugFileMetadata {
                path: path_string,
                present: false,
                bytes: None,
                modified_at: None,
                error: None,
            };
        }
        Err(err) => {
            diagnostics.record_file_error("metadata", path_string.clone(), &err);
            return DebugFileMetadata {
                path: path_string,
                present: false,
                bytes: None,
                modified_at: None,
                error: Some(err.to_string()),
            };
        }
    };

    if !metadata.is_file() {
        return DebugFileMetadata {
            path: path_string,
            present: false,
            bytes: None,
            modified_at: None,
            error: Some("not a regular file".to_string()),
        };
    }

    let modified_at = match metadata.modified() {
        Ok(modified_at) => Some(DateTime::<Utc>::from(modified_at).to_rfc3339()),
        Err(err) => {
            diagnostics.record_file_error("modified", path_string.clone(), &err);
            None
        }
    };

    DebugFileMetadata {
        path: path_string,
        present: true,
        bytes: Some(metadata.len()),
        modified_at,
        error: None,
    }
}

fn matches_log_name(file_name: &str) -> bool {
    LOG_BASENAMES.iter().any(|base_name| {
        file_name == *base_name
            || file_name
                .strip_prefix(base_name)
                .is_some_and(|suffix| suffix.starts_with('.'))
    })
}

fn log_artifacts_for_bundle_capture(log_artifacts: &[FileArtifact]) -> Vec<FileArtifact> {
    log_artifacts
        .iter()
        .filter(|artifact| should_capture_log_artifact(artifact))
        .cloned()
        .collect()
}

fn should_capture_log_artifact(artifact: &FileArtifact) -> bool {
    artifact
        .source_path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(log_rotation_index)
        .is_some_and(|rotation| rotation <= log_capture_rotation_limit())
}

fn log_capture_bytes_limit() -> u64 {
    env_u64(LOG_CAPTURE_BYTES_ENV)
        .filter(|limit| *limit > 0)
        .unwrap_or(DEBUG_BUNDLE_LOG_CAPTURE_BYTES_LIMIT)
}

fn log_capture_rotation_limit() -> u32 {
    env_u64(LOG_CAPTURE_ROTATIONS_ENV)
        .and_then(|limit| u32::try_from(limit).ok())
        .unwrap_or(DEBUG_BUNDLE_LOG_CAPTURE_ROTATION_LIMIT)
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse::<u64>().ok()
}

fn log_rotation_index(file_name: &str) -> Option<u32> {
    for base_name in LOG_BASENAMES {
        if file_name == *base_name {
            return Some(0);
        }

        let Some(suffix) = file_name.strip_prefix(base_name) else {
            continue;
        };
        let Some(rotation) = suffix.strip_prefix('.') else {
            continue;
        };
        if rotation.is_empty() || !rotation.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }
        if let Ok(rotation) = rotation.parse::<u32>() {
            return Some(rotation);
        }
    }

    None
}

fn capture_log_artifact(
    artifact: &FileArtifact,
    diagnostics: &mut BundleDiagnostics,
) -> Option<(CapturedFileEntry, Vec<u8>)> {
    let mut file = match fs::File::open(&artifact.source_path) {
        Ok(file) => file,
        Err(err) => {
            diagnostics.record_file_error("open", artifact.source_path.display().to_string(), &err);
            diagnostics.add_warning(format!("Failed to open {}", artifact.source_path.display()));
            return None;
        }
    };
    let source_bytes = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(err) => {
            diagnostics.record_file_error(
                "metadata",
                artifact.source_path.display().to_string(),
                &err,
            );
            diagnostics.add_warning(format!(
                "Failed to read metadata for {}",
                artifact.source_path.display()
            ));
            return None;
        }
    };

    let read_start = source_bytes.saturating_sub(log_capture_bytes_limit());
    if read_start > 0 {
        if let Err(err) = file.seek(SeekFrom::Start(read_start)) {
            diagnostics.record_file_error("seek", artifact.source_path.display().to_string(), &err);
            diagnostics.add_warning(format!("Failed to seek {}", artifact.source_path.display()));
            return None;
        }
    }

    let capture_len = source_bytes.saturating_sub(read_start) as usize;
    let mut bytes = Vec::with_capacity(capture_len);
    if let Err(err) = file.read_to_end(&mut bytes) {
        diagnostics.record_file_error("read", artifact.source_path.display().to_string(), &err);
        diagnostics.add_warning(format!("Failed to read {}", artifact.source_path.display()));
        return None;
    }

    let modified_at = path_modified_rfc3339(&artifact.source_path, diagnostics);
    let truncated = read_start > 0;
    Some((
        CapturedFileEntry {
            archive_path: artifact.archive_path.clone(),
            source_path: artifact.source_path.display().to_string(),
            bytes: bytes.len(),
            source_bytes: truncated.then_some(source_bytes),
            truncated: truncated.then_some(true),
            modified_at,
        },
        bytes,
    ))
}

fn capture_artifact(
    artifact: &FileArtifact,
    diagnostics: &mut BundleDiagnostics,
) -> Option<(CapturedFileEntry, Vec<u8>)> {
    let file = match fs::File::open(&artifact.source_path) {
        Ok(file) => file,
        Err(err) => {
            diagnostics.record_file_error("open", artifact.source_path.display().to_string(), &err);
            diagnostics.add_warning(format!("Failed to open {}", artifact.source_path.display()));
            return None;
        }
    };
    let source_bytes = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(err) => {
            diagnostics.record_file_error(
                "metadata",
                artifact.source_path.display().to_string(),
                &err,
            );
            return None;
        }
    };
    let limit = artifact.bytes_limit.unwrap_or(source_bytes);
    let mut bytes = Vec::with_capacity(limit.min(source_bytes).min(1024 * 1024) as usize);
    if let Err(err) = file.take(limit.saturating_add(1)).read_to_end(&mut bytes) {
        diagnostics.record_file_error("read", artifact.source_path.display().to_string(), &err);
        diagnostics.add_warning(format!("Failed to read {}", artifact.source_path.display()));
        return None;
    }
    let truncated = bytes.len() as u64 > limit;
    if truncated {
        bytes.truncate(limit as usize);
        diagnostics.add_warning(format!(
            "Truncated {} to its {} byte bundle limit.",
            artifact.source_path.display(),
            limit
        ));
    }

    let modified_at = path_modified_rfc3339(&artifact.source_path, diagnostics);
    Some((
        CapturedFileEntry {
            archive_path: artifact.archive_path.clone(),
            source_path: artifact.source_path.display().to_string(),
            bytes: bytes.len(),
            source_bytes: truncated.then_some(source_bytes),
            truncated: truncated.then_some(true),
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

pub(crate) fn build_runtime_health_json(
    state: &SharedState,
    generated_at: DateTime<Utc>,
) -> Result<String> {
    let now_epoch_ms = generated_at.timestamp_millis();
    let now_instant = std::time::Instant::now();
    let s = lock_state(state)?;

    let mut hubs = s
        .hubs
        .iter()
        .map(|(hub_key, hub)| {
            let credentials = s.hub_credentials.get(hub_key);
            RuntimeHubHealth {
                hub_key: hub_key.to_string(),
                hub_type: hub_key.hub_type.as_str().to_string(),
                address: hub_key.address.clone(),
                connected: s.hub_is_connected(hub_key),
                seen_connected_once: s.hub_seen_connected_once(hub_key),
                sync_in_progress: s.hub_sync_in_progress.contains(hub_key),
                runtime_present: hub.runtime.is_some(),
                registry_present: hub.registry.is_some(),
                discovery_present: hub.discovery.is_some(),
                shutdown_requested: hub.shutdown.load(std::sync::atomic::Ordering::Relaxed),
                credentials_present: credentials.is_some(),
                credentials_can_connect: credentials
                    .is_some_and(|credentials| credentials.can_connect()),
                credentials_secrets_redacted: credentials
                    .map(|credentials| credentials.secrets_redacted)
                    .unwrap_or(false),
                pending_disconnect_remaining_secs: s.hub_pending_disconnect_at.get(hub_key).map(
                    |deadline| {
                        deadline
                            .checked_duration_since(now_instant)
                            .unwrap_or_default()
                            .as_secs_f64()
                    },
                ),
                reconnect_sync_age_secs: s
                    .hub_reconnect_sync_at
                    .get(hub_key)
                    .map(|last| now_instant.duration_since(*last).as_secs_f64()),
                startup_retry: s.hub_startup_retry(hub_key).map(|retry| {
                    RuntimeHubStartupRetryHealth {
                        attempt_count: retry.attempt_count,
                        first_failure_epoch_ms: retry.first_failure_epoch_ms,
                        last_failure_epoch_ms: retry.last_failure_epoch_ms,
                        next_retry_epoch_ms: retry.next_retry_epoch_ms,
                        manual_retry_required: retry.manual_retry_required,
                        last_error: retry.last_error.clone(),
                    }
                }),
                hue_sse: runtime_hue_sse_health(hub),
            }
        })
        .collect::<Vec<_>>();
    for (hub_key, credentials) in &s.hub_credentials {
        if s.hubs.contains_key(hub_key) {
            continue;
        }
        hubs.push(RuntimeHubHealth {
            hub_key: hub_key.to_string(),
            hub_type: hub_key.hub_type.as_str().to_string(),
            address: hub_key.address.clone(),
            connected: s.hub_is_connected(hub_key),
            seen_connected_once: s.hub_seen_connected_once(hub_key),
            sync_in_progress: s.hub_sync_in_progress.contains(hub_key),
            runtime_present: false,
            registry_present: false,
            discovery_present: false,
            shutdown_requested: false,
            credentials_present: true,
            credentials_can_connect: credentials.can_connect(),
            credentials_secrets_redacted: credentials.secrets_redacted,
            pending_disconnect_remaining_secs: s.hub_pending_disconnect_at.get(hub_key).map(
                |deadline| {
                    deadline
                        .checked_duration_since(now_instant)
                        .unwrap_or_default()
                        .as_secs_f64()
                },
            ),
            reconnect_sync_age_secs: s
                .hub_reconnect_sync_at
                .get(hub_key)
                .map(|last| now_instant.duration_since(*last).as_secs_f64()),
            startup_retry: s
                .hub_startup_retry(hub_key)
                .map(|retry| RuntimeHubStartupRetryHealth {
                    attempt_count: retry.attempt_count,
                    first_failure_epoch_ms: retry.first_failure_epoch_ms,
                    last_failure_epoch_ms: retry.last_failure_epoch_ms,
                    next_retry_epoch_ms: retry.next_retry_epoch_ms,
                    manual_retry_required: retry.manual_retry_required,
                    last_error: retry.last_error.clone(),
                }),
            hue_sse: None,
        });
    }
    hubs.sort_by(|left, right| left.hub_key.cmp(&right.hub_key));

    let mut motion_entries = s
        .motion_snapshots
        .iter()
        .map(|(node_id, snap)| RuntimeMotionEntry {
            node_id: node_id.clone(),
            motion_active: snap.motion_active,
            motion_owned: snap.motion_owned,
            remaining_secs: snap.remaining_secs,
            timeout_secs: snap.timeout_secs,
            warning_active: snap.warning_active,
        })
        .collect::<Vec<_>>();
    motion_entries.sort_by(|left, right| left.node_id.cmp(&right.node_id));

    let mut observed_power_entries = s
        .room_observed_power
        .iter()
        .map(|(cache_key, observed)| RuntimeObservedPowerEntry {
            cache_key: cache_key.clone(),
            lights_on: observed.lights_on,
            observed_at_epoch_ms: observed.observed_at_epoch_ms,
            source: observed.source.as_str(),
        })
        .collect::<Vec<_>>();
    observed_power_entries.sort_by(|left, right| {
        right
            .observed_at_epoch_ms
            .cmp(&left.observed_at_epoch_ms)
            .then_with(|| left.cache_key.cmp(&right.cache_key))
    });
    observed_power_entries.truncate(75);

    let active_light_node_count = s.topology.periodic_light_nodes(&s.canonical_registry).len();
    let last_tick_age_secs = if s.last_tick_epoch_ms == 0 {
        None
    } else {
        Some(((now_epoch_ms - s.last_tick_epoch_ms as i64) / 1000).max(0))
    };
    let last_change_age_secs = s
        .last_active_mode_change_utc_ms
        .map(|epoch_ms| ((now_epoch_ms - epoch_ms) / 1000).max(0));
    let oldest_pending_periodic_tick = s
        .pending_periodic_ticks
        .iter()
        .max_by_key(|(_, pending)| now_instant.saturating_duration_since(pending.enqueued_at));
    let oldest_pending_periodic_tick_node_id =
        oldest_pending_periodic_tick.map(|(node_id, _)| node_id.clone());
    let oldest_pending_periodic_tick_age_secs = oldest_pending_periodic_tick.map(|(_, pending)| {
        now_instant
            .saturating_duration_since(pending.enqueued_at)
            .as_secs_f64()
    });

    let snapshot = RuntimeHealthSnapshot {
        schema_version: DEBUG_BUNDLE_SCHEMA_VERSION,
        generated_at: generated_at.to_rfc3339(),
        now_epoch_ms,
        platform: RuntimePlatformHealth {
            firmware_version: s.firmware_version.to_string(),
            platform_type: s.platform_type.to_string(),
            platform_context: s.platform_context.to_string(),
            data_dir: s.data_dir.clone(),
            listen_port: s.listen_port,
            eager_tls_warmup: s.platform.eager_tls_warmup,
            full_device_discovery: s.platform.full_device_discovery,
        },
        mode: RuntimeModeHealth {
            active_mode: format!("{:?}", s.active_mode),
            last_change_cause: format!("{:?}", s.last_active_mode_cause),
            last_change_transition_id: s.last_active_mode_transition_id.clone(),
            last_change_epoch_ms: s.last_active_mode_change_utc_ms,
            last_change_age_secs,
        },
        timing: RuntimeTimingHealth {
            last_tick_epoch_ms: s.last_tick_epoch_ms,
            last_tick_age_secs,
            last_check_hour: s.last_check_hour,
            last_check_age_secs: s
                .last_check_instant
                .map(|instant| now_instant.duration_since(instant).as_secs_f64()),
            last_check_utc_offset_hours: s.last_check_utc_offset_hours,
            default_motion_timeout_secs: s.default_motion_timeout_secs,
            default_fade_ms: s.default_fade_ms,
            utc_offset_hours: s.utc_offset_hours,
            timezone_name: s.timezone_name.clone(),
            latitude: s.latitude,
            longitude: s.longitude,
        },
        logging: RuntimeLoggingHealth {
            dropped_lines: rhythm_os::logging::native_log_dropped_lines(),
        },
        queues: RuntimeQueueHealth {
            work_queue_configured: s.work_tx.is_some(),
            periodic_work_queue_configured: s.periodic_work_tx.is_some(),
            pending_periodic_ticks: s.pending_periodic_ticks.len(),
            oldest_pending_periodic_tick_node_id,
            oldest_pending_periodic_tick_age_secs,
            pending_hub_event_receivers: s.pending_hub_event_rxs.len(),
            pending_motion_clear: s.pending_motion_clear.len(),
            pending_motion_seed: s.pending_motion_seed.len(),
            event_broadcast_receiver_count: s.event_tx.as_ref().map(|tx| tx.receiver_count()),
            hub_bootstrap_worker_running: s.hub_bootstrap_worker_running,
        },
        hubs,
        topology: RuntimeTopologyHealth {
            room_count: s.topology.rooms().count(),
            device_node_count: s.topology.device_nodes().count(),
            canonical_device_count: s.canonical_registry.device_count(),
            pending_triage_count: s.canonical_registry.triage().pending_count(),
            room_mode_transition_count: s.room_mode_transitions.len(),
            active_light_node_count,
        },
        motion: RuntimeMotionHealth {
            snapshot_count: motion_entries.len(),
            active_count: motion_entries
                .iter()
                .filter(|entry| entry.motion_active)
                .count(),
            owned_count: motion_entries
                .iter()
                .filter(|entry| entry.motion_owned)
                .count(),
            warning_count: motion_entries
                .iter()
                .filter(|entry| entry.warning_active)
                .count(),
            snapshots: motion_entries,
        },
        observed_power: RuntimeObservedPowerHealth {
            entry_count: s.room_observed_power.len(),
            on_count: s
                .room_observed_power
                .values()
                .filter(|observed| observed.lights_on)
                .count(),
            recent_entries: observed_power_entries,
        },
    };

    serde_json::to_string_pretty(&snapshot).context("serializing runtime health snapshot")
}

fn build_process_resources_json(
    generated_at: DateTime<Utc>,
    data_dir: &str,
    diagnostics: &mut BundleDiagnostics,
) -> Result<String> {
    #[cfg(target_os = "linux")]
    let mut snapshot = {
        let mut snapshot = ProcessResourceSnapshot {
            schema_version: DEBUG_BUNDLE_SCHEMA_VERSION,
            generated_at: generated_at.to_rfc3339(),
            target_os: std::env::consts::OS.to_string(),
            open_fd_count: None,
            open_fds: Vec::new(),
            thread_count: None,
            thread_entries_truncated: None,
            threads: Vec::new(),
            status: None,
            limits: None,
            loadavg: None,
            meminfo: None,
            sockstat: None,
            sockstat6: None,
            tcp: None,
            tcp6: None,
            udp: None,
            udp6: None,
            route: None,
            net_dev: None,
            wireless: None,
            arp: None,
            mounts: None,
            partitions: None,
            data_dir_filesystem: None,
        };

        snapshot.open_fds = linux_open_fd_snapshot(diagnostics);
        snapshot.open_fd_count = Some(snapshot.open_fds.len());
        let thread_snapshot = linux_thread_snapshot(diagnostics);
        snapshot.thread_count = Some(thread_snapshot.total_count);
        snapshot.thread_entries_truncated = thread_snapshot.truncated.then_some(true);
        snapshot.threads = thread_snapshot.entries;
        snapshot.status = linux_read_optional_proc_file("/proc/self/status", diagnostics);
        snapshot.limits = linux_read_optional_proc_file("/proc/self/limits", diagnostics);
        snapshot.loadavg = linux_read_optional_proc_file("/proc/loadavg", diagnostics);
        snapshot.meminfo = linux_read_optional_proc_file("/proc/meminfo", diagnostics);
        snapshot.sockstat = linux_read_optional_proc_file("/proc/net/sockstat", diagnostics);
        snapshot.sockstat6 = linux_read_optional_proc_file("/proc/net/sockstat6", diagnostics);
        snapshot.tcp = linux_read_optional_proc_file("/proc/net/tcp", diagnostics);
        snapshot.tcp6 = linux_read_optional_proc_file("/proc/net/tcp6", diagnostics);
        snapshot.udp = linux_read_optional_proc_file("/proc/net/udp", diagnostics);
        snapshot.udp6 = linux_read_optional_proc_file("/proc/net/udp6", diagnostics);
        snapshot.route = linux_read_optional_proc_file("/proc/net/route", diagnostics);
        snapshot.net_dev = linux_read_optional_proc_file("/proc/net/dev", diagnostics);
        snapshot.wireless = linux_read_optional_proc_file("/proc/net/wireless", diagnostics);
        snapshot.arp = linux_read_optional_proc_file("/proc/net/arp", diagnostics);
        snapshot.mounts = linux_read_optional_proc_file("/proc/mounts", diagnostics);
        snapshot.partitions = linux_read_optional_proc_file("/proc/partitions", diagnostics);
        snapshot
    };

    #[cfg(not(target_os = "linux"))]
    let mut snapshot = ProcessResourceSnapshot {
        schema_version: DEBUG_BUNDLE_SCHEMA_VERSION,
        generated_at: generated_at.to_rfc3339(),
        target_os: std::env::consts::OS.to_string(),
        open_fd_count: None,
        open_fds: Vec::new(),
        thread_count: None,
        thread_entries_truncated: None,
        threads: Vec::new(),
        status: None,
        limits: None,
        loadavg: None,
        meminfo: None,
        sockstat: None,
        sockstat6: None,
        tcp: None,
        tcp6: None,
        udp: None,
        udp6: None,
        route: None,
        net_dev: None,
        wireless: None,
        arp: None,
        mounts: None,
        partitions: None,
        data_dir_filesystem: None,
    };

    snapshot.data_dir_filesystem = unix_data_dir_filesystem(data_dir, diagnostics);

    serde_json::to_string_pretty(&snapshot).context("serializing process resource snapshot")
}

#[cfg(unix)]
fn unix_data_dir_filesystem(
    data_dir: &str,
    diagnostics: &mut BundleDiagnostics,
) -> Option<DataDirFilesystem> {
    let path_cstr = match std::ffi::CString::new(data_dir) {
        Ok(c) => c,
        Err(err) => {
            diagnostics.add_warning(format!(
                "data_dir contains an interior NUL byte; skipping statvfs capture: {err}"
            ));
            return None;
        }
    };
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(path_cstr.as_ptr(), &mut stat) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        diagnostics.record_file_error("statvfs", data_dir, &err);
        return None;
    }
    let frsize = stat.f_frsize as u64;
    let bsize = stat.f_bsize as u64;
    let block_size_bytes = if frsize > 0 { frsize } else { bsize };
    let blocks = stat.f_blocks as u64;
    let bavail = stat.f_bavail as u64;
    let total_bytes = blocks.saturating_mul(block_size_bytes);
    let available_bytes = bavail.saturating_mul(block_size_bytes);
    let used_bytes = total_bytes.saturating_sub(available_bytes);
    Some(DataDirFilesystem {
        path: data_dir.to_string(),
        block_size_bytes,
        total_bytes,
        available_bytes,
        used_bytes,
        inodes_total: stat.f_files as u64,
        inodes_available: stat.f_favail as u64,
    })
}

#[cfg(not(unix))]
fn unix_data_dir_filesystem(
    _data_dir: &str,
    _diagnostics: &mut BundleDiagnostics,
) -> Option<DataDirFilesystem> {
    None
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

#[cfg(target_os = "linux")]
#[derive(Clone, Debug)]
struct LinuxTaskEntry {
    tid: String,
    task_dir: PathBuf,
}

#[cfg(target_os = "linux")]
#[derive(Clone, Debug)]
struct LinuxThreadSnapshot {
    entries: Vec<ThreadSnapshotEntry>,
    total_count: usize,
    truncated: bool,
}

#[cfg(target_os = "linux")]
fn linux_thread_snapshot(diagnostics: &mut BundleDiagnostics) -> LinuxThreadSnapshot {
    let entries = match fs::read_dir("/proc/self/task") {
        Ok(entries) => entries,
        Err(err) => {
            diagnostics.record_file_error("read_dir", "/proc/self/task", &err);
            return LinuxThreadSnapshot {
                entries: Vec::new(),
                total_count: 0,
                truncated: false,
            };
        }
    };

    let mut tasks = Vec::<LinuxTaskEntry>::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                diagnostics.record_file_error("read_dir_entry", "/proc/self/task", &err);
                continue;
            }
        };
        let tid = entry.file_name().to_string_lossy().to_string();
        tasks.push(LinuxTaskEntry {
            tid,
            task_dir: entry.path(),
        });
    }

    tasks.sort_by(|left, right| {
        let left_tid = left.tid.parse::<u64>().ok();
        let right_tid = right.tid.parse::<u64>().ok();
        left_tid
            .cmp(&right_tid)
            .then_with(|| left.tid.cmp(&right.tid))
    });

    let total_count = tasks.len();
    let truncated = total_count > THREAD_SNAPSHOT_ENTRY_LIMIT;
    tasks.truncate(THREAD_SNAPSHOT_ENTRY_LIMIT);

    let mut threads = Vec::<ThreadSnapshotEntry>::new();
    for task in tasks {
        let LinuxTaskEntry { tid, task_dir } = task;
        let comm_path = task_dir.join("comm");
        let status_path = task_dir.join("status");
        let name = fs::read_to_string(&comm_path)
            .map(|value| value.trim().to_string())
            .ok();
        let state = match fs::read_to_string(&status_path) {
            Ok(status) => status_field(&status, "State").map(str::to_string),
            Err(err) => {
                diagnostics.record_file_error("read", status_path.display().to_string(), &err);
                None
            }
        };
        let wait_channel =
            linux_read_task_text_limited(&task_dir.join("wchan"), THREAD_WAIT_CHANNEL_BYTES_LIMIT)
                .value;
        threads.push(ThreadSnapshotEntry {
            tid,
            name,
            state,
            wait_channel,
            kernel_stack: None,
            kernel_stack_truncated: None,
            kernel_stack_error: None,
        });
    }

    let mut stack_captures = 0usize;
    for thread in &mut threads {
        if stack_captures >= THREAD_KERNEL_STACK_CAPTURE_LIMIT {
            break;
        }
        if !linux_thread_kernel_stack_candidate(thread.name.as_deref()) {
            continue;
        }

        stack_captures += 1;
        let stack_path = Path::new("/proc/self/task").join(&thread.tid).join("stack");
        let stack = linux_read_task_text_limited(&stack_path, THREAD_KERNEL_STACK_BYTES_LIMIT);
        thread.kernel_stack = stack.value;
        thread.kernel_stack_truncated = stack.truncated.then_some(true);
        thread.kernel_stack_error = stack.error;
    }

    LinuxThreadSnapshot {
        entries: threads,
        total_count,
        truncated,
    }
}

#[cfg(target_os = "linux")]
#[derive(Clone, Debug)]
struct LimitedTaskTextRead {
    value: Option<String>,
    truncated: bool,
    error: Option<String>,
}

#[cfg(target_os = "linux")]
fn linux_read_task_text_limited(path: &Path, byte_limit: usize) -> LimitedTaskTextRead {
    use std::io::Read as _;

    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(err) => {
            return LimitedTaskTextRead {
                value: None,
                truncated: false,
                error: Some(err.to_string()),
            };
        }
    };

    let mut bytes = Vec::with_capacity(byte_limit.saturating_add(1).min(4096));
    let mut reader = file.take(byte_limit.saturating_add(1) as u64);
    if let Err(err) = reader.read_to_end(&mut bytes) {
        return LimitedTaskTextRead {
            value: None,
            truncated: false,
            error: Some(err.to_string()),
        };
    }

    let truncated = bytes.len() > byte_limit;
    bytes.truncate(byte_limit);
    let value = String::from_utf8_lossy(&bytes).trim().to_string();
    LimitedTaskTextRead {
        value: (!value.is_empty()).then_some(value),
        truncated,
        error: None,
    }
}

#[cfg(target_os = "linux")]
fn linux_thread_kernel_stack_candidate(name: Option<&str>) -> bool {
    let Some(name) = name else {
        return false;
    };

    matches!(
        name,
        "periodic"
            | "periodic-worker"
            | "cmd-worker"
            | "event-loop"
            | "liveness-watchd"
            | "liveness-watchdog"
            | "reboot-fallback"
            | "restart-persist"
    ) || name.starts_with("rhythm-main")
}

#[cfg(target_os = "linux")]
fn status_field<'a>(status: &'a str, field: &str) -> Option<&'a str> {
    let prefix = format!("{field}:");
    status
        .lines()
        .find_map(|line| line.strip_prefix(&prefix).map(str::trim))
}

fn build_log_summary_json(
    log_artifacts: &[FileArtifact],
    generated_at: DateTime<Utc>,
    diagnostics: &mut BundleDiagnostics,
) -> Result<String> {
    const RECENT_WARNINGS_LIMIT: usize = 200;
    const RECENT_ERRORS_LIMIT: usize = 200;
    const RECENT_HUB_FULL_LIMIT: usize = 100;
    const RECENT_SSE_LIMIT: usize = 150;
    const RECENT_PERIODIC_LIMIT: usize = 50;
    const RECENT_LAUNCH_LIMIT: usize = 50;
    const TAIL_LIMIT: usize = 250;

    let mut files = Vec::<LogFileSummary>::new();
    let mut total_lines_scanned = 0usize;
    let mut warning_count = 0usize;
    let mut error_count = 0usize;
    let mut hub_event_channel_full_count = 0usize;
    let mut sse_line_count = 0usize;
    let mut periodic_cycle_count = 0usize;
    let mut launch_line_count = 0usize;

    let mut recent_warnings = VecDeque::<LogLineEntry>::new();
    let mut recent_errors = VecDeque::<LogLineEntry>::new();
    let mut recent_hub_event_channel_full = VecDeque::<LogLineEntry>::new();
    let mut recent_sse_lines = VecDeque::<LogLineEntry>::new();
    let mut recent_periodic_cycles = VecDeque::<LogLineEntry>::new();
    let mut recent_launches = VecDeque::<LogLineEntry>::new();
    let mut tail = VecDeque::<LogLineEntry>::new();

    for artifact in log_artifacts {
        let file = match fs::File::open(&artifact.source_path) {
            Ok(file) => file,
            Err(err) => {
                diagnostics.record_file_error(
                    "open_for_summary",
                    artifact.source_path.display().to_string(),
                    &err,
                );
                continue;
            }
        };

        let mut summary = LogFileSummary {
            archive_path: artifact.archive_path.clone(),
            source_path: artifact.source_path.display().to_string(),
            lines_scanned: 0,
            warning_count: 0,
            error_count: 0,
            hub_event_channel_full_count: 0,
            sse_line_count: 0,
            periodic_cycle_count: 0,
            launch_line_count: 0,
        };

        for line in bounded_lines(file) {
            let line = match line {
                Ok(line) => line,
                Err(err) => {
                    diagnostics.record_file_error(
                        "read_line_for_summary",
                        artifact.source_path.display().to_string(),
                        &err,
                    );
                    break;
                }
            };
            summary.lines_scanned += 1;
            total_lines_scanned += 1;

            // The tracing subscriber historically wrote ANSI color codes into
            // the log files, wrapping the level token as `\x1b[33m WARN\x1b[0m`
            // — which ` WARN ` never matches. Classify (and store) the
            // escape-stripped text so colored logs summarize correctly.
            let line = strip_ansi_codes(&line);
            let line = line.as_ref();

            let text = truncate_log_line(line);
            let entry = LogLineEntry {
                archive_path: artifact.archive_path.clone(),
                line_number: summary.lines_scanned,
                text,
            };

            push_limited(&mut tail, TAIL_LIMIT, entry.clone());

            let is_warning = line.contains(" WARN ") || line.contains("\tWARN ");
            let is_error = line.contains(" ERROR ") || line.contains("\tERROR ");
            let is_hub_full = line.contains("Hub event channel full");
            let is_sse = line.contains("hue-sse") || line.contains("sse:");
            let is_periodic =
                line.contains("event=\"periodic_cycle\"") || line.contains("Periodic cycle");
            let is_launch = line.starts_with("rhythm-launch:")
                || line.contains(" Rhythm Linux Appliance ")
                || line.contains(" Rhythm Server ");

            if is_warning {
                summary.warning_count += 1;
                warning_count += 1;
                push_limited(&mut recent_warnings, RECENT_WARNINGS_LIMIT, entry.clone());
            }
            if is_error {
                summary.error_count += 1;
                error_count += 1;
                push_limited(&mut recent_errors, RECENT_ERRORS_LIMIT, entry.clone());
            }
            if is_hub_full {
                summary.hub_event_channel_full_count += 1;
                hub_event_channel_full_count += 1;
                push_limited(
                    &mut recent_hub_event_channel_full,
                    RECENT_HUB_FULL_LIMIT,
                    entry.clone(),
                );
            }
            if is_sse {
                summary.sse_line_count += 1;
                sse_line_count += 1;
                push_limited(&mut recent_sse_lines, RECENT_SSE_LIMIT, entry.clone());
            }
            if is_periodic {
                summary.periodic_cycle_count += 1;
                periodic_cycle_count += 1;
                push_limited(
                    &mut recent_periodic_cycles,
                    RECENT_PERIODIC_LIMIT,
                    entry.clone(),
                );
            }
            if is_launch {
                summary.launch_line_count += 1;
                launch_line_count += 1;
                push_limited(&mut recent_launches, RECENT_LAUNCH_LIMIT, entry);
            }
        }

        files.push(summary);
    }

    let snapshot = LogSummarySnapshot {
        schema_version: DEBUG_BUNDLE_SCHEMA_VERSION,
        generated_at: generated_at.to_rfc3339(),
        files,
        total_lines_scanned,
        warning_count,
        error_count,
        hub_event_channel_full_count,
        sse_line_count,
        periodic_cycle_count,
        launch_line_count,
        recent_warnings: recent_warnings.into(),
        recent_errors: recent_errors.into(),
        recent_hub_event_channel_full: recent_hub_event_channel_full.into(),
        recent_sse_lines: recent_sse_lines.into(),
        recent_periodic_cycles: recent_periodic_cycles.into(),
        recent_launches: recent_launches.into(),
        tail: tail.into(),
    };

    serde_json::to_string_pretty(&snapshot).context("serializing log summary snapshot")
}

/// Strip ANSI CSI escape sequences (`ESC [ ... final-byte`) from a log line.
fn strip_ansi_codes(line: &str) -> std::borrow::Cow<'_, str> {
    if !line.contains('\x1b') {
        return std::borrow::Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            out.push(ch);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
    }
    std::borrow::Cow::Owned(out)
}

fn push_limited<T>(items: &mut VecDeque<T>, limit: usize, item: T) {
    if items.len() >= limit {
        items.pop_front();
    }
    items.push_back(item);
}

fn truncate_log_line(line: &str) -> String {
    const MAX_CHARS: usize = 2_000;
    let mut chars = line.chars();
    let mut truncated = String::new();
    for _ in 0..MAX_CHARS {
        let Some(ch) = chars.next() else {
            return line.to_string();
        };
        truncated.push(ch);
    }
    if chars.next().is_some() {
        truncated.push_str("...<truncated>");
    }
    truncated
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
        .filter(|node| canonical_registry.get(&node.canonical_device_id).is_none())
        .map(|node| node.canonical_device_id.clone())
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
    use chrono::TimeZone;
    use flate2::read::GzDecoder;
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::io::Read;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvRestore {
        values: Vec<(&'static str, Option<OsString>)>,
    }

    impl EnvRestore {
        fn new(names: &[&'static str]) -> Self {
            Self {
                values: names
                    .iter()
                    .map(|name| (*name, std::env::var_os(name)))
                    .collect(),
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (name, value) in &self.values {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

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
    fn file_name_and_log_dir_helpers_sanitize_dedup_and_honor_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _restore = EnvRestore::new(&["RHYTHM_LOG_DIR", "HOME"]);
        let root = unique_test_dir("helpers");
        let env_log_dir = root.join("logs");
        let data_dir = root.join("data");
        let home = root.join("home");
        std::env::set_var("RHYTHM_LOG_DIR", &env_log_dir);
        std::env::set_var("HOME", &home);

        let created_at = Utc
            .with_ymd_and_hms(2026, 5, 20, 12, 34, 56)
            .single()
            .unwrap();
        let runtime = RuntimeSnapshot {
            firmware_version: "1.2.3".to_string(),
            platform_type: "appliance".to_string(),
            platform_context: "rpiz/dev unit".to_string(),
            data_dir: data_dir.display().to_string(),
        };

        assert_eq!(
            bundle_file_name(&runtime, created_at),
            "rhythm-debug-bundle-rpiz-dev-unit-20260520T123456Z.tar.gz"
        );
        assert_eq!(sanitize_filename_component("///"), "");
        assert_eq!(sanitize_filename_component("rpiz_01-beta"), "rpiz_01-beta");

        let dirs = discover_log_dirs(&RuntimeSnapshot {
            platform_context: "server".to_string(),
            ..runtime
        });
        assert_eq!(dirs[0], env_log_dir);
        assert!(dirs.contains(&data_dir.join("log")));
        if cfg!(target_os = "macos") {
            assert!(dirs.contains(&home.join("Library").join("Logs").join("Rhythm")));
        }

        std::env::set_var("RHYTHM_LOG_DIR", data_dir.join("log"));
        let deduped = discover_log_dirs(&RuntimeSnapshot {
            firmware_version: "1.2.3".to_string(),
            platform_type: "server".to_string(),
            platform_context: "server".to_string(),
            data_dir: data_dir.display().to_string(),
        });
        assert_eq!(
            deduped
                .iter()
                .filter(|dir| **dir == data_dir.join("log"))
                .count(),
            1
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn diagnostics_finish_dedups_and_sorts_reported_findings() {
        let started_at = Utc
            .with_ymd_and_hms(2026, 5, 20, 12, 0, 0)
            .single()
            .unwrap();
        let completed_at = started_at + chrono::Duration::milliseconds(25);
        let mut diagnostics = BundleDiagnostics::new(
            started_at,
            "/data".to_string(),
            vec!["/var/log/rhythm".to_string()],
        );
        diagnostics.add_warning("z-warning");
        diagnostics.add_warning("a-warning");
        diagnostics.add_warning("a-warning");
        diagnostics
            .missing_persisted_files
            .push("rooms.json".to_string());
        diagnostics
            .missing_persisted_files
            .push("canonical_registry.json".to_string());
        diagnostics
            .missing_persisted_files
            .push("rooms.json".to_string());
        diagnostics.record_file_error("read", "/b", "later");
        diagnostics.record_file_error("read", "/a", "earlier");

        diagnostics.finish(
            completed_at,
            HostMetadata {
                os: "test-os".to_string(),
                ..HostMetadata::default()
            },
            ProcessMetadata {
                pid: 42,
                ..ProcessMetadata::default()
            },
        );

        assert_eq!(diagnostics.completed_at, Some(completed_at.to_rfc3339()));
        assert_eq!(diagnostics.duration_ms, Some(25));
        assert_eq!(
            diagnostics.missing_persisted_files,
            vec!["canonical_registry.json", "rooms.json"]
        );
        assert_eq!(diagnostics.warnings, vec!["a-warning", "z-warning"]);
        assert_eq!(diagnostics.file_errors[0].path, "/a");
        assert_eq!(diagnostics.host.os, "test-os");
        assert_eq!(diagnostics.process.pid, 42);
    }

    #[test]
    fn discover_artifacts_records_missing_files_and_sorts_matches() {
        let root = unique_test_dir("artifact-discovery");
        let data_dir = root.join("data");
        let log_dir = data_dir.join("log");
        fs::create_dir_all(&log_dir).unwrap();
        fs::write(log_dir.join("rhythm-server.log"), b"active").unwrap();
        fs::write(log_dir.join("rhythm-server.log.1"), b"rotated").unwrap();
        fs::write(log_dir.join("not-rhythm.log"), b"ignored").unwrap();
        fs::create_dir_all(log_dir.join("rhythm-matter.log")).unwrap();
        fs::write(data_dir.join("topology.json"), b"{}").unwrap();
        fs::write(data_dir.join("canonical_registry.json"), b"{}").unwrap();
        fs::write(data_dir.join("hub_registry_z.json"), b"{}").unwrap();
        fs::write(data_dir.join("hub_registry_a.json"), b"{}").unwrap();
        fs::create_dir_all(data_dir.join("ota")).unwrap();
        fs::write(
            data_dir.join(crate::auto_update::AUTO_UPDATE_STATE_RELATIVE_PATH),
            br#"{"schema_version":1}"#,
        )
        .unwrap();

        let generated_at = Utc
            .with_ymd_and_hms(2026, 5, 20, 12, 0, 0)
            .single()
            .unwrap();
        let mut diagnostics =
            BundleDiagnostics::new(generated_at, data_dir.display().to_string(), Vec::new());
        let logs = discover_log_artifacts(std::slice::from_ref(&log_dir), &mut diagnostics);
        assert_eq!(
            logs.iter()
                .map(|artifact| artifact.archive_path.as_str())
                .collect::<Vec<_>>(),
            vec!["logs/rhythm-server.log", "logs/rhythm-server.log.1"]
        );
        assert!(matches_log_name("rhythm-matter.log.4"));
        assert!(!matches_log_name("other-rhythm-server.log"));

        let runtime = RuntimeSnapshot {
            firmware_version: "1.2.3".to_string(),
            platform_type: "appliance".to_string(),
            platform_context: "rpiz".to_string(),
            data_dir: data_dir.display().to_string(),
        };
        let persisted = discover_persisted_artifacts(&runtime, &mut diagnostics);
        assert_eq!(
            persisted
                .iter()
                .map(|artifact| artifact.archive_path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "persisted/canonical_registry.json",
                "persisted/hub_registry_a.json",
                "persisted/hub_registry_z.json",
                "persisted/ota/auto-update-state.json",
                "persisted/topology.json"
            ]
        );
        assert_eq!(diagnostics.missing_persisted_files, vec!["rooms.json"]);

        let missing_runtime = RuntimeSnapshot {
            data_dir: root.join("missing").display().to_string(),
            ..runtime
        };
        let missing = discover_persisted_artifacts(&missing_runtime, &mut diagnostics);
        assert!(missing.is_empty());
        assert!(diagnostics
            .warnings
            .iter()
            .any(|warning| warning.contains("does not exist")));

        let empty_runtime = RuntimeSnapshot {
            data_dir: String::new(),
            ..missing_runtime
        };
        let empty = discover_persisted_artifacts(&empty_runtime, &mut diagnostics);
        assert!(empty.is_empty());
        assert!(diagnostics
            .warnings
            .iter()
            .any(|warning| warning.contains("No data_dir configured")));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn matter_controller_debug_reports_identity_variants_and_missing_identity_warning() {
        let root = unique_test_dir("matter-debug");
        let matter_dir = root.join("matter");
        let chip_dir = matter_dir.join("chip");
        fs::create_dir_all(&chip_dir).unwrap();
        let identity_path = matter_dir.join("fabric-identity.json");
        let generated_at = Utc
            .with_ymd_and_hms(2026, 5, 20, 12, 0, 0)
            .single()
            .unwrap();
        let mut diagnostics =
            BundleDiagnostics::new(generated_at, root.display().to_string(), Vec::new());

        let missing = build_matter_fabric_identity_debug(&identity_path, &mut diagnostics);
        assert!(!missing.file.present);
        assert!(missing.parse_error.is_none());

        fs::write(&identity_path, b"{not json").unwrap();
        let invalid = build_matter_fabric_identity_debug(&identity_path, &mut diagnostics);
        assert!(invalid.file.present);
        assert!(invalid.parse_error.is_some());

        fs::write(
            &identity_path,
            br#"{"schema_version":2,"label":"primary","operational_fabric_id":4660,"ipk_hex":""}"#,
        )
        .unwrap();
        let parsed = build_matter_fabric_identity_debug(&identity_path, &mut diagnostics);
        assert_eq!(parsed.schema_version, Some(2));
        assert_eq!(parsed.label.as_deref(), Some("primary"));
        assert_eq!(
            parsed.operational_fabric_id_hex.as_deref(),
            Some("0x0000000000001234")
        );
        assert!(!parsed.ipk_hex_present);

        fs::remove_file(&identity_path).unwrap();
        fs::write(chip_dir.join("controller-storage.json"), b"storage").unwrap();
        let runtime = RuntimeSnapshot {
            firmware_version: "1.2.3".to_string(),
            platform_type: "appliance".to_string(),
            platform_context: "rpiz".to_string(),
            data_dir: root.display().to_string(),
        };
        let json =
            build_matter_controller_debug_json(&runtime, generated_at, &mut diagnostics).unwrap();
        let snapshot: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(snapshot["storage_without_identity"], true);
        assert!(diagnostics
            .warnings
            .iter()
            .any(|warning| warning.contains("without")));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn file_metadata_capture_and_process_resources_report_edge_cases() {
        let root = unique_test_dir("metadata");
        let file = root.join("file.json");
        let dir = root.join("not-file");
        fs::write(&file, b"hello").unwrap();
        fs::create_dir_all(&dir).unwrap();
        let generated_at = Utc
            .with_ymd_and_hms(2026, 5, 20, 12, 0, 0)
            .single()
            .unwrap();
        let mut diagnostics =
            BundleDiagnostics::new(generated_at, root.display().to_string(), Vec::new());

        let present = debug_file_metadata(&file, &mut diagnostics);
        assert!(present.present);
        assert_eq!(present.bytes, Some(5));
        assert!(present.modified_at.is_some());

        let not_file = debug_file_metadata(&dir, &mut diagnostics);
        assert!(!not_file.present);
        assert_eq!(not_file.error.as_deref(), Some("not a regular file"));

        let missing = debug_file_metadata(&root.join("missing.json"), &mut diagnostics);
        assert!(!missing.present);
        assert!(missing.error.is_none());

        let artifact = FileArtifact {
            source_path: file.clone(),
            archive_path: "persisted/file.json".to_string(),
            bytes_limit: None,
        };
        let (captured, bytes) = capture_artifact(&artifact, &mut diagnostics).unwrap();
        assert_eq!(captured.archive_path, "persisted/file.json");
        assert_eq!(captured.bytes, 5);
        assert_eq!(bytes, b"hello");

        let missing_artifact = FileArtifact {
            source_path: root.join("gone.json"),
            archive_path: "persisted/gone.json".to_string(),
            bytes_limit: None,
        };
        assert!(capture_artifact(&missing_artifact, &mut diagnostics).is_none());
        assert!(diagnostics
            .warnings
            .iter()
            .any(|warning| warning.contains("Failed to open")));

        let process_json =
            build_process_resources_json(generated_at, "bad\0path", &mut diagnostics).unwrap();
        let process: Value = serde_json::from_str(&process_json).unwrap();
        assert_eq!(process["data_dir_filesystem"], Value::Null);
        assert!(diagnostics
            .warnings
            .iter()
            .any(|warning| warning.contains("interior NUL byte")));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn host_recorder_bundle_discovery_is_allowlisted_and_byte_bounded() {
        let root = unique_test_dir("host-recorder-artifacts");
        let recorder = root
            .join(rhythm_host_recorder::ROOT_RELATIVE_PATH)
            .join(rhythm_host_recorder::CURRENT_DIR);
        fs::create_dir_all(&recorder).unwrap();
        fs::write(recorder.join("manifest.json"), b"{}").unwrap();
        fs::write(
            recorder.join("segment-000.ndjson"),
            vec![b'x'; HOST_RECORDER_SEGMENT_BYTES_LIMIT as usize + 1024],
        )
        .unwrap();
        fs::write(
            root.join(rhythm_host_recorder::ROOT_RELATIVE_PATH)
                .join("credentials.txt"),
            b"secret",
        )
        .unwrap();
        let runtime = RuntimeSnapshot {
            firmware_version: "test".to_string(),
            platform_type: "appliance".to_string(),
            platform_context: "rpiz".to_string(),
            data_dir: root.display().to_string(),
        };
        let mut diagnostics =
            BundleDiagnostics::new(Utc::now(), root.display().to_string(), Vec::new());
        let artifacts = discover_host_recorder_artifacts(&runtime, &mut diagnostics);
        assert_eq!(artifacts.len(), 2);
        assert!(!artifacts
            .iter()
            .any(|artifact| artifact.archive_path.contains("credentials")));
        let segment = artifacts
            .iter()
            .find(|artifact| artifact.archive_path.ends_with("segment-000.ndjson"))
            .unwrap();
        let (captured, bytes) = capture_artifact(segment, &mut diagnostics).unwrap();
        assert_eq!(bytes.len(), HOST_RECORDER_SEGMENT_BYTES_LIMIT as usize);
        assert_eq!(captured.truncated, Some(true));
        assert!(diagnostics
            .warnings
            .iter()
            .any(|warning| warning.contains("byte bundle limit")));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_debug_bundle_caps_raw_log_tails_and_skips_deep_rotations() {
        let root = unique_test_dir("bundle-log-cap");
        let data_dir = root.join("data");
        let log_dir = data_dir.join("log");
        fs::create_dir_all(&log_dir).unwrap();

        let limit = DEBUG_BUNDLE_LOG_CAPTURE_BYTES_LIMIT as usize;
        let mut active_log = b"DROP-ACTIVE-PREFIX\n".to_vec();
        active_log.extend(std::iter::repeat(b'a').take(limit));
        active_log.extend_from_slice(b"ACTIVE-TAIL\n");
        fs::write(log_dir.join("rhythm-server.log"), &active_log).unwrap();

        let mut rotated_log = b"DROP-ROTATED-PREFIX\n".to_vec();
        rotated_log.extend(std::iter::repeat(b'b').take(limit));
        rotated_log.extend_from_slice(b"ROTATED-TAIL\n");
        fs::write(log_dir.join("rhythm-server.log.1"), &rotated_log).unwrap();
        fs::write(
            log_dir.join("rhythm-server.log.2"),
            b"deep rotation should not be scanned or archived\n",
        )
        .unwrap();
        fs::write(
            log_dir.join("rhythm-matter.log.2"),
            b"matter deep rotation should not be scanned or archived\n",
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
        let files = unpack_bundle(&bundle.bytes);

        let active_capture = files.get("logs/rhythm-server.log").unwrap();
        assert_eq!(active_capture.len(), limit);
        let active_text = String::from_utf8_lossy(active_capture);
        assert!(active_text.ends_with("ACTIVE-TAIL\n"));
        assert!(!active_text.contains("DROP-ACTIVE-PREFIX"));

        let rotated_capture = files.get("logs/rhythm-server.log.1").unwrap();
        assert_eq!(rotated_capture.len(), limit);
        let rotated_text = String::from_utf8_lossy(rotated_capture);
        assert!(rotated_text.ends_with("ROTATED-TAIL\n"));
        assert!(!rotated_text.contains("DROP-ROTATED-PREFIX"));

        assert!(!files.contains_key("logs/rhythm-server.log.2"));
        assert!(!files.contains_key("logs/rhythm-matter.log.2"));

        let manifest: Value = serde_json::from_slice(files.get("manifest.json").unwrap()).unwrap();
        let captured_logs = manifest["captured_logs"].as_array().unwrap();
        assert_eq!(captured_logs.len(), 2);
        assert!(captured_logs
            .iter()
            .all(|entry| !entry["archive_path"].as_str().unwrap().ends_with(".2")));
        let active_entry = captured_logs
            .iter()
            .find(|entry| entry["archive_path"] == "logs/rhythm-server.log")
            .unwrap();
        assert_eq!(active_entry["bytes"], Value::from(limit as u64));
        assert_eq!(
            active_entry["source_bytes"],
            Value::from(active_log.len() as u64)
        );
        assert_eq!(active_entry["truncated"], true);

        let log_summary: Value =
            serde_json::from_slice(files.get("log_summary.json").unwrap()).unwrap();
        let summary_paths = log_summary["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["archive_path"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(!summary_paths.contains(&"logs/rhythm-server.log.2"));
        assert!(!summary_paths.contains(&"logs/rhythm-matter.log.2"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn log_summary_classifies_patterns_and_truncates_long_lines() {
        let root = unique_test_dir("log-summary");
        let log = root.join("rhythm-server.log");
        let long_line = "x".repeat(2_050);
        fs::write(
            &log,
            format!(
                "2026 INFO sys: Rhythm Server started\n\
                 2026 WARN sys: warning happened\n\
                 2026\tERROR sys: error happened\n\
                 2026 INFO sys: Hub event channel full\n\
                 2026 INFO hue-sse: event arrived\n\
                 2026 INFO sys: sse: reconnect\n\
                 2026 INFO sys: Periodic cycle event=\"periodic_cycle\"\n\
                 rhythm-launch: boot marker\n\
                 {long_line}\n"
            ),
        )
        .unwrap();
        let generated_at = Utc
            .with_ymd_and_hms(2026, 5, 20, 12, 0, 0)
            .single()
            .unwrap();
        let mut diagnostics =
            BundleDiagnostics::new(generated_at, root.display().to_string(), Vec::new());
        let summary = build_log_summary_json(
            &[FileArtifact {
                source_path: log,
                archive_path: "logs/rhythm-server.log".to_string(),
                bytes_limit: None,
            }],
            generated_at,
            &mut diagnostics,
        )
        .unwrap();
        let summary: Value = serde_json::from_str(&summary).unwrap();

        assert_eq!(summary["total_lines_scanned"], 9);
        assert_eq!(summary["warning_count"], 1);
        assert_eq!(summary["error_count"], 1);
        assert_eq!(summary["hub_event_channel_full_count"], 1);
        assert_eq!(summary["sse_line_count"], 2);
        assert_eq!(summary["periodic_cycle_count"], 1);
        assert_eq!(summary["launch_line_count"], 2);
        assert_eq!(
            summary["recent_errors"][0]["line_number"],
            Value::from(3_u64)
        );
        assert!(summary["tail"].as_array().unwrap().last().unwrap()["text"]
            .as_str()
            .unwrap()
            .ends_with("...<truncated>"));

        let _ = fs::remove_dir_all(root);
    }

    /// Regression for issue #123 triage: the tracing subscriber wrote ANSI
    /// color codes into the log files (`\x1b[33m WARN\x1b[0m`), so the plain
    /// ` WARN ` / ` ERROR ` matchers counted 0/0 for every real bundle and
    /// the synthesis card steered triage away from the logs.
    #[test]
    fn log_summary_counts_ansi_colored_levels_and_stores_stripped_text() {
        let root = unique_test_dir("log-summary-ansi");
        let log = root.join("rhythm-server.log");
        fs::write(
            &log,
            concat!(
                "\x1b[2m2026-07-08T17:39:57-04:00\x1b[0m \x1b[31mERROR\x1b[0m pair: commissioning failed\n",
                "\x1b[2m2026-07-08T17:38:25-04:00\x1b[0m \x1b[33m WARN\x1b[0m pair: force-removed node 102\n",
                "\x1b[2m2026-07-08T17:38:26-04:00\x1b[0m \x1b[32m INFO\x1b[0m sys: all good\n",
            ),
        )
        .unwrap();
        let generated_at = Utc
            .with_ymd_and_hms(2026, 7, 8, 21, 41, 0)
            .single()
            .unwrap();
        let mut diagnostics =
            BundleDiagnostics::new(generated_at, root.display().to_string(), Vec::new());
        let summary = build_log_summary_json(
            &[FileArtifact {
                source_path: log,
                archive_path: "logs/rhythm-server.log".to_string(),
                bytes_limit: None,
            }],
            generated_at,
            &mut diagnostics,
        )
        .unwrap();
        let summary: Value = serde_json::from_str(&summary).unwrap();

        assert_eq!(summary["warning_count"], 1);
        assert_eq!(summary["error_count"], 1);
        let error_text = summary["recent_errors"][0]["text"].as_str().unwrap();
        assert!(
            !error_text.contains('\u{1b}'),
            "summary entries should store escape-stripped text, got {error_text:?}"
        );
        assert!(error_text.contains("ERROR pair: commissioning failed"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_debug_bundle_includes_logs_persisted_state_and_diagnostics() {
        let root = unique_test_dir("bundle");
        let data_dir = root.join("data");
        let log_dir = data_dir.join("log");
        fs::create_dir_all(&log_dir).unwrap();
        fs::write(log_dir.join("rhythm-server.log"), b"server-log").unwrap();
        fs::write(log_dir.join("rhythm-matter.log.1"), b"matter-log").unwrap();
        fs::write(log_dir.join("cloudflared.log"), b"cloudflared-log").unwrap();
        fs::write(data_dir.join("topology.json"), br#"{"rooms":[]}"#).unwrap();
        fs::write(
            data_dir.join("rooms.json"),
            br#"{"rooms":[{"id":"office","profile_settings":{"motion_activation_enabled":false}}]}"#,
        )
        .unwrap();
        fs::write(
            data_dir.join("activity_history.json"),
            br#"{"activities":[{"action_id":"set_motion_activation","correlation_id":"motion-test-1","payload":{"requested_enabled":false,"status":"applied"}}]}"#,
        )
        .unwrap();
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
        fs::create_dir_all(data_dir.join("matter").join("chip")).unwrap();
        fs::write(
            data_dir.join("matter").join("fabric-identity.json"),
            br#"{"schema_version":1,"label":"default","operational_fabric_id":100,"ipk_hex":"00112233445566778899aabbccddeeff"}"#,
        )
        .unwrap();
        fs::write(
            data_dir
                .join("matter")
                .join("chip")
                .join("chip_tool_config.controller-storage.ini"),
            b"controller-storage",
        )
        .unwrap();
        fs::write(
            data_dir.join("matter").join("chip").join("devices.json"),
            b"devices",
        )
        .unwrap();
        fs::create_dir_all(data_dir.join("cloudflared")).unwrap();
        fs::write(
            data_dir.join("cloudflared").join("hostname"),
            b"hub.devices.rhythm.lighting\n",
        )
        .unwrap();
        fs::write(
            data_dir.join("cloudflared").join("status.env"),
            b"state=running\nrestart_count=1\n",
        )
        .unwrap();
        fs::write(
            data_dir.join("cloudflared").join("connector_token"),
            b"secret-token",
        )
        .unwrap();
        fs::write(
            data_dir.join("remote_access.json"),
            br#"{"schema_version":1,"enabled":true,"hostname":"hub.devices.rhythm.lighting","connector_token":"secret-token","tunnel_id":"tunnel-id","tunnel_name":"tunnel-name","updated_at_epoch_ms":1780588319000}"#,
        )
        .unwrap();
        fs::create_dir_all(data_dir.join("ota")).unwrap();
        fs::write(
            data_dir.join(crate::auto_update::AUTO_UPDATE_STATE_RELATIVE_PATH),
            br#"{"schema_version":1,"last_check":{"decision":"up_to_date"}}"#,
        )
        .unwrap();
        fs::create_dir_all(data_dir.join(crate::boot_diagnostics::PSTORE_DIR)).unwrap();
        fs::write(
            data_dir.join(crate::boot_diagnostics::PREVIOUS_BOOT_FILE),
            br#"{"schema_version":1,"classification":"unplanned_host_restart_unknown"}"#,
        )
        .unwrap();
        fs::write(
            data_dir
                .join(crate::boot_diagnostics::PSTORE_DIR)
                .join("dmesg-ramoops-0"),
            b"prior-kernel-panic",
        )
        .unwrap();
        let mut host_recorder = rhythm_host_recorder::RingWriter::open(
            &data_dir,
            "test-boot-id",
            rhythm_host_recorder::RingConfig::default(),
        )
        .unwrap();
        host_recorder
            .append(
                "test-boot-id",
                12_000,
                "summary",
                &serde_json::json!({"watchdog":{"status":"ok","value":{"state":"active"}}}),
                true,
            )
            .unwrap();
        fs::write(
            rhythm_host_recorder::recorder_root(&data_dir).join("secret.env"),
            b"TOKEN=must-not-ship",
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
            guard.storage = Some(std::sync::Arc::new(
                rhythm_os::storage::FileStorage::new(data_dir.to_str().unwrap()).unwrap(),
            ));
            let dispatch_generation = guard.light_dispatch_generation;
            guard.pending_periodic_ticks.insert(
                "stalled-node".to_string(),
                rhythm_os::state::PendingPeriodicTick {
                    current_hour: 17.0,
                    dispatch_generation,
                    enqueued_at: std::time::Instant::now() - std::time::Duration::from_secs(42),
                },
            );
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
            files.get("logs/cloudflared.log").map(Vec::as_slice),
            Some(b"cloudflared-log".as_slice())
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
            files.get("persisted/rooms.json").map(Vec::as_slice),
            Some(
                br#"{"rooms":[{"id":"office","profile_settings":{"motion_activation_enabled":false}}]}"#
                    .as_slice()
            )
        );
        assert_eq!(
            files
                .get("persisted/activity_history.json")
                .map(Vec::as_slice),
            Some(
                br#"{"activities":[{"action_id":"set_motion_activation","correlation_id":"motion-test-1","payload":{"requested_enabled":false,"status":"applied"}}]}"#
                    .as_slice()
            )
        );
        assert_eq!(
            files
                .get("persisted/hub_registry_mock_local.json")
                .map(Vec::as_slice),
            Some(br#"{"rooms":[{"id":"office"}]}"#.as_slice())
        );
        assert_eq!(
            files
                .get("persisted/cloudflared/hostname")
                .map(Vec::as_slice),
            Some(b"hub.devices.rhythm.lighting\n".as_slice())
        );
        assert_eq!(
            files
                .get("persisted/cloudflared/status.env")
                .map(Vec::as_slice),
            Some(b"state=running\nrestart_count=1\n".as_slice())
        );
        assert_eq!(
            files
                .get("persisted/ota/auto-update-state.json")
                .map(Vec::as_slice),
            Some(br#"{"schema_version":1,"last_check":{"decision":"up_to_date"}}"#.as_slice())
        );
        assert_eq!(
            files
                .get("persisted/boot-diagnostics/previous-boot.json")
                .map(Vec::as_slice),
            Some(
                br#"{"schema_version":1,"classification":"unplanned_host_restart_unknown"}"#
                    .as_slice()
            )
        );
        assert_eq!(
            files
                .get("persisted/boot-diagnostics/pstore/dmesg-ramoops-0")
                .map(Vec::as_slice),
            Some(b"prior-kernel-panic".as_slice())
        );
        assert!(files
            .contains_key("persisted/boot-diagnostics/host-flight-recorder/current/manifest.json"));
        assert!(files.contains_key(
            "persisted/boot-diagnostics/host-flight-recorder/current/segment-000.ndjson"
        ));
        assert!(!files.contains_key("persisted/boot-diagnostics/host-flight-recorder/secret.env"));
        assert!(!files.contains_key("persisted/cloudflared/connector_token"));
        assert!(files.contains_key("state.json"));
        assert!(files.contains_key("profile_bundle.json"));
        assert!(files.contains_key("topology_debug.json"));
        assert!(files.contains_key("triage_queue.json"));
        assert!(files.contains_key("runtime_health.json"));
        assert!(files.contains_key("remote_access_status.json"));
        assert!(files.contains_key("log_summary.json"));
        assert!(files.contains_key("process_resources.json"));
        assert!(files.contains_key("matter_controller.json"));
        assert!(files.contains_key("host_flight_recorder_summary.json"));
        assert!(files.contains_key("bundle_diagnostics.json"));
        assert!(files.contains_key("manifest.json"));

        let manifest: Value = serde_json::from_slice(files.get("manifest.json").unwrap()).unwrap();
        assert_eq!(manifest["kind"], "debug_bundle");
        assert_eq!(manifest["platform_context"], "rpiz");
        assert_eq!(manifest["captured_logs"].as_array().unwrap().len(), 3);
        assert_eq!(
            manifest["captured_persisted_files"]
                .as_array()
                .unwrap()
                .len(),
            12
        );
        assert!(!manifest["missing_persisted_files"]
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
        assert!(manifest["generated_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value["archive_path"] == "matter_controller.json"));
        assert!(manifest["generated_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value["archive_path"] == "host_flight_recorder_summary.json"));
        assert!(manifest["generated_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value["archive_path"] == "runtime_health.json"));
        assert!(manifest["generated_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value["archive_path"] == "remote_access_status.json"));
        assert!(manifest["generated_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value["archive_path"] == "log_summary.json"));
        assert!(manifest["process"]["pid"]
            .as_u64()
            .is_some_and(|pid| pid > 0));

        let diagnostics: Value =
            serde_json::from_slice(files.get("bundle_diagnostics.json").unwrap()).unwrap();
        assert_eq!(diagnostics["kind"], "debug_bundle_diagnostics");
        let remote_access_status: Value =
            serde_json::from_slice(files.get("remote_access_status.json").unwrap()).unwrap();
        assert_eq!(
            remote_access_status["hostname"],
            "hub.devices.rhythm.lighting"
        );
        assert_eq!(remote_access_status["configured"], true);
        assert!(
            !String::from_utf8_lossy(files.get("remote_access_status.json").unwrap())
                .contains("secret-token")
        );
        assert_eq!(
            diagnostics["persisted_data_dir"],
            data_dir.display().to_string()
        );
        assert!(!diagnostics["missing_persisted_files"]
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

        let matter_controller: Value =
            serde_json::from_slice(files.get("matter_controller.json").unwrap()).unwrap();
        assert_eq!(
            matter_controller["schema_version"],
            DEBUG_BUNDLE_SCHEMA_VERSION
        );
        assert_eq!(matter_controller["fabric_identity"]["label"], "default");
        assert_eq!(
            matter_controller["fabric_identity"]["operational_fabric_id_hex"],
            "0x0000000000000064"
        );
        assert_eq!(
            matter_controller["fabric_identity"]["ipk_hex_present"],
            true
        );
        assert!(matter_controller["fabric_identity"]
            .get("ipk_hex")
            .is_none());
        assert_eq!(
            matter_controller["chip_controller_storage"]["present"],
            true
        );
        assert!(matter_controller["chip_controller_storage"]["path"]
            .as_str()
            .unwrap()
            .ends_with("chip_tool_config.controller-storage.ini"));
        assert!(matter_controller["chip_controller_storage_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value["path"]
                .as_str()
                .unwrap()
                .ends_with("controller-storage.json")));
        assert_eq!(matter_controller["storage_without_identity"], false);
        let matter_controller_text =
            std::str::from_utf8(files.get("matter_controller.json").unwrap()).unwrap();
        assert!(
            !matter_controller_text.contains("00112233445566778899aabbccddeeff"),
            "Matter debug metadata must not include IPK contents"
        );

        let runtime_health: Value =
            serde_json::from_slice(files.get("runtime_health.json").unwrap()).unwrap();
        assert_eq!(
            runtime_health["schema_version"],
            DEBUG_BUNDLE_SCHEMA_VERSION
        );
        assert_eq!(runtime_health["platform"]["platform_context"], "rpiz");
        assert_eq!(runtime_health["logging"]["dropped_lines"], 0);
        assert_eq!(
            runtime_health["queues"]["pending_periodic_ticks"],
            Value::from(1_u64)
        );
        assert_eq!(
            runtime_health["queues"]["oldest_pending_periodic_tick_node_id"],
            "stalled-node"
        );
        assert!(
            runtime_health["queues"]["oldest_pending_periodic_tick_age_secs"]
                .as_f64()
                .is_some_and(|age| age >= 40.0),
            "runtime health should report the pending tick age: {runtime_health}"
        );

        let log_summary: Value =
            serde_json::from_slice(files.get("log_summary.json").unwrap()).unwrap();
        assert_eq!(log_summary["schema_version"], DEBUG_BUNDLE_SCHEMA_VERSION);
        assert_eq!(log_summary["total_lines_scanned"], 3);
        assert_eq!(log_summary["files"].as_array().unwrap().len(), 3);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn process_resources_capture_filesystem_state_for_data_dir() {
        let root = unique_test_dir("disk-capture");
        let data_dir = root.join("data");
        fs::create_dir_all(&data_dir).unwrap();

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
        let files = unpack_bundle(&bundle.bytes);
        let process_resources: Value =
            serde_json::from_slice(files.get("process_resources.json").unwrap()).unwrap();

        if cfg!(unix) {
            let fs_info = &process_resources["data_dir_filesystem"];
            assert!(
                fs_info.is_object(),
                "data_dir_filesystem missing on unix: {process_resources}"
            );
            assert_eq!(fs_info["path"], data_dir.display().to_string());
            assert!(fs_info["block_size_bytes"].as_u64().is_some_and(|n| n > 0));
            assert!(fs_info["total_bytes"].as_u64().is_some_and(|n| n > 0));
            assert!(fs_info["available_bytes"].as_u64().is_some());
            assert!(fs_info["used_bytes"].as_u64().is_some());
        }

        if cfg!(target_os = "linux") {
            let thread_count = process_resources["thread_count"]
                .as_u64()
                .expect("thread_count missing on linux") as usize;
            let captured_threads = process_resources["threads"]
                .as_array()
                .expect("threads missing on linux");
            assert!(
                thread_count >= captured_threads.len(),
                "thread_count should report the total observed thread count"
            );
            if process_resources["thread_entries_truncated"] == true {
                assert!(
                    thread_count > captured_threads.len(),
                    "thread_entries_truncated requires omitted thread rows"
                );
            }
            assert!(
                process_resources["mounts"].is_string(),
                "/proc/mounts capture missing on linux"
            );
            assert!(
                process_resources["partitions"].is_string(),
                "/proc/partitions capture missing on linux"
            );
        }

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn task_text_limited_caps_bytes_and_reports_truncation() {
        let root = unique_test_dir("task-text-limit");
        let path = root.join("task-file");
        fs::write(&path, "abcdef\n").unwrap();

        let read = linux_read_task_text_limited(&path, 3);

        assert_eq!(read.value.as_deref(), Some("abc"));
        assert!(read.truncated);
        assert!(read.error.is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kernel_stack_capture_targets_runtime_control_threads() {
        assert!(linux_thread_kernel_stack_candidate(Some("periodic")));
        assert!(linux_thread_kernel_stack_candidate(Some("periodic-worker")));
        assert!(linux_thread_kernel_stack_candidate(Some("cmd-worker")));
        assert!(linux_thread_kernel_stack_candidate(Some("event-loop")));
        assert!(linux_thread_kernel_stack_candidate(Some("liveness-watchd")));
        assert!(linux_thread_kernel_stack_candidate(Some("rhythm-main-rt")));
        assert!(!linux_thread_kernel_stack_candidate(Some(
            "reqwest-internal"
        )));
        assert!(!linux_thread_kernel_stack_candidate(None));
    }

    #[test]
    fn log_summary_recent_windows_capture_newest_log_entries() {
        let root = unique_test_dir("log-recency");
        let data_dir = root.join("data");
        let log_dir = data_dir.join("log");
        fs::create_dir_all(&log_dir).unwrap();

        // Older rotated log: more than RECENT_PERIODIC_LIMIT (50) periodic
        // cycle lines, so if iteration order were wrong the rotated file's
        // entries would entirely crowd out the newest log's marker.
        let mut old_lines = String::new();
        for i in 0..60 {
            old_lines.push_str(&format!(
                "2026-04-27T19:00:{:02}-04:00  INFO periodic sys: Periodic cycle event=\"periodic_cycle\" marker=OLD-{}\n",
                i % 60,
                i
            ));
        }
        let old_path = log_dir.join("rhythm-server.log.1");
        fs::write(&old_path, &old_lines).unwrap();

        let new_line = "2026-05-19T07:49:39-04:00  INFO periodic sys: Periodic cycle event=\"periodic_cycle\" marker=NEW-LINE\n";
        let new_path = log_dir.join("rhythm-server.log");
        fs::write(&new_path, new_line).unwrap();

        // Force mtimes so the rotated log is unambiguously older than the
        // active log, regardless of filesystem mtime granularity.
        let now = SystemTime::now();
        let old_time = now - std::time::Duration::from_secs(86_400);
        fs::File::options()
            .write(true)
            .open(&old_path)
            .unwrap()
            .set_modified(old_time)
            .unwrap();
        fs::File::options()
            .write(true)
            .open(&new_path)
            .unwrap()
            .set_modified(now)
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
        let files = unpack_bundle(&bundle.bytes);
        let log_summary: Value =
            serde_json::from_slice(files.get("log_summary.json").unwrap()).unwrap();

        let recent = log_summary["recent_periodic_cycles"].as_array().unwrap();
        assert_eq!(recent.len(), 50, "sliding window should be at capacity");
        assert!(
            recent.iter().any(|entry| entry["text"]
                .as_str()
                .is_some_and(|text| text.contains("marker=NEW-LINE"))),
            "recent_periodic_cycles should retain the newest log's entry, got: {recent:#?}"
        );
        let last = recent.last().unwrap();
        assert_eq!(
            last["archive_path"], "logs/rhythm-server.log",
            "newest entry should sit at the back of the sliding window"
        );

        let tail = log_summary["tail"].as_array().unwrap();
        assert!(
            tail.iter()
                .any(|entry| entry["archive_path"] == "logs/rhythm-server.log"),
            "tail should include lines from the active log"
        );

        let _ = fs::remove_dir_all(root);
    }
}
