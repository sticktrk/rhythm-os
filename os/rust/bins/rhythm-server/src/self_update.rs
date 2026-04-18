//! Self-update from a static OTA feed.
//!
//! Prefers a per-platform `manifest.json` feed hosted outside the repo, with
//! an optional GitHub-release fallback for legacy/manual flows.

use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tar::Archive;

const GITHUB_REPO: &str = "sticktrk/rhythm-os";
const GITHUB_API: &str = "https://api.github.com";
const CHECKSUM_ASSET_NAME: &str = "SHA256SUMS.txt";
const DEFAULT_UPDATE_BASE_URL: &str = "https://dl.rhythm.lighting/server";
const EMBEDDED_INSTALL_PATH: &str = "/usr/bin/rhythm-server";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RestartStrategy {
    SupervisorExit,
    EmbeddedReboot,
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
    pub checked_at_epoch_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum_verified: Option<bool>,
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
                checked_at_epoch_ms: None,
                checksum_verified: None,
                message: None,
                last_error: None,
            })),
        }
    }

    pub fn capabilities(&self) -> OtaCapabilities {
        OtaCapabilities {
            strategy: "self_pull",
            scope: "binary",
            can_check: true,
            can_update: true,
            can_upload: false,
            requires_restart: true,
            rollback: "manual",
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
                checked_at_epoch_ms: Some(now_ms()),
                checksum_verified: None,
                message: Some("OTA state lock poisoned".to_string()),
                last_error: Some("OTA state lock poisoned".to_string()),
            })
    }

    pub fn mark_checking(&self) {
        self.with_status(|status| {
            status.state = OtaUpdateState::Checking;
            status.checked_at_epoch_ms = Some(now_ms());
            status.target_version = None;
            status.checksum_verified = None;
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
            status.checked_at_epoch_ms = Some(now_ms());
            status.target_version = None;
            status.checksum_verified = None;
            status.message = Some(if info.update_available {
                format!("Update available: v{}", info.latest_version)
            } else {
                "Already up to date".to_string()
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
            status.checked_at_epoch_ms = Some(now_ms());
            status.checksum_verified = checksum_verified;
            status.message = Some(format!(
                "Updated from v{} to v{}, restarting...",
                previous_version, new_version
            ));
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
    pub download_url: Option<String>,
    pub checksum_url: Option<String>,
    pub expected_sha256: Option<String>,
    pub asset_name: Option<String>,
}

pub struct ApplyResult {
    pub checksum_verified: Option<bool>,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct UpdateManifest {
    version: String,
    url: String,
    #[serde(default)]
    sha256: Option<String>,
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn platform_asset_names() -> Option<Vec<&'static str>> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some(vec![
            "rhythm-server-macos-arm64",
            "rhythm-server-macos-arm64.tar.gz",
        ])
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some(vec![
            "rhythm-server-macos-x86_64",
            "rhythm-server-macos-x86_64.tar.gz",
        ])
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(vec![
            "rhythm-server-linux-amd64",
            "rhythm-server-linux-amd64.tar.gz",
        ])
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some(vec![
            "rhythm-server-linux-aarch64",
            "rhythm-server-linux-aarch64.tar.gz",
        ])
    } else if cfg!(all(target_os = "linux", target_arch = "arm")) {
        Some(vec!["rhythm-server-rpiz", "rhythm-server-rpiz.tar.gz"])
    } else {
        None
    }
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

fn release_version(tag_name: &str) -> Option<&str> {
    tag_name
        .strip_prefix("server-v")
        .or_else(|| tag_name.strip_prefix('v'))
}

/// Check the configured OTA feed for an available update (blocking).
pub fn check_blocking(current_version: &str) -> Result<UpdateInfo, String> {
    match check_manifest_blocking(current_version) {
        Ok(info) => return Ok(info),
        Err(manifest_error) => {
            if std::env::var_os("RHYTHM_UPDATE_MANIFEST_URL").is_some()
                || std::env::var_os("RHYTHM_UPDATE_BASE_URL").is_some()
            {
                return Err(manifest_error);
            }

            if std::env::var("RHYTHM_UPDATE_GITHUB_FALLBACK")
                .ok()
                .as_deref()
                != Some("1")
            {
                return Err(manifest_error);
            }
        }
    }

    check_github_blocking(current_version)
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
    let download_url = resolve_download_url(&manifest_url, &manifest.url)?;
    let asset_name = asset_name_from_url(&download_url)?;
    let update_available = manifest.version != current_version;

    Ok(UpdateInfo {
        current_version: current_version.to_string(),
        latest_version: manifest.version.clone(),
        update_available,
        download_url: update_available.then_some(download_url),
        checksum_url: None,
        expected_sha256: update_available.then_some(manifest.sha256).flatten(),
        asset_name: update_available.then_some(asset_name),
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

fn check_github_blocking(current_version: &str) -> Result<UpdateInfo, String> {
    let asset_names = platform_asset_names().ok_or("Unsupported platform for self-update")?;
    let url = format!("{}/repos/{}/releases?per_page=20", GITHUB_API, GITHUB_REPO);

    let client = reqwest::blocking::Client::builder()
        .user_agent("rhythm-server")
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("GitHub API request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("GitHub API returned {}", resp.status()));
    }

    let releases: Vec<Release> = resp
        .json()
        .map_err(|e| format!("Failed to parse releases: {}", e))?;

    // GitHub returns releases newest-first; use the first compatible asset we find.
    for release in &releases {
        let version = match release_version(&release.tag_name) {
            Some(v) => v,
            None => continue,
        };

        for asset_name in &asset_names {
            if let Some(asset) = release.assets.iter().find(|a| a.name == *asset_name) {
                let checksum_url = release
                    .assets
                    .iter()
                    .find(|a| a.name == CHECKSUM_ASSET_NAME)
                    .map(|a| a.browser_download_url.clone());

                let update_available = version != current_version;
                return Ok(UpdateInfo {
                    current_version: current_version.to_string(),
                    latest_version: version.to_string(),
                    update_available,
                    download_url: update_available.then(|| asset.browser_download_url.clone()),
                    checksum_url: update_available.then_some(checksum_url).flatten(),
                    expected_sha256: None,
                    asset_name: update_available.then(|| asset.name.clone()),
                });
            }
        }
    }

    Ok(UpdateInfo {
        current_version: current_version.to_string(),
        latest_version: current_version.to_string(),
        update_available: false,
        download_url: None,
        checksum_url: None,
        expected_sha256: None,
        asset_name: None,
    })
}

/// Download and install a new binary, replacing the current executable (blocking).
pub fn apply_blocking(
    download_url: &str,
    asset_name: &str,
    expected_sha256: Option<&str>,
    checksum_url: Option<&str>,
) -> Result<ApplyResult, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("rhythm-server")
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let current_exe = install_target_executable()?;
    let download_path = current_exe.with_extension("download");
    let new_path = current_exe.with_extension("new");
    let old_path = current_exe.with_extension("old");

    remove_if_exists(&download_path);
    remove_if_exists(&new_path);

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
        None => match checksum_url {
            Some(url) => {
                let expected = fetch_expected_sha256(&client, url, asset_name)?;
                let actual = compute_sha256_hex(&download_path)?;
                if actual != expected {
                    remove_if_exists(&download_path);
                    return Err(format!(
                        "SHA256 mismatch for {}: expected {}, got {}",
                        asset_name, expected, actual
                    ));
                }
                Some(true)
            }
            None => None,
        },
    };

    if let Err(e) = install_downloaded_binary(&download_path, &new_path, asset_name) {
        remove_if_exists(&download_path);
        remove_if_exists(&new_path);
        return Err(e);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Failed to set permissions: {}", e))?;
    }

    if old_path.exists() {
        std::fs::remove_file(&old_path).ok();
    }

    std::fs::rename(&current_exe, &old_path)
        .map_err(|e| format!("Failed to backup current binary: {}", e))?;

    if let Err(e) = std::fs::rename(&new_path, &current_exe) {
        let _ = std::fs::rename(&old_path, &current_exe);
        remove_if_exists(&download_path);
        remove_if_exists(&new_path);
        return Err(format!("Failed to install new binary: {}", e));
    }

    remove_if_exists(&download_path);

    Ok(ApplyResult { checksum_verified })
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

    std::env::current_exe().map_err(|e| format!("Cannot determine exe path: {}", e))
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

fn fetch_expected_sha256(
    client: &reqwest::blocking::Client,
    checksum_url: &str,
    asset_name: &str,
) -> Result<String, String> {
    let resp = client
        .get(checksum_url)
        .send()
        .map_err(|e| format!("Failed to fetch checksums: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Checksum download returned {}", resp.status()));
    }

    let body = resp
        .text()
        .map_err(|e| format!("Failed to read checksums: {}", e))?;
    parse_sha256sums(&body, asset_name)
        .ok_or_else(|| format!("Missing checksum entry for {}", asset_name))
}

fn parse_sha256sums(body: &str, asset_name: &str) -> Option<String> {
    for line in body.lines() {
        let mut parts = line.split_whitespace();
        let Some(digest) = parts.next() else { continue };
        let Some(file_name) = parts.next() else {
            continue;
        };
        let file_name = file_name.trim_start_matches('*');
        if file_name == asset_name {
            return Some(digest.to_ascii_lowercase());
        }
    }
    None
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

fn install_downloaded_binary(
    download_path: &Path,
    new_path: &Path,
    asset_name: &str,
) -> Result<(), String> {
    if asset_name.ends_with(".tar.gz") {
        extract_binary_from_archive(download_path, new_path)
    } else {
        std::fs::copy(download_path, new_path)
            .map(|_| ())
            .map_err(|e| format!("Failed to copy downloaded binary: {}", e))
    }
}

fn extract_binary_from_archive(archive_path: &Path, destination: &Path) -> Result<(), String> {
    let file = File::open(archive_path).map_err(|e| format!("Failed to open archive: {}", e))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|e| format!("Failed to read archive entries: {}", e))?;

    for entry in entries {
        let mut entry = entry.map_err(|e| format!("Failed to read archive entry: {}", e))?;
        let path = entry
            .path()
            .map_err(|e| format!("Failed to inspect archive entry: {}", e))?;
        if path.file_name().and_then(|name| name.to_str()) == Some("rhythm-server") {
            entry
                .unpack(destination)
                .map_err(|e| format!("Failed to extract rhythm-server: {}", e))?;
            return Ok(());
        }
    }

    Err("Archive did not contain rhythm-server".to_string())
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
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn unique_test_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rhythm-server-{}-{}", name, nanos));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn release_version_supports_current_and_legacy_tags() {
        assert_eq!(release_version("v1.2.3"), Some("1.2.3"));
        assert_eq!(release_version("server-v1.2.3"), Some("1.2.3"));
        assert_eq!(release_version("not-a-release"), None);
    }

    #[test]
    fn parse_sha256sums_finds_named_entry() {
        let body = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  rhythm-server-linux-amd64.tar.gz\n\
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  rhythm-server-rpiz.tar.gz\n";

        assert_eq!(
            parse_sha256sums(body, "rhythm-server-rpiz.tar.gz"),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string())
        );
        assert_eq!(parse_sha256sums(body, "missing.tar.gz"), None);
    }

    #[test]
    fn extract_binary_from_archive_unpacks_rhythm_server() {
        let dir = unique_test_dir("extract");
        let archive_path = dir.join("release.tar.gz");
        let extracted_path = dir.join("rhythm-server.new");

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
            builder.append(&header, &payload[..]).unwrap();
            builder.finish().unwrap();
        }

        extract_binary_from_archive(&archive_path, &extracted_path).unwrap();
        assert_eq!(
            std::fs::read(&extracted_path).unwrap(),
            b"fake-rhythm-server"
        );

        let _ = std::fs::remove_dir_all(dir);
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
    fn install_target_executable_prefers_embedded_install_path() {
        std::env::set_var("RHYTHM_PLATFORM_TYPE", "embedded");
        std::env::set_var("RHYTHM_PLATFORM_CONTEXT", "rpiz");

        let path = install_target_executable().unwrap();
        assert_eq!(path, PathBuf::from(EMBEDDED_INSTALL_PATH));

        std::env::remove_var("RHYTHM_PLATFORM_TYPE");
        std::env::remove_var("RHYTHM_PLATFORM_CONTEXT");
    }
}
