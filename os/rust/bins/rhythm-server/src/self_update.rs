//! Self-update from a static OTA feed.
//!
//! Prefers a per-platform `manifest.json` feed hosted outside the repo. The
//! manifest advertises an archive bundle for coordinated `rhythm-server` /
//! `rhythm-chipd` updates and optional full-image artifacts for appliance
//! update flows.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};

use chrono::Utc;
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tar::Archive;

use crate::bootstate;
use rhythm_os::server_event::OtaUpdateStage;

const DEFAULT_UPDATE_BASE_URL: &str = "https://dl.rhythm.lighting/server";
const APPLIANCE_INSTALL_PATH: &str = "/usr/bin/rhythm-server";
const APPLIANCE_BOOT_MOUNT: &str = "/boot";
const APPLIANCE_BOOT_DEVICE: &str = "/dev/mmcblk0p1";
const APPLIANCE_ROOTFS_A_DEVICE: &str = "/dev/mmcblk0p2";
const APPLIANCE_ROOTFS_B_DEVICE: &str = "/dev/mmcblk0p3";
const APPLIANCE_OTA_STAGING_DIR: &str = "/data/ota";
const APPLIANCE_CMDLINE_PATH: &str = "/boot/cmdline.txt";
const APPLIANCE_CMDLINE_BACKUP_PATH: &str = "/boot/cmdline.txt.bak";
const APPLIANCE_BOOT_STATE_PATH: &str = "/boot/rhythm-bootstate.env";
const APPLIANCE_BOOT_STATE_BACKUP_PATH: &str = "/boot/rhythm-bootstate.env.bak";
const APPLIANCE_REBOOT_FALLBACK_SECS: u64 = 30;

static APPLY_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RestartStrategy {
    SupervisorExit,
    ApplianceReboot,
}

/// Which OTA feed to read from.
///
/// `Beta` is the rolling feed populated by every `v*` tag on CI. `Stable` is a
/// curated subset populated only by manual `scripts/release.sh --promote-stable`
/// runs and is what auto-updating appliances follow overnight.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateChannel {
    Beta,
    Stable,
}

impl UpdateChannel {
    fn feed_suffix(self) -> Option<&'static str> {
        match self {
            UpdateChannel::Beta => None,
            UpdateChannel::Stable => Some("-stable"),
        }
    }
}

