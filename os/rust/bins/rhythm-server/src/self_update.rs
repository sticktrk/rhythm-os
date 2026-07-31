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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::thread::JoinHandle;
use std::time::Duration;

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
const APPLIANCE_INACTIVE_ROOT_MOUNT: &str = "/data/ota/inactive-rootfs";
const APPLIANCE_CMDLINE_PATH: &str = "/boot/cmdline.txt";
const APPLIANCE_CMDLINE_BACKUP_PATH: &str = "/boot/cmdline.txt.bak";
const APPLIANCE_BOOT_STATE_PATH: &str = "/boot/rhythm-bootstate.env";
const APPLIANCE_BOOT_STATE_BACKUP_PATH: &str = "/boot/rhythm-bootstate.env.bak";
const APPLIANCE_BOOT_STATE_MIRROR_PATH: &str = "/data/ota/bootstate.env";
const APPLIANCE_IMAGE_VERSION_PATH: &str = "/etc/rhythm-image-version";
const APPLIANCE_REBOOT_FALLBACK_SECS: u64 = 30;
const APPLIANCE_ROOTFS_APPLY_GUARD_SECS: u64 = 10 * 60;

static APPLY_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RestartStrategy {
    SupervisorExit,
    ApplianceReboot,
}

pub use rhythm_os::state::UpdateChannel;

/// Read the effective OTA channel from shared state.
///
/// An explicit `update_channel` setting wins; otherwise appliances default to
/// the curated stable feed and desktop runtimes to the rolling beta feed (see
/// `AppState::resolved_update_channel`). Independent of `auto_update`, which
/// only controls whether updates apply automatically.
pub fn channel_from_state(state: &rhythm_os::state::SharedState) -> UpdateChannel {
    match state.lock() {
        Ok(s) => s.resolved_update_channel(),
        // Lock poisoning must not silently move an appliance onto the rolling
        // beta feed; fall back to the platform default.
        Err(_) => {
            if is_appliance_runtime_default() {
                UpdateChannel::Stable
            } else {
                UpdateChannel::Beta
            }
        }
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
    ImageBaseDrift,
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
    pub version: Option<String>,
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
    pub current_package_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_package_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_image_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_image_version: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_rollback: Option<LastRollback>,
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
                current_package_version: Some(current_version.to_string()),
                latest_package_version: None,
                current_image_version: appliance_image_base_version(),
                latest_image_version: None,
                target_version: None,
                update_available: None,
                update_reason: None,
                checked_at_epoch_ms: None,
                checksum_verified: None,
                install_targets: Vec::new(),
                image_assets: Vec::new(),
                message: None,
                last_error: None,
                last_rollback: None,
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
        let mut snapshot = self
            .inner
            .lock()
            .map(|status| status.clone())
            .unwrap_or_else(|_| OtaStatus {
                state: OtaUpdateState::Error,
                current_version: "unknown".to_string(),
                latest_version: None,
                current_package_version: None,
                latest_package_version: None,
                current_image_version: None,
                latest_image_version: None,
                target_version: None,
                update_available: None,
                update_reason: None,
                checked_at_epoch_ms: Some(now_ms()),
                checksum_verified: None,
                install_targets: Vec::new(),
                image_assets: Vec::new(),
                message: Some("OTA state lock poisoned".to_string()),
                last_error: Some("OTA state lock poisoned".to_string()),
                last_rollback: None,
            });
        // Read fresh on every snapshot: rollbacks are recorded by the boot
        // path (S41bootstate, startup health check), not by this process.
        snapshot.last_rollback = last_rollback();
        snapshot
    }

    pub fn mark_checking(&self) {
        self.with_status(|status| {
            status.state = OtaUpdateState::Checking;
            status.checked_at_epoch_ms = Some(now_ms());
            status.current_package_version = Some(status.current_version.clone());
            status.latest_package_version = None;
            status.current_image_version = appliance_image_base_version();
            status.latest_image_version = None;
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
            status.current_version = info.current_version.clone();
            status.latest_version = Some(info.latest_version.clone());
            status.current_package_version = Some(info.current_package_version.clone());
            status.latest_package_version = Some(info.latest_package_version.clone());
            status.current_image_version = info.current_image_version.clone();
            status.latest_image_version = info.latest_image_version.clone();
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
                Some(UpdateReason::ImageBaseDrift) => match info.latest_image_version.as_deref() {
                    Some(image_version) => format!(
                        "Updating appliance image to v{} before v{}",
                        image_version, info.latest_version
                    ),
                    None => format!("Updating appliance image before v{}", info.latest_version),
                },
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
            status.current_package_version = Some(previous_version.to_string());
            status.latest_package_version = Some(new_version.to_string());
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
    pub current_package_version: String,
    pub latest_package_version: String,
    pub current_image_version: Option<String>,
    pub latest_image_version: Option<String>,
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

struct PackageInstallPlan<'a> {
    download_url: &'a str,
    asset_name: &'a str,
    expected_sha256: Option<&'a str>,
    install_targets: &'a [InstallTarget],
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
        if self.update_reason == Some(UpdateReason::ComponentDrift) {
            // Recorded before the install so a crash mid-apply still counts
            // against the repair budget for this release.
            record_drift_repair_attempt(&self.latest_version);
        }

        if restart_strategy() == RestartStrategy::ApplianceReboot {
            if let Some(image_asset) = self.preferred_appliance_image_asset() {
                let package_plan = self.package_install_plan()?;
                return apply_appliance_image_blocking(
                    image_asset,
                    &self.current_version,
                    &self.latest_version,
                    package_plan.as_ref(),
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
            LiveInstallVersions {
                previous: &self.current_version,
                target: &self.latest_version,
            },
            &progress,
        )
    }

    fn preferred_appliance_image_asset(&self) -> Option<&UpdateImageAsset> {
        preferred_rootfs_image_asset(
            &self.image_assets,
            appliance_image_base_version().as_deref(),
        )
    }

    fn package_install_plan(&self) -> Result<Option<PackageInstallPlan<'_>>, String> {
        match (self.download_url.as_deref(), self.asset_name.as_deref()) {
            (Some(download_url), Some(asset_name)) => Ok(Some(PackageInstallPlan {
                download_url,
                asset_name,
                expected_sha256: self.expected_sha256.as_deref(),
                install_targets: &self.resolved_install_targets,
            })),
            (None, None) => Ok(None),
            (Some(_), None) => Err("No release asset for this platform".to_string()),
            (None, Some(_)) => Err("No download URL for this platform".to_string()),
        }
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
    version: Option<String>,
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
    version: String,
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

    // Explicit timeout: the blocking client's implicit 30s default is fine
    // for a small manifest, but make the bound visible and add a connect
    // deadline so a black-holed feed can't stall the auto-update thread.
    let client = reqwest::blocking::Client::builder()
        .user_agent("rhythm-server")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
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
        .map(|image| resolve_manifest_image(&manifest_url, image, &manifest.version))
        .collect::<Result<Vec<_>, _>>()?;
    let package = manifest
        .package
        .as_ref()
        .map(|artifact| {
            resolve_package_artifact(&manifest_url, artifact, &install_root, &manifest.version)
        })
        .transpose()?;
    let appliance = restart_strategy() == RestartStrategy::ApplianceReboot;
    let appliance_image_base_version = appliance.then(appliance_image_base_version).flatten();
    let latest_image_version = latest_rootfs_image_version(&image_assets);
    let latest_package_version = package
        .as_ref()
        .map(|package| package.version.clone())
        .unwrap_or_else(|| {
            normalize_version_candidate(&manifest.version)
                .unwrap_or_else(|| manifest.version.clone())
        });
    let appliance_rootfs_update = appliance
        && preferred_rootfs_image_asset(&image_assets, appliance_image_base_version.as_deref())
            .is_some();
    let appliance_rootfs_only = appliance && has_rootfs_image(&image_assets);

    if package.is_none() && !appliance_rootfs_only {
        return Err("Update manifest did not provide an archive bundle payload".to_string());
    }

    let package_update = version_needs_update(&latest_package_version, current_version);
    let package_downgrade = version_is_older(&latest_package_version, current_version);
    if appliance_rootfs_update && package_downgrade && package.is_some() {
        return Err(format!(
            "Update manifest package version {} is older than current {}; refusing appliance image update without a non-downgrade package overlay",
            latest_package_version, current_version
        ));
    }
    if appliance_rootfs_update
        && package.is_none()
        && latest_image_version
            .as_deref()
            .is_some_and(|image_version| version_is_older(image_version, current_version))
    {
        let image_version = latest_image_version.as_deref().unwrap_or("unknown");
        return Err(format!(
            "Update manifest image version {} is older than current package {}; refusing image-only update",
            image_version, current_version
        ));
    }

    let drift_check_applies = !package_update && !package_downgrade && !appliance_rootfs_update;
    let drift_detected = drift_check_applies
        && package
            .as_ref()
            .map(|package| {
                detect_component_drift(
                    &latest_package_version,
                    &install_root,
                    &package.install_targets,
                )
            })
            .unwrap_or(false);
    let drift_state_path = drift_repair_state_path(&install_root);
    let component_drift = if drift_detected {
        if drift_repair_exhausted_at(&drift_state_path, &latest_package_version) {
            log::warn!(
                target: "sys",
                "Component drift persists after {} repair attempts for v{}; suppressing further automatic repairs",
                MAX_DRIFT_REPAIR_ATTEMPTS,
                latest_package_version
            );
            false
        } else {
            true
        }
    } else {
        if drift_check_applies && package.is_some() {
            // The bundle is fully consistent at this version: future drift
            // (e.g. after the next release) starts with a fresh budget.
            clear_drift_repair_state_at(&drift_state_path);
        }
        false
    };

    let update_reason = if package_update {
        Some(UpdateReason::VersionMismatch)
    } else if appliance_rootfs_update {
        Some(UpdateReason::ImageBaseDrift)
    } else if component_drift {
        Some(UpdateReason::ComponentDrift)
    } else {
        None
    };
    let update_available = update_reason.is_some();

    Ok(UpdateInfo {
        current_version: current_version.to_string(),
        latest_version: latest_package_version.clone(),
        current_package_version: current_version.to_string(),
        latest_package_version: latest_package_version.clone(),
        current_image_version: appliance_image_base_version,
        latest_image_version,
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
        current_package_version: current_version.to_string(),
        latest_package_version: current_version.to_string(),
        current_image_version: appliance_image_base_version(),
        latest_image_version: None,
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
    manifest_version: &str,
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
    let version = manifest_artifact_version(artifact, &download_url, manifest_version)
        .unwrap_or_else(|| manifest_version.to_string());
    let install_targets = if artifact.install.is_empty() {
        default_bundle_install_targets(install_root)
    } else {
        resolve_manifest_install_targets(&artifact.install, install_root)?
    };

    Ok(ResolvedPackage {
        version,
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
    manifest_version: &str,
) -> Result<UpdateImageAsset, String> {
    let download_url = resolve_download_url(manifest_url, &artifact.url)?;
    let name = if artifact.name.is_empty() {
        asset_name_from_url(&download_url)?
    } else {
        artifact.name.clone()
    };
    let kind = artifact.kind.unwrap_or_else(|| infer_image_kind(&name));
    let version = manifest_artifact_version(artifact, &download_url, manifest_version);

    Ok(UpdateImageAsset {
        name,
        kind,
        url: download_url,
        version,
        sha256: artifact.sha256.clone(),
        size: artifact.size,
        compression: artifact.compression.clone(),
    })
}

fn manifest_artifact_version(
    artifact: &ManifestArtifact,
    download_url: &str,
    manifest_version: &str,
) -> Option<String> {
    artifact
        .version
        .as_deref()
        .and_then(normalize_version_candidate)
        .or_else(|| infer_release_version_from_url(&artifact.url))
        .or_else(|| infer_release_version_from_url(download_url))
        .or_else(|| normalize_version_candidate(manifest_version))
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

fn latest_rootfs_image_version(image_assets: &[UpdateImageAsset]) -> Option<String> {
    image_assets
        .iter()
        .filter(|asset| asset.kind == ReleaseArtifactKind::RootfsImage)
        .filter_map(|asset| asset.version.clone())
        .max_by(|left, right| compare_optional_release_versions(Some(left), Some(right)))
}

fn preferred_rootfs_image_asset<'a>(
    image_assets: &'a [UpdateImageAsset],
    local_image_version: Option<&str>,
) -> Option<&'a UpdateImageAsset> {
    image_assets
        .iter()
        .filter(|asset| asset.kind == ReleaseArtifactKind::RootfsImage)
        .filter(|asset| image_asset_requires_apply(asset, local_image_version))
        .max_by(|left, right| compare_rootfs_image_assets(left, right))
}

fn compare_rootfs_image_assets(
    left: &UpdateImageAsset,
    right: &UpdateImageAsset,
) -> std::cmp::Ordering {
    compare_optional_release_versions(left.version.as_deref(), right.version.as_deref())
        .then_with(|| artifact_uses_gzip(left).cmp(&artifact_uses_gzip(right)))
}

fn compare_optional_release_versions(
    left: Option<&str>,
    right: Option<&str>,
) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => {
            compare_release_versions(left, right).unwrap_or_else(|| left.cmp(right))
        }
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn image_asset_requires_apply(asset: &UpdateImageAsset, local_image_version: Option<&str>) -> bool {
    let Some(remote_version) = asset.version.as_deref() else {
        return local_image_version.is_none();
    };

    match local_image_version {
        Some(local_version) => version_needs_update(remote_version, local_version),
        None => true,
    }
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

fn ensure_mount(path: &str, device: &str, fs_types: &[&str]) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("Failed to create {}: {}", path, e))?;
    if mountpoint_is_active(path) {
        return Ok(());
    }

    let mut failures = Vec::new();
    for fs_type in fs_types {
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

        if status.success() {
            return Ok(());
        }
        failures.push(format!("{} (status {:?})", fs_type, status.code()));
    }

    Err(format!(
        "Mounting {} on {} failed as {}",
        device,
        path,
        failures.join(", ")
    ))
}

fn unmount_mountpoint(path: &Path) -> Result<(), String> {
    let path_str = path
        .to_str()
        .ok_or_else(|| format!("Mount path is not valid UTF-8: {}", path.display()))?;
    if !mountpoint_is_active(path_str) {
        return Ok(());
    }

    let status = Command::new("/bin/umount")
        .arg(path)
        .status()
        .or_else(|_| Command::new("umount").arg(path).status())
        .map_err(|e| format!("Failed to unmount {}: {}", path.display(), e))?;

    if !status.success() {
        return Err(format!(
            "Unmounting {} failed with status {:?}",
            path.display(),
            status.code()
        ));
    }

    Ok(())
}

fn mount_appliance_rootfs_slot(slot: ApplianceSlot) -> Result<PathBuf, String> {
    let mount_path = PathBuf::from(APPLIANCE_INACTIVE_ROOT_MOUNT);
    if mountpoint_is_active(APPLIANCE_INACTIVE_ROOT_MOUNT) {
        unmount_mountpoint(&mount_path)?;
    }
    // Buildroot generates the rootfs as ext4 (BR2_TARGET_ROOTFS_EXT2_4); the
    // kernel refuses to mount an extents-enabled filesystem as plain ext2, so
    // ext4 must be tried first. ext2 remains as a fallback for older images.
    ensure_mount(
        APPLIANCE_INACTIVE_ROOT_MOUNT,
        slot.root_device(),
        &["ext4", "ext2"],
    )?;
    Ok(mount_path)
}

fn appliance_pending_boot_state_body(
    current_slot: ApplianceSlot,
    target_slot: ApplianceSlot,
    current_version: &str,
    pending_version: &str,
) -> String {
    format!(
        "RHYTHM_ACTIVE_SLOT={}\nRHYTHM_LAST_GOOD_SLOT={}\nRHYTHM_PENDING_SLOT={}\nRHYTHM_PENDING_VERSION={}\nRHYTHM_ACTIVE_VERSION={}\nRHYTHM_BOOT_STATUS=pending\nRHYTHM_LAST_UPDATE_EPOCH_MS={}\nRHYTHM_LAST_ROLLBACK_SLOT=\nRHYTHM_LAST_ROLLBACK_VERSION=\nRHYTHM_LAST_ROLLBACK_EPOCH_MS=\n",
        current_slot.as_str(),
        current_slot.as_str(),
        target_slot.as_str(),
        pending_version,
        current_version,
        now_ms()
    )
}

fn appliance_idle_boot_state_body(active_slot: ApplianceSlot, active_version: &str) -> String {
    format!(
        "RHYTHM_ACTIVE_SLOT={}\nRHYTHM_LAST_GOOD_SLOT={}\nRHYTHM_PENDING_SLOT=\nRHYTHM_PENDING_VERSION=\nRHYTHM_ACTIVE_VERSION={}\nRHYTHM_BOOT_STATUS=idle\nRHYTHM_LAST_UPDATE_EPOCH_MS={}\nRHYTHM_LAST_ROLLBACK_SLOT=\nRHYTHM_LAST_ROLLBACK_VERSION=\nRHYTHM_LAST_ROLLBACK_EPOCH_MS=\n",
        active_slot.as_str(),
        active_slot.as_str(),
        active_version,
        now_ms()
    )
}

fn write_appliance_boot_state(
    current_slot: ApplianceSlot,
    target_slot: ApplianceSlot,
    current_version: &str,
    pending_version: &str,
) -> Result<(), String> {
    let body = appliance_pending_boot_state_body(
        current_slot,
        target_slot,
        current_version,
        pending_version,
    );
    bootstate::write_with_backup(
        Path::new(APPLIANCE_BOOT_STATE_PATH),
        Path::new(APPLIANCE_BOOT_STATE_BACKUP_PATH),
        &body,
    )
}

fn write_appliance_idle_boot_state(
    active_slot: ApplianceSlot,
    active_version: &str,
) -> Result<(), String> {
    let body = appliance_idle_boot_state_body(active_slot, active_version);
    bootstate::write_with_backup(
        Path::new(APPLIANCE_BOOT_STATE_PATH),
        Path::new(APPLIANCE_BOOT_STATE_BACKUP_PATH),
        &body,
    )
}

fn finalize_appliance_boot_switch<W, C, R>(
    current_slot: ApplianceSlot,
    target_slot: ApplianceSlot,
    current_version: &str,
    latest_version: &str,
    mut write_pending_boot_state: W,
    mut rewrite_cmdline: C,
    mut write_idle_boot_state: R,
) -> Result<(), String>
where
    W: FnMut(ApplianceSlot, ApplianceSlot, &str, &str) -> Result<(), String>,
    C: FnMut(ApplianceSlot) -> Result<(), String>,
    R: FnMut(ApplianceSlot, &str) -> Result<(), String>,
{
    // The boot marker must be durable before cmdline points at the new slot.
    // If power dies after cmdline is rewritten, S41bootstate can still treat
    // the next boot as pending and roll back if the process fails early.
    write_pending_boot_state(current_slot, target_slot, current_version, latest_version)?;

    if let Err(error) = rewrite_cmdline(target_slot) {
        // Cmdline still points at the current slot, so return bootstate to an
        // idle state. This avoids a stale pending marker confusing the next
        // ordinary boot if /boot was writable enough for the marker but not
        // for cmdline.txt.
        if let Err(reset_error) = write_idle_boot_state(current_slot, current_version) {
            return Err(format!(
                "{}; additionally failed to restore bootstate for slot {}: {}",
                error,
                current_slot.as_str(),
                reset_error
            ));
        }
        return Err(error);
    }

    Ok(())
}

fn update_appliance_cmdline_for_slot(slot: ApplianceSlot) -> Result<(), String> {
    ensure_mount(APPLIANCE_BOOT_MOUNT, APPLIANCE_BOOT_DEVICE, &["vfat"])?;
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

fn open_image_target(target_device: &Path) -> Result<File, String> {
    let target = File::options()
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
    Ok(target)
}

struct ProgressHashReader<R, F> {
    inner: R,
    hasher: Sha256,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    next_progress_bytes: u64,
    progress: F,
}

impl<R, F> ProgressHashReader<R, F>
where
    F: Fn(u64, Option<u64>),
{
    fn new(inner: R, total_bytes: Option<u64>, progress: F) -> Self {
        progress(0, total_bytes);
        Self {
            inner,
            hasher: Sha256::new(),
            downloaded_bytes: 0,
            total_bytes,
            next_progress_bytes: 0,
            progress,
        }
    }

    fn finish(self) -> String {
        (self.progress)(self.downloaded_bytes, self.total_bytes);
        hex_string(&self.hasher.finalize())
    }
}

impl<R, F> Read for ProgressHashReader<R, F>
where
    R: Read,
    F: Fn(u64, Option<u64>),
{
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        if read == 0 {
            return Ok(0);
        }

        self.hasher.update(&buf[..read]);
        self.downloaded_bytes = self.downloaded_bytes.saturating_add(read as u64);
        let reached_total = self.total_bytes == Some(self.downloaded_bytes);
        if self.downloaded_bytes >= self.next_progress_bytes || reached_total {
            (self.progress)(self.downloaded_bytes, self.total_bytes);
            self.next_progress_bytes = self.downloaded_bytes.saturating_add(512 * 1024);
        }
        Ok(read)
    }
}

fn stream_image_artifact_to_device<F>(
    client: &reqwest::blocking::Client,
    download_url: &str,
    target_device: &Path,
    gzip: bool,
    expected_sha256: Option<&str>,
    progress: F,
) -> Result<Option<bool>, String>
where
    F: Fn(u64, Option<u64>),
{
    let response = client
        .get(download_url)
        .send()
        .map_err(|e| format!("Download failed: {}", e))?;
    if !response.status().is_success() {
        return Err(format!("Download returned {}", response.status()));
    }

    let total_bytes = response.content_length();
    let source = ProgressHashReader::new(response, total_bytes, progress);
    let mut target = open_image_target(target_device)?;
    let actual_sha256 = if gzip {
        let mut decoder = GzDecoder::new(source);
        std::io::copy(&mut decoder, &mut target).map_err(|e| {
            format!(
                "Failed to stream update image to {}: {}",
                target_device.display(),
                e
            )
        })?;
        decoder.into_inner().finish()
    } else {
        let mut source = source;
        std::io::copy(&mut source, &mut target).map_err(|e| {
            format!(
                "Failed to stream update image to {}: {}",
                target_device.display(),
                e
            )
        })?;
        source.finish()
    };

    if let Some(expected) = expected_sha256 {
        if actual_sha256 != expected.to_ascii_lowercase() {
            return Err(format!(
                "SHA256 mismatch for update image: expected {}, got {}",
                expected, actual_sha256
            ));
        }
    }

    target
        .flush()
        .map_err(|e| format!("Failed to flush {}: {}", target_device.display(), e))?;
    target
        .sync_all()
        .map_err(|e| format!("Failed to sync {}: {}", target_device.display(), e))?;

    let _ = Command::new("sync").status();

    Ok(expected_sha256.map(|_| true))
}

fn apply_appliance_image_blocking(
    image_asset: &UpdateImageAsset,
    current_version: &str,
    latest_version: &str,
    package_plan: Option<&PackageInstallPlan<'_>>,
    progress: &(impl Fn(UpdateProgress) + Send + Sync),
) -> Result<ApplyResult, String> {
    // Long explicit timeout: reqwest's blocking client applies an implicit
    // 30-second WHOLE-REQUEST deadline by default, which a multi-hundred-MB
    // image over Pi Zero Wi-Fi always exceeds — nightly updates would fail
    // forever. One hour bounds a wedged transfer without breaking slow ones.
    let client = reqwest::blocking::Client::builder()
        .user_agent("rhythm-server")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60 * 60))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let current_slot = current_appliance_slot()?;
    let target_slot = current_slot.inactive();

    ensure_mount(APPLIANCE_BOOT_MOUNT, APPLIANCE_BOOT_DEVICE, &["vfat"])?;
    fs::create_dir_all(APPLIANCE_OTA_STAGING_DIR)
        .map_err(|e| format!("Failed to create {}: {}", APPLIANCE_OTA_STAGING_DIR, e))?;

    let apply_guard = ApplianceApplyGuard::arm(
        "rootfs slot apply",
        Duration::from_secs(APPLIANCE_ROOTFS_APPLY_GUARD_SECS),
    );
    let apply_result = (|| {
        // The inactive A/B rootfs slot is already a safe staging target. Stream
        // the compressed image there directly instead of first consuming
        // persistent /data capacity with a second full copy. A checksum error
        // leaves only the inactive slot unusable; the boot switch below is
        // never armed, so the current slot remains authoritative.
        cleanup_stale_downloads(Path::new(APPLIANCE_OTA_STAGING_DIR));
        let download_message = format!("Downloading {}", image_asset.name);
        progress(UpdateProgress::stage(
            OtaUpdateStage::Downloading,
            download_message.clone(),
        ));
        let checksum_verified = stream_image_artifact_to_device(
            &client,
            &image_asset.url,
            Path::new(target_slot.root_device()),
            artifact_uses_gzip(image_asset),
            image_asset.sha256.as_deref(),
            |downloaded, total| {
                apply_guard.progress();
                progress(UpdateProgress::downloading(
                    download_message.clone(),
                    downloaded,
                    total,
                ));
            },
        )?;
        progress(UpdateProgress::stage(
            OtaUpdateStage::Finalizing,
            "Preparing updated boot slot",
        ));
        let overlay_result =
            customize_inactive_rootfs(target_slot, image_asset, package_plan, progress)?;
        finalize_appliance_boot_switch(
            current_slot,
            target_slot,
            current_version,
            latest_version,
            write_appliance_boot_state,
            update_appliance_cmdline_for_slot,
            write_appliance_idle_boot_state,
        )?;
        Ok::<(Option<bool>, Option<bool>, Vec<String>), String>((
            checksum_verified,
            overlay_result.0,
            overlay_result.1,
        ))
    })();
    apply_guard.disarm();

    let (checksum_verified, package_checksum_verified, overlay_targets) = apply_result?;

    let mut installed_targets = vec![format!("rootfs_{}", target_slot.as_str())];
    installed_targets.extend(overlay_targets);
    installed_targets.push(format!("boot_slot_{}", target_slot.as_str()));

    Ok(ApplyResult {
        checksum_verified: combine_image_package_checksum(
            checksum_verified,
            package_checksum_verified,
            package_plan.is_some(),
        ),
        installed_targets,
    })
}

fn customize_inactive_rootfs(
    target_slot: ApplianceSlot,
    image_asset: &UpdateImageAsset,
    package_plan: Option<&PackageInstallPlan<'_>>,
    progress: &(impl Fn(UpdateProgress) + Send + Sync),
) -> Result<(Option<bool>, Vec<String>), String> {
    progress(UpdateProgress::stage(
        OtaUpdateStage::Finalizing,
        format!("Mounting inactive rootfs slot {}", target_slot.as_str()),
    ));
    let mount_path = mount_appliance_rootfs_slot(target_slot)?;
    let customize_result = (|| {
        write_appliance_image_version_marker(&mount_path, image_asset)?;

        let Some(package_plan) = package_plan else {
            return Ok((None, Vec::new()));
        };

        let remapped_targets =
            remap_install_targets_to_root(package_plan.install_targets, &mount_path)?;
        let package_download_path = artifact_staging_path(package_plan.asset_name);
        let package_result = apply_payload_blocking_with_download_path(
            package_plan.download_url,
            package_plan.asset_name,
            package_plan.expected_sha256,
            &remapped_targets,
            &package_download_path,
            // Overlay into the inactive rootfs: the A/B bootstate machinery
            // owns rollback there, so backups inside that root are just trash.
            None,
            progress,
        )?;
        let installed_targets = package_result
            .installed_targets
            .into_iter()
            .map(|target| format!("inactive_rootfs/{}", target))
            .collect();
        Ok((package_result.checksum_verified, installed_targets))
    })();
    let unmount_result = unmount_mountpoint(&mount_path);

    match (customize_result, unmount_result) {
        (Ok(result), Ok(())) => Ok(result),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(unmount_error)) => Err(format!(
            "{}; additionally failed to unmount inactive rootfs: {}",
            error, unmount_error
        )),
    }
}

fn write_appliance_image_version_marker(
    root: &Path,
    image_asset: &UpdateImageAsset,
) -> Result<(), String> {
    let marker_value = appliance_image_marker_value(image_asset);
    let marker_path = root.join(APPLIANCE_IMAGE_VERSION_PATH.trim_start_matches('/'));
    if let Some(parent) = marker_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to prepare {}: {}", parent.display(), e))?;
    }
    fs::write(&marker_path, format!("{}\n", marker_value))
        .map_err(|e| format!("Failed to write {}: {}", marker_path.display(), e))?;
    Ok(())
}

/// The identity to record for a freshly flashed rootfs image. Prefers the
/// release version, then the artifact checksum, then the asset name. The
/// marker must never be skipped: a slot without one reads as "unknown image"
/// and gets re-flashed by every later check — a nightly reflash loop that
/// burns out the SD card.
fn appliance_image_marker_value(asset: &UpdateImageAsset) -> String {
    if let Some(version) = asset
        .version
        .as_deref()
        .and_then(normalize_version_candidate)
    {
        return version;
    }
    if let Some(sha256) = asset
        .sha256
        .as_deref()
        .map(str::trim)
        .filter(|sha| !sha.is_empty())
    {
        let prefix: String = sha256.chars().take(16).collect();
        return format!("sha256-{}", sanitize_marker_token(&prefix));
    }
    sanitize_marker_token(&asset.name)
}

fn sanitize_marker_token(token: &str) -> String {
    let sanitized: String = token
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unversioned".to_string()
    } else {
        sanitized
    }
}

fn remap_install_targets_to_root(
    install_targets: &[InstallTarget],
    root: &Path,
) -> Result<Vec<InstallTarget>, String> {
    install_targets
        .iter()
        .map(|target| {
            let relative = destination_relative_to_root(&target.destination)?;
            Ok(InstallTarget {
                archive_path: target.archive_path.clone(),
                destination: root.join(relative),
                required: target.required,
            })
        })
        .collect()
}

fn destination_relative_to_root(destination: &Path) -> Result<PathBuf, String> {
    let relative = if destination.is_absolute() {
        destination
            .strip_prefix("/")
            .map_err(|e| format!("Invalid install target {}: {}", destination.display(), e))?
    } else {
        destination
    };

    if relative.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::Prefix(_)
        )
    }) {
        return Err(format!(
            "Install target cannot escape inactive rootfs: {}",
            destination.display()
        ));
    }

    Ok(relative.to_path_buf())
}

