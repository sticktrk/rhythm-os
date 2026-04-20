//! Self-update from a static OTA feed.
//!
//! Prefers a per-platform `manifest.json` feed hosted outside the repo. The
//! manifest advertises an archive bundle for coordinated `rhythm-server` /
//! `rhythm-chipd` updates and optional full-image artifacts for appliance
//! update flows.

use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tar::Archive;

const DEFAULT_UPDATE_BASE_URL: &str = "https://dl.rhythm.lighting/server";
const EMBEDDED_INSTALL_PATH: &str = "/usr/bin/rhythm-server";
const EMBEDDED_BOOT_MOUNT: &str = "/boot";
const EMBEDDED_BOOT_DEVICE: &str = "/dev/mmcblk0p1";
const EMBEDDED_ROOTFS_A_DEVICE: &str = "/dev/mmcblk0p2";
const EMBEDDED_ROOTFS_B_DEVICE: &str = "/dev/mmcblk0p3";
const EMBEDDED_OTA_STAGING_DIR: &str = "/data/ota";
const EMBEDDED_CMDLINE_PATH: &str = "/boot/cmdline.txt";
const EMBEDDED_CMDLINE_BACKUP_PATH: &str = "/boot/cmdline.txt.bak";
const EMBEDDED_BOOT_STATE_PATH: &str = "/boot/rhythm-bootstate.env";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RestartStrategy {
    SupervisorExit,
    EmbeddedReboot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EmbeddedSlot {
    A,
    B,
}

impl EmbeddedSlot {
    fn as_str(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
        }
    }

    fn root_device(self) -> &'static str {
        match self {
            Self::A => EMBEDDED_ROOTFS_A_DEVICE,
            Self::B => EMBEDDED_ROOTFS_B_DEVICE,
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
        let embedded = restart_strategy() == RestartStrategy::EmbeddedReboot;
        OtaCapabilities {
            strategy: "self_pull",
            scope: if embedded {
                "rootfs_slot"
            } else {
                "component_bundle"
            },
            can_check: true,
            can_update: true,
            can_upload: false,
            requires_restart: true,
            rollback: if embedded {
                "slot_switch"
            } else {
                "backup_files"
            },
            payloads: if embedded {
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

impl UpdateInfo {
    pub fn apply_blocking(&self) -> Result<ApplyResult, String> {
        if restart_strategy() == RestartStrategy::EmbeddedReboot {
            if let Some(image_asset) = self.preferred_embedded_image_asset() {
                return apply_embedded_image_blocking(
                    image_asset,
                    &self.latest_version,
                    self.expected_sha256.as_deref(),
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
        )
    }

    fn preferred_embedded_image_asset(&self) -> Option<&UpdateImageAsset> {
        self.image_assets
            .iter()
            .filter(|asset| asset.kind == ReleaseArtifactKind::RootfsImage)
            .min_by_key(|asset| if artifact_uses_gzip(asset) { 0 } else { 1 })
    }
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
pub fn check_blocking(current_version: &str) -> Result<UpdateInfo, String> {
    check_manifest_blocking(current_version)
}

fn check_manifest_blocking(current_version: &str) -> Result<UpdateInfo, String> {
    let manifest_url = configured_manifest_url()?;

    let client = reqwest::blocking::Client::builder()
        .user_agent("rhythm-server")
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let resp = client
        .get(&manifest_url)
        .send()
        .map_err(|e| format!("Update manifest request failed: {}", e))?;

    if !resp.status().is_success() {
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
    let embedded_rootfs_only =
        restart_strategy() == RestartStrategy::EmbeddedReboot && has_rootfs_image(&image_assets);

    if package.is_none() && !embedded_rootfs_only {
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

fn configured_manifest_url() -> Result<String, String> {
    if let Ok(url) = std::env::var("RHYTHM_UPDATE_MANIFEST_URL") {
        return Ok(url);
    }

    let base_url = std::env::var("RHYTHM_UPDATE_BASE_URL")
        .unwrap_or_else(|_| DEFAULT_UPDATE_BASE_URL.to_string());
    let platform = platform_feed_name().ok_or("Unsupported platform for self-update")?;
    Ok(format!(
        "{}/{}/manifest.json",
        base_url.trim_end_matches('/'),
        platform
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

fn parse_embedded_slot_from_cmdline(cmdline: &str) -> Result<EmbeddedSlot, String> {
    for token in cmdline.split_whitespace() {
        if let Some(root_device) = token.strip_prefix("root=") {
            return embedded_slot_from_root_device(root_device);
        }
    }

    Err("Kernel command line did not include a root= device".to_string())
}

fn embedded_slot_from_root_device(root_device: &str) -> Result<EmbeddedSlot, String> {
    match root_device {
        EMBEDDED_ROOTFS_A_DEVICE => Ok(EmbeddedSlot::A),
        EMBEDDED_ROOTFS_B_DEVICE => Ok(EmbeddedSlot::B),
        other => Err(format!("Unsupported embedded root device {}", other)),
    }
}

fn current_embedded_slot() -> Result<EmbeddedSlot, String> {
    let cmdline = fs::read_to_string("/proc/cmdline")
        .map_err(|e| format!("Failed to read /proc/cmdline: {}", e))?;
    parse_embedded_slot_from_cmdline(&cmdline)
}

fn embedded_root_arg_for_slot(slot: EmbeddedSlot) -> String {
    format!("root={}", slot.root_device())
}

fn rewrite_cmdline_root_device(cmdline: &str, slot: EmbeddedSlot) -> Result<String, String> {
    let replacement = embedded_root_arg_for_slot(slot);
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

fn write_embedded_boot_state(
    current_slot: EmbeddedSlot,
    target_slot: EmbeddedSlot,
    version: &str,
) -> Result<(), String> {
    let body = format!(
        "RHYTHM_ACTIVE_SLOT={}\nRHYTHM_LAST_GOOD_SLOT={}\nRHYTHM_PENDING_SLOT={}\nRHYTHM_PENDING_VERSION={}\nRHYTHM_LAST_UPDATE_EPOCH_MS={}\n",
        current_slot.as_str(),
        current_slot.as_str(),
        target_slot.as_str(),
        version,
        now_ms()
    );
    fs::write(EMBEDDED_BOOT_STATE_PATH, body)
        .map_err(|e| format!("Failed to write {}: {}", EMBEDDED_BOOT_STATE_PATH, e))
}

fn update_embedded_cmdline_for_slot(slot: EmbeddedSlot) -> Result<(), String> {
    ensure_mount(EMBEDDED_BOOT_MOUNT, EMBEDDED_BOOT_DEVICE, "vfat")?;

    let current_cmdline = fs::read_to_string(EMBEDDED_CMDLINE_PATH)
        .map_err(|e| format!("Failed to read {}: {}", EMBEDDED_CMDLINE_PATH, e))?;
    let rewritten = rewrite_cmdline_root_device(&current_cmdline, slot)?;

    let _ = fs::copy(EMBEDDED_CMDLINE_PATH, EMBEDDED_CMDLINE_BACKUP_PATH);
    fs::write(EMBEDDED_CMDLINE_PATH, format!("{}\n", rewritten))
        .map_err(|e| format!("Failed to write {}: {}", EMBEDDED_CMDLINE_PATH, e))
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
    Path::new(EMBEDDED_OTA_STAGING_DIR).join(format!("{}.download", safe_name))
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

fn apply_embedded_image_blocking(
    image_asset: &UpdateImageAsset,
    latest_version: &str,
    fallback_sha256: Option<&str>,
) -> Result<ApplyResult, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("rhythm-server")
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let current_slot = current_embedded_slot()?;
    let target_slot = current_slot.inactive();

    ensure_mount(EMBEDDED_BOOT_MOUNT, EMBEDDED_BOOT_DEVICE, "vfat")?;
    fs::create_dir_all(EMBEDDED_OTA_STAGING_DIR)
        .map_err(|e| format!("Failed to create {}: {}", EMBEDDED_OTA_STAGING_DIR, e))?;

    let download_path = artifact_staging_path(&image_asset.name);
    remove_if_exists(&download_path);
    download_release(&client, &image_asset.url, &download_path)?;

    let expected_sha256 = image_asset.sha256.as_deref().or(fallback_sha256);
    let checksum_verified = match expected_sha256 {
        Some(expected) => {
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
        write_image_artifact_to_device(
            &download_path,
            Path::new(target_slot.root_device()),
            artifact_uses_gzip(image_asset),
        )?;
        update_embedded_cmdline_for_slot(target_slot)?;
        write_embedded_boot_state(current_slot, target_slot, latest_version)?;
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

    download_release(&client, download_url, &download_path)?;

    let checksum_verified = match expected_sha256 {
        Some(expected) => {
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

    let staged_targets = stage_install_targets(&download_path, asset_name, &resolved_targets)
        .map_err(|error| {
            cleanup_install_artifacts(&resolved_targets);
            remove_if_exists(&download_path);
            error
        })?;
    let installed_targets = commit_staged_targets(&staged_targets).map_err(|error| {
        cleanup_staged_files(&staged_targets);
        remove_if_exists(&download_path);
        error
    })?;

    remove_if_exists(&download_path);

    Ok(ApplyResult {
        checksum_verified,
        installed_targets,
    })
}

fn download_release(
    client: &reqwest::blocking::Client,
    download_url: &str,
    destination: &Path,
) -> Result<(), String> {
    let mut resp = client
        .get(download_url)
        .send()
        .map_err(|e| format!("Download failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Download returned {}", resp.status()));
    }

    let mut file =
        File::create(destination).map_err(|e| format!("Failed to create download: {}", e))?;
    std::io::copy(&mut resp, &mut file).map_err(|e| format!("Failed to write download: {}", e))?;
    file.flush()
        .map_err(|e| format!("Failed to flush download: {}", e))?;
    Ok(())
}

fn install_target_executable() -> Result<PathBuf, String> {
    let platform_type = std::env::var("RHYTHM_PLATFORM_TYPE").ok();
    let platform_context = std::env::var("RHYTHM_PLATFORM_CONTEXT").ok();

    if platform_type.as_deref() == Some("embedded")
        && matches!(
            platform_context.as_deref(),
            Some("rpiz") | Some("linux-embedded")
        )
    {
        return Ok(PathBuf::from(EMBEDDED_INSTALL_PATH));
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
    let platform_context = std::env::var("RHYTHM_PLATFORM_CONTEXT").ok();

    if platform_type.as_deref() == Some("embedded")
        && matches!(
            platform_context.as_deref(),
            Some("rpiz") | Some("linux-embedded")
        )
    {
        RestartStrategy::EmbeddedReboot
    } else {
        RestartStrategy::SupervisorExit
    }
}

pub fn schedule_post_update_restart() {
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(1));
        match restart_strategy() {
            RestartStrategy::EmbeddedReboot => {
                log::info!(target: "sys", "Rebooting appliance after self-update...");

                let reboot_result = Command::new("/sbin/reboot")
                    .status()
                    .or_else(|_| Command::new("reboot").status());

                match reboot_result {
                    Ok(status) if status.success() => {}
                    Ok(status) => {
                        log::error!(
                            target: "sys",
                            "Embedded reboot command exited with status {:?}; falling back to process exit",
                            status.code()
                        );
                        std::process::exit(1);
                    }
                    Err(error) => {
                        log::error!(
                            target: "sys",
                            "Failed to invoke embedded reboot after self-update: {}; falling back to process exit",
                            error
                        );
                        std::process::exit(1);
                    }
                }
            }
            RestartStrategy::SupervisorExit => {
                log::info!(target: "sys", "Restarting after self-update...");
                std::process::exit(1);
            }
        }
    });
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
    use std::sync::Mutex;

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
    fn parse_embedded_slot_from_cmdline_reads_root_partition() {
        assert_eq!(
            parse_embedded_slot_from_cmdline("console=tty1 root=/dev/mmcblk0p2 rootwait rw")
                .unwrap(),
            EmbeddedSlot::A
        );
        assert_eq!(
            parse_embedded_slot_from_cmdline("console=tty1 root=/dev/mmcblk0p3 rootwait rw")
                .unwrap(),
            EmbeddedSlot::B
        );
    }

    #[test]
    fn rewrite_cmdline_root_device_swaps_slots() {
        assert_eq!(
            rewrite_cmdline_root_device(
                "console=tty1 root=/dev/mmcblk0p2 rootwait rw",
                EmbeddedSlot::B
            )
            .unwrap(),
            "console=tty1 root=/dev/mmcblk0p3 rootwait rw"
        );
    }

    #[test]
    fn embedded_update_prefers_gzip_rootfs_asset() {
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
            info.preferred_embedded_image_asset()
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
        let chipd_path = dir.join("rhythm-chipd");
        write_executable(&install_root, "#!/bin/sh\necho \"rhythm-server 0.4.146\"\n");
        write_executable(&chipd_path, "#!/bin/sh\necho \"rhythm-chipd 0.4.146\"\n");

        let mut targets = default_bundle_install_targets(&install_root);
        targets[2].required = false;
        assert!(!detect_component_drift("0.4.146", &install_root, &targets));

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
    fn install_target_executable_prefers_embedded_install_path() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_PLATFORM_TYPE", "embedded");
        std::env::set_var("RHYTHM_PLATFORM_CONTEXT", "rpiz");

        let path = install_target_executable().unwrap();
        assert_eq!(path, PathBuf::from(EMBEDDED_INSTALL_PATH));

        std::env::remove_var("RHYTHM_PLATFORM_TYPE");
        std::env::remove_var("RHYTHM_PLATFORM_CONTEXT");
    }

    #[test]
    fn restart_strategy_uses_embedded_reboot_for_rpiz() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_PLATFORM_TYPE", "embedded");
        std::env::set_var("RHYTHM_PLATFORM_CONTEXT", "rpiz");

        assert_eq!(restart_strategy(), RestartStrategy::EmbeddedReboot);

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
}