/// Read the user's preferred OTA channel from shared state.
///
/// `auto_update == true` (the factory default) → stable; the user has opted
/// into curated overnight updates. `false` → beta; the user wants the rolling
/// CI feed and manual control over when updates apply. Non-appliance runtimes
/// stay on beta because stable promotion is only defined for the rpiz feed.
pub fn channel_from_state(state: &rhythm_os::state::SharedState) -> UpdateChannel {
    if !is_appliance_runtime_default() {
        return UpdateChannel::Beta;
    }

    let auto_update = state.lock().map(|s| s.auto_update).unwrap_or(true);
    if auto_update {
        UpdateChannel::Stable
    } else {
        UpdateChannel::Beta
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ApplianceSlot {
    A,
    B,
}

impl ApplianceSlot {
    fn as_str(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
        }
    }

    fn root_device(self) -> &'static str {
        match self {
            Self::A => APPLIANCE_ROOTFS_A_DEVICE,
            Self::B => APPLIANCE_ROOTFS_B_DEVICE,
        }
    }

    fn inactive(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OtaUpdateState {
    Idle,
    Checking,
    Ready,
    Updating,
    Restarting,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateReason {
    VersionMismatch,
    ComponentDrift,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseArtifactKind {
    ArchiveBundle,
    DiskImage,
    RootfsImage,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Serialize)]
pub struct OtaCapabilities {
    pub strategy: &'static str,
    pub scope: &'static str,
    pub can_check: bool,
    pub can_update: bool,
    pub can_upload: bool,
    pub requires_restart: bool,
    pub rollback: &'static str,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub payloads: Vec<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UpdateTargetSummary {
    pub archive_path: String,
    pub destination: String,
    pub required: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UpdateImageAsset {
    pub name: String,
    pub kind: ReleaseArtifactKind,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compression: Option<String>,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Serialize)]
pub struct OtaStatus {
    pub state: OtaUpdateState,
    pub current_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_available: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_reason: Option<UpdateReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at_epoch_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum_verified: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub install_targets: Vec<UpdateTargetSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_assets: Vec<UpdateImageAsset>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct OtaStatusHandle {
    inner: Arc<Mutex<OtaStatus>>,
}

#[allow(dead_code)]
impl OtaStatusHandle {
    pub fn new(current_version: &str) -> Self {
        Self {
            inner: Arc::new(Mutex::new(OtaStatus {
                state: OtaUpdateState::Idle,
                current_version: current_version.to_string(),
                latest_version: None,
                target_version: None,
                update_available: None,
                update_reason: None,
                checked_at_epoch_ms: None,
                checksum_verified: None,
                install_targets: Vec::new(),
                image_assets: Vec::new(),
                message: None,
                last_error: None,
            })),
        }
    }

    pub fn capabilities(&self) -> OtaCapabilities {
        let appliance = restart_strategy() == RestartStrategy::ApplianceReboot;
        OtaCapabilities {
            strategy: "self_pull",
            scope: if appliance {
                "rootfs_slot"
            } else {
                "component_bundle"
            },
            can_check: true,
            can_update: true,
            can_upload: false,
            requires_restart: true,
            rollback: if appliance {
                "automatic_slot_switch"
            } else {
                "backup_files"
            },
            payloads: if appliance {
                vec!["archive_bundle", "rootfs_image"]
            } else {
                vec!["archive_bundle"]
            },
        }
    }

    pub fn snapshot(&self) -> OtaStatus {
        self.inner
            .lock()
            .map(|status| status.clone())
            .unwrap_or_else(|_| OtaStatus {
                state: OtaUpdateState::Error,
                current_version: "unknown".to_string(),
                latest_version: None,
                target_version: None,
                update_available: None,
                update_reason: None,
                checked_at_epoch_ms: Some(now_ms()),
                checksum_verified: None,
                install_targets: Vec::new(),
                image_assets: Vec::new(),
                message: Some("OTA state lock poisoned".to_string()),
                last_error: Some("OTA state lock poisoned".to_string()),
            })
    }

    pub fn mark_checking(&self) {
        self.with_status(|status| {
            status.state = OtaUpdateState::Checking;
            status.checked_at_epoch_ms = Some(now_ms());
            status.target_version = None;
            status.update_reason = None;
            status.checksum_verified = None;
            status.install_targets.clear();
            status.image_assets.clear();
            status.message = Some("Checking for updates...".to_string());
            status.last_error = None;
        });
    }

    pub fn record_check_result(&self, info: &UpdateInfo) {
        self.with_status(|status| {
            status.state = if info.update_available {
                OtaUpdateState::Ready
            } else {
                OtaUpdateState::Idle
            };
            status.latest_version = Some(info.latest_version.clone());
            status.update_available = Some(info.update_available);
            status.update_reason = info.update_reason;
            status.checked_at_epoch_ms = Some(now_ms());
            status.target_version = None;
            status.checksum_verified = None;
            status.install_targets = info.install_targets.clone();
            status.image_assets = info.image_assets.clone();
            status.message = Some(match info.update_reason {
                Some(UpdateReason::VersionMismatch) => {
                    format!("Update available: v{}", info.latest_version)
                }
                Some(UpdateReason::ComponentDrift) => {
                    format!("Repairing OTA bundle for v{}", info.latest_version)
                }
                None => "Already up to date".to_string(),
            });
            status.last_error = None;
        });
    }

    pub fn begin_update(&self, target_version: &str) -> Result<(), String> {
        let mut status = self
            .inner
            .lock()
            .map_err(|_| "OTA state lock poisoned".to_string())?;

        if matches!(
            status.state,
            OtaUpdateState::Updating | OtaUpdateState::Restarting
        ) {
            return Err("Update already in progress".to_string());
        }

        status.state = OtaUpdateState::Updating;
        status.target_version = Some(target_version.to_string());
        status.message = Some(format!("Installing v{}...", target_version));
        status.last_error = None;
        status.checksum_verified = None;
        Ok(())
    }

    pub fn mark_restarting(
        &self,
        previous_version: &str,
        new_version: &str,
        checksum_verified: Option<bool>,
    ) {
        self.with_status(|status| {
            status.state = OtaUpdateState::Restarting;
            status.latest_version = Some(new_version.to_string());
            status.target_version = Some(new_version.to_string());
            status.update_available = Some(false);
            status.update_reason = None;
            status.checked_at_epoch_ms = Some(now_ms());
            status.checksum_verified = checksum_verified;
            status.message = Some(if previous_version == new_version {
                format!("Updated OTA bundle for v{}, restarting...", new_version)
            } else {
                format!(
                    "Updated from v{} to v{}, restarting...",
                    previous_version, new_version
                )
            });
            status.last_error = None;
        });
    }

    pub fn mark_error(&self, error: impl Into<String>) {
        let error = error.into();
        self.with_status(|status| {
            status.state = OtaUpdateState::Error;
            status.checked_at_epoch_ms = Some(now_ms());
            status.checksum_verified = None;
            status.message = Some("Update failed".to_string());
            status.last_error = Some(error.clone());
        });
    }

    fn with_status(&self, f: impl FnOnce(&mut OtaStatus)) {
        if let Ok(mut status) = self.inner.lock() {
            f(&mut status);
        }
    }
}

pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub update_reason: Option<UpdateReason>,
    pub download_url: Option<String>,
    pub expected_sha256: Option<String>,
    pub asset_name: Option<String>,
    pub install_targets: Vec<UpdateTargetSummary>,
    pub image_assets: Vec<UpdateImageAsset>,
    resolved_install_targets: Vec<InstallTarget>,
}

pub struct ApplyResult {
    pub checksum_verified: Option<bool>,
    pub installed_targets: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct UpdateProgress {
    pub stage: OtaUpdateStage,
    pub message: String,
    pub downloaded_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
}

impl UpdateProgress {
    fn stage(stage: OtaUpdateStage, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
            downloaded_bytes: None,
            total_bytes: None,
        }
    }

    fn downloading(
        message: impl Into<String>,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    ) -> Self {
        Self {
            stage: OtaUpdateStage::Downloading,
            message: message.into(),
            downloaded_bytes: Some(downloaded_bytes),
            total_bytes,
        }
    }
}

impl UpdateInfo {
    pub fn apply_blocking(&self) -> Result<ApplyResult, String> {
        self.apply_blocking_with_progress(|_| {})
    }

    pub fn apply_blocking_with_progress<F>(&self, progress: F) -> Result<ApplyResult, String>
    where
        F: Fn(UpdateProgress) + Send + Sync,
    {
        let _guard = acquire_apply_lock()?;
        self.apply_blocking_with_progress_locked(progress)
    }

    fn apply_blocking_with_progress_locked<F>(&self, progress: F) -> Result<ApplyResult, String>
    where
        F: Fn(UpdateProgress) + Send + Sync,
    {
        if restart_strategy() == RestartStrategy::ApplianceReboot {
            if let Some(image_asset) = self.preferred_appliance_image_asset() {
                return apply_appliance_image_blocking(
                    image_asset,
                    &self.current_version,
                    &self.latest_version,
                    self.expected_sha256.as_deref(),
                    &progress,
                );
            }
        }

        let download_url = self
            .download_url
            .as_deref()
            .ok_or_else(|| "No download URL for this platform".to_string())?;
        let asset_name = self
            .asset_name
            .as_deref()
            .ok_or_else(|| "No release asset for this platform".to_string())?;

        apply_payload_blocking(
            download_url,
            asset_name,
            self.expected_sha256.as_deref(),
            &self.resolved_install_targets,
            &progress,
        )
    }

    fn preferred_appliance_image_asset(&self) -> Option<&UpdateImageAsset> {
        self.image_assets
            .iter()
            .filter(|asset| asset.kind == ReleaseArtifactKind::RootfsImage)
            .min_by_key(|asset| if artifact_uses_gzip(asset) { 0 } else { 1 })
    }
}

fn acquire_apply_lock() -> Result<MutexGuard<'static, ()>, String> {
    APPLY_LOCK.try_lock().map_err(|e| match e {
        TryLockError::WouldBlock => "Update already in progress".to_string(),
        TryLockError::Poisoned(_) => "OTA apply lock poisoned".to_string(),
    })
}

#[derive(Deserialize)]
struct UpdateManifest {
    version: String,
    #[serde(default)]
    package: Option<ManifestArtifact>,
    #[serde(default)]
    images: Vec<ManifestArtifact>,
}

#[derive(Clone, Deserialize)]
struct ManifestArtifact {
    name: String,
    url: String,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    kind: Option<ReleaseArtifactKind>,
    #[serde(default)]
    compression: Option<String>,
    #[serde(default)]
    install: Vec<ManifestInstallTarget>,
}

#[derive(Clone, Deserialize)]
struct ManifestInstallTarget {
    #[serde(default)]
    archive_path: Option<String>,
    #[serde(default)]
    slot: InstallSlot,
    #[serde(default)]
    path: Option<String>,
    #[serde(default = "default_required")]
    required: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
enum InstallSlot {
    #[default]
    #[serde(rename = "self")]
    Current,
    #[serde(rename = "sibling")]
    Sibling,
    #[serde(rename = "absolute")]
    Absolute,
}

#[derive(Clone, Debug)]
struct ResolvedPackage {
    asset_name: String,
    download_url: String,
    expected_sha256: Option<String>,
    install_targets: Vec<InstallTarget>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InstallTarget {
    archive_path: String,
    destination: PathBuf,
    required: bool,
}

#[derive(Clone, Debug)]
struct StagedInstallTarget {
    spec: InstallTarget,
    stage_path: PathBuf,
    backup_path: PathBuf,
}

#[derive(Clone, Debug)]
struct AppliedInstallTarget {
    destination: PathBuf,
    backup_path: PathBuf,
    previously_existed: bool,
}

fn default_required() -> bool {
    true
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn platform_feed_name() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("macos-arm64")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("macos-x86_64")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("linux-amd64")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("linux-aarch64")
    } else if cfg!(all(target_os = "linux", target_arch = "arm")) {
        Some("rpiz")
    } else {
        None
    }
}

/// Check the configured OTA feed for an available update (blocking).
pub fn check_blocking(current_version: &str, channel: UpdateChannel) -> Result<UpdateInfo, String> {
    check_manifest_blocking(current_version, channel)
}

fn check_manifest_blocking(
    current_version: &str,
    channel: UpdateChannel,
) -> Result<UpdateInfo, String> {
    let manifest_url_overridden = std::env::var("RHYTHM_UPDATE_MANIFEST_URL").is_ok();
    let manifest_url = configured_manifest_url(channel)?;

    let client = reqwest::blocking::Client::builder()
        .user_agent("rhythm-server")
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let resp = client
        .get(&manifest_url)
        .send()
        .map_err(|e| format!("Update manifest request failed: {}", e))?;

    if !resp.status().is_success() {
        if channel == UpdateChannel::Stable
            && resp.status() == reqwest::StatusCode::NOT_FOUND
            && !manifest_url_overridden
        {
            return Ok(no_update_info(current_version));
        }
        return Err(format!("Update manifest returned {}", resp.status()));
    }

    let manifest: UpdateManifest = resp
        .json()
        .map_err(|e| format!("Failed to parse update manifest: {}", e))?;
    let install_root = install_target_executable()?;
    let image_assets = manifest
        .images
        .iter()
        .map(|image| resolve_manifest_image(&manifest_url, image))
        .collect::<Result<Vec<_>, _>>()?;
    let package = manifest
        .package
        .as_ref()
        .map(|artifact| resolve_package_artifact(&manifest_url, artifact, &install_root))
        .transpose()?;
    let appliance_rootfs_only =
        restart_strategy() == RestartStrategy::ApplianceReboot && has_rootfs_image(&image_assets);

    if package.is_none() && !appliance_rootfs_only {
        return Err("Update manifest did not provide an archive bundle payload".to_string());
    }

    let version_mismatch = manifest.version != current_version;
    let component_drift = !version_mismatch
        && package
            .as_ref()
            .map(|package| {
                detect_component_drift(&manifest.version, &install_root, &package.install_targets)
            })
            .unwrap_or(false);

    let update_reason = if version_mismatch {
        Some(UpdateReason::VersionMismatch)
    } else if component_drift {
        Some(UpdateReason::ComponentDrift)
    } else {
        None
    };
    let update_available = update_reason.is_some();

    Ok(UpdateInfo {
        current_version: current_version.to_string(),
        latest_version: manifest.version.clone(),
        update_available,
        update_reason,
        download_url: update_available
            .then_some(package.as_ref().map(|package| package.download_url.clone()))
            .flatten(),
        expected_sha256: update_available
            .then_some(
                package
                    .as_ref()
                    .and_then(|package| package.expected_sha256.clone()),
            )
            .flatten(),
        asset_name: update_available
            .then_some(package.as_ref().map(|package| package.asset_name.clone()))
            .flatten(),
        install_targets: package
            .as_ref()
            .map(|package| install_targets_to_summaries(&package.install_targets))
            .unwrap_or_default(),
        image_assets,
        resolved_install_targets: package
            .map(|package| package.install_targets)
            .unwrap_or_default(),
    })
}

fn no_update_info(current_version: &str) -> UpdateInfo {
    UpdateInfo {
        current_version: current_version.to_string(),
        latest_version: current_version.to_string(),
        update_available: false,
        update_reason: None,
        download_url: None,
        expected_sha256: None,
        asset_name: None,
        install_targets: Vec::new(),
        image_assets: Vec::new(),
        resolved_install_targets: Vec::new(),
    }
}

fn configured_manifest_url(channel: UpdateChannel) -> Result<String, String> {
    if let Ok(url) = std::env::var("RHYTHM_UPDATE_MANIFEST_URL") {
        return Ok(url);
    }

    let base_url = std::env::var("RHYTHM_UPDATE_BASE_URL")
        .unwrap_or_else(|_| DEFAULT_UPDATE_BASE_URL.to_string());
    let platform = platform_feed_name().ok_or("Unsupported platform for self-update")?;
    let feed = match channel.feed_suffix() {
        Some(suffix) => format!("{}{}", platform, suffix),
        None => platform.to_string(),
    };
    Ok(format!(
        "{}/{}/manifest.json",
        base_url.trim_end_matches('/'),
        feed
    ))
}

fn resolve_download_url(manifest_url: &str, asset_url: &str) -> Result<String, String> {
    if reqwest::Url::parse(asset_url).is_ok() {
        return Ok(asset_url.to_string());
    }

    let manifest_url =
        reqwest::Url::parse(manifest_url).map_err(|e| format!("Invalid manifest URL: {}", e))?;
    manifest_url
        .join(asset_url)
        .map(|url| url.to_string())
        .map_err(|e| format!("Invalid manifest asset URL: {}", e))
}

fn asset_name_from_url(download_url: &str) -> Result<String, String> {
    let url =
        reqwest::Url::parse(download_url).map_err(|e| format!("Invalid download URL: {}", e))?;
    url.path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.to_string())
        .ok_or_else(|| "Update URL did not contain a filename".to_string())
}

fn resolve_package_artifact(
    manifest_url: &str,
    artifact: &ManifestArtifact,
    install_root: &Path,
) -> Result<ResolvedPackage, String> {
    if artifact.kind != Some(ReleaseArtifactKind::ArchiveBundle) {
        return Err("Manifest package kind must be archive_bundle".to_string());
    }

    let download_url = resolve_download_url(manifest_url, &artifact.url)?;
    let asset_name = if artifact.name.is_empty() {
        asset_name_from_url(&download_url)?
    } else {
        artifact.name.clone()
    };
    if !asset_name.ends_with(".tar.gz") {
        return Err("Manifest package must reference a .tar.gz archive bundle".to_string());
    }
    let install_targets = if artifact.install.is_empty() {
        default_bundle_install_targets(install_root)
    } else {
        resolve_manifest_install_targets(&artifact.install, install_root)?
    };

    Ok(ResolvedPackage {
        asset_name,
        download_url,
        expected_sha256: artifact.sha256.clone(),
        install_targets,
    })
}

fn resolve_manifest_install_targets(
    specs: &[ManifestInstallTarget],
    install_root: &Path,
) -> Result<Vec<InstallTarget>, String> {
    let mut targets = Vec::with_capacity(specs.len());
    for spec in specs {
        let destination =
            match spec.slot {
                InstallSlot::Current => install_root.to_path_buf(),
                InstallSlot::Sibling => {
                    let relative = spec.path.as_deref().ok_or_else(|| {
                        "Install target with slot=sibling requires a path".to_string()
                    })?;
                    let parent = install_root
                        .parent()
                        .ok_or_else(|| "Install target had no parent directory".to_string())?;
                    parent.join(relative)
                }
                InstallSlot::Absolute => PathBuf::from(spec.path.as_deref().ok_or_else(|| {
                    "Install target with slot=absolute requires a path".to_string()
                })?),
            };

        let archive_path = match &spec.archive_path {
            Some(path) => path.clone(),
            None => default_archive_path(spec, &destination)?,
        };

        targets.push(InstallTarget {
            archive_path,
            destination,
            required: spec.required,
        });
    }

    Ok(targets)
}

fn default_archive_path(
    spec: &ManifestInstallTarget,
    destination: &Path,
) -> Result<String, String> {
    if spec.slot == InstallSlot::Current {
        return Ok("rhythm-server".to_string());
    }

    destination
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
        .ok_or_else(|| {
            format!(
                "Cannot infer archive path for install target {}",
                destination.display()
            )
        })
}

fn resolve_manifest_image(
    manifest_url: &str,
    artifact: &ManifestArtifact,
) -> Result<UpdateImageAsset, String> {
    let download_url = resolve_download_url(manifest_url, &artifact.url)?;
    let name = if artifact.name.is_empty() {
        asset_name_from_url(&download_url)?
    } else {
        artifact.name.clone()
    };
    let kind = artifact.kind.unwrap_or_else(|| infer_image_kind(&name));

    Ok(UpdateImageAsset {
        name,
        kind,
        url: download_url,
        sha256: artifact.sha256.clone(),
        size: artifact.size,
        compression: artifact.compression.clone(),
    })
}

fn infer_image_kind(name: &str) -> ReleaseArtifactKind {
    if name.starts_with("rootfs.") {
        ReleaseArtifactKind::RootfsImage
    } else {
        ReleaseArtifactKind::DiskImage
    }
}

fn has_rootfs_image(image_assets: &[UpdateImageAsset]) -> bool {
    image_assets
        .iter()
        .any(|asset| asset.kind == ReleaseArtifactKind::RootfsImage)
}

fn parse_appliance_slot_from_cmdline(cmdline: &str) -> Result<ApplianceSlot, String> {
    for token in cmdline.split_whitespace() {
        if let Some(root_device) = token.strip_prefix("root=") {
            return appliance_slot_from_root_device(root_device);
        }
    }

    Err("Kernel command line did not include a root= device".to_string())
}

fn appliance_slot_from_root_device(root_device: &str) -> Result<ApplianceSlot, String> {
    match root_device {
        APPLIANCE_ROOTFS_A_DEVICE => Ok(ApplianceSlot::A),
        APPLIANCE_ROOTFS_B_DEVICE => Ok(ApplianceSlot::B),
        other => Err(format!("Unsupported appliance root device {}", other)),
    }
}

fn current_appliance_slot() -> Result<ApplianceSlot, String> {
    let cmdline = fs::read_to_string("/proc/cmdline")
        .map_err(|e| format!("Failed to read /proc/cmdline: {}", e))?;
    parse_appliance_slot_from_cmdline(&cmdline)
}

fn appliance_root_arg_for_slot(slot: ApplianceSlot) -> String {
    format!("root={}", slot.root_device())
}

fn rewrite_cmdline_root_device(cmdline: &str, slot: ApplianceSlot) -> Result<String, String> {
    let replacement = appliance_root_arg_for_slot(slot);
    let mut found_root = false;
    let rewritten = cmdline
        .split_whitespace()
        .map(|token| {
            if token.starts_with("root=") {
                found_root = true;
                replacement.clone()
            } else {
                token.to_string()
            }
        })
        .collect::<Vec<_>>();

    if !found_root {
        return Err("cmdline.txt did not contain a root= argument".to_string());
    }

    Ok(rewritten.join(" "))
}

fn mountpoint_is_active(path: &str) -> bool {
    fs::read_to_string("/proc/mounts")
        .ok()
        .map(|mounts| {
            mounts.lines().any(|line| {
                let mut parts = line.split_whitespace();
                let _source = parts.next();
                parts.next() == Some(path)
            })
        })
        .unwrap_or(false)
}

fn ensure_mount(path: &str, device: &str, fs_type: &str) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("Failed to create {}: {}", path, e))?;
    if mountpoint_is_active(path) {
        return Ok(());
    }

    let status = Command::new("/bin/mount")
        .arg("-t")
        .arg(fs_type)
        .arg(device)
        .arg(path)
        .status()
        .or_else(|_| {
            Command::new("mount")
                .arg("-t")
                .arg(fs_type)
                .arg(device)
                .arg(path)
                .status()
        })
        .map_err(|e| format!("Failed to mount {} on {}: {}", device, path, e))?;

    if !status.success() {
        return Err(format!(
            "Mounting {} on {} failed with status {:?}",
            device,
            path,
            status.code()
        ));
    }

    Ok(())
}