fn combine_image_package_checksum(
    image_checksum_verified: Option<bool>,
    package_checksum_verified: Option<bool>,
    package_present: bool,
) -> Option<bool> {
    if package_present {
        return (image_checksum_verified == Some(true) && package_checksum_verified == Some(true))
            .then_some(true);
    }

    image_checksum_verified
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

    // Bounded wait: this executes an arbitrary installed binary on every OTA
    // check. `.output()` has no timeout — a broken sibling binary blocking on
    // stdin would wedge the auto-update thread permanently.
    let mut child = Command::new(path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let output = loop {
        match child.try_wait() {
            Ok(Some(_)) => break child.wait_with_output().ok()?,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
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

fn appliance_image_base_version() -> Option<String> {
    let path = std::env::var_os("RHYTHM_IMAGE_VERSION_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(APPLIANCE_IMAGE_VERSION_PATH));
    read_version_marker(&path)
}

fn read_version_marker(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let value = raw.lines().next()?.trim();
    // Accept the fallback identities (`sha256-…`, asset names) written when a
    // release version is unavailable — rejecting them would read as "unknown
    // image" and re-trigger a flash on every check. Still refuse junk so a
    // corrupted marker can't masquerade as an identity.
    if value.is_empty() || value.len() > 128 {
        return None;
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return None;
    }
    Some(normalize_version_candidate(value).unwrap_or_else(|| value.to_string()))
}

fn infer_release_version_from_url(asset_url: &str) -> Option<String> {
    asset_url.split(['/', '?', '#']).find_map(|segment| {
        segment
            .strip_prefix('v')
            .and_then(normalize_version_candidate)
    })
}

fn version_needs_update(candidate: &str, current: &str) -> bool {
    match compare_release_versions(candidate, current) {
        Some(std::cmp::Ordering::Greater) => true,
        Some(_) => false,
        None => {
            // Semver comparison failed. If the candidate is a well-formed
            // release, allow recovery from a garbled local version. If the
            // candidate itself doesn't parse, refuse: treating a malformed
            // feed version as forever-newer makes the device reinstall the
            // same artifact on every check — a nightly reflash loop on the
            // appliance.
            if parse_release_version(candidate).is_some() {
                normalize_version_candidate(candidate) != normalize_version_candidate(current)
            } else {
                log::warn!(
                    target: "sys",
                    "Ignoring unparseable update version candidate '{}' (current '{}')",
                    candidate,
                    current
                );
                false
            }
        }
    }
}

fn version_is_older(candidate: &str, current: &str) -> bool {
    compare_release_versions(candidate, current) == Some(std::cmp::Ordering::Less)
}

fn compare_release_versions(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    let left = parse_release_version(left)?;
    let right = parse_release_version(right)?;

    Some(
        left.major
            .cmp(&right.major)
            .then_with(|| left.minor.cmp(&right.minor))
            .then_with(|| left.patch.cmp(&right.patch))
            .then_with(|| match (&left.pre, &right.pre) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (Some(left_pre), Some(right_pre)) => left_pre.cmp(right_pre),
            }),
    )
}

struct ParsedReleaseVersion {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Option<String>,
}

fn parse_release_version(value: &str) -> Option<ParsedReleaseVersion> {
    let normalized = normalize_version_candidate(value)?;
    let without_metadata = normalized
        .split_once('+')
        .map_or(normalized.as_str(), |(core, _)| core);
    let (core, pre) = without_metadata
        .split_once('-')
        .map_or((without_metadata, None), |(core, pre)| {
            (core, (!pre.is_empty()).then(|| pre.to_string()))
        });
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }

    Some(ParsedReleaseVersion {
        major,
        minor,
        patch,
        pre,
    })
}

/// Download and install a new payload, replacing the current server bundle.
fn apply_payload_blocking(
    download_url: &str,
    asset_name: &str,
    expected_sha256: Option<&str>,
    install_targets: &[InstallTarget],
    versions: LiveInstallVersions<'_>,
    progress: &(impl Fn(UpdateProgress) + Send + Sync),
) -> Result<ApplyResult, String> {
    let install_root = install_target_executable()?;
    let resolved_targets = if install_targets.is_empty() {
        default_bundle_install_targets(&install_root)
    } else {
        install_targets.to_vec()
    };
    let download_path = install_root.with_extension("download");

    apply_payload_blocking_with_download_path(
        download_url,
        asset_name,
        expected_sha256,
        &resolved_targets,
        &download_path,
        // Installing over the running system: keep backups and arm the
        // pending-update marker so a crash-looping build rolls back.
        Some(versions),
        progress,
    )
}

fn apply_payload_blocking_with_download_path(
    download_url: &str,
    asset_name: &str,
    expected_sha256: Option<&str>,
    resolved_targets: &[InstallTarget],
    download_path: &Path,
    live_install: Option<LiveInstallVersions<'_>>,
    progress: &(impl Fn(UpdateProgress) + Send + Sync),
) -> Result<ApplyResult, String> {
    // Long explicit timeout: reqwest's blocking client applies an implicit
    // 30-second WHOLE-REQUEST deadline by default, which a multi-hundred-MB
    // image over Pi Zero Wi-Fi always exceeds — nightly updates would fail
    // forever. One hour bounds a wedged transfer without breaking slow ones.
    let client = reqwest::blocking::Client::builder()
        .user_agent("rhythm-server")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60 * 60))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    remove_if_exists(download_path);
    cleanup_install_artifacts(resolved_targets);

    let download_message = format!("Downloading {}", asset_name);
    progress(UpdateProgress::stage(
        OtaUpdateStage::Downloading,
        download_message.clone(),
    ));
    download_release_with_progress(&client, download_url, download_path, |downloaded, total| {
        progress(UpdateProgress::downloading(
            download_message.clone(),
            downloaded,
            total,
        ));
    })?;

    let checksum_verified = match expected_sha256 {
        Some(expected) => {
            progress(UpdateProgress::stage(
                OtaUpdateStage::Verifying,
                "Verifying update bundle checksum",
            ));
            let actual = compute_sha256_hex(download_path)?;
            if actual != expected.to_ascii_lowercase() {
                remove_if_exists(download_path);
                return Err(format!(
                    "SHA256 mismatch for {}: expected {}, got {}",
                    asset_name, expected, actual
                ));
            }
            Some(true)
        }
        None => {
            // An unverified archive that is corrupt at origin installs a
            // broken binary that may never exec — a state the probation
            // counter cannot detect. Surface it loudly.
            log::warn!(
                target: "sys",
                "Update manifest for {} has no sha256; installing UNVERIFIED payload",
                asset_name
            );
            None
        }
    };

    progress(UpdateProgress::stage(
        OtaUpdateStage::Staging,
        "Staging update bundle",
    ));
    let staged_targets = stage_install_targets(download_path, asset_name, resolved_targets)
        .inspect_err(|_error| {
            cleanup_install_artifacts(resolved_targets);
            remove_if_exists(download_path);
        })?;
    progress(UpdateProgress::stage(
        OtaUpdateStage::Installing,
        "Installing update bundle",
    ));
    // Arm the rollback marker BEFORE the commit renames: a crash/power cut
    // between the backup rename and the install rename would otherwise leave
    // no binary AND no marker — nothing for the next boot to restore from.
    if let Some(versions) = live_install {
        let planned: Vec<AppliedInstallTarget> = staged_targets
            .iter()
            .filter(|staged| staged.stage_path.exists())
            .map(|staged| AppliedInstallTarget {
                destination: staged.spec.destination.clone(),
                backup_path: staged.backup_path.clone(),
                previously_existed: staged.spec.destination.exists(),
            })
            .collect();
        if let Err(error) = write_pending_update_marker(&planned, versions) {
            log::warn!(
                target: "sys",
                "Failed to arm pre-commit rollback marker: {}; continuing",
                error
            );
        }
    }

    let outcome = commit_staged_targets(&staged_targets).inspect_err(|_error| {
        cleanup_staged_files(&staged_targets);
        remove_if_exists(download_path);
        // Commit rolled itself back — disarm the pre-commit marker so the
        // next boots don't count probation attempts against the restored
        // (healthy) build.
        if live_install.is_some() {
            remove_pending_update_marker();
        }
    })?;

    if let Some(versions) = live_install {
        if let Err(error) = write_pending_update_marker(&outcome.applied, versions) {
            // Without a marker nothing would ever clean the .old backups up,
            // so fall back to the historical no-rollback cleanup.
            log::warn!(
                target: "sys",
                "Failed to arm pending-update rollback marker: {}; removing backups",
                error
            );
            remove_pending_update_marker();
            cleanup_backup_files(&outcome.applied);
        }
    } else {
        cleanup_backup_files(&outcome.applied);
    }

    progress(UpdateProgress::stage(
        OtaUpdateStage::Finalizing,
        "Cleaning up update artifacts",
    ));
    remove_if_exists(download_path);

    Ok(ApplyResult {
        checksum_verified,
        installed_targets: outcome.installed_targets,
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

/// Best-effort persistence before a planned restart/reboot.
///
/// This asks the event loop to flush its owned motion timers, then captures
/// canonical registry and room runtime state before an OTA or explicit restart.
pub fn persist_before_restart(state: &rhythm_os::state::SharedState) {
    log::info!(target: "sys", "Persisting runtime state before restart");
    if !rhythm_os::event_loop::request_motion_timer_persist(
        state,
        std::time::Duration::from_millis(750),
    ) {
        log::warn!(
            target: "sys",
            "Timed out waiting for motion timer persistence before restart"
        );
    }
    rhythm_os::commands::persist_state(state);
}

fn schedule_restart_with_best_effort_persist<S, P>(
    state: rhythm_os::state::SharedState,
    schedule: S,
    spawn_persist: P,
) -> std::io::Result<()>
where
    S: FnOnce(),
    P: FnOnce(rhythm_os::state::SharedState) -> std::io::Result<()>,
{
    schedule();
    spawn_persist(state)
}

fn spawn_restart_persist_worker(state: rhythm_os::state::SharedState) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("restart-persist".to_string())
        .spawn(move || persist_before_restart(&state))
        .map(|_| ())
}

/// Schedule a remotely requested restart and best-effort state persistence.
///
/// This intentionally uses the same restart scheduler as OTA. On rpiz
/// appliances it reboots the device; on supervised binary installs it exits so
/// the service manager can restart the process. The scheduler is armed before
/// persistence starts so a stuck state/event-loop lock cannot prevent reboot.
pub fn schedule_user_initiated_restart_with_best_effort_persist(
    state: rhythm_os::state::SharedState,
) -> std::io::Result<()> {
    schedule_restart_with_best_effort_persist(
        state,
        schedule_user_initiated_restart,
        spawn_restart_persist_worker,
    )
}

/// Schedule a post-update restart and best-effort state persistence.
///
/// OTA and manual restart both use the same restart scheduler; this wrapper
/// keeps the reboot/exit armed even if persistence gets stuck.
pub fn schedule_post_update_restart_with_best_effort_persist(
    state: rhythm_os::state::SharedState,
) -> std::io::Result<()> {
    schedule_restart_with_best_effort_persist(
        state,
        schedule_post_update_restart,
        spawn_restart_persist_worker,
    )
}

#[derive(Debug, PartialEq, Eq)]
enum ApplianceApplyGuardOutcome {
    Completed,
    DryRunSkipped,
}

struct ApplianceApplyGuard {
    completed: Arc<AtomicBool>,
    progress_tx: std::sync::mpsc::Sender<()>,
    handle: Option<JoinHandle<ApplianceApplyGuardOutcome>>,
}

impl ApplianceApplyGuard {
    fn arm(reason: &'static str, timeout: Duration) -> Self {
        let completed = Arc::new(AtomicBool::new(false));
        let completed_for_thread = completed.clone();
        let (progress_tx, progress_rx) = std::sync::mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("ota-apply-guard".to_string())
            .spawn(move || {
                loop {
                    match progress_rx.recv_timeout(timeout) {
                        Ok(()) => {
                            if completed_for_thread.load(Ordering::Acquire) {
                                return ApplianceApplyGuardOutcome::Completed;
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            std::thread::sleep(timeout);
                            break;
                        }
                    }
                }

                if completed_for_thread.load(Ordering::Acquire) {
                    return ApplianceApplyGuardOutcome::Completed;
                }

                log::error!(
                    target: "sys",
                    "Appliance OTA {} made no progress for {}s before slot switch; forcing reboot back to current slot",
                    reason,
                    timeout.as_secs()
                );
                if std::env::var_os("RHYTHM_RESTART_DRY_RUN").is_some() {
                    log::info!(
                        target: "sys",
                        "Restart dry-run ({} guard); skipping forced reboot",
                        reason
                    );
                    return ApplianceApplyGuardOutcome::DryRunSkipped;
                }

                force_appliance_reboot_or_exit(reason);
            });

        if let Err(error) = &handle {
            log::error!(
                target: "sys",
                "Failed to spawn appliance OTA apply guard: {}",
                error
            );
        }

        Self {
            completed,
            progress_tx,
            handle: handle.ok(),
        }
    }

    fn progress(&self) {
        let _ = self.progress_tx.send(());
    }

    fn disarm(&self) {
        self.completed.store(true, Ordering::Release);
        let _ = self.progress_tx.send(());
    }

    #[cfg(test)]
    fn join_for_test(mut self) -> ApplianceApplyGuardOutcome {
        self.handle
            .take()
            .expect("guard thread should be present")
            .join()
            .expect("guard thread should not panic")
    }
}

impl Drop for ApplianceApplyGuard {
    fn drop(&mut self) {
        let _ = self.handle.take();
    }
}

pub fn appliance_rollback_version_matches(version: &str) -> bool {
    if version.trim().is_empty() {
        return false;
    }

    let Some(rolled_back) = fs::read_to_string(APPLIANCE_BOOT_STATE_MIRROR_PATH)
        .ok()
        .and_then(|raw| bootstate_body_from_text(&raw))
        .and_then(|body| last_appliance_rollback_version_from_body(&body))
    else {
        return false;
    };

    rolled_back == version
}

fn bootstate_body_from_text(raw: &str) -> Option<String> {
    if raw.starts_with(bootstate::HASH_HEADER_PREFIX) {
        bootstate::verify_and_extract(raw)
    } else {
        Some(raw.to_string())
    }
}

fn last_appliance_rollback_version_from_body(body: &str) -> Option<String> {
    bootstate_value(body, "RHYTHM_LAST_ROLLBACK_VERSION")
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn bootstate_value<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    body.lines()
        .filter_map(|line| line.split_once('='))
        .find_map(|(k, v)| (k == key).then_some(v.trim()))
}

pub fn schedule_post_update_restart() {
    schedule_restart("self-update");
}

pub fn schedule_user_initiated_restart() {
    schedule_restart("user request");
}

pub fn schedule_factory_reset_restart() {
    schedule_restart("factory reset");
}

pub fn schedule_liveness_restart() {
    schedule_restart("liveness watchdog");
}

fn schedule_restart(reason: &'static str) {
    let dry_run = restart_dry_run_enabled();
    if !dry_run {
        if let Err(error) = crate::boot_diagnostics::record_restart_intent(reason) {
            log::warn!(
                target: "sys",
                "Failed to persist restart intent ({}): {}",
                reason,
                error
            );
        }
    }
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if dry_run {
            log::info!(target: "sys", "Restart dry-run ({}); skipping reboot/exit", reason);
            return;
        }
        match restart_strategy() {
            RestartStrategy::ApplianceReboot => {
                log::info!(target: "sys", "Rebooting appliance after {}...", reason);
                spawn_forced_reboot_fallback(reason);

                if let Err(error) = spawn_external_reboot_command(false) {
                    log::error!(
                        target: "sys",
                        "Failed to invoke appliance reboot after {}: {}; forcing reboot",
                        reason,
                        error
                    );
                    force_appliance_reboot_or_exit(reason);
                }
            }
            RestartStrategy::SupervisorExit => {
                log::info!(target: "sys", "Restarting after {}...", reason);
                std::process::exit(1);
            }
        }
    });
}

#[cfg(test)]
fn restart_dry_run_enabled() -> bool {
    true
}

#[cfg(not(test))]
fn restart_dry_run_enabled() -> bool {
    std::env::var_os("RHYTHM_RESTART_DRY_RUN").is_some()
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
            force_appliance_reboot_or_exit(reason);
        });

    if let Err(error) = spawn_result {
        log::error!(
            target: "sys",
            "Failed to spawn forced reboot fallback: {}",
            error
        );
    }
}

fn spawn_external_reboot_command(forced: bool) -> std::io::Result<()> {
    let args = external_reboot_args(forced);
    let mut command = Command::new("/sbin/reboot");
    command.args(args);
    match command.spawn() {
        Ok(_) => Ok(()),
        Err(primary_error) => {
            let mut fallback = Command::new("reboot");
            fallback.args(args);
            fallback.spawn().map(|_| ()).map_err(|fallback_error| {
                std::io::Error::new(
                    fallback_error.kind(),
                    format!(
                        "/sbin/reboot failed: {}; reboot failed: {}",
                        primary_error, fallback_error
                    ),
                )
            })
        }
    }
}

fn external_reboot_args(forced: bool) -> &'static [&'static str] {
    if forced {
        &["-f"]
    } else {
        &[]
    }
}

fn force_appliance_reboot_or_exit(reason: &'static str) -> ! {
    if let Err(error) = request_kernel_reboot() {
        log::error!(
            target: "sys",
            "Kernel reboot syscall failed for {}: {}; trying sysrq",
            reason,
            error
        );
    } else {
        log::error!(
            target: "sys",
            "Kernel reboot syscall returned for {}; continuing forced fallback",
            reason
        );
    }

    if let Err(error) = trigger_sysrq_reboot() {
        log::error!(
            target: "sys",
            "SysRq reboot trigger failed for {}: {}; spawning forced reboot command",
            reason,
            error
        );
    } else {
        log::error!(
            target: "sys",
            "SysRq reboot trigger returned for {}; spawning forced reboot command",
            reason
        );
    }

    if let Err(error) = spawn_external_reboot_command(true) {
        log::error!(
            target: "sys",
            "Failed to spawn forced appliance reboot command for {}: {}; exiting process",
            reason,
            error
        );
    }

    std::process::exit(1);
}

#[cfg(target_os = "linux")]
fn request_kernel_reboot() -> std::io::Result<()> {
    let rc = unsafe { libc::reboot(libc::LINUX_REBOOT_CMD_RESTART) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn request_kernel_reboot() -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "kernel reboot syscall is only available on Linux",
    ))
}

#[cfg(target_os = "linux")]
fn trigger_sysrq_reboot() -> std::io::Result<()> {
    use std::io::Write as _;

    let mut trigger = fs::OpenOptions::new()
        .write(true)
        .open("/proc/sysrq-trigger")?;
    trigger.write_all(b"b\n")
}