fn write_appliance_boot_state(
    current_slot: ApplianceSlot,
    target_slot: ApplianceSlot,
    current_version: &str,
    pending_version: &str,
) -> Result<(), String> {
    let body = format!(
        "RHYTHM_ACTIVE_SLOT={}\nRHYTHM_LAST_GOOD_SLOT={}\nRHYTHM_PENDING_SLOT={}\nRHYTHM_PENDING_VERSION={}\nRHYTHM_ACTIVE_VERSION={}\nRHYTHM_BOOT_STATUS=pending\nRHYTHM_LAST_UPDATE_EPOCH_MS={}\nRHYTHM_LAST_ROLLBACK_SLOT=\nRHYTHM_LAST_ROLLBACK_VERSION=\nRHYTHM_LAST_ROLLBACK_EPOCH_MS=\n",
        current_slot.as_str(),
        current_slot.as_str(),
        target_slot.as_str(),
        pending_version,
        current_version,
        now_ms()
    );
    bootstate::write_with_backup(
        Path::new(APPLIANCE_BOOT_STATE_PATH),
        Path::new(APPLIANCE_BOOT_STATE_BACKUP_PATH),
        &body,
    )
}

fn update_appliance_cmdline_for_slot(slot: ApplianceSlot) -> Result<(), String> {
    ensure_mount(APPLIANCE_BOOT_MOUNT, APPLIANCE_BOOT_DEVICE, "vfat")?;
    rewrite_cmdline_file(
        Path::new(APPLIANCE_CMDLINE_PATH),
        Path::new(APPLIANCE_CMDLINE_BACKUP_PATH),
        slot,
    )
}

fn rewrite_cmdline_file(
    cmdline_path: &Path,
    backup_path: &Path,
    slot: ApplianceSlot,
) -> Result<(), String> {
    let current_cmdline = fs::read_to_string(cmdline_path)
        .map_err(|e| format!("Failed to read {}: {}", cmdline_path.display(), e))?;
    let rewritten = rewrite_cmdline_root_device(&current_cmdline, slot)?;
    let new_contents = format!("{}\n", rewritten);

    // Sync the backup before touching the primary so a power loss mid-rewrite
    // leaves a recoverable copy on /boot.
    bootstate::atomic_write_with_sync(backup_path, current_cmdline.as_bytes())?;
    bootstate::atomic_write_with_sync(cmdline_path, new_contents.as_bytes())?;
    Ok(())
}

fn artifact_uses_gzip(asset: &UpdateImageAsset) -> bool {
    asset.compression.as_deref() == Some("gzip") || asset.name.ends_with(".gz")
}

fn artifact_staging_path(asset_name: &str) -> PathBuf {
    let safe_name = asset_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    Path::new(APPLIANCE_OTA_STAGING_DIR).join(format!("{}.download", safe_name))
}

fn write_image_artifact_to_device(
    artifact_path: &Path,
    target_device: &Path,
    gzip: bool,
) -> Result<(), String> {
    let source_file = File::open(artifact_path)
        .map_err(|e| format!("Failed to open {}: {}", artifact_path.display(), e))?;
    let mut source: Box<dyn Read> = if gzip {
        Box::new(GzDecoder::new(source_file))
    } else {
        Box::new(source_file)
    };

    let mut target = File::options()
        .write(true)
        .open(target_device)
        .map_err(|e| {
            format!(
                "Failed to open {} for writing: {}",
                target_device.display(),
                e
            )
        })?;
    let metadata = target
        .metadata()
        .map_err(|e| format!("Failed to stat {}: {}", target_device.display(), e))?;
    if metadata.is_file() {
        target
            .set_len(0)
            .map_err(|e| format!("Failed to reset {}: {}", target_device.display(), e))?;
    }

    std::io::copy(&mut source, &mut target)
        .map_err(|e| format!("Failed to write {}: {}", target_device.display(), e))?;
    target
        .flush()
        .map_err(|e| format!("Failed to flush {}: {}", target_device.display(), e))?;
    target
        .sync_all()
        .map_err(|e| format!("Failed to sync {}: {}", target_device.display(), e))?;

    let _ = Command::new("sync").status();

    Ok(())
}

fn apply_appliance_image_blocking(
    image_asset: &UpdateImageAsset,
    current_version: &str,
    latest_version: &str,
    fallback_sha256: Option<&str>,
    progress: &(impl Fn(UpdateProgress) + Send + Sync),
) -> Result<ApplyResult, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("rhythm-server")
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let current_slot = current_appliance_slot()?;
    let target_slot = current_slot.inactive();

    ensure_mount(APPLIANCE_BOOT_MOUNT, APPLIANCE_BOOT_DEVICE, "vfat")?;
    fs::create_dir_all(APPLIANCE_OTA_STAGING_DIR)
        .map_err(|e| format!("Failed to create {}: {}", APPLIANCE_OTA_STAGING_DIR, e))?;

    let download_path = artifact_staging_path(&image_asset.name);
    remove_if_exists(&download_path);
    let download_message = format!("Downloading {}", image_asset.name);
    progress(UpdateProgress::stage(
        OtaUpdateStage::Downloading,
        download_message.clone(),
    ));
    download_release_with_progress(
        &client,
        &image_asset.url,
        &download_path,
        |downloaded, total| {
            progress(UpdateProgress::downloading(
                download_message.clone(),
                downloaded,
                total,
            ));
        },
    )?;

    let expected_sha256 = image_asset.sha256.as_deref().or(fallback_sha256);
    let checksum_verified = match expected_sha256 {
        Some(expected) => {
            progress(UpdateProgress::stage(
                OtaUpdateStage::Verifying,
                "Verifying update image checksum",
            ));
            let actual = compute_sha256_hex(&download_path)?;
            if actual != expected.to_ascii_lowercase() {
                remove_if_exists(&download_path);
                return Err(format!(
                    "SHA256 mismatch for {}: expected {}, got {}",
                    image_asset.name, expected, actual
                ));
            }
            Some(true)
        }
        None => None,
    };

    if let Err(error) = (|| {
        progress(UpdateProgress::stage(
            OtaUpdateStage::Installing,
            format!(
                "Writing update to inactive rootfs slot {}",
                target_slot.as_str()
            ),
        ));
        write_image_artifact_to_device(
            &download_path,
            Path::new(target_slot.root_device()),
            artifact_uses_gzip(image_asset),
        )?;
        progress(UpdateProgress::stage(
            OtaUpdateStage::Finalizing,
            "Preparing updated boot slot",
        ));
        update_appliance_cmdline_for_slot(target_slot)?;
        write_appliance_boot_state(current_slot, target_slot, current_version, latest_version)?;
        Ok::<(), String>(())
    })() {
        remove_if_exists(&download_path);
        return Err(error);
    }
    remove_if_exists(&download_path);

    Ok(ApplyResult {
        checksum_verified,
        installed_targets: vec![
            format!("rootfs_{}", target_slot.as_str()),
            format!("boot_slot_{}", target_slot.as_str()),
        ],
    })
}

fn default_bundle_install_targets(install_root: &Path) -> Vec<InstallTarget> {
    let parent = install_root.parent().unwrap_or_else(|| Path::new("."));
    vec![
        InstallTarget {
            archive_path: "rhythm-server".to_string(),
            destination: install_root.to_path_buf(),
            required: true,
        },
        InstallTarget {
            archive_path: "rhythm-chipd".to_string(),
            destination: parent.join("rhythm-chipd"),
            required: true,
        },
        InstallTarget {
            archive_path: "rhythm-cli".to_string(),
            destination: parent.join("rhythm-cli"),
            required: false,
        },
    ]
}

fn install_targets_to_summaries(targets: &[InstallTarget]) -> Vec<UpdateTargetSummary> {
    targets
        .iter()
        .map(|target| UpdateTargetSummary {
            archive_path: target.archive_path.clone(),
            destination: target.destination.display().to_string(),
            required: target.required,
        })
        .collect()
}

fn detect_component_drift(
    latest_version: &str,
    install_root: &Path,
    targets: &[InstallTarget],
) -> bool {
    targets
        .iter()
        .filter(|target| target.required)
        .any(|target| {
            if target.destination == install_root {
                return false;
            }

            match read_installed_binary_version(&target.destination) {
                Some(installed_version) => installed_version != latest_version,
                None => true,
            }
        })
}

fn read_installed_binary_version(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }

    let output = Command::new(path).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    parse_version_output(&stdout).or_else(|| parse_version_output(&stderr))
}

fn parse_version_output(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .find_map(normalize_version_candidate)
}

fn normalize_version_candidate(token: &str) -> Option<String> {
    let trimmed = token
        .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-' && c != '_')
        .trim_start_matches('v');

    if trimmed.is_empty() || !trimmed.contains('.') {
        return None;
    }
    if !trimmed
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
    {
        return None;
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return None;
    }
    Some(trimmed.to_string())
}

/// Download and install a new payload, replacing the current server bundle.
fn apply_payload_blocking(
    download_url: &str,
    asset_name: &str,
    expected_sha256: Option<&str>,
    install_targets: &[InstallTarget],
    progress: &(impl Fn(UpdateProgress) + Send + Sync),
) -> Result<ApplyResult, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("rhythm-server")
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let install_root = install_target_executable()?;
    let resolved_targets = if install_targets.is_empty() {
        default_bundle_install_targets(&install_root)
    } else {
        install_targets.to_vec()
    };
    let download_path = install_root.with_extension("download");

    remove_if_exists(&download_path);
    cleanup_install_artifacts(&resolved_targets);

    let download_message = format!("Downloading {}", asset_name);
    progress(UpdateProgress::stage(
        OtaUpdateStage::Downloading,
        download_message.clone(),
    ));
    download_release_with_progress(
        &client,
        download_url,
        &download_path,
        |downloaded, total| {
            progress(UpdateProgress::downloading(
                download_message.clone(),
                downloaded,
                total,
            ));
        },
    )?;

    let checksum_verified = match expected_sha256 {
        Some(expected) => {
            progress(UpdateProgress::stage(
                OtaUpdateStage::Verifying,
                "Verifying update bundle checksum",
            ));
            let actual = compute_sha256_hex(&download_path)?;
            if actual != expected.to_ascii_lowercase() {
                remove_if_exists(&download_path);
                return Err(format!(
                    "SHA256 mismatch for {}: expected {}, got {}",
                    asset_name, expected, actual
                ));
            }
            Some(true)
        }
        None => None,
    };

    progress(UpdateProgress::stage(
        OtaUpdateStage::Staging,
        "Staging update bundle",
    ));
    let staged_targets = stage_install_targets(&download_path, asset_name, &resolved_targets)
        .inspect_err(|_error| {
            cleanup_install_artifacts(&resolved_targets);
            remove_if_exists(&download_path);
        })?;
    progress(UpdateProgress::stage(
        OtaUpdateStage::Installing,
        "Installing update bundle",
    ));
    let installed_targets = commit_staged_targets(&staged_targets).inspect_err(|_error| {
        cleanup_staged_files(&staged_targets);
        remove_if_exists(&download_path);
    })?;

    progress(UpdateProgress::stage(
        OtaUpdateStage::Finalizing,
        "Cleaning up update artifacts",
    ));
    remove_if_exists(&download_path);

    Ok(ApplyResult {
        checksum_verified,
        installed_targets,
    })
}

/// Download `download_url` to `destination` durably.
///
/// Streams to a sibling `.tmp` file, fsyncs, then atomically renames into
/// place so a partial file can never be mistaken for a complete one. On any
/// error (HTTP failure or stream interruption) the `.tmp` is cleaned up and
/// the destination is left untouched.
pub fn download_release(
    client: &reqwest::blocking::Client,
    download_url: &str,
    destination: &Path,
) -> Result<(), String> {
    download_release_with_progress(client, download_url, destination, |_, _| {})
}

fn download_release_with_progress(
    client: &reqwest::blocking::Client,
    download_url: &str,
    destination: &Path,
    progress: impl Fn(u64, Option<u64>),
) -> Result<(), String> {
    let mut resp = client
        .get(download_url)
        .send()
        .map_err(|e| format!("Download failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Download returned {}", resp.status()));
    }

    let tmp = bootstate::tmp_sibling_path(destination);
    // Remove any stale .tmp from a prior crashed download before streaming.
    remove_if_exists(&tmp);

    {
        let mut file =
            File::create(&tmp).map_err(|e| format!("Failed to create {}: {}", tmp.display(), e))?;
        let total_bytes = resp.content_length();
        let mut downloaded_bytes = 0_u64;
        let mut next_progress_bytes = 0_u64;
        let mut buf = [0_u8; 64 * 1024];
        progress(downloaded_bytes, total_bytes);

        loop {
            let read = match resp.read(&mut buf) {
                Ok(0) => break,
                Ok(read) => read,
                Err(e) => {
                    drop(file);
                    remove_if_exists(&tmp);
                    return Err(format!("Failed to write download: {}", e));
                }
            };

            if let Err(e) = file.write_all(&buf[..read]) {
                drop(file);
                remove_if_exists(&tmp);
                return Err(format!("Failed to write download: {}", e));
            }

            downloaded_bytes = downloaded_bytes.saturating_add(read as u64);
            let reached_total = total_bytes == Some(downloaded_bytes);
            if downloaded_bytes >= next_progress_bytes || reached_total {
                progress(downloaded_bytes, total_bytes);
                next_progress_bytes = downloaded_bytes.saturating_add(512 * 1024);
            }
        }
        file.flush()
            .map_err(|e| format!("Failed to flush {}: {}", tmp.display(), e))?;
        file.sync_all()
            .map_err(|e| format!("Failed to sync {}: {}", tmp.display(), e))?;
    }

    if let Err(e) = fs::rename(&tmp, destination) {
        remove_if_exists(&tmp);
        return Err(format!(
            "Failed to promote {} -> {}: {}",
            tmp.display(),
            destination.display(),
            e
        ));
    }

    Ok(())
}