#[cfg(not(target_os = "linux"))]
fn trigger_sysrq_reboot() -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "sysrq reboot trigger is only available on Linux",
    ))
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
            // Durability before the commit rename (see extract path).
            if let Ok(staged_file) = File::open(&staged.stage_path) {
                let _ = staged_file.sync_all();
            }
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
            // fsync before the commit rename: without it, power loss shortly
            // after install can leave a zero-length destination binary on
            // ext4 — a crash-loop the probation counter can never catch
            // because the dead binary never runs to increment it.
            if let Ok(staged_file) = File::open(&target.stage_path) {
                let _ = staged_file.sync_all();
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

/// Result of committing staged targets: display names for status reporting
/// plus the applied records (with backup paths) the caller needs to either
/// arm a pending-update marker or clean the backups up.
struct CommitOutcome {
    installed_targets: Vec<String>,
    applied: Vec<AppliedInstallTarget>,
}

fn commit_staged_targets(staged_targets: &[StagedInstallTarget]) -> Result<CommitOutcome, String> {
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

    Ok(CommitOutcome {
        installed_targets,
        applied,
    })
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

/// Marker recording a freshly installed bundle whose first healthy startup
/// hasn't happened yet. Lives next to the install root so the updater and the
/// next process generation agree on its location without sharing state.
const PENDING_UPDATE_MARKER_FILE: &str = ".rhythm-update-pending.json";

/// How many process starts the new build gets before the previous binaries
/// are restored. Crash-looping builds typically die in milliseconds, so three
/// attempts is enough to ride out one-off flukes without leaving a supervisor
/// restart-looping a dead server forever.
const MAX_PENDING_START_ATTEMPTS: u32 = 3;

/// How long after the HTTP listener binds the new build must stay alive
/// before its rollback backups are discarded.
const STARTUP_VERIFY_GRACE_SECS: u64 = 30;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PendingUpdateTargetRecord {
    destination: PathBuf,
    backup: PathBuf,
    previously_existed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PendingUpdateMarker {
    start_attempts: u32,
    targets: Vec<PendingUpdateTargetRecord>,
    #[serde(default)]
    previous_version: Option<String>,
    #[serde(default)]
    target_version: Option<String>,
}

/// Version context for a live bundle install, recorded in the pending-update
/// marker so a later rollback can report which release was rolled back.
#[derive(Clone, Copy, Debug)]
struct LiveInstallVersions<'a> {
    previous: &'a str,
    target: &'a str,
}

/// Outcome of [`startup_update_health_check`].
#[derive(Debug, PartialEq, Eq)]
pub enum StartupUpdateDisposition {
    /// No update is awaiting verification.
    NoPendingUpdate,
    /// A freshly installed update is on probation; this is start attempt
    /// `attempt` of [`MAX_PENDING_START_ATTEMPTS`].
    PendingVerification { attempt: u32 },
    /// The new build failed to start too many times; the previous binaries
    /// were restored. The caller should exit so the supervisor restarts into
    /// the restored build.
    RolledBack {
        restored: Vec<PathBuf>,
        previous_version: Option<String>,
        target_version: Option<String>,
    },
}

fn pending_update_marker_path(install_root: &Path) -> PathBuf {
    install_root
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(PENDING_UPDATE_MARKER_FILE)
}

fn remove_pending_update_marker() {
    if let Ok(install_root) = install_target_executable() {
        remove_if_exists(&pending_update_marker_path(&install_root));
    }
}

fn write_pending_update_marker(
    applied: &[AppliedInstallTarget],
    versions: LiveInstallVersions<'_>,
) -> Result<(), String> {
    let install_root = install_target_executable()?;
    write_pending_update_marker_at(
        &pending_update_marker_path(&install_root),
        applied,
        Some(versions),
    )
}

fn write_pending_update_marker_at(
    marker_path: &Path,
    applied: &[AppliedInstallTarget],
    versions: Option<LiveInstallVersions<'_>>,
) -> Result<(), String> {
    let marker = PendingUpdateMarker {
        start_attempts: 0,
        targets: applied
            .iter()
            .map(|target| PendingUpdateTargetRecord {
                destination: target.destination.clone(),
                backup: target.backup_path.clone(),
                previously_existed: target.previously_existed,
            })
            .collect(),
        previous_version: versions.map(|versions| versions.previous.to_string()),
        target_version: versions.map(|versions| versions.target.to_string()),
    };
    write_pending_marker_file(marker_path, &marker)
}

fn write_pending_marker_file(
    marker_path: &Path,
    marker: &PendingUpdateMarker,
) -> Result<(), String> {
    let body = serde_json::to_vec_pretty(marker)
        .map_err(|e| format!("Failed to encode pending-update marker: {}", e))?;
    // Durable write (fsync file + parent dir): the marker is the rollback
    // safety net — losing it to power loss leaves a broken install with no
    // record that backups exist.
    bootstate::atomic_write_with_sync(marker_path, &body)
}

/// Startup gate for the bundle self-update flow.
///
/// Call once early in `main`. Counts process starts while an update awaits
/// verification; after [`MAX_PENDING_START_ATTEMPTS`] failed starts, restores
/// the `.old` backups so the supervisor's next restart runs the previous
/// build instead of crash-looping a broken one forever.
pub fn startup_update_health_check() -> StartupUpdateDisposition {
    let Ok(install_root) = install_target_executable() else {
        return StartupUpdateDisposition::NoPendingUpdate;
    };
    startup_update_health_check_at(&pending_update_marker_path(&install_root))
}

fn startup_update_health_check_at(marker_path: &Path) -> StartupUpdateDisposition {
    let raw = match fs::read_to_string(marker_path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return StartupUpdateDisposition::NoPendingUpdate;
        }
        Err(error) => {
            log::warn!(
                target: "sys",
                "Failed to read pending-update marker {}: {}; discarding it",
                marker_path.display(),
                error
            );
            remove_if_exists(marker_path);
            return StartupUpdateDisposition::NoPendingUpdate;
        }
    };

    let mut marker: PendingUpdateMarker = match serde_json::from_str(&raw) {
        Ok(marker) => marker,
        Err(error) => {
            log::warn!(
                target: "sys",
                "Pending-update marker {} is corrupt: {}; discarding it",
                marker_path.display(),
                error
            );
            remove_if_exists(marker_path);
            return StartupUpdateDisposition::NoPendingUpdate;
        }
    };

    marker.start_attempts = marker.start_attempts.saturating_add(1);
    if marker.start_attempts > MAX_PENDING_START_ATTEMPTS {
        let restored = roll_back_pending_update(&marker);
        record_bundle_rollback_at(
            &bundle_rollback_record_path_for_marker(marker_path),
            &marker,
        );
        remove_if_exists(marker_path);
        return StartupUpdateDisposition::RolledBack {
            restored,
            previous_version: marker.previous_version.clone(),
            target_version: marker.target_version.clone(),
        };
    }

    if let Err(error) = write_pending_marker_file(marker_path, &marker) {
        log::warn!(
            target: "sys",
            "Failed to record pending-update start attempt: {}",
            error
        );
    }
    StartupUpdateDisposition::PendingVerification {
        attempt: marker.start_attempts,
    }
}

fn roll_back_pending_update(marker: &PendingUpdateMarker) -> Vec<PathBuf> {
    let mut restored = Vec::new();
    for target in marker.targets.iter().rev() {
        if target.previously_existed {
            if !target.backup.exists() {
                log::warn!(
                    target: "sys",
                    "Pending-update rollback: backup {} is missing; leaving {} in place",
                    target.backup.display(),
                    target.destination.display()
                );
                continue;
            }
            let _ = fs::remove_file(&target.destination);
            match fs::rename(&target.backup, &target.destination) {
                Ok(()) => restored.push(target.destination.clone()),
                Err(error) => log::warn!(
                    target: "sys",
                    "Pending-update rollback: failed to restore {}: {}",
                    target.destination.display(),
                    error
                ),
            }
        } else {
            // The update introduced this file; restoring means removing it.
            let _ = fs::remove_file(&target.destination);
            restored.push(target.destination.clone());
        }
    }
    restored
}

/// Discard the pending-update marker and its rollback backups after the new
/// build proved healthy. `data_dir`, when known, receives an OTA-history
/// "verified" entry.
pub fn mark_update_verified(data_dir: Option<&Path>) {
    let Ok(install_root) = install_target_executable() else {
        return;
    };
    let marker_path = pending_update_marker_path(&install_root);
    let marker_versions = fs::read_to_string(&marker_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<PendingUpdateMarker>(&raw).ok())
        .map(|marker| (marker.previous_version, marker.target_version));
    if mark_update_verified_at(&marker_path) {
        log::info!(
            target: "sys",
            "Update verified after healthy startup; cleared rollback backups"
        );
        if let (Some(data_dir), Some((previous, target))) = (data_dir, marker_versions) {
            crate::ota_history::record(
                data_dir,
                crate::ota_history::entry(
                    previous.as_deref(),
                    target.as_deref(),
                    "startup",
                    "verified",
                ),
            );
        }
    }
}

fn mark_update_verified_at(marker_path: &Path) -> bool {
    let Ok(raw) = fs::read_to_string(marker_path) else {
        return false;
    };
    if let Ok(marker) = serde_json::from_str::<PendingUpdateMarker>(&raw) {
        for target in &marker.targets {
            if target.previously_existed {
                remove_if_exists(&target.backup);
            }
        }
    }
    remove_if_exists(marker_path);
    // A newer update survived verification; an older rollback is stale news.
    remove_if_exists(&bundle_rollback_record_path_for_marker(marker_path));
    true
}

/// Sidecar file recording the most recent bundle (component) rollback so it
/// can be reported through `/api/ota/status` after the previous build is back
/// up. The appliance's A/B image rollbacks are recorded separately by
/// S41bootstate in the bootstate mirror.
const BUNDLE_ROLLBACK_RECORD_FILE: &str = ".rhythm-last-rollback.json";

/// How a rolled-back update had been installed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RollbackKind {
    /// A/B rootfs slot switch reverted by the appliance boot machinery.
    ImageSlot,
    /// Live binary bundle restored from `.old` backups after failed starts.
    ComponentBundle,
}

/// The most recent rolled-back update, for surfacing in OTA status.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LastRollback {
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at_epoch_ms: Option<i64>,
    pub kind: RollbackKind,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct BundleRollbackRecord {
    #[serde(default)]
    previous_version: Option<String>,
    #[serde(default)]
    target_version: Option<String>,
    #[serde(default)]
    at_epoch_ms: Option<i64>,
}

fn bundle_rollback_record_path_for_marker(marker_path: &Path) -> PathBuf {
    marker_path.with_file_name(BUNDLE_ROLLBACK_RECORD_FILE)
}

fn bundle_rollback_record_path() -> Option<PathBuf> {
    let install_root = install_target_executable().ok()?;
    Some(pending_update_marker_path(&install_root).with_file_name(BUNDLE_ROLLBACK_RECORD_FILE))
}

fn record_bundle_rollback_at(record_path: &Path, marker: &PendingUpdateMarker) {
    let record = BundleRollbackRecord {
        previous_version: marker.previous_version.clone(),
        target_version: marker.target_version.clone(),
        at_epoch_ms: Some(now_ms()),
    };
    match serde_json::to_vec_pretty(&record) {
        Ok(body) => {
            if let Err(error) = fs::write(record_path, body) {
                log::warn!(
                    target: "sys",
                    "Failed to record bundle rollback at {}: {}",
                    record_path.display(),
                    error
                );
            }
        }
        Err(error) => log::warn!(
            target: "sys",
            "Failed to encode bundle rollback record: {}",
            error
        ),
    }
}

fn load_bundle_rollback_at(record_path: &Path) -> Option<LastRollback> {
    let raw = fs::read_to_string(record_path).ok()?;
    let record: BundleRollbackRecord = serde_json::from_str(&raw).ok()?;
    Some(LastRollback {
        version: record.target_version.filter(|v| !v.is_empty())?,
        from_version: record.previous_version.filter(|v| !v.is_empty()),
        at_epoch_ms: record.at_epoch_ms,
        kind: RollbackKind::ComponentBundle,
    })
}

fn appliance_image_rollback() -> Option<LastRollback> {
    let raw = fs::read_to_string(APPLIANCE_BOOT_STATE_MIRROR_PATH).ok()?;
    let body = bootstate_body_from_text(&raw)?;
    appliance_rollback_from_bootstate_body(&body)
}

fn appliance_rollback_from_bootstate_body(body: &str) -> Option<LastRollback> {
    let version = last_appliance_rollback_version_from_body(body)?;
    let at_epoch_ms = bootstate_value(body, "RHYTHM_LAST_ROLLBACK_EPOCH_MS")
        .and_then(|value| value.parse::<i64>().ok());
    Some(LastRollback {
        version,
        from_version: bootstate_value(body, "RHYTHM_ACTIVE_VERSION")
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        at_epoch_ms,
        kind: RollbackKind::ImageSlot,
    })
}

/// The most recent rollback across both rollback mechanisms, for OTA status
/// reporting. `None` when this install has never rolled an update back (or
/// the records have been cleared by a later verified update).
pub fn last_rollback() -> Option<LastRollback> {
    let bundle = bundle_rollback_record_path()
        .as_deref()
        .and_then(load_bundle_rollback_at);
    merge_rollbacks(bundle, appliance_image_rollback())
}

fn merge_rollbacks(
    left: Option<LastRollback>,
    right: Option<LastRollback>,
) -> Option<LastRollback> {
    match (left, right) {
        (Some(left), Some(right)) => {
            if right.at_epoch_ms.unwrap_or(0) > left.at_epoch_ms.unwrap_or(0) {
                Some(right)
            } else {
                Some(left)
            }
        }
        (left, right) => left.or(right),
    }
}

/// Sidecar file tracking how often a component-drift repair was attempted for
/// a given release. Without a cap, a component whose `--version` can never
/// match (missing shared library, foreign build) re-triggers a repair install
/// and restart on every nightly check, forever.
const DRIFT_REPAIR_STATE_FILE: &str = ".rhythm-drift-repair.json";
const MAX_DRIFT_REPAIR_ATTEMPTS: u32 = 3;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct DriftRepairState {
    target_version: String,
    attempts: u32,
}

fn drift_repair_state_path(install_root: &Path) -> PathBuf {
    install_root
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(DRIFT_REPAIR_STATE_FILE)
}