/// Sweep incomplete download artefacts from the staging dir before starting a
/// fresh OTA check. Removes any `*.download.tmp` left behind by a crashed
/// download and any stale `*.download` from a prior run that didn't flash.
///
/// Safe to call at startup (idempotent, logs and swallows per-file errors).
pub fn cleanup_stale_downloads(staging_dir: &Path) {
    let entries = match fs::read_dir(staging_dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name.ends_with(".download") || name.ends_with(".download.tmp") {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Appliance-default staging directory for OTA artifacts. Exposed so the
/// appliance binary can call [`cleanup_stale_downloads`] at boot without
/// duplicating the path.
pub const APPLIANCE_OTA_STAGING_DIR_PATH: &str = APPLIANCE_OTA_STAGING_DIR;

fn install_target_executable() -> Result<PathBuf, String> {
    let platform_type = std::env::var("RHYTHM_PLATFORM_TYPE").ok();

    if is_appliance_runtime(platform_type.as_deref()) {
        return Ok(PathBuf::from(APPLIANCE_INSTALL_PATH));
    }

    let current_exe =
        std::env::current_exe().map_err(|e| format!("Cannot determine exe path: {}", e))?;
    Ok(normalize_server_install_path(current_exe))
}

fn normalize_server_install_path(current_exe: PathBuf) -> PathBuf {
    if current_exe.file_name().and_then(|name| name.to_str()) == Some("rhythm-cli") {
        return current_exe.with_file_name("rhythm-server");
    }
    current_exe
}

fn restart_strategy() -> RestartStrategy {
    let platform_type = std::env::var("RHYTHM_PLATFORM_TYPE").ok();

    if is_appliance_runtime(platform_type.as_deref()) {
        RestartStrategy::ApplianceReboot
    } else {
        RestartStrategy::SupervisorExit
    }
}

fn is_appliance_runtime(platform_type: Option<&str>) -> bool {
    matches!(platform_type, Some("appliance"))
}

/// Returns true when the current process is running on an appliance build
/// (rpiz), reading `RHYTHM_PLATFORM_TYPE` from the environment. Mirrors what
/// `install_target_executable` and `restart_strategy` already use, so the
/// auto-update loop and OTA install share a single source of truth.
pub fn is_appliance_runtime_default() -> bool {
    let platform_type = std::env::var("RHYTHM_PLATFORM_TYPE").ok();
    is_appliance_runtime(platform_type.as_deref())
}

pub fn schedule_post_update_restart() {
    schedule_restart("self-update");
}

pub fn schedule_user_initiated_restart() {
    schedule_restart("user request");
}

pub fn schedule_liveness_restart() {
    schedule_restart("liveness watchdog");
}

fn schedule_restart(reason: &'static str) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if std::env::var_os("RHYTHM_RESTART_DRY_RUN").is_some() {
            log::info!(target: "sys", "Restart dry-run ({}); skipping reboot/exit", reason);
            return;
        }
        match restart_strategy() {
            RestartStrategy::ApplianceReboot => {
                log::info!(target: "sys", "Rebooting appliance after {}...", reason);
                spawn_forced_reboot_fallback(reason);

                let reboot_result = Command::new("/sbin/reboot")
                    .status()
                    .or_else(|_| Command::new("reboot").status());

                match reboot_result {
                    Ok(status) if status.success() => {}
                    Ok(status) => {
                        log::error!(
                            target: "sys",
                            "Appliance reboot command exited with status {:?}; falling back to process exit",
                            status.code()
                        );
                        std::process::exit(1);
                    }
                    Err(error) => {
                        log::error!(
                            target: "sys",
                            "Failed to invoke appliance reboot after {}: {}; falling back to process exit",
                            reason,
                            error
                        );
                        std::process::exit(1);
                    }
                }
            }
            RestartStrategy::SupervisorExit => {
                log::info!(target: "sys", "Restarting after {}...", reason);
                std::process::exit(1);
            }
        }
    });
}

fn spawn_forced_reboot_fallback(reason: &'static str) {
    let spawn_result = std::thread::Builder::new()
        .name("reboot-fallback".to_string())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(
                APPLIANCE_REBOOT_FALLBACK_SECS,
            ));
            log::error!(
                target: "sys",
                "Graceful appliance reboot still running after {}s for {}; forcing reboot",
                APPLIANCE_REBOOT_FALLBACK_SECS,
                reason
            );

            let forced_result = Command::new("/sbin/reboot")
                .arg("-f")
                .status()
                .or_else(|_| Command::new("reboot").arg("-f").status());

            match forced_result {
                Ok(status) if status.success() => {
                    log::error!(
                        target: "sys",
                        "Forced appliance reboot command returned; exiting process"
                    );
                    std::process::exit(1);
                }
                Ok(status) => {
                    log::error!(
                        target: "sys",
                        "Forced appliance reboot command exited with status {:?}; exiting process",
                        status.code()
                    );
                    std::process::exit(1);
                }
                Err(error) => {
                    log::error!(
                        target: "sys",
                        "Failed to invoke forced appliance reboot: {}; exiting process",
                        error
                    );
                    std::process::exit(1);
                }
            }
        });

    if let Err(error) = spawn_result {
        log::error!(
            target: "sys",
            "Failed to spawn forced reboot fallback: {}",
            error
        );
    }
}

fn compute_sha256_hex(path: &Path) -> Result<String, String> {
    let file = File::open(path).map_err(|e| format!("Failed to open file for hashing: {}", e))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];

    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("Failed to read file for hashing: {}", e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok(hex_string(&hasher.finalize()))
}

fn stage_install_targets(
    download_path: &Path,
    asset_name: &str,
    install_targets: &[InstallTarget],
) -> Result<Vec<StagedInstallTarget>, String> {
    let staged_targets = install_targets
        .iter()
        .cloned()
        .map(|spec| StagedInstallTarget {
            stage_path: spec.destination.with_extension("new"),
            backup_path: spec.destination.with_extension("old"),
            spec,
        })
        .collect::<Vec<_>>();

    if asset_name.ends_with(".tar.gz") {
        ensure_stage_space(download_path, &staged_targets)?;
        extract_targets_from_archive(download_path, &staged_targets)?;
    } else {
        if staged_targets.len() != 1 {
            return Err(format!(
                "Binary payload {} cannot satisfy multi-file install plan",
                asset_name
            ));
        }
        let staged = &staged_targets[0];
        if let Some(parent) = staged.stage_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to prepare {}: {}", parent.display(), e))?;
        }
        fs::copy(download_path, &staged.stage_path)
            .map_err(|e| format!("Failed to copy downloaded binary: {}", e))?;
    }

    for staged in &staged_targets {
        if staged.stage_path.exists() {
            set_executable_permissions(&staged.stage_path)?;
        }
    }

    Ok(staged_targets)
}

#[cfg(unix)]
fn ensure_stage_space(
    archive_path: &Path,
    install_targets: &[StagedInstallTarget],
) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;

    let mut required_by_device = HashMap::<u64, (PathBuf, u64)>::new();
    let mut target_devices = Vec::with_capacity(install_targets.len());

    for target in install_targets {
        let stage_dir = target
            .stage_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        fs::create_dir_all(&stage_dir)
            .map_err(|e| format!("Failed to prepare {}: {}", stage_dir.display(), e))?;
        let device_id = fs::metadata(&stage_dir)
            .map_err(|e| format!("Failed to inspect {}: {}", stage_dir.display(), e))?
            .dev();
        target_devices.push(device_id);
        required_by_device
            .entry(device_id)
            .or_insert((stage_dir, 0));
    }

    let file = File::open(archive_path).map_err(|e| format!("Failed to open archive: {}", e))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|e| format!("Failed to read archive entries: {}", e))?;
    let mut found = vec![false; install_targets.len()];

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read archive entry: {}", e))?;
        let entry_path = entry
            .path()
            .map_err(|e| format!("Failed to inspect archive entry: {}", e))?;
        let entry_name = entry_path.to_string_lossy();
        let entry_file_name = entry_path.file_name().and_then(|name| name.to_str());

        for (index, target) in install_targets.iter().enumerate() {
            let matches = entry_name == target.spec.archive_path
                || entry_file_name == Some(target.spec.archive_path.as_str());
            if !matches {
                continue;
            }

            if let Some((_stage_dir, required_bytes)) =
                required_by_device.get_mut(&target_devices[index])
            {
                *required_bytes = required_bytes.saturating_add(entry.size());
            }
            found[index] = true;
            break;
        }
    }

    for (index, target) in install_targets.iter().enumerate() {
        if !found[index] && target.spec.required {
            return Err(format!(
                "Archive did not contain required payload {}",
                target.spec.archive_path
            ));
        }
    }

    for (_device_id, (stage_dir, required_bytes)) in required_by_device {
        if required_bytes == 0 {
            continue;
        }
        let available_bytes = available_bytes_for_path(&stage_dir)?;
        if available_bytes < required_bytes {
            return Err(format!(
                "Insufficient free space in {} to stage update: need {} bytes, have {} bytes available",
                stage_dir.display(),
                required_bytes,
                available_bytes
            ));
        }
    }

    Ok(())
}

#[cfg(not(unix))]
fn ensure_stage_space(
    _archive_path: &Path,
    _install_targets: &[StagedInstallTarget],
) -> Result<(), String> {
    Ok(())
}

fn extract_targets_from_archive(
    archive_path: &Path,
    install_targets: &[StagedInstallTarget],
) -> Result<(), String> {
    let file = File::open(archive_path).map_err(|e| format!("Failed to open archive: {}", e))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|e| format!("Failed to read archive entries: {}", e))?;
    let mut found = vec![false; install_targets.len()];

    for entry in entries {
        let mut entry = entry.map_err(|e| format!("Failed to read archive entry: {}", e))?;
        let entry_path = entry
            .path()
            .map_err(|e| format!("Failed to inspect archive entry: {}", e))?;
        let entry_name = entry_path.to_string_lossy();
        let entry_file_name = entry_path.file_name().and_then(|name| name.to_str());

        for (index, target) in install_targets.iter().enumerate() {
            let matches = entry_name == target.spec.archive_path
                || entry_file_name == Some(target.spec.archive_path.as_str());
            if !matches {
                continue;
            }

            if let Some(parent) = target.stage_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to prepare {}: {}", parent.display(), e))?;
            }
            entry
                .unpack(&target.stage_path)
                .map_err(|e| format!("Failed to extract {}: {}", target.spec.archive_path, e))?;
            found[index] = true;
            break;
        }
    }

    for (index, target) in install_targets.iter().enumerate() {
        if !found[index] && target.spec.required {
            return Err(format!(
                "Archive did not contain required payload {}",
                target.spec.archive_path
            ));
        }
    }

    Ok(())
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast)]
fn available_bytes_for_path(path: &Path) -> Result<u64, String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path_bytes = path.as_os_str().as_bytes();
    let c_path = CString::new(path_bytes)
        .map_err(|_| format!("Path contains interior NUL byte: {}", path.display()))?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let result = unsafe { libc::statvfs(c_path.as_ptr(), stats.as_mut_ptr()) };
    if result != 0 {
        return Err(format!(
            "Failed to inspect free space for {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }

    let stats = unsafe { stats.assume_init() };
    let fragment_size = if stats.f_frsize == 0 {
        stats.f_bsize
    } else {
        stats.f_frsize
    };

    Ok((stats.f_bavail as u64).saturating_mul(fragment_size as u64))
}

fn commit_staged_targets(staged_targets: &[StagedInstallTarget]) -> Result<Vec<String>, String> {
    let mut applied = Vec::<AppliedInstallTarget>::new();
    let mut installed_targets = Vec::new();

    for staged in staged_targets {
        if !staged.stage_path.exists() {
            continue;
        }

        if let Some(parent) = staged.spec.destination.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to prepare {}: {}", parent.display(), e))?;
        }

        let previously_existed = staged.spec.destination.exists();
        if previously_existed {
            remove_if_exists(&staged.backup_path);
            fs::rename(&staged.spec.destination, &staged.backup_path).map_err(|e| {
                format!(
                    "Failed to backup {}: {}",
                    staged.spec.destination.display(),
                    e
                )
            })?;
        }

        if let Err(error) = fs::rename(&staged.stage_path, &staged.spec.destination) {
            if previously_existed {
                let _ = fs::rename(&staged.backup_path, &staged.spec.destination);
            }
            rollback_applied_targets(&applied);
            return Err(format!(
                "Failed to install {}: {}",
                staged.spec.destination.display(),
                error
            ));
        }

        applied.push(AppliedInstallTarget {
            destination: staged.spec.destination.clone(),
            backup_path: staged.backup_path.clone(),
            previously_existed,
        });
        installed_targets.push(
            staged
                .spec
                .destination
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_else(|| {
                    staged
                        .spec
                        .destination
                        .as_os_str()
                        .to_str()
                        .unwrap_or("unknown")
                })
                .to_string(),
        );
    }

    cleanup_backup_files(&applied);

    Ok(installed_targets)
}

fn rollback_applied_targets(applied: &[AppliedInstallTarget]) {
    for target in applied.iter().rev() {
        if target.destination.exists() {
            let _ = fs::remove_file(&target.destination);
        }
        if target.previously_existed && target.backup_path.exists() {
            let _ = fs::rename(&target.backup_path, &target.destination);
        }
    }
}

fn cleanup_backup_files(applied: &[AppliedInstallTarget]) {
    for target in applied {
        if target.previously_existed {
            remove_if_exists(&target.backup_path);
        }
    }
}

fn cleanup_install_artifacts(install_targets: &[InstallTarget]) {
    for target in install_targets {
        remove_if_exists(&target.destination.with_extension("new"));
    }
}

fn cleanup_staged_files(staged_targets: &[StagedInstallTarget]) {
    for target in staged_targets {
        remove_if_exists(&target.stage_path);
    }
}

fn set_executable_permissions(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Failed to set permissions on {}: {}", path.display(), e))?;
    }

    Ok(())
}

fn hex_string(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{:02x}", byte);
    }
    out
}