fn load_drift_repair_state(path: &Path) -> DriftRepairState {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn drift_repair_exhausted_at(path: &Path, target_version: &str) -> bool {
    let state = load_drift_repair_state(path);
    state.target_version == target_version && state.attempts >= MAX_DRIFT_REPAIR_ATTEMPTS
}

fn record_drift_repair_attempt_at(path: &Path, target_version: &str) {
    let mut state = load_drift_repair_state(path);
    if state.target_version != target_version {
        state = DriftRepairState {
            target_version: target_version.to_string(),
            attempts: 0,
        };
    }
    state.attempts = state.attempts.saturating_add(1);
    match serde_json::to_vec_pretty(&state) {
        Ok(body) => {
            if let Err(error) = fs::write(path, body) {
                log::warn!(
                    target: "sys",
                    "Failed to record drift repair attempt at {}: {}",
                    path.display(),
                    error
                );
            }
        }
        Err(error) => log::warn!(
            target: "sys",
            "Failed to encode drift repair state: {}",
            error
        ),
    }
}

fn record_drift_repair_attempt(target_version: &str) {
    if let Ok(install_root) = install_target_executable() {
        record_drift_repair_attempt_at(&drift_repair_state_path(&install_root), target_version);
    }
}

fn clear_drift_repair_state_at(path: &Path) {
    remove_if_exists(path);
}

/// Spawn the post-startup verification thread. Call after the HTTP listener
/// has bound; once [`STARTUP_VERIFY_GRACE_SECS`] pass with the process still
/// alive, the pending update is considered good and its backups are removed.
pub fn spawn_update_verification_marker(data_dir: Option<PathBuf>) {
    let spawn_result = std::thread::Builder::new()
        .name("update-verify".to_string())
        .spawn(move || {
            std::thread::sleep(Duration::from_secs(STARTUP_VERIFY_GRACE_SECS));
            mark_update_verified(data_dir.as_deref());
        });
    if let Err(error) = spawn_result {
        log::warn!(
            target: "sys",
            "Failed to spawn update verification thread: {}",
            error
        );
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
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

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

    fn spawn_json_fixture(body: &'static str) -> String {
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
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(headers.as_bytes());
            let _ = stream.write_all(body.as_bytes());
        });
        thread::sleep(Duration::from_millis(10));
        format!("http://127.0.0.1:{}/feeds/manifest.json", port)
    }

    fn test_client() -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("client")
    }

    fn string_error<T>(result: Result<T, String>) -> String {
        match result {
            Ok(_) => panic!("expected error"),
            Err(error) => error,
        }
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
    fn appliance_pending_boot_state_body_marks_target_slot_pending() {
        let body =
            appliance_pending_boot_state_body(ApplianceSlot::A, ApplianceSlot::B, "0.4.1", "0.4.2");

        assert!(body.contains("RHYTHM_ACTIVE_SLOT=a\n"));
        assert!(body.contains("RHYTHM_LAST_GOOD_SLOT=a\n"));
        assert!(body.contains("RHYTHM_PENDING_SLOT=b\n"));
        assert!(body.contains("RHYTHM_PENDING_VERSION=0.4.2\n"));
        assert!(body.contains("RHYTHM_ACTIVE_VERSION=0.4.1\n"));
        assert!(body.contains("RHYTHM_BOOT_STATUS=pending\n"));
        assert!(body.contains("RHYTHM_LAST_ROLLBACK_VERSION=\n"));
    }

    #[test]
    fn appliance_idle_boot_state_body_clears_pending_marker() {
        let body = appliance_idle_boot_state_body(ApplianceSlot::A, "0.4.1");

        assert!(body.contains("RHYTHM_ACTIVE_SLOT=a\n"));
        assert!(body.contains("RHYTHM_LAST_GOOD_SLOT=a\n"));
        assert!(body.contains("RHYTHM_PENDING_SLOT=\n"));
        assert!(body.contains("RHYTHM_PENDING_VERSION=\n"));
        assert!(body.contains("RHYTHM_ACTIVE_VERSION=0.4.1\n"));
        assert!(body.contains("RHYTHM_BOOT_STATUS=idle\n"));
    }

    #[test]
    fn finalize_appliance_boot_switch_writes_pending_marker_before_cmdline() {
        let steps = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let pending_steps = steps.clone();
        let cmdline_steps = steps.clone();
        let reset_steps = steps.clone();

        finalize_appliance_boot_switch(
            ApplianceSlot::A,
            ApplianceSlot::B,
            "0.4.1",
            "0.4.2",
            move |current, target, current_version, latest_version| {
                assert_eq!(current, ApplianceSlot::A);
                assert_eq!(target, ApplianceSlot::B);
                assert_eq!(current_version, "0.4.1");
                assert_eq!(latest_version, "0.4.2");
                pending_steps.lock().unwrap().push("bootstate");
                Ok(())
            },
            move |target| {
                assert_eq!(target, ApplianceSlot::B);
                cmdline_steps.lock().unwrap().push("cmdline");
                Ok(())
            },
            move |_, _| {
                reset_steps.lock().unwrap().push("reset");
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(*steps.lock().unwrap(), vec!["bootstate", "cmdline"]);
    }

    #[test]
    fn finalize_appliance_boot_switch_resets_pending_marker_when_cmdline_fails() {
        let steps = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let pending_steps = steps.clone();
        let cmdline_steps = steps.clone();
        let reset_steps = steps.clone();

        let result = finalize_appliance_boot_switch(
            ApplianceSlot::A,
            ApplianceSlot::B,
            "0.4.1",
            "0.4.2",
            move |_, _, _, _| {
                pending_steps.lock().unwrap().push("bootstate");
                Ok(())
            },
            move |_| {
                cmdline_steps.lock().unwrap().push("cmdline");
                Err("cmdline rewrite failed".to_string())
            },
            move |slot, version| {
                assert_eq!(slot, ApplianceSlot::A);
                assert_eq!(version, "0.4.1");
                reset_steps.lock().unwrap().push("reset");
                Ok(())
            },
        );

        assert_eq!(result.unwrap_err(), "cmdline rewrite failed");
        assert_eq!(
            *steps.lock().unwrap(),
            vec!["bootstate", "cmdline", "reset"]
        );
    }

    #[test]
    fn appliance_slot_helpers_cover_invalid_roots_and_reset_failures() {
        assert_eq!(
            parse_appliance_slot_from_cmdline("console=tty1 rootwait rw").unwrap_err(),
            "Kernel command line did not include a root= device"
        );
        assert_eq!(
            appliance_slot_from_root_device("/dev/sda1").unwrap_err(),
            "Unsupported appliance root device /dev/sda1"
        );
        assert_eq!(
            appliance_root_arg_for_slot(ApplianceSlot::A),
            format!("root={}", APPLIANCE_ROOTFS_A_DEVICE)
        );

        let result = finalize_appliance_boot_switch(
            ApplianceSlot::A,
            ApplianceSlot::B,
            "0.4.1",
            "0.4.2",
            |_, _, _, _| Ok(()),
            |_| Err("cmdline rewrite failed".to_string()),
            |_, _| Err("reset failed".to_string()),
        );
        assert_eq!(
            result.unwrap_err(),
            "cmdline rewrite failed; additionally failed to restore bootstate for slot a: reset failed"
        );
    }

    #[test]
    fn rollback_version_parser_reads_plain_and_hashed_bootstate() {
        let body = "RHYTHM_ACTIVE_SLOT=b\nRHYTHM_LAST_ROLLBACK_VERSION=0.4.2\n";
        assert_eq!(
            last_appliance_rollback_version_from_body(body).as_deref(),
            Some("0.4.2")
        );

        let composed = String::from_utf8(bootstate::compose(body)).unwrap();
        let extracted = bootstate_body_from_text(&composed).unwrap();
        assert_eq!(
            last_appliance_rollback_version_from_body(&extracted).as_deref(),
            Some("0.4.2")
        );

        let tampered = composed.replace("0.4.2", "0.4.3");
        assert_eq!(bootstate_body_from_text(&tampered), None);
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
    fn appliance_apply_guard_exits_cleanly_after_disarm() {
        let guard = ApplianceApplyGuard::arm("test apply", Duration::from_millis(1));
        guard.disarm();

        assert_eq!(guard.join_for_test(), ApplianceApplyGuardOutcome::Completed);
    }

    #[test]
    fn appliance_apply_guard_would_force_reboot_when_apply_stalls() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_RESTART_DRY_RUN", "1");

        let guard = ApplianceApplyGuard::arm("test apply", Duration::from_millis(1));
        let outcome = guard.join_for_test();

        std::env::remove_var("RHYTHM_RESTART_DRY_RUN");
        assert_eq!(outcome, ApplianceApplyGuardOutcome::DryRunSkipped);
    }

    #[test]
    fn appliance_apply_guard_resets_timeout_when_streaming_progresses() {
        let guard = ApplianceApplyGuard::arm("test apply", Duration::from_millis(40));
        std::thread::sleep(Duration::from_millis(25));
        guard.progress();
        std::thread::sleep(Duration::from_millis(25));
        guard.progress();
        guard.disarm();

        assert_eq!(guard.join_for_test(), ApplianceApplyGuardOutcome::Completed);
    }

    #[test]
    fn dry_run_restart_schedulers_return_without_rebooting() {
        schedule_post_update_restart();
        schedule_user_initiated_restart();
        schedule_factory_reset_restart();
        schedule_liveness_restart();
        std::thread::sleep(Duration::from_millis(1200));
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
    fn appliance_image_helpers_classify_rootfs_and_compression() {
        let rootfs = UpdateImageAsset {
            name: "rootfs.ext2".to_string(),
            kind: ReleaseArtifactKind::RootfsImage,
            url: "https://example.invalid/rootfs.ext2".to_string(),
            version: Some("0.4.147".to_string()),
            sha256: None,
            size: None,
            compression: None,
        };
        let gz_rootfs = UpdateImageAsset {
            name: "rootfs.ext2.gz".to_string(),
            kind: ReleaseArtifactKind::RootfsImage,
            url: "https://example.invalid/rootfs.ext2.gz".to_string(),
            version: Some("0.4.147".to_string()),
            sha256: None,
            size: None,
            compression: Some("gzip".to_string()),
        };
        let disk = UpdateImageAsset {
            name: "appliance.img".to_string(),
            kind: ReleaseArtifactKind::DiskImage,
            url: "https://example.invalid/appliance.img".to_string(),
            version: Some("0.4.147".to_string()),
            sha256: None,
            size: None,
            compression: None,
        };

        assert!(has_rootfs_image(&[disk.clone(), rootfs.clone()]));
        assert!(!has_rootfs_image(std::slice::from_ref(&disk)));
        assert!(!artifact_uses_gzip(&rootfs));
        assert!(artifact_uses_gzip(&gz_rootfs));
        assert_eq!(
            infer_image_kind("rootfs.ext2"),
            ReleaseArtifactKind::RootfsImage
        );
        assert_eq!(infer_image_kind("disk.img"), ReleaseArtifactKind::DiskImage);
    }

    #[test]
    fn appliance_update_prefers_gzip_rootfs_asset() {
        let info = UpdateInfo {
            current_version: "0.4.146".to_string(),
            latest_version: "0.4.147".to_string(),
            current_package_version: "0.4.146".to_string(),
            latest_package_version: "0.4.147".to_string(),
            current_image_version: Some("0.4.146".to_string()),
            latest_image_version: Some("0.4.147".to_string()),
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
                    version: Some("0.4.147".to_string()),
                    sha256: None,
                    size: None,
                    compression: None,
                },
                UpdateImageAsset {
                    name: "rootfs.ext2.gz".to_string(),
                    kind: ReleaseArtifactKind::RootfsImage,
                    url: "https://example.invalid/rootfs.ext2.gz".to_string(),
                    version: Some("0.4.147".to_string()),
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
    fn appliance_rootfs_selection_uses_image_base_version() {
        let rootfs = UpdateImageAsset {
            name: "rootfs.ext2.gz".to_string(),
            kind: ReleaseArtifactKind::RootfsImage,
            url: "https://example.invalid/rpiz/v0.4.256/rootfs.ext2.gz".to_string(),
            version: Some("0.4.256".to_string()),
            sha256: None,
            size: None,
            compression: Some("gzip".to_string()),
        };

        assert!(
            image_asset_requires_apply(&rootfs, None),
            "missing local image marker should force a rootfs update"
        );
        assert!(image_asset_requires_apply(&rootfs, Some("0.4.200")));
        assert!(!image_asset_requires_apply(&rootfs, Some("0.4.256")));
        assert!(
            !image_asset_requires_apply(&rootfs, Some("0.4.257")),
            "a newer local image marker must not be downgraded"
        );
        assert_eq!(
            preferred_rootfs_image_asset(std::slice::from_ref(&rootfs), Some("0.4.200"))
                .map(|asset| asset.name.as_str()),
            Some("rootfs.ext2.gz")
        );
        assert!(
            preferred_rootfs_image_asset(std::slice::from_ref(&rootfs), Some("0.4.256")).is_none()
        );

        let newer_uncompressed = UpdateImageAsset {
            name: "rootfs.ext2".to_string(),
            kind: ReleaseArtifactKind::RootfsImage,
            url: "https://example.invalid/rpiz/v0.4.257/rootfs.ext2".to_string(),
            version: Some("0.4.257".to_string()),
            sha256: None,
            size: None,
            compression: None,
        };
        assert_eq!(
            preferred_rootfs_image_asset(&[rootfs, newer_uncompressed], Some("0.4.200"))
                .map(|asset| asset.version.as_deref()),
            Some(Some("0.4.257"))
        );
    }

    #[test]
    fn release_version_helpers_handle_urls_and_prereleases() {
        assert_eq!(
            infer_release_version_from_url("../rpiz/v0.4.256-beta/rootfs.ext2.gz").as_deref(),
            Some("0.4.256-beta")
        );
        assert_eq!(
            infer_release_version_from_url("../rpiz-stable/v0.4.257-stable/rootfs.ext2.gz")
                .as_deref(),
            Some("0.4.257-stable")
        );
        assert!(version_needs_update("0.4.257-beta", "0.4.256-beta"));
        assert!(version_needs_update("0.4.257", "0.4.257-beta"));
        assert!(version_needs_update("0.4.257-stable", "0.4.257-beta"));
        assert!(!version_needs_update("0.4.257-beta", "0.4.257"));
        assert!(!version_needs_update("0.4.257-beta", "0.4.257-stable"));
        assert!(!version_needs_update("0.4.256", "0.4.257"));
    }

    #[test]
    fn version_needs_update_fails_closed_on_unparseable_candidates() {
        // A malformed feed version must not read as forever-newer: that turns
        // every nightly check into a reinstall of the same artifact.
        assert!(!version_needs_update("latest", "0.4.257"));
        assert!(!version_needs_update("", "0.4.257"));
        assert!(!version_needs_update("2024.06.10.1", "0.4.257"));
        assert!(!version_needs_update("latest", "latest"));

        // But a well-formed candidate still recovers a garbled local version.
        assert!(version_needs_update("0.4.258", "unknown"));
        assert!(version_needs_update("0.4.258", ""));
    }

    #[test]
    fn appliance_image_marker_value_prefers_version_then_checksum_then_name() {
        let mut asset = UpdateImageAsset {
            name: "rootfs.ext2.gz".to_string(),
            kind: ReleaseArtifactKind::RootfsImage,
            url: "https://example.invalid/rootfs.ext2.gz".to_string(),
            version: Some("v0.4.257".to_string()),
            sha256: Some("ABCDEF0123456789abcdef0123456789".to_string()),
            size: None,
            compression: Some("gzip".to_string()),
        };
        assert_eq!(appliance_image_marker_value(&asset), "0.4.257");

        asset.version = None;
        assert_eq!(
            appliance_image_marker_value(&asset),
            "sha256-ABCDEF0123456789"
        );

        asset.sha256 = None;
        assert_eq!(appliance_image_marker_value(&asset), "rootfs.ext2.gz");

        asset.name = "weird name!?.gz".to_string();
        assert_eq!(appliance_image_marker_value(&asset), "weird_name__.gz");
    }

    #[test]
    fn image_version_marker_is_written_even_without_a_release_version() {
        // Regression guard for the nightly reflash loop: skipping the marker
        // leaves the slot reading as "unknown image", so every later check
        // re-flashes it.
        let root = unique_test_dir("image-marker-fallback");
        let asset = UpdateImageAsset {
            name: "rootfs.ext2.gz".to_string(),
            kind: ReleaseArtifactKind::RootfsImage,
            url: "https://example.invalid/rootfs.ext2.gz".to_string(),
            version: Some("not-a-version".to_string()),
            sha256: None,
            size: None,
            compression: Some("gzip".to_string()),
        };

        write_appliance_image_version_marker(&root, &asset).unwrap();

        let marker_path = root.join(APPLIANCE_IMAGE_VERSION_PATH.trim_start_matches('/'));
        let marker = read_version_marker(&marker_path);
        assert_eq!(marker.as_deref(), Some("rootfs.ext2.gz"));
        assert!(
            !image_asset_requires_apply(
                &UpdateImageAsset {
                    version: None,
                    ..asset.clone()
                },
                marker.as_deref()
            ),
            "an unversioned image must not require re-apply once its marker exists"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn read_version_marker_accepts_fallback_identities_and_rejects_junk() {
        let dir = unique_test_dir("version-marker-read");
        let marker = dir.join("marker");

        fs::write(&marker, "v0.4.257-beta\n").unwrap();
        assert_eq!(
            read_version_marker(&marker).as_deref(),
            Some("0.4.257-beta")
        );

        fs::write(&marker, "sha256-abcdef0123456789\n").unwrap();
        assert_eq!(
            read_version_marker(&marker).as_deref(),
            Some("sha256-abcdef0123456789")
        );

        fs::write(&marker, "rootfs.ext2.gz\n").unwrap();
        assert_eq!(
            read_version_marker(&marker).as_deref(),
            Some("rootfs.ext2.gz")
        );

        fs::write(&marker, "\n").unwrap();
        assert_eq!(read_version_marker(&marker), None);

        fs::write(&marker, "garbage with spaces\n").unwrap();
        assert_eq!(read_version_marker(&marker), None);

        fs::write(&marker, format!("{}\n", "x".repeat(200))).unwrap();
        assert_eq!(read_version_marker(&marker), None);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn drift_repair_attempts_are_capped_per_version_and_reset_on_new_versions() {
        let dir = unique_test_dir("drift-cap");
        let state_path = dir.join(DRIFT_REPAIR_STATE_FILE);

        assert!(!drift_repair_exhausted_at(&state_path, "1.0.0"));

        for _ in 0..MAX_DRIFT_REPAIR_ATTEMPTS {
            record_drift_repair_attempt_at(&state_path, "1.0.0");
        }
        assert!(
            drift_repair_exhausted_at(&state_path, "1.0.0"),
            "repairs for the same release must stop after {} attempts",
            MAX_DRIFT_REPAIR_ATTEMPTS
        );

        // A new release gets a fresh budget.
        assert!(!drift_repair_exhausted_at(&state_path, "1.0.1"));
        record_drift_repair_attempt_at(&state_path, "1.0.1");
        assert!(!drift_repair_exhausted_at(&state_path, "1.0.1"));
        assert!(
            !drift_repair_exhausted_at(&state_path, "1.0.0"),
            "recording a newer version resets the counter entirely"
        );

        // Clearing the state restores the full budget.
        for _ in 0..MAX_DRIFT_REPAIR_ATTEMPTS {
            record_drift_repair_attempt_at(&state_path, "1.0.1");
        }
        assert!(drift_repair_exhausted_at(&state_path, "1.0.1"));
        clear_drift_repair_state_at(&state_path);
        assert!(!drift_repair_exhausted_at(&state_path, "1.0.1"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn drift_repair_state_tolerates_corrupt_file() {
        let dir = unique_test_dir("drift-corrupt");
        let state_path = dir.join(DRIFT_REPAIR_STATE_FILE);
        fs::write(&state_path, b"{not json").unwrap();

        assert!(!drift_repair_exhausted_at(&state_path, "1.0.0"));
        record_drift_repair_attempt_at(&state_path, "1.0.0");
        let state = load_drift_repair_state(&state_path);
        assert_eq!(state.target_version, "1.0.0");
        assert_eq!(state.attempts, 1);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn remap_install_targets_keeps_package_inside_inactive_rootfs() {
        let root = PathBuf::from("/data/ota/inactive-rootfs");
        let targets = vec![
            InstallTarget {
                archive_path: "rhythm-server".to_string(),
                destination: PathBuf::from("/usr/bin/rhythm-server"),
                required: true,
            },
            InstallTarget {
                archive_path: "rhythm-chipd".to_string(),
                destination: PathBuf::from("usr/bin/rhythm-chipd"),
                required: true,
            },
        ];

        let remapped = remap_install_targets_to_root(&targets, &root).unwrap();

        assert_eq!(
            remapped[0].destination,
            PathBuf::from("/data/ota/inactive-rootfs/usr/bin/rhythm-server")
        );
        assert_eq!(
            remapped[1].destination,
            PathBuf::from("/data/ota/inactive-rootfs/usr/bin/rhythm-chipd")
        );

        let escape = remap_install_targets_to_root(
            &[InstallTarget {
                archive_path: "bad".to_string(),
                destination: PathBuf::from("../bad"),
                required: true,
            }],
            &root,
        )
        .unwrap_err();
        assert!(escape.contains("cannot escape inactive rootfs"));
    }

    #[test]
    fn write_appliance_image_version_marker_writes_normalized_version() {
        let dir = unique_test_dir("ota-image-version-write");
        let asset = UpdateImageAsset {
            name: "rootfs.ext2.gz".to_string(),
            kind: ReleaseArtifactKind::RootfsImage,
            url: "https://example.invalid/rootfs.ext2.gz".to_string(),
            version: Some("v2.0.0-beta".to_string()),
            sha256: None,
            size: None,
            compression: Some("gzip".to_string()),
        };
        write_appliance_image_version_marker(&dir, &asset).unwrap();

        assert_eq!(
            fs::read_to_string(dir.join("etc/rhythm-image-version")).unwrap(),
            "2.0.0-beta\n"
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn combine_image_package_checksum_requires_both_when_package_is_present() {
        assert_eq!(
            combine_image_package_checksum(Some(true), None, false),
            Some(true)
        );
        assert_eq!(
            combine_image_package_checksum(Some(true), Some(true), true),
            Some(true)
        );
        assert_eq!(combine_image_package_checksum(Some(true), None, true), None);
        assert_eq!(combine_image_package_checksum(None, Some(true), true), None);
    }

    #[test]
    fn update_progress_and_apply_errors_cover_non_download_paths() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_PLATFORM_TYPE", "desktop");
        std::env::set_var("RHYTHM_PLATFORM_CONTEXT", "server");

        let staged = UpdateProgress::stage(OtaUpdateStage::Staging, "Staging bundle");
        assert_eq!(staged.stage, OtaUpdateStage::Staging);
        assert_eq!(staged.message, "Staging bundle");
        assert_eq!(staged.downloaded_bytes, None);
        assert_eq!(staged.total_bytes, None);

        let downloading = UpdateProgress::downloading("Downloading bundle", 42, Some(128));
        assert_eq!(downloading.stage, OtaUpdateStage::Downloading);
        assert_eq!(downloading.message, "Downloading bundle");
        assert_eq!(downloading.downloaded_bytes, Some(42));
        assert_eq!(downloading.total_bytes, Some(128));

        let mut info = no_update_info("1.0.0");
        assert_eq!(
            string_error(info.apply_blocking()),
            "No download URL for this platform"
        );

        info.download_url = Some("https://example.invalid/rhythm.tar.gz".to_string());
        assert_eq!(
            string_error(info.apply_blocking_with_progress(|_| {})),
            "No release asset for this platform"
        );

        let _apply_guard = APPLY_LOCK.lock().unwrap();
        assert_eq!(
            string_error(info.apply_blocking()),
            "Update already in progress"
        );
        drop(_apply_guard);

        std::env::remove_var("RHYTHM_PLATFORM_TYPE");
        std::env::remove_var("RHYTHM_PLATFORM_CONTEXT");
    }

    #[test]
    fn stream_image_artifact_to_device_uses_inactive_slot_without_data_staging() {
        let dir = unique_test_dir("rootfs-stream");
        let target = dir.join("rootfs-slot.img");
        let rootfs = b"fake-rootfs".repeat(4096);

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&rootfs).unwrap();
        let compressed = encoder.finish().unwrap();
        let expected_sha256 = hex_string(&Sha256::digest(&compressed));
        let url = spawn_download_fixture(compressed.clone());
        let progress = Arc::new(Mutex::new(Vec::<(u64, Option<u64>)>::new()));

        File::create(&target).unwrap();
        let verified = stream_image_artifact_to_device(
            &test_client(),
            &url,
            &target,
            true,
            Some(&expected_sha256),
            {
                let progress = progress.clone();
                move |downloaded, total| progress.lock().unwrap().push((downloaded, total))
            },
        )
        .unwrap();

        assert_eq!(verified, Some(true));
        assert_eq!(fs::read(&target).unwrap(), rootfs);
        assert!(
            !dir.join("rootfs.ext2.gz.download").exists(),
            "rootfs images must stream to the inactive slot, not consume /data staging space"
        );
        assert!(progress
            .lock()
            .unwrap()
            .last()
            .is_some_and(|(downloaded, total)| *downloaded == compressed.len() as u64
                && *total == Some(compressed.len() as u64)));

        let mismatch_url = spawn_download_fixture(compressed);
        let error = stream_image_artifact_to_device(
            &test_client(),
            &mismatch_url,
            &target,
            true,
            Some("deadbeef"),
            |_, _| {},
        )
        .unwrap_err();
        assert!(error.starts_with("SHA256 mismatch for update image:"));

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
                version: None,
                sha256: None,
                size: None,
                kind: Some(ReleaseArtifactKind::DiskImage),
                compression: None,
                install: Vec::new(),
            },
            &install_root,
            "1.0.0",
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
                version: None,
                sha256: None,
                size: None,
                kind: Some(ReleaseArtifactKind::ArchiveBundle),
                compression: None,
                install: Vec::new(),
            },
            &install_root,
            "1.0.0",
        )
        .unwrap_err();

        assert_eq!(
            error,
            "Manifest package must reference a .tar.gz archive bundle"
        );
    }

    #[test]
    fn url_and_artifact_helpers_resolve_relative_assets_and_errors() {
        assert_eq!(
            resolve_download_url(
                "https://updates.example/releases/linux/manifest.json",
                "rhythm-server.tar.gz"
            )
            .unwrap(),
            "https://updates.example/releases/linux/rhythm-server.tar.gz"
        );
        assert_eq!(
            resolve_download_url(
                "https://updates.example/releases/linux/manifest.json",
                "https://cdn.example/rhythm-server.tar.gz"
            )
            .unwrap(),
            "https://cdn.example/rhythm-server.tar.gz"
        );
        assert!(
            resolve_download_url("not a valid manifest URL", "rhythm-server.tar.gz")
                .unwrap_err()
                .starts_with("Invalid manifest URL:")
        );

        assert_eq!(
            asset_name_from_url("https://cdn.example/releases/rhythm-server.tar.gz?token=redacted")
                .unwrap(),
            "rhythm-server.tar.gz"
        );
        assert_eq!(
            asset_name_from_url("https://cdn.example/releases/").unwrap_err(),
            "Update URL did not contain a filename"
        );

        let install_root = PathBuf::from("/tmp/rhythm-server");
        let package = resolve_package_artifact(
            "https://updates.example/releases/linux/manifest.json",
            &ManifestArtifact {
                name: String::new(),
                url: "rhythm-server.tar.gz".to_string(),
                version: None,
                sha256: Some("abc123".to_string()),
                size: Some(2048),
                kind: Some(ReleaseArtifactKind::ArchiveBundle),
                compression: None,
                install: Vec::new(),
            },
            &install_root,
            "1.2.3",
        )
        .unwrap();
        assert_eq!(package.version, "1.2.3");
        assert_eq!(package.asset_name, "rhythm-server.tar.gz");
        assert_eq!(
            package.download_url,
            "https://updates.example/releases/linux/rhythm-server.tar.gz"
        );
        assert_eq!(package.expected_sha256.as_deref(), Some("abc123"));
        assert_eq!(
            install_targets_to_summaries(&package.install_targets),
            vec![
                UpdateTargetSummary {
                    archive_path: "rhythm-server".to_string(),
                    destination: "/tmp/rhythm-server".to_string(),
                    required: true,
                },
                UpdateTargetSummary {
                    archive_path: "rhythm-chipd".to_string(),
                    destination: "/tmp/rhythm-chipd".to_string(),
                    required: true,
                },
                UpdateTargetSummary {
                    archive_path: "rhythm-cli".to_string(),
                    destination: "/tmp/rhythm-cli".to_string(),
                    required: false,
                },
            ]
        );

        let rootfs = resolve_manifest_image(
            "https://updates.example/releases/linux/manifest.json",
            &ManifestArtifact {
                name: String::new(),
                url: "rootfs.ext2.gz".to_string(),
                version: None,
                sha256: Some("def456".to_string()),
                size: Some(4096),
                kind: None,
                compression: Some("gzip".to_string()),
                install: Vec::new(),
            },
            "2.0.0",
        )
        .unwrap();
        assert_eq!(rootfs.name, "rootfs.ext2.gz");
        assert_eq!(rootfs.kind, ReleaseArtifactKind::RootfsImage);
        assert_eq!(rootfs.version.as_deref(), Some("2.0.0"));
        assert_eq!(rootfs.size, Some(4096));
        assert_eq!(rootfs.compression.as_deref(), Some("gzip"));

        let disk = resolve_manifest_image(
            "https://updates.example/releases/linux/manifest.json",
            &ManifestArtifact {
                name: "appliance.img".to_string(),
                url: "appliance.img".to_string(),
                version: None,
                sha256: None,
                size: None,
                kind: None,
                compression: None,
                install: Vec::new(),
            },
            "2.0.0",
        )
        .unwrap();
        assert_eq!(disk.kind, ReleaseArtifactKind::DiskImage);
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
    fn resolve_manifest_install_targets_infers_archive_paths_and_validates_slots() {
        let root = PathBuf::from("/tmp/rhythm/rhythm-server");
        let targets = resolve_manifest_install_targets(
            &[
                ManifestInstallTarget {
                    archive_path: None,
                    slot: InstallSlot::Current,
                    path: None,
                    required: true,
                },
                ManifestInstallTarget {
                    archive_path: None,
                    slot: InstallSlot::Sibling,
                    path: Some("rhythm-chipd".to_string()),
                    required: false,
                },
                ManifestInstallTarget {
                    archive_path: None,
                    slot: InstallSlot::Absolute,
                    path: Some("/usr/local/bin/rhythm-cli".to_string()),
                    required: true,
                },
            ],
            &root,
        )
        .unwrap();

        assert_eq!(targets[0].destination, root);
        assert_eq!(targets[0].archive_path, "rhythm-server");
        assert!(targets[0].required);

        assert_eq!(
            targets[1].destination,
            PathBuf::from("/tmp/rhythm/rhythm-chipd")
        );
        assert_eq!(targets[1].archive_path, "rhythm-chipd");
        assert!(!targets[1].required);

        assert_eq!(
            targets[2].destination,
            PathBuf::from("/usr/local/bin/rhythm-cli")
        );
        assert_eq!(targets[2].archive_path, "rhythm-cli");
        assert!(targets[2].required);

        let sibling_error = resolve_manifest_install_targets(
            &[ManifestInstallTarget {
                archive_path: None,
                slot: InstallSlot::Sibling,
                path: None,
                required: true,
            }],
            &PathBuf::from("/tmp/rhythm-server"),
        )
        .unwrap_err();
        assert_eq!(
            sibling_error,
            "Install target with slot=sibling requires a path"
        );

        let absolute_error = resolve_manifest_install_targets(
            &[ManifestInstallTarget {
                archive_path: None,
                slot: InstallSlot::Absolute,
                path: None,
                required: true,
            }],
            &PathBuf::from("/tmp/rhythm-server"),
        )
        .unwrap_err();
        assert_eq!(
            absolute_error,
            "Install target with slot=absolute requires a path"
        );

        let archive_path_error = resolve_manifest_install_targets(
            &[ManifestInstallTarget {
                archive_path: None,
                slot: InstallSlot::Absolute,
                path: Some("/".to_string()),
                required: true,
            }],
            &PathBuf::from("/tmp/rhythm-server"),
        )
        .unwrap_err();
        assert_eq!(
            archive_path_error,
            "Cannot infer archive path for install target /"
        );
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
    fn sha256_file_hash_and_binary_staging_paths_are_stable() {
        let dir = unique_test_dir("binary-stage");
        let download = dir.join("rhythm-server.download");
        let destination = dir.join("rhythm-server");
        fs::write(&download, b"downloaded-server").unwrap();

        let hash = compute_sha256_hex(&download).unwrap();
        assert_eq!(hash, hex_string(&Sha256::digest(b"downloaded-server")));

        let staged = stage_install_targets(
            &download,
            "rhythm-server",
            &[InstallTarget {
                archive_path: "rhythm-server".to_string(),
                destination: destination.clone(),
                required: true,
            }],
        )
        .unwrap();
        assert_eq!(
            fs::read(destination.with_extension("new")).unwrap(),
            b"downloaded-server"
        );
        cleanup_staged_files(&staged);

        let err = stage_install_targets(
            &download,
            "rhythm-server",
            &[
                InstallTarget {
                    archive_path: "rhythm-server".to_string(),
                    destination: destination.clone(),
                    required: true,
                },
                InstallTarget {
                    archive_path: "rhythm-chipd".to_string(),
                    destination: dir.join("rhythm-chipd"),
                    required: true,
                },
            ],
        )
        .unwrap_err();
        assert_eq!(
            err,
            "Binary payload rhythm-server cannot satisfy multi-file install plan"
        );

        let missing_hash = compute_sha256_hex(&dir.join("missing")).unwrap_err();
        assert!(missing_hash.starts_with("Failed to open file for hashing:"));
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
    fn commit_staged_targets_keeps_backups_for_caller_to_resolve() {
        let dir = unique_test_dir("commit-cleanup");
        let destination = dir.join("rhythm-server");
        let stage_path = dir.join("rhythm-server.new");
        let backup_path = dir.join("rhythm-server.old");

        fs::write(&destination, b"old-server").unwrap();
        fs::write(&stage_path, b"new-server").unwrap();

        let outcome = commit_staged_targets(&[StagedInstallTarget {
            spec: InstallTarget {
                archive_path: "rhythm-server".to_string(),
                destination: destination.clone(),
                required: true,
            },
            stage_path: stage_path.clone(),
            backup_path: backup_path.clone(),
        }])
        .unwrap();

        assert_eq!(outcome.installed_targets, vec!["rhythm-server".to_string()]);
        assert_eq!(fs::read(&destination).unwrap(), b"new-server");
        assert!(!stage_path.exists(), "stage file should be promoted");
        assert!(
            backup_path.exists(),
            "backup must survive commit so a failed startup can roll back"
        );
        assert_eq!(fs::read(&backup_path).unwrap(), b"old-server");

        cleanup_backup_files(&outcome.applied);
        assert!(
            !backup_path.exists(),
            "explicit cleanup removes the backup once the caller decides to"
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn commit_staged_targets_skips_missing_stage_and_installs_new_target() {
        let dir = unique_test_dir("commit-new-target");
        let missing_stage = dir.join("missing.new");
        let destination = dir.join("rhythm-server");
        let stage_path = destination.with_extension("new");
        fs::write(&stage_path, b"new-server").unwrap();

        let outcome = commit_staged_targets(&[
            StagedInstallTarget {
                spec: InstallTarget {
                    archive_path: "missing".to_string(),
                    destination: dir.join("missing"),
                    required: false,
                },
                stage_path: missing_stage,
                backup_path: dir.join("missing.old"),
            },
            StagedInstallTarget {
                spec: InstallTarget {
                    archive_path: "rhythm-server".to_string(),
                    destination: destination.clone(),
                    required: true,
                },
                stage_path,
                backup_path: destination.with_extension("old"),
            },
        ])
        .unwrap();

        assert_eq!(outcome.installed_targets, vec!["rhythm-server".to_string()]);
        assert_eq!(fs::read(&destination).unwrap(), b"new-server");
        assert!(!destination.with_extension("old").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn pending_update_marker_rolls_back_after_repeated_failed_starts() {
        let dir = unique_test_dir("pending-rollback");
        let marker_path = dir.join(PENDING_UPDATE_MARKER_FILE);
        let destination = dir.join("rhythm-server");
        let backup_path = dir.join("rhythm-server.old");
        let added_file = dir.join("rhythm-cli");
        fs::write(&destination, b"new-server").unwrap();
        fs::write(&backup_path, b"old-server").unwrap();
        fs::write(&added_file, b"new-cli").unwrap();

        write_pending_update_marker_at(
            &marker_path,
            &[
                AppliedInstallTarget {
                    destination: destination.clone(),
                    backup_path: backup_path.clone(),
                    previously_existed: true,
                },
                AppliedInstallTarget {
                    destination: added_file.clone(),
                    backup_path: added_file.with_extension("old"),
                    previously_existed: false,
                },
            ],
            Some(LiveInstallVersions {
                previous: "0.4.263-beta",
                target: "0.4.264-beta",
            }),
        )
        .unwrap();

        // The new build gets MAX_PENDING_START_ATTEMPTS starts on probation.
        for attempt in 1..=MAX_PENDING_START_ATTEMPTS {
            assert_eq!(
                startup_update_health_check_at(&marker_path),
                StartupUpdateDisposition::PendingVerification { attempt }
            );
            assert_eq!(fs::read(&destination).unwrap(), b"new-server");
        }

        // One more failed start restores the previous build.
        let disposition = startup_update_health_check_at(&marker_path);
        let StartupUpdateDisposition::RolledBack { restored, .. } = disposition else {
            panic!("expected rollback, got {:?}", disposition);
        };
        assert_eq!(restored.len(), 2);
        assert_eq!(
            fs::read(&destination).unwrap(),
            b"old-server",
            "previous binary must be restored"
        );
        assert!(!backup_path.exists(), "backup is consumed by the restore");
        assert!(
            !added_file.exists(),
            "files the update introduced are removed on rollback"
        );
        assert!(!marker_path.exists(), "marker is cleared after rollback");

        assert_eq!(
            startup_update_health_check_at(&marker_path),
            StartupUpdateDisposition::NoPendingUpdate,
            "the restored build must boot without a pending marker"
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn mark_update_verified_clears_marker_and_backups() {
        let dir = unique_test_dir("pending-verified");
        let marker_path = dir.join(PENDING_UPDATE_MARKER_FILE);
        let destination = dir.join("rhythm-server");
        let backup_path = dir.join("rhythm-server.old");
        fs::write(&destination, b"new-server").unwrap();
        fs::write(&backup_path, b"old-server").unwrap();

        write_pending_update_marker_at(
            &marker_path,
            &[AppliedInstallTarget {
                destination: destination.clone(),
                backup_path: backup_path.clone(),
                previously_existed: true,
            }],
            Some(LiveInstallVersions {
                previous: "0.4.263-beta",
                target: "0.4.264-beta",
            }),
        )
        .unwrap();

        // One probationary start, then the build proves healthy.
        assert_eq!(
            startup_update_health_check_at(&marker_path),
            StartupUpdateDisposition::PendingVerification { attempt: 1 }
        );
        assert!(mark_update_verified_at(&marker_path));

        assert!(!marker_path.exists());
        assert!(!backup_path.exists(), "backups are discarded once verified");
        assert_eq!(
            fs::read(&destination).unwrap(),
            b"new-server",
            "the verified build stays installed"
        );
        assert!(
            !mark_update_verified_at(&marker_path),
            "verification is idempotent once the marker is gone"
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn bundle_rollback_is_recorded_and_reported_after_failed_verification() {
        let dir = unique_test_dir("rollback-record");
        let marker_path = dir.join(PENDING_UPDATE_MARKER_FILE);
        let record_path = bundle_rollback_record_path_for_marker(&marker_path);
        let destination = dir.join("rhythm-server");
        let backup_path = dir.join("rhythm-server.old");
        fs::write(&destination, b"new-server").unwrap();
        fs::write(&backup_path, b"old-server").unwrap();

        write_pending_update_marker_at(
            &marker_path,
            &[AppliedInstallTarget {
                destination: destination.clone(),
                backup_path: backup_path.clone(),
                previously_existed: true,
            }],
            Some(LiveInstallVersions {
                previous: "0.4.263-beta",
                target: "0.4.264-beta",
            }),
        )
        .unwrap();

        for _ in 0..=MAX_PENDING_START_ATTEMPTS {
            let _ = startup_update_health_check_at(&marker_path);
        }

        let rollback = load_bundle_rollback_at(&record_path).expect("rollback recorded");
        assert_eq!(rollback.version, "0.4.264-beta");
        assert_eq!(rollback.from_version.as_deref(), Some("0.4.263-beta"));
        assert_eq!(rollback.kind, RollbackKind::ComponentBundle);
        assert!(rollback.at_epoch_ms.is_some());

        // A later update that verifies clears the stale rollback record.
        fs::write(&backup_path, b"old-server").unwrap();
        write_pending_update_marker_at(
            &marker_path,
            &[AppliedInstallTarget {
                destination,
                backup_path,
                previously_existed: true,
            }],
            Some(LiveInstallVersions {
                previous: "0.4.263-beta",
                target: "0.4.265-beta",
            }),
        )
        .unwrap();
        assert!(mark_update_verified_at(&marker_path));
        assert!(
            load_bundle_rollback_at(&record_path).is_none(),
            "a verified later update clears the rollback record"
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn bundle_rollback_record_without_target_version_is_not_reported() {
        let dir = unique_test_dir("rollback-record-anon");
        let marker_path = dir.join(PENDING_UPDATE_MARKER_FILE);
        let record_path = bundle_rollback_record_path_for_marker(&marker_path);
        let destination = dir.join("rhythm-server");
        fs::write(&destination, b"new-server").unwrap();

        // Marker written without version context (e.g. by an older build).
        write_pending_update_marker_at(
            &marker_path,
            &[AppliedInstallTarget {
                destination,
                backup_path: dir.join("rhythm-server.old"),
                previously_existed: false,
            }],
            None,
        )
        .unwrap();
        for _ in 0..=MAX_PENDING_START_ATTEMPTS {
            let _ = startup_update_health_check_at(&marker_path);
        }

        assert!(
            load_bundle_rollback_at(&record_path).is_none(),
            "an anonymous rollback can't be attributed to a version"
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn pending_update_marker_without_version_fields_still_parses() {
        // Markers written by builds predating the version fields must load.
        let raw = r#"{
            "start_attempts": 1,
            "targets": [{
                "destination": "/usr/bin/rhythm-server",
                "backup": "/usr/bin/rhythm-server.old",
                "previously_existed": true
            }]
        }"#;
        let marker: PendingUpdateMarker = serde_json::from_str(raw).unwrap();
        assert_eq!(marker.start_attempts, 1);
        assert_eq!(marker.previous_version, None);
        assert_eq!(marker.target_version, None);
    }

    #[test]
    fn appliance_rollback_parses_version_epoch_and_source_from_bootstate_body() {
        let body = concat!(
            "RHYTHM_ACTIVE_SLOT=a\n",
            "RHYTHM_LAST_GOOD_SLOT=a\n",
            "RHYTHM_PENDING_SLOT=\n",
            "RHYTHM_PENDING_VERSION=\n",
            "RHYTHM_ACTIVE_VERSION=0.4.263-beta\n",
            "RHYTHM_BOOT_STATUS=idle\n",
            "RHYTHM_LAST_UPDATE_EPOCH_MS=1770000000000\n",
            "RHYTHM_LAST_ROLLBACK_SLOT=b\n",
            "RHYTHM_LAST_ROLLBACK_VERSION=0.4.264-beta\n",
            "RHYTHM_LAST_ROLLBACK_EPOCH_MS=1770000123456\n",
        );

        let rollback = appliance_rollback_from_bootstate_body(body).unwrap();
        assert_eq!(rollback.version, "0.4.264-beta");
        assert_eq!(rollback.from_version.as_deref(), Some("0.4.263-beta"));
        assert_eq!(rollback.at_epoch_ms, Some(1_770_000_123_456));
        assert_eq!(rollback.kind, RollbackKind::ImageSlot);

        let idle_body = concat!(
            "RHYTHM_ACTIVE_SLOT=a\n",
            "RHYTHM_LAST_ROLLBACK_VERSION=\n",
            "RHYTHM_LAST_ROLLBACK_EPOCH_MS=\n",
        );
        assert!(appliance_rollback_from_bootstate_body(idle_body).is_none());
    }

    #[test]
    fn merge_rollbacks_prefers_the_most_recent_record() {
        let bundle = LastRollback {
            version: "0.4.264-beta".to_string(),
            from_version: None,
            at_epoch_ms: Some(200),
            kind: RollbackKind::ComponentBundle,
        };
        let image = LastRollback {
            version: "0.4.262-beta".to_string(),
            from_version: None,
            at_epoch_ms: Some(100),
            kind: RollbackKind::ImageSlot,
        };

        assert_eq!(
            merge_rollbacks(Some(bundle.clone()), Some(image.clone())),
            Some(bundle.clone())
        );
        assert_eq!(
            merge_rollbacks(Some(image.clone()), Some(bundle.clone())),
            Some(bundle.clone())
        );
        assert_eq!(merge_rollbacks(None, Some(image.clone())), Some(image));
        assert_eq!(merge_rollbacks(Some(bundle.clone()), None), Some(bundle));
        assert_eq!(merge_rollbacks(None, None), None);
    }

    #[test]
    fn last_rollback_serializes_snake_case_for_the_api() {
        let rollback = LastRollback {
            version: "0.4.264-beta".to_string(),
            from_version: Some("0.4.263-beta".to_string()),
            at_epoch_ms: Some(1_770_000_123_456),
            kind: RollbackKind::ComponentBundle,
        };
        let json = serde_json::to_value(&rollback).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "version": "0.4.264-beta",
                "from_version": "0.4.263-beta",
                "at_epoch_ms": 1_770_000_123_456_i64,
                "kind": "component_bundle",
            })
        );
    }

    #[test]
    fn startup_update_health_check_discards_corrupt_marker() {
        let dir = unique_test_dir("pending-corrupt");
        let marker_path = dir.join(PENDING_UPDATE_MARKER_FILE);
        fs::write(&marker_path, b"{not json").unwrap();

        assert_eq!(
            startup_update_health_check_at(&marker_path),
            StartupUpdateDisposition::NoPendingUpdate
        );
        assert!(
            !marker_path.exists(),
            "a corrupt marker must not wedge startup forever"
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn startup_update_health_check_without_marker_is_noop() {
        let dir = unique_test_dir("pending-absent");
        let marker_path = dir.join(PENDING_UPDATE_MARKER_FILE);

        assert_eq!(
            startup_update_health_check_at(&marker_path),
            StartupUpdateDisposition::NoPendingUpdate
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn pending_update_rollback_survives_missing_backup() {
        let dir = unique_test_dir("pending-missing-backup");
        let marker_path = dir.join(PENDING_UPDATE_MARKER_FILE);
        let destination = dir.join("rhythm-server");
        fs::write(&destination, b"new-server").unwrap();

        // Backup never written (e.g. deleted out-of-band).
        write_pending_update_marker_at(
            &marker_path,
            &[AppliedInstallTarget {
                destination: destination.clone(),
                backup_path: dir.join("rhythm-server.old"),
                previously_existed: true,
            }],
            None,
        )
        .unwrap();

        for _ in 0..MAX_PENDING_START_ATTEMPTS {
            let _ = startup_update_health_check_at(&marker_path);
        }
        let disposition = startup_update_health_check_at(&marker_path);
        let StartupUpdateDisposition::RolledBack { restored, .. } = disposition else {
            panic!("expected rollback, got {:?}", disposition);
        };

        assert!(restored.is_empty(), "nothing restorable without a backup");
        assert_eq!(
            fs::read(&destination).unwrap(),
            b"new-server",
            "without a backup the installed binary must be left alone"
        );
        assert!(!marker_path.exists());

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
    fn ota_status_handle_tracks_check_update_restart_and_error() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_PLATFORM_TYPE", "desktop");
        std::env::set_var("RHYTHM_PLATFORM_CONTEXT", "server");

        let handle = OtaStatusHandle::new("1.0.0");
        let capabilities = handle.capabilities();
        assert_eq!(capabilities.strategy, "self_pull");
        assert_eq!(capabilities.scope, "component_bundle");
        assert_eq!(capabilities.rollback, "backup_files");
        assert_eq!(capabilities.payloads, vec!["archive_bundle"]);

        handle.mark_checking();
        let checking = handle.snapshot();
        assert_eq!(checking.state, OtaUpdateState::Checking);
        assert_eq!(checking.message.as_deref(), Some("Checking for updates..."));
        assert!(checking.checked_at_epoch_ms.is_some());
        assert_eq!(checking.target_version, None);
        assert_eq!(checking.update_reason, None);
        assert!(checking.install_targets.is_empty());
        assert!(checking.image_assets.is_empty());
        assert_eq!(checking.last_error, None);

        let update = UpdateInfo {
            current_version: "1.0.0".to_string(),
            latest_version: "1.1.0".to_string(),
            current_package_version: "1.0.0".to_string(),
            latest_package_version: "1.1.0".to_string(),
            current_image_version: Some("1.0.0".to_string()),
            latest_image_version: Some("1.1.0".to_string()),
            update_available: true,
            update_reason: Some(UpdateReason::VersionMismatch),
            download_url: Some("https://example.invalid/rhythm.tar.gz".to_string()),
            expected_sha256: Some("abc123".to_string()),
            asset_name: Some("rhythm.tar.gz".to_string()),
            install_targets: vec![UpdateTargetSummary {
                archive_path: "rhythm-server".to_string(),
                destination: "/opt/rhythm/rhythm-server".to_string(),
                required: true,
            }],
            image_assets: vec![UpdateImageAsset {
                name: "rootfs.ext2.gz".to_string(),
                kind: ReleaseArtifactKind::RootfsImage,
                url: "https://example.invalid/rootfs.ext2.gz".to_string(),
                version: Some("1.1.0".to_string()),
                sha256: Some("def456".to_string()),
                size: Some(128),
                compression: Some("gzip".to_string()),
            }],
            resolved_install_targets: Vec::new(),
        };

        handle.record_check_result(&update);
        let ready = handle.snapshot();
        assert_eq!(ready.state, OtaUpdateState::Ready);
        assert_eq!(ready.latest_version.as_deref(), Some("1.1.0"));
        assert_eq!(ready.current_package_version.as_deref(), Some("1.0.0"));
        assert_eq!(ready.latest_package_version.as_deref(), Some("1.1.0"));
        assert_eq!(ready.current_image_version.as_deref(), Some("1.0.0"));
        assert_eq!(ready.latest_image_version.as_deref(), Some("1.1.0"));
        assert_eq!(ready.update_available, Some(true));
        assert_eq!(ready.update_reason, Some(UpdateReason::VersionMismatch));
        assert_eq!(ready.message.as_deref(), Some("Update available: v1.1.0"));
        assert_eq!(ready.install_targets, update.install_targets);
        assert_eq!(ready.image_assets, update.image_assets);
        assert_eq!(ready.checksum_verified, None);

        handle.begin_update("1.1.0").unwrap();
        let updating = handle.snapshot();
        assert_eq!(updating.state, OtaUpdateState::Updating);
        assert_eq!(updating.target_version.as_deref(), Some("1.1.0"));
        assert_eq!(updating.message.as_deref(), Some("Installing v1.1.0..."));
        assert_eq!(
            handle.begin_update("1.1.0").unwrap_err(),
            "Update already in progress"
        );

        handle.mark_restarting("1.0.0", "1.1.0", Some(true));
        let restarting = handle.snapshot();
        assert_eq!(restarting.state, OtaUpdateState::Restarting);
        assert_eq!(restarting.latest_version.as_deref(), Some("1.1.0"));
        assert_eq!(restarting.target_version.as_deref(), Some("1.1.0"));
        assert_eq!(restarting.update_available, Some(false));
        assert_eq!(restarting.checksum_verified, Some(true));
        assert_eq!(
            restarting.message.as_deref(),
            Some("Updated from v1.0.0 to v1.1.0, restarting...")
        );

        let mut drift = no_update_info("1.1.0");
        drift.update_available = true;
        drift.update_reason = Some(UpdateReason::ComponentDrift);
        handle.record_check_result(&drift);
        let repairing = handle.snapshot();
        assert_eq!(repairing.state, OtaUpdateState::Ready);
        assert_eq!(
            repairing.message.as_deref(),
            Some("Repairing OTA bundle for v1.1.0")
        );

        handle.record_check_result(&no_update_info("1.1.0"));
        let idle = handle.snapshot();
        assert_eq!(idle.state, OtaUpdateState::Idle);
        assert_eq!(idle.update_available, Some(false));
        assert_eq!(idle.update_reason, None);
        assert_eq!(idle.message.as_deref(), Some("Already up to date"));

        handle.mark_error("boom");
        let error = handle.snapshot();
        assert_eq!(error.state, OtaUpdateState::Error);
        assert_eq!(error.message.as_deref(), Some("Update failed"));
        assert_eq!(error.last_error.as_deref(), Some("boom"));
        assert_eq!(error.checksum_verified, None);

        std::env::remove_var("RHYTHM_PLATFORM_TYPE");
        std::env::remove_var("RHYTHM_PLATFORM_CONTEXT");
    }

    #[test]
    fn ota_status_handle_reports_poisoned_lock_as_error_snapshot() {
        let handle = OtaStatusHandle::new("1.0.0");
        let inner = handle.inner.clone();
        let _ = thread::spawn(move || {
            let _guard = inner.lock().unwrap();
            panic!("poison OTA status lock for test");
        })
        .join();

        let snapshot = handle.snapshot();
        assert_eq!(snapshot.state, OtaUpdateState::Error);
        assert_eq!(snapshot.current_version, "unknown");
        assert_eq!(
            snapshot.last_error.as_deref(),
            Some("OTA state lock poisoned")
        );
        assert_eq!(
            string_error(handle.begin_update("1.1.0")),
            "OTA state lock poisoned"
        );
        handle.mark_checking();
        assert_eq!(handle.snapshot().state, OtaUpdateState::Error);
    }

    #[test]
    fn acquire_apply_lock_reports_contention_without_blocking() {
        let _guard = APPLY_LOCK.lock().unwrap();

        assert_eq!(
            string_error(acquire_apply_lock()),
            "Update already in progress"
        );
    }

    #[test]
    fn update_channel_progress_and_no_update_helpers_cover_default_shapes() {
        assert_eq!(UpdateChannel::Beta.feed_suffix(), None);
        assert_eq!(UpdateChannel::Stable.feed_suffix(), Some("-stable"));
        assert!(default_required());

        assert_eq!(ApplianceSlot::A.as_str(), "a");
        assert_eq!(ApplianceSlot::B.as_str(), "b");
        assert_eq!(ApplianceSlot::A.root_device(), APPLIANCE_ROOTFS_A_DEVICE);
        assert_eq!(ApplianceSlot::B.root_device(), APPLIANCE_ROOTFS_B_DEVICE);
        assert_eq!(ApplianceSlot::A.inactive(), ApplianceSlot::B);
        assert_eq!(ApplianceSlot::B.inactive(), ApplianceSlot::A);

        let staged = UpdateProgress::stage(OtaUpdateStage::Staging, "stage message");
        assert_eq!(staged.stage, OtaUpdateStage::Staging);
        assert_eq!(staged.message, "stage message");
        assert_eq!(staged.downloaded_bytes, None);
        assert_eq!(staged.total_bytes, None);

        let downloading = UpdateProgress::downloading("download", 17, Some(128));
        assert_eq!(downloading.stage, OtaUpdateStage::Downloading);
        assert_eq!(downloading.message, "download");
        assert_eq!(downloading.downloaded_bytes, Some(17));
        assert_eq!(downloading.total_bytes, Some(128));

        let info = no_update_info("1.2.3");
        assert_eq!(info.current_version, "1.2.3");
        assert_eq!(info.latest_version, "1.2.3");
        assert!(!info.update_available);
        assert_eq!(info.update_reason, None);
        assert_eq!(info.download_url, None);
        assert!(info.install_targets.is_empty());
        assert!(info.image_assets.is_empty());
        assert!(info.resolved_install_targets.is_empty());
        assert_eq!(info.current_package_version, "1.2.3");
        assert_eq!(info.latest_package_version, "1.2.3");
    }

    #[test]
    fn install_artifact_cleanup_helpers_remove_staged_backup_and_rollback_files() {
        let dir = unique_test_dir("cleanup-helpers");
        let destination = dir.join("rhythm-server");
        let stage_path = destination.with_extension("new");
        let backup_path = destination.with_extension("old");
        fs::write(&destination, b"installed").unwrap();
        fs::write(&stage_path, b"staged").unwrap();
        fs::write(&backup_path, b"backup").unwrap();

        let install_target = InstallTarget {
            archive_path: "rhythm-server".to_string(),
            destination: destination.clone(),
            required: true,
        };
        cleanup_install_artifacts(std::slice::from_ref(&install_target));
        assert!(!stage_path.exists());
        assert!(backup_path.exists());

        fs::write(&stage_path, b"staged").unwrap();
        let staged = StagedInstallTarget {
            spec: install_target.clone(),
            stage_path: stage_path.clone(),
            backup_path: backup_path.clone(),
        };
        cleanup_staged_files(std::slice::from_ref(&staged));
        assert!(!stage_path.exists());

        rollback_applied_targets(&[AppliedInstallTarget {
            destination: destination.clone(),
            backup_path: backup_path.clone(),
            previously_existed: true,
        }]);
        assert_eq!(fs::read(&destination).unwrap(), b"backup");
        assert!(!backup_path.exists());

        fs::write(&destination, b"fresh").unwrap();
        rollback_applied_targets(&[AppliedInstallTarget {
            destination: destination.clone(),
            backup_path: backup_path.clone(),
            previously_existed: false,
        }]);
        assert!(!destination.exists());

        fs::write(&backup_path, b"old").unwrap();
        cleanup_backup_files(&[AppliedInstallTarget {
            destination: destination.clone(),
            backup_path: backup_path.clone(),
            previously_existed: true,
        }]);
        assert!(!backup_path.exists());

        remove_if_exists(&backup_path);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn download_release_reports_http_status_errors_without_promoting_tmp_file() {
        let dir = unique_test_dir("download-status-error");
        let destination = dir.join("artifact.tar.gz");
        let error = string_error(download_release(
            &test_client(),
            &spawn_status_fixture(500),
            &destination,
        ));

        assert_eq!(error, "Download returned 500 Internal Server Error");
        assert!(!destination.exists());
        assert!(!bootstate::tmp_sibling_path(&destination).exists());

        let _ = fs::remove_dir_all(dir);
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
    fn external_reboot_args_are_empty_for_graceful_and_forced_for_fallback() {
        assert!(external_reboot_args(false).is_empty());
        assert_eq!(external_reboot_args(true), &["-f"]);
    }

    #[test]
    fn restart_with_best_effort_persist_arms_restart_before_persist_worker() {
        let state: rhythm_os::state::SharedState =
            Arc::new(Mutex::new(rhythm_os::state::AppState::default()));
        let calls = Arc::new(Mutex::new(Vec::new()));

        let schedule_calls = calls.clone();
        let persist_calls = calls.clone();
        schedule_restart_with_best_effort_persist(
            state,
            move || schedule_calls.lock().unwrap().push("schedule"),
            move |_| {
                persist_calls.lock().unwrap().push("spawn_persist");
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(
            *calls.lock().unwrap(),
            vec!["schedule", "spawn_persist"],
            "restart must be armed before best-effort persistence can block"
        );
    }

    #[test]
    fn restart_with_best_effort_persist_stays_armed_when_persist_spawn_fails() {
        let state: rhythm_os::state::SharedState =
            Arc::new(Mutex::new(rhythm_os::state::AppState::default()));
        let calls = Arc::new(Mutex::new(Vec::new()));

        let schedule_calls = calls.clone();
        let result = schedule_restart_with_best_effort_persist(
            state,
            move || schedule_calls.lock().unwrap().push("schedule"),
            |_| Err(std::io::Error::other("spawn failed")),
        );

        assert!(result.is_err());
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["schedule"],
            "restart should remain armed even when persistence worker spawn fails"
        );
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
    fn channel_from_state_appliance_defaults_to_stable_regardless_of_auto_update() {
        let state: rhythm_os::state::SharedState =
            Arc::new(Mutex::new(rhythm_os::state::AppState::default()));
        state.lock().unwrap().platform_type = "appliance";
        assert_eq!(channel_from_state(&state), UpdateChannel::Stable);

        state.lock().unwrap().auto_update = false;
        assert_eq!(channel_from_state(&state), UpdateChannel::Stable);
    }

    #[test]
    fn channel_from_state_honors_explicit_beta_on_appliance() {
        let state: rhythm_os::state::SharedState =
            Arc::new(Mutex::new(rhythm_os::state::AppState::default()));
        {
            let mut s = state.lock().unwrap();
            s.platform_type = "appliance";
            s.update_channel = Some(UpdateChannel::Beta);
        }

        assert_eq!(channel_from_state(&state), UpdateChannel::Beta);
    }

    #[test]
    fn channel_from_state_keeps_desktop_default_on_beta_feed() {
        let state: rhythm_os::state::SharedState =
            Arc::new(Mutex::new(rhythm_os::state::AppState::default()));

        assert_eq!(channel_from_state(&state), UpdateChannel::Beta);
    }

    #[test]
    fn channel_from_state_honors_explicit_stable_on_desktop() {
        let state: rhythm_os::state::SharedState =
            Arc::new(Mutex::new(rhythm_os::state::AppState::default()));
        state.lock().unwrap().update_channel = Some(UpdateChannel::Stable);

        assert_eq!(channel_from_state(&state), UpdateChannel::Stable);
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
    fn stable_manifest_404_with_explicit_override_is_error() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_UPDATE_MANIFEST_URL", spawn_status_fixture(404));

        let error = string_error(check_blocking("1.2.3", UpdateChannel::Stable));

        std::env::remove_var("RHYTHM_UPDATE_MANIFEST_URL");

        assert_eq!(error, "Update manifest returned 404 Not Found");
    }

    #[test]
    fn check_manifest_blocking_resolves_package_images_and_update_reason() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_PLATFORM_TYPE", "desktop");
        std::env::set_var("RHYTHM_PLATFORM_CONTEXT", "server");
        let manifest_url = spawn_json_fixture(
            r#"{
                "version": "2.0.0",
                "package": {
                    "name": "",
                    "url": "release/rhythm-server.tar.gz",
                    "version": "2.0.0",
                    "sha256": "abc123",
                    "kind": "archive_bundle",
                    "install": [
                        {"slot": "self", "required": true},
                        {"slot": "sibling", "path": "rhythm-chipd", "required": false}
                    ]
                },
                "images": [
                    {
                        "name": "",
                        "url": "images/rootfs.ext2.gz",
                        "version": "2.0.0",
                        "kind": "rootfs_image",
                        "compression": "gzip",
                        "sha256": "def456",
                        "size": 4096
                    }
                ]
            }"#,
        );
        std::env::set_var("RHYTHM_UPDATE_MANIFEST_URL", manifest_url);

        let info = check_blocking("1.0.0", UpdateChannel::Beta).unwrap();

        std::env::remove_var("RHYTHM_UPDATE_MANIFEST_URL");
        std::env::remove_var("RHYTHM_PLATFORM_TYPE");
        std::env::remove_var("RHYTHM_PLATFORM_CONTEXT");

        assert_eq!(info.current_version, "1.0.0");
        assert_eq!(info.latest_version, "2.0.0");
        assert_eq!(info.current_package_version, "1.0.0");
        assert_eq!(info.latest_package_version, "2.0.0");
        assert_eq!(info.latest_image_version.as_deref(), Some("2.0.0"));
        assert!(info.update_available);
        assert_eq!(info.update_reason, Some(UpdateReason::VersionMismatch));
        assert_eq!(info.asset_name.as_deref(), Some("rhythm-server.tar.gz"));
        assert_eq!(info.expected_sha256.as_deref(), Some("abc123"));
        assert!(
            info.download_url
                .as_deref()
                .is_some_and(|url| url.ends_with("/feeds/release/rhythm-server.tar.gz")),
            "unexpected download URL: {:?}",
            info.download_url
        );
        assert_eq!(info.install_targets.len(), 2);
        assert_eq!(info.install_targets[0].archive_path, "rhythm-server");
        assert!(info.install_targets[0].required);
        assert_eq!(info.install_targets[1].archive_path, "rhythm-chipd");
        assert!(!info.install_targets[1].required);
        assert_eq!(info.image_assets.len(), 1);
        assert_eq!(info.image_assets[0].name, "rootfs.ext2.gz");
        assert_eq!(info.image_assets[0].kind, ReleaseArtifactKind::RootfsImage);
        assert_eq!(info.image_assets[0].compression.as_deref(), Some("gzip"));
        assert_eq!(info.resolved_install_targets.len(), 2);
    }

    #[test]
    fn check_manifest_blocking_reports_image_base_drift_when_marker_is_older() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = unique_test_dir("ota-image-marker-older");
        let marker = dir.join("rhythm-image-version");
        fs::write(&marker, "1.0.0\n").unwrap();
        std::env::set_var("RHYTHM_IMAGE_VERSION_PATH", &marker);
        std::env::set_var("RHYTHM_PLATFORM_TYPE", "appliance");
        std::env::set_var("RHYTHM_PLATFORM_CONTEXT", "server");
        let manifest_url = spawn_json_fixture(
            r#"{
                "version": "2.0.0",
                "package": {
                    "name": "rhythm-server-rpiz.tar.gz",
                    "url": "v2.0.0/rhythm-server-rpiz.tar.gz",
                    "version": "2.0.0",
                    "kind": "archive_bundle",
                    "install": [
                        {"slot": "self", "required": true}
                    ]
                },
                "images": [
                    {
                        "name": "rootfs.ext2.gz",
                        "url": "v2.0.0/rootfs.ext2.gz",
                        "version": "2.0.0",
                        "kind": "rootfs_image",
                        "compression": "gzip"
                    }
                ]
            }"#,
        );
        std::env::set_var("RHYTHM_UPDATE_MANIFEST_URL", manifest_url);

        let info = check_blocking("2.0.0", UpdateChannel::Stable).unwrap();

        std::env::remove_var("RHYTHM_UPDATE_MANIFEST_URL");
        std::env::remove_var("RHYTHM_IMAGE_VERSION_PATH");
        std::env::remove_var("RHYTHM_PLATFORM_TYPE");
        std::env::remove_var("RHYTHM_PLATFORM_CONTEXT");
        let _ = fs::remove_dir_all(dir);

        assert!(info.update_available);
        assert_eq!(info.update_reason, Some(UpdateReason::ImageBaseDrift));
        assert_eq!(info.current_package_version, "2.0.0");
        assert_eq!(info.latest_package_version, "2.0.0");
        assert_eq!(info.current_image_version.as_deref(), Some("1.0.0"));
        assert_eq!(info.latest_image_version.as_deref(), Some("2.0.0"));
        assert!(
            info.download_url.is_some(),
            "image-base drift with a package must overlay binaries into the inactive rootfs"
        );
    }

    #[test]
    fn check_manifest_blocking_reports_image_base_drift_when_marker_is_missing() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = unique_test_dir("ota-image-marker-missing");
        std::env::set_var("RHYTHM_IMAGE_VERSION_PATH", dir.join("missing-marker"));
        std::env::set_var("RHYTHM_PLATFORM_TYPE", "appliance");
        std::env::set_var("RHYTHM_PLATFORM_CONTEXT", "server");
        let manifest_url = spawn_json_fixture(
            r#"{
                "version": "2.0.0",
                "package": {
                    "name": "rhythm-server-rpiz.tar.gz",
                    "url": "v2.0.0/rhythm-server-rpiz.tar.gz",
                    "version": "2.0.0",
                    "kind": "archive_bundle",
                    "install": [
                        {"slot": "self", "required": true}
                    ]
                },
                "images": [
                    {
                        "name": "rootfs.ext2.gz",
                        "url": "v2.0.0/rootfs.ext2.gz",
                        "version": "2.0.0",
                        "kind": "rootfs_image",
                        "compression": "gzip"
                    }
                ]
            }"#,
        );
        std::env::set_var("RHYTHM_UPDATE_MANIFEST_URL", manifest_url);

        let info = check_blocking("2.0.0", UpdateChannel::Stable).unwrap();

        std::env::remove_var("RHYTHM_UPDATE_MANIFEST_URL");
        std::env::remove_var("RHYTHM_IMAGE_VERSION_PATH");
        std::env::remove_var("RHYTHM_PLATFORM_TYPE");
        std::env::remove_var("RHYTHM_PLATFORM_CONTEXT");
        let _ = fs::remove_dir_all(dir);

        assert!(info.update_available);
        assert_eq!(info.update_reason, Some(UpdateReason::ImageBaseDrift));
        assert_eq!(info.current_image_version, None);
        assert_eq!(info.latest_image_version.as_deref(), Some("2.0.0"));
    }

    #[test]
    fn check_manifest_blocking_is_up_to_date_when_package_and_image_versions_match() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = unique_test_dir("ota-image-marker-current");
        let marker = dir.join("rhythm-image-version");
        fs::write(&marker, "2.0.0\n").unwrap();
        std::env::set_var("RHYTHM_IMAGE_VERSION_PATH", &marker);
        std::env::set_var("RHYTHM_PLATFORM_TYPE", "appliance");
        std::env::set_var("RHYTHM_PLATFORM_CONTEXT", "server");
        let manifest_url = spawn_json_fixture(
            r#"{
                "version": "2.0.0",
                "package": {
                    "name": "rhythm-server-rpiz.tar.gz",
                    "url": "v2.0.0/rhythm-server-rpiz.tar.gz",
                    "version": "2.0.0",
                    "kind": "archive_bundle",
                    "install": [
                        {"slot": "self", "required": true}
                    ]
                },
                "images": [
                    {
                        "name": "rootfs.ext2.gz",
                        "url": "v2.0.0/rootfs.ext2.gz",
                        "version": "2.0.0",
                        "kind": "rootfs_image",
                        "compression": "gzip"
                    }
                ]
            }"#,
        );
        std::env::set_var("RHYTHM_UPDATE_MANIFEST_URL", manifest_url);

        let info = check_blocking("2.0.0", UpdateChannel::Stable).unwrap();

        std::env::remove_var("RHYTHM_UPDATE_MANIFEST_URL");
        std::env::remove_var("RHYTHM_IMAGE_VERSION_PATH");
        std::env::remove_var("RHYTHM_PLATFORM_TYPE");
        std::env::remove_var("RHYTHM_PLATFORM_CONTEXT");
        let _ = fs::remove_dir_all(dir);

        assert!(!info.update_available);
        assert_eq!(info.update_reason, None);
        assert_eq!(info.current_image_version.as_deref(), Some("2.0.0"));
        assert_eq!(info.latest_image_version.as_deref(), Some("2.0.0"));
        assert_eq!(info.download_url, None);
    }

    #[test]
    fn check_manifest_blocking_uses_package_version_over_manifest_version() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_PLATFORM_TYPE", "desktop");
        std::env::set_var("RHYTHM_PLATFORM_CONTEXT", "server");
        let manifest_url = spawn_json_fixture(
            r#"{
                "version": "99.0.0",
                "package": {
                    "name": "rhythm-server-linux-amd64.tar.gz",
                    "url": "v2.0.0/rhythm-server-linux-amd64.tar.gz",
                    "version": "2.0.0",
                    "kind": "archive_bundle"
                },
                "images": [
                    {
                        "name": "rootfs.ext2.gz",
                        "url": "v3.0.0/rootfs.ext2.gz",
                        "version": "3.0.0",
                        "kind": "rootfs_image",
                        "compression": "gzip"
                    }
                ]
            }"#,
        );
        std::env::set_var("RHYTHM_UPDATE_MANIFEST_URL", manifest_url);

        let info = check_blocking("1.0.0", UpdateChannel::Beta).unwrap();

        std::env::remove_var("RHYTHM_UPDATE_MANIFEST_URL");
        std::env::remove_var("RHYTHM_PLATFORM_TYPE");
        std::env::remove_var("RHYTHM_PLATFORM_CONTEXT");

        assert!(info.update_available);
        assert_eq!(info.update_reason, Some(UpdateReason::VersionMismatch));
        assert_eq!(info.latest_version, "2.0.0");
        assert_eq!(info.latest_package_version, "2.0.0");
        assert_eq!(info.latest_image_version.as_deref(), Some("3.0.0"));
    }

    #[test]
    fn check_manifest_blocking_does_not_install_older_package_without_image_drift() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_PLATFORM_TYPE", "desktop");
        std::env::set_var("RHYTHM_PLATFORM_CONTEXT", "server");
        let manifest_url = spawn_json_fixture(
            r#"{
                "version": "1.0.0",
                "package": {
                    "name": "rhythm-server-linux-amd64.tar.gz",
                    "url": "v1.0.0/rhythm-server-linux-amd64.tar.gz",
                    "version": "1.0.0",
                    "kind": "archive_bundle"
                }
            }"#,
        );
        std::env::set_var("RHYTHM_UPDATE_MANIFEST_URL", manifest_url);

        let info = check_blocking("2.0.0", UpdateChannel::Beta).unwrap();

        std::env::remove_var("RHYTHM_UPDATE_MANIFEST_URL");
        std::env::remove_var("RHYTHM_PLATFORM_TYPE");
        std::env::remove_var("RHYTHM_PLATFORM_CONTEXT");

        assert!(!info.update_available);
        assert_eq!(info.update_reason, None);
        assert_eq!(info.current_package_version, "2.0.0");
        assert_eq!(info.latest_package_version, "1.0.0");
        assert_eq!(info.download_url, None);
    }

    #[test]
    fn check_manifest_blocking_refuses_appliance_image_when_package_would_downgrade() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_PLATFORM_TYPE", "appliance");
        std::env::set_var("RHYTHM_PLATFORM_CONTEXT", "server");
        let manifest_url = spawn_json_fixture(
            r#"{
                "version": "1.0.0",
                "package": {
                    "name": "rhythm-server-rpiz.tar.gz",
                    "url": "v1.0.0/rhythm-server-rpiz.tar.gz",
                    "version": "1.0.0",
                    "kind": "archive_bundle"
                },
                "images": [
                    {
                        "name": "rootfs.ext2.gz",
                        "url": "v999.0.0/rootfs.ext2.gz",
                        "version": "999.0.0",
                        "kind": "rootfs_image",
                        "compression": "gzip"
                    }
                ]
            }"#,
        );
        std::env::set_var("RHYTHM_UPDATE_MANIFEST_URL", manifest_url);

        let error = string_error(check_blocking("2.0.0", UpdateChannel::Stable));

        std::env::remove_var("RHYTHM_UPDATE_MANIFEST_URL");
        std::env::remove_var("RHYTHM_PLATFORM_TYPE");
        std::env::remove_var("RHYTHM_PLATFORM_CONTEXT");

        assert!(
            error.contains("package version 1.0.0 is older than current 2.0.0"),
            "unexpected error: {error}"
        );
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