fn remove_if_exists(path: &Path) {
    if path.exists() {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn unique_test_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rhythm-server-{}-{}", name, nanos));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_executable(path: &Path, body: &str) {
        let mut file = File::create(path).unwrap();
        writeln!(file, "{}", body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn spawn_download_fixture(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = [0_u8; 1024];
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            let _ = stream.read(&mut request);
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(headers.as_bytes());
            let _ = stream.write_all(&body);
        });
        thread::sleep(Duration::from_millis(10));
        format!("http://127.0.0.1:{}/artifact", port)
    }

    fn spawn_status_fixture(status: u16) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = [0_u8; 1024];
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            let _ = stream.read(&mut request);
            let reason = match status {
                404 => "Not Found",
                500 => "Internal Server Error",
                _ => "OK",
            };
            let headers = format!(
                "HTTP/1.1 {} {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                status, reason
            );
            let _ = stream.write_all(headers.as_bytes());
        });
        thread::sleep(Duration::from_millis(10));
        format!("http://127.0.0.1:{}", port)
    }

    fn test_client() -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("client")
    }

    #[test]
    fn parse_version_output_finds_semver_token() {
        assert_eq!(
            parse_version_output("rhythm-chipd 0.4.146\n"),
            Some("0.4.146".to_string())
        );
        assert_eq!(
            parse_version_output("Usage: rhythm-chipd --socket /tmp"),
            None
        );
    }

    #[test]
    fn parse_appliance_slot_from_cmdline_reads_root_partition() {
        assert_eq!(
            parse_appliance_slot_from_cmdline("console=tty1 root=/dev/mmcblk0p2 rootwait rw")
                .unwrap(),
            ApplianceSlot::A
        );
        assert_eq!(
            parse_appliance_slot_from_cmdline("console=tty1 root=/dev/mmcblk0p3 rootwait rw")
                .unwrap(),
            ApplianceSlot::B
        );
    }

    #[test]
    fn rewrite_cmdline_root_device_swaps_slots() {
        assert_eq!(
            rewrite_cmdline_root_device(
                "console=tty1 root=/dev/mmcblk0p2 rootwait rw",
                ApplianceSlot::B
            )
            .unwrap(),
            "console=tty1 root=/dev/mmcblk0p3 rootwait rw"
        );
    }

    #[test]
    fn rewrite_cmdline_file_updates_primary_and_preserves_backup() {
        let dir = unique_test_dir("cmdline-rewrite");
        let cmdline = dir.join("cmdline.txt");
        let backup = dir.join("cmdline.txt.bak");
        fs::write(&cmdline, "console=tty1 root=/dev/mmcblk0p2 rootwait rw\n").unwrap();

        rewrite_cmdline_file(&cmdline, &backup, ApplianceSlot::B).unwrap();

        assert_eq!(
            fs::read_to_string(&cmdline).unwrap(),
            "console=tty1 root=/dev/mmcblk0p3 rootwait rw\n"
        );
        assert_eq!(
            fs::read_to_string(&backup).unwrap(),
            "console=tty1 root=/dev/mmcblk0p2 rootwait rw\n",
            "backup should contain the pre-rewrite cmdline so a mid-rewrite power loss is recoverable"
        );
        assert!(
            !bootstate::tmp_sibling_path(&cmdline).exists(),
            "temp for primary should be cleaned up"
        );
        assert!(
            !bootstate::tmp_sibling_path(&backup).exists(),
            "temp for backup should be cleaned up"
        );
    }

    #[test]
    fn rewrite_cmdline_file_errors_when_no_root_arg() {
        let dir = unique_test_dir("cmdline-no-root");
        let cmdline = dir.join("cmdline.txt");
        let backup = dir.join("cmdline.txt.bak");
        fs::write(&cmdline, "console=tty1 rootwait rw\n").unwrap();

        let result = rewrite_cmdline_file(&cmdline, &backup, ApplianceSlot::B);
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(&cmdline).unwrap(),
            "console=tty1 rootwait rw\n",
            "primary must be unchanged when rewrite validation fails"
        );
        assert!(
            !backup.exists(),
            "backup should not be written when validation fails before any write"
        );
    }

    #[test]
    fn rewrite_cmdline_file_is_idempotent_when_slot_matches() {
        let dir = unique_test_dir("cmdline-idempotent");
        let cmdline = dir.join("cmdline.txt");
        let backup = dir.join("cmdline.txt.bak");
        let initial = "console=tty1 root=/dev/mmcblk0p2 rootwait rw\n";
        fs::write(&cmdline, initial).unwrap();

        rewrite_cmdline_file(&cmdline, &backup, ApplianceSlot::A).unwrap();

        assert_eq!(fs::read_to_string(&cmdline).unwrap(), initial);
        assert_eq!(fs::read_to_string(&backup).unwrap(), initial);
    }

    #[test]
    fn cleanup_stale_downloads_removes_download_and_tmp_files() {
        let dir = unique_test_dir("ota-cleanup");
        fs::write(dir.join("rootfs.ext2.gz.download"), "partial").unwrap();
        fs::write(dir.join("rootfs.ext2.gz.download.tmp"), "partial-tmp").unwrap();
        fs::write(dir.join("bundle.tar.gz.download"), "other partial").unwrap();
        fs::write(dir.join("manifest.json"), "{\"keep\":true}").unwrap();
        fs::write(dir.join("random.txt"), "unrelated").unwrap();

        cleanup_stale_downloads(&dir);

        assert!(!dir.join("rootfs.ext2.gz.download").exists());
        assert!(!dir.join("rootfs.ext2.gz.download.tmp").exists());
        assert!(!dir.join("bundle.tar.gz.download").exists());
        assert!(
            dir.join("manifest.json").exists(),
            "cleanup must not touch files outside the download-staging convention"
        );
        assert!(dir.join("random.txt").exists());
    }

    #[test]
    fn cleanup_stale_downloads_noop_when_directory_missing() {
        let dir = unique_test_dir("ota-cleanup-missing");
        let missing = dir.join("nonexistent");
        cleanup_stale_downloads(&missing);
        // Must not panic or error.
    }

    #[test]
    fn download_release_with_progress_reports_initial_and_final_bytes() {
        let payload = b"rhythm-update-payload".to_vec();
        let payload_len = payload.len() as u64;
        let url = spawn_download_fixture(payload.clone());
        let dir = unique_test_dir("download-progress");
        let dest = dir.join("artifact.download");
        let progress = Arc::new(Mutex::new(Vec::<(u64, Option<u64>)>::new()));

        download_release_with_progress(&test_client(), &url, &dest, {
            let progress = progress.clone();
            move |downloaded, total| progress.lock().unwrap().push((downloaded, total))
        })
        .expect("download succeeds");

        assert_eq!(fs::read(&dest).unwrap(), payload);
        let progress = progress.lock().unwrap();
        assert!(
            progress
                .first()
                .is_some_and(|(downloaded, total)| *downloaded == 0 && *total == Some(payload_len)),
            "expected initial 0-byte progress with total, got {:?}",
            *progress
        );
        assert!(
            progress.last().is_some_and(
                |(downloaded, total)| *downloaded == payload_len && *total == Some(payload_len)
            ),
            "expected final complete progress with total, got {:?}",
            *progress
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn artifact_staging_path_sanitizes_asset_name() {
        let unsafe_name = "rootfs.ext2.gz?version=1&token=abc";
        let path = artifact_staging_path(unsafe_name);
        let file = path.file_name().and_then(|n| n.to_str()).unwrap();
        assert!(
            file.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')),
            "staging filename must not contain URL-unsafe characters, got {}",
            file
        );
        assert!(file.ends_with(".download"));
    }

    #[test]
    fn appliance_update_prefers_gzip_rootfs_asset() {
        let info = UpdateInfo {
            current_version: "0.4.146".to_string(),
            latest_version: "0.4.147".to_string(),
            update_available: true,
            update_reason: Some(UpdateReason::VersionMismatch),
            download_url: None,
            expected_sha256: None,
            asset_name: None,
            install_targets: Vec::new(),
            image_assets: vec![
                UpdateImageAsset {
                    name: "rootfs.ext2".to_string(),
                    kind: ReleaseArtifactKind::RootfsImage,
                    url: "https://example.invalid/rootfs.ext2".to_string(),
                    sha256: None,
                    size: None,
                    compression: None,
                },
                UpdateImageAsset {
                    name: "rootfs.ext2.gz".to_string(),
                    kind: ReleaseArtifactKind::RootfsImage,
                    url: "https://example.invalid/rootfs.ext2.gz".to_string(),
                    sha256: None,
                    size: None,
                    compression: Some("gzip".to_string()),
                },
            ],
            resolved_install_targets: Vec::new(),
        };

        assert_eq!(
            info.preferred_appliance_image_asset()
                .map(|asset| asset.name.as_str()),
            Some("rootfs.ext2.gz")
        );
    }

    #[test]
    fn write_image_artifact_to_device_handles_gzip() {
        let dir = unique_test_dir("rootfs-gzip");
        let artifact = dir.join("rootfs.ext2.gz");
        let target = dir.join("rootfs-slot.img");

        {
            let tar_gz = File::create(&artifact).unwrap();
            let mut encoder = flate2::write::GzEncoder::new(tar_gz, flate2::Compression::default());
            encoder.write_all(b"fake-rootfs").unwrap();
            encoder.finish().unwrap();
        }

        File::create(&target).unwrap();
        write_image_artifact_to_device(&artifact, &target, true).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"fake-rootfs");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn resolve_package_artifact_requires_archive_bundle_kind() {
        let install_root = PathBuf::from("/tmp/rhythm-server");
        let error = resolve_package_artifact(
            "https://example.invalid/manifest.json",
            &ManifestArtifact {
                name: "rhythm-server-rpiz.tar.gz".to_string(),
                url: "v1/rhythm-server-rpiz.tar.gz".to_string(),
                sha256: None,
                size: None,
                kind: Some(ReleaseArtifactKind::DiskImage),
                compression: None,
                install: Vec::new(),
            },
            &install_root,
        )
        .unwrap_err();

        assert_eq!(error, "Manifest package kind must be archive_bundle");
    }

    #[test]
    fn resolve_package_artifact_requires_tar_gz_name() {
        let install_root = PathBuf::from("/tmp/rhythm-server");
        let error = resolve_package_artifact(
            "https://example.invalid/manifest.json",
            &ManifestArtifact {
                name: "rhythm-server-rpiz.zip".to_string(),
                url: "v1/rhythm-server-rpiz.zip".to_string(),
                sha256: None,
                size: None,
                kind: Some(ReleaseArtifactKind::ArchiveBundle),
                compression: None,
                install: Vec::new(),
            },
            &install_root,
        )
        .unwrap_err();

        assert_eq!(
            error,
            "Manifest package must reference a .tar.gz archive bundle"
        );
    }

    #[test]
    fn resolve_manifest_install_targets_maps_self_and_sibling() {
        let root = PathBuf::from("/tmp/rhythm-server");
        let targets = resolve_manifest_install_targets(
            &[
                ManifestInstallTarget {
                    archive_path: Some("rhythm-server".to_string()),
                    slot: InstallSlot::Current,
                    path: None,
                    required: true,
                },
                ManifestInstallTarget {
                    archive_path: Some("rhythm-chipd".to_string()),
                    slot: InstallSlot::Sibling,
                    path: Some("rhythm-chipd".to_string()),
                    required: true,
                },
            ],
            &root,
        )
        .unwrap();

        assert_eq!(targets[0].destination, PathBuf::from("/tmp/rhythm-server"));
        assert_eq!(targets[1].destination, PathBuf::from("/tmp/rhythm-chipd"));
    }

    #[test]
    fn component_drift_detects_missing_required_chipd() {
        let dir = unique_test_dir("drift-missing");
        let install_root = dir.join("rhythm-server");
        write_executable(&install_root, "#!/bin/sh\necho \"rhythm-server 0.4.146\"\n");

        assert!(detect_component_drift(
            "0.4.146",
            &install_root,
            &default_bundle_install_targets(&install_root)
        ));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn component_drift_ignores_optional_cli() {
        let dir = unique_test_dir("drift-optional");
        let install_root = dir.join("rhythm-server");
        let cli_path = dir.join("rhythm-cli");
        write_executable(&install_root, "#!/bin/sh\necho \"rhythm-server 0.4.146\"\n");

        let mut targets = vec![
            InstallTarget {
                archive_path: "rhythm-server".to_string(),
                destination: install_root.clone(),
                required: true,
            },
            InstallTarget {
                archive_path: "rhythm-cli".to_string(),
                destination: cli_path,
                required: false,
            },
        ];
        assert!(!detect_component_drift("0.4.146", &install_root, &targets));

        targets[1].required = true;
        assert!(detect_component_drift("0.4.146", &install_root, &targets));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn component_drift_detects_old_chipd_without_version_flag() {
        let dir = unique_test_dir("drift-old-chipd");
        let install_root = dir.join("rhythm-server");
        let chipd_path = dir.join("rhythm-chipd");
        write_executable(&install_root, "#!/bin/sh\necho \"rhythm-server 0.4.146\"\n");
        write_executable(
            &chipd_path,
            "#!/bin/sh\necho \"Usage: rhythm-chipd --socket /tmp\" >&2\nexit 1\n",
        );

        assert!(detect_component_drift(
            "0.4.146",
            &install_root,
            &default_bundle_install_targets(&install_root)
        ));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn stage_archive_rejects_when_required_entry_missing() {
        let dir = unique_test_dir("stage-archive-missing");
        let archive_path = dir.join("release.tar.gz");
        let install_targets = vec![
            InstallTarget {
                archive_path: "rhythm-server".to_string(),
                destination: dir.join("rhythm-server"),
                required: true,
            },
            InstallTarget {
                archive_path: "rhythm-chipd".to_string(),
                destination: dir.join("rhythm-chipd"),
                required: true,
            },
        ];

        // Build an archive that only contains rhythm-server — required chipd
        // entry is missing, which must surface as an error.
        {
            let tar_gz = File::create(&archive_path).unwrap();
            let encoder = flate2::write::GzEncoder::new(tar_gz, flate2::Compression::default());
            let mut builder = tar::Builder::new(encoder);
            let payload = b"fake-rhythm-server";
            let mut header = tar::Header::new_gnu();
            header.set_path("rhythm-server").unwrap();
            header.set_size(payload.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append(&header, payload.as_slice()).unwrap();
            builder.finish().unwrap();
        }

        let result =
            stage_install_targets(&archive_path, "rhythm-server-rpiz.tar.gz", &install_targets);
        assert!(result.is_err(), "missing required entry must error");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("rhythm-chipd"),
            "error should mention the missing payload, got {}",
            msg
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn stage_archive_tolerates_extra_entries() {
        let dir = unique_test_dir("stage-archive-extra");
        let archive_path = dir.join("release.tar.gz");
        let install_targets = vec![InstallTarget {
            archive_path: "rhythm-server".to_string(),
            destination: dir.join("rhythm-server"),
            required: true,
        }];

        {
            let tar_gz = File::create(&archive_path).unwrap();
            let encoder = flate2::write::GzEncoder::new(tar_gz, flate2::Compression::default());
            let mut builder = tar::Builder::new(encoder);
            for (name, payload) in [
                ("rhythm-server", b"fake-server".as_slice()),
                ("README.txt", b"ship notes".as_slice()),
                ("CHANGELOG.md", b"v1\n".as_slice()),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_path(name).unwrap();
                header.set_size(payload.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder.append(&header, payload).unwrap();
            }
            builder.finish().unwrap();
        }

        let staged =
            stage_install_targets(&archive_path, "rhythm-server-rpiz.tar.gz", &install_targets)
                .expect("extra entries must not fail extraction");
        assert_eq!(
            fs::read(dir.join("rhythm-server.new")).unwrap(),
            b"fake-server"
        );
        cleanup_staged_files(&staged);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn stage_archive_rejects_corrupt_gzip() {
        let dir = unique_test_dir("stage-archive-corrupt");
        let archive_path = dir.join("release.tar.gz");
        fs::write(&archive_path, b"this is not a gzipped tar").unwrap();

        let install_targets = vec![InstallTarget {
            archive_path: "rhythm-server".to_string(),
            destination: dir.join("rhythm-server"),
            required: true,
        }];

        let result =
            stage_install_targets(&archive_path, "rhythm-server-rpiz.tar.gz", &install_targets);
        assert!(result.is_err(), "corrupt archive must surface as an error");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn manifest_parses_minimal_package_json() {
        let json = r#"{
            "version": "0.4.200",
            "package": {
                "name": "rhythm-server-rpiz.tar.gz",
                "url": "https://example.test/rhythm-server-rpiz.tar.gz",
                "sha256": "deadbeef",
                "size": 12345,
                "kind": "archive_bundle",
                "install": [
                    {"archive_path": "rhythm-server", "slot": "self", "required": true},
                    {"archive_path": "rhythm-chipd", "slot": "sibling", "path": "rhythm-chipd"}
                ]
            }
        }"#;
        let manifest: UpdateManifest = serde_json::from_str(json).expect("valid manifest parses");
        assert_eq!(manifest.version, "0.4.200");
        let pkg = manifest.package.expect("package present");
        assert_eq!(pkg.name, "rhythm-server-rpiz.tar.gz");
        assert_eq!(pkg.sha256.as_deref(), Some("deadbeef"));
        assert_eq!(pkg.install.len(), 2);
        assert_eq!(pkg.install[0].slot, InstallSlot::Current);
        assert_eq!(pkg.install[1].slot, InstallSlot::Sibling);
    }

    #[test]
    fn manifest_parse_rejects_missing_version() {
        let json = r#"{ "package": { "name": "x", "url": "http://x/y" } }"#;
        let result: Result<UpdateManifest, _> = serde_json::from_str(json);
        assert!(result.is_err(), "manifest without version must not parse");
    }

    #[test]
    fn manifest_parse_rejects_invalid_json() {
        let result: Result<UpdateManifest, _> = serde_json::from_str("{ not valid json");
        assert!(result.is_err());
    }

    #[test]
    fn manifest_parses_empty_images_and_package_optional() {
        // Server-only manifest (no appliance image). `package` must be present
        // for a download to be offered, but parsing a manifest with only
        // `version` must still succeed so a client can observe the version and
        // fall back to "no update available".
        let json = r#"{ "version": "0.4.161-beta" }"#;
        let manifest: UpdateManifest = serde_json::from_str(json).expect("parses");
        assert_eq!(manifest.version, "0.4.161-beta");
        assert!(manifest.package.is_none());
        assert!(manifest.images.is_empty());
    }

    #[test]
    fn manifest_parses_multi_image_appliance_payload() {
        let json = r#"{
            "version": "0.4.200",
            "images": [
                {
                    "name": "rootfs.ext2",
                    "url": "https://example.test/rootfs.ext2",
                    "kind": "rootfs_image"
                },
                {
                    "name": "rootfs.ext2.gz",
                    "url": "https://example.test/rootfs.ext2.gz",
                    "kind": "rootfs_image",
                    "compression": "gzip",
                    "sha256": "abc123"
                }
            ]
        }"#;
        let manifest: UpdateManifest = serde_json::from_str(json).expect("parses");
        assert_eq!(manifest.images.len(), 2);
        assert_eq!(
            manifest.images[1].kind,
            Some(ReleaseArtifactKind::RootfsImage)
        );
        assert_eq!(manifest.images[1].compression.as_deref(), Some("gzip"));
    }

    #[test]
    fn sha256_of_known_input_produces_stable_hex() {
        // Sanity-check the hex encoding used for downloaded artifact checksums:
        // the same input must produce a 64-char lowercase hex digest regardless
        // of platform, and identical inputs must match.
        let digest = Sha256::digest(b"rhythm");
        let hex = hex_string(&digest);
        assert_eq!(hex.len(), 64);
        assert!(hex
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
        let digest2 = Sha256::digest(b"rhythm");
        assert_eq!(hex, hex_string(&digest2));
        let digest_other = Sha256::digest(b"rhythm-");
        assert_ne!(hex, hex_string(&digest_other));
    }

    #[test]
    fn stage_archive_installs_server_and_chipd() {
        let dir = unique_test_dir("stage-archive");
        let archive_path = dir.join("release.tar.gz");
        let server_stage = dir.join("rhythm-server.new");
        let chipd_stage = dir.join("rhythm-chipd.new");
        let install_targets = vec![
            InstallTarget {
                archive_path: "rhythm-server".to_string(),
                destination: dir.join("rhythm-server"),
                required: true,
            },
            InstallTarget {
                archive_path: "rhythm-chipd".to_string(),
                destination: dir.join("rhythm-chipd"),
                required: true,
            },
        ];

        {
            let tar_gz = File::create(&archive_path).unwrap();
            let encoder = flate2::write::GzEncoder::new(tar_gz, flate2::Compression::default());
            let mut builder = tar::Builder::new(encoder);

            for (name, payload) in [
                ("rhythm-server", b"fake-rhythm-server".as_slice()),
                ("rhythm-chipd", b"fake-rhythm-chipd".as_slice()),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_path(name).unwrap();
                header.set_size(payload.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                builder.append(&header, payload).unwrap();
            }
            builder.finish().unwrap();
        }

        let staged =
            stage_install_targets(&archive_path, "rhythm-server-rpiz.tar.gz", &install_targets)
                .unwrap();

        assert_eq!(fs::read(server_stage).unwrap(), b"fake-rhythm-server");
        assert_eq!(fs::read(chipd_stage).unwrap(), b"fake-rhythm-chipd");
        cleanup_staged_files(&staged);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn commit_staged_targets_removes_backup_files_after_success() {
        let dir = unique_test_dir("commit-cleanup");
        let destination = dir.join("rhythm-server");
        let stage_path = dir.join("rhythm-server.new");
        let backup_path = dir.join("rhythm-server.old");

        fs::write(&destination, b"old-server").unwrap();
        fs::write(&stage_path, b"new-server").unwrap();

        let installed = commit_staged_targets(&[StagedInstallTarget {
            spec: InstallTarget {
                archive_path: "rhythm-server".to_string(),
                destination: destination.clone(),
                required: true,
            },
            stage_path: stage_path.clone(),
            backup_path: backup_path.clone(),
        }])
        .unwrap();

        assert_eq!(installed, vec!["rhythm-server".to_string()]);
        assert_eq!(fs::read(&destination).unwrap(), b"new-server");
        assert!(!stage_path.exists(), "stage file should be promoted");
        assert!(!backup_path.exists(), "backup file should be cleaned up");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ota_status_handle_starts_idle() {
        let handle = OtaStatusHandle::new("1.0.0");
        let snapshot = handle.snapshot();

        assert_eq!(snapshot.state, OtaUpdateState::Idle);
        assert_eq!(snapshot.current_version, "1.0.0");
        assert_eq!(snapshot.latest_version, None);
    }

    #[test]
    fn normalize_server_install_path_prefers_server_for_cli() {
        assert_eq!(
            normalize_server_install_path(PathBuf::from("/tmp/rhythm-cli")),
            PathBuf::from("/tmp/rhythm-server")
        );
        assert_eq!(
            normalize_server_install_path(PathBuf::from("/tmp/rhythm-server")),
            PathBuf::from("/tmp/rhythm-server")
        );
    }

    #[test]
    fn install_target_executable_prefers_appliance_install_path() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_PLATFORM_TYPE", "appliance");
        std::env::set_var("RHYTHM_PLATFORM_CONTEXT", "rpiz");

        let path = install_target_executable().unwrap();
        assert_eq!(path, PathBuf::from(APPLIANCE_INSTALL_PATH));

        std::env::remove_var("RHYTHM_PLATFORM_TYPE");
        std::env::remove_var("RHYTHM_PLATFORM_CONTEXT");
    }

    #[test]
    fn restart_strategy_uses_appliance_reboot_for_rpiz() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_PLATFORM_TYPE", "appliance");
        std::env::set_var("RHYTHM_PLATFORM_CONTEXT", "rpiz");

        assert_eq!(restart_strategy(), RestartStrategy::ApplianceReboot);

        std::env::remove_var("RHYTHM_PLATFORM_TYPE");
        std::env::remove_var("RHYTHM_PLATFORM_CONTEXT");
    }

    #[test]
    fn restart_strategy_uses_supervisor_exit_for_desktop_server() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_PLATFORM_TYPE", "desktop");
        std::env::set_var("RHYTHM_PLATFORM_CONTEXT", "server");

        assert_eq!(restart_strategy(), RestartStrategy::SupervisorExit);

        std::env::remove_var("RHYTHM_PLATFORM_TYPE");
        std::env::remove_var("RHYTHM_PLATFORM_CONTEXT");
    }

    #[test]
    fn configured_manifest_url_appends_stable_suffix() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("RHYTHM_UPDATE_MANIFEST_URL");
        std::env::set_var("RHYTHM_UPDATE_BASE_URL", "https://example/server");

        let beta = configured_manifest_url(UpdateChannel::Beta).unwrap();
        let stable = configured_manifest_url(UpdateChannel::Stable).unwrap();

        std::env::remove_var("RHYTHM_UPDATE_BASE_URL");

        let Some(platform) = platform_feed_name() else {
            return;
        };
        assert_eq!(
            beta,
            format!("https://example/server/{}/manifest.json", platform)
        );
        assert_eq!(
            stable,
            format!("https://example/server/{}-stable/manifest.json", platform)
        );
    }

    #[test]
    fn channel_from_state_uses_stable_only_for_auto_updating_appliance() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_PLATFORM_TYPE", "appliance");

        let state: rhythm_os::state::SharedState =
            Arc::new(Mutex::new(rhythm_os::state::AppState::default()));
        assert_eq!(channel_from_state(&state), UpdateChannel::Stable);

        state.lock().unwrap().auto_update = false;
        assert_eq!(channel_from_state(&state), UpdateChannel::Beta);

        std::env::remove_var("RHYTHM_PLATFORM_TYPE");
    }

    #[test]
    fn channel_from_state_keeps_desktop_on_beta_feed() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_PLATFORM_TYPE", "desktop");

        let state: rhythm_os::state::SharedState =
            Arc::new(Mutex::new(rhythm_os::state::AppState::default()));

        assert_eq!(channel_from_state(&state), UpdateChannel::Beta);

        std::env::remove_var("RHYTHM_PLATFORM_TYPE");
    }

    #[test]
    fn stable_manifest_404_without_explicit_manifest_override_is_no_update() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("RHYTHM_UPDATE_MANIFEST_URL");
        std::env::set_var("RHYTHM_UPDATE_BASE_URL", spawn_status_fixture(404));

        let info = check_blocking("1.2.3", UpdateChannel::Stable).unwrap();

        std::env::remove_var("RHYTHM_UPDATE_BASE_URL");

        assert!(!info.update_available);
        assert_eq!(info.current_version, "1.2.3");
        assert_eq!(info.latest_version, "1.2.3");
    }

    #[test]
    fn configured_manifest_url_env_override_wins_over_channel() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var(
            "RHYTHM_UPDATE_MANIFEST_URL",
            "https://override/manifest.json",
        );

        let beta = configured_manifest_url(UpdateChannel::Beta).unwrap();
        let stable = configured_manifest_url(UpdateChannel::Stable).unwrap();

        std::env::remove_var("RHYTHM_UPDATE_MANIFEST_URL");

        assert_eq!(beta, "https://override/manifest.json");
        assert_eq!(stable, "https://override/manifest.json");
    }
}
