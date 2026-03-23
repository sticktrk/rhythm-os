//! Self-update from GitHub releases.
//!
//! Looks for releases tagged `server-v{version}` with a platform-specific
//! asset named `rhythm-server-{platform}`.

use serde::Deserialize;
use std::path::PathBuf;

const GITHUB_REPO: &str = "sticktrk/rhythm-os";
const TAG_PREFIX: &str = "server-v";
const GITHUB_API: &str = "https://api.github.com";

pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub download_url: Option<String>,
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

fn platform_asset_name() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("rhythm-server-macos-arm64")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("rhythm-server-macos-x86_64")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("rhythm-server-linux-amd64")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("rhythm-server-linux-aarch64")
    } else {
        None
    }
}

/// Check GitHub releases for an available update (blocking).
pub fn check_blocking(current_version: &str) -> Result<UpdateInfo, String> {
    let asset_name = platform_asset_name().ok_or("Unsupported platform for self-update")?;
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

    // Find the most recent release tagged server-v* with our platform asset
    for release in &releases {
        let version = match release.tag_name.strip_prefix(TAG_PREFIX) {
            Some(v) => v,
            None => continue,
        };

        if let Some(matching) = release.assets.iter().find(|a| a.name == asset_name) {
            return Ok(UpdateInfo {
                current_version: current_version.to_string(),
                latest_version: version.to_string(),
                update_available: version != current_version,
                download_url: if version != current_version {
                    Some(matching.browser_download_url.clone())
                } else {
                    None
                },
            });
        }
    }

    // No matching release found
    Ok(UpdateInfo {
        current_version: current_version.to_string(),
        latest_version: current_version.to_string(),
        update_available: false,
        download_url: None,
    })
}

/// Download and install a new binary, replacing the current executable (blocking).
///
/// Returns the path to the installed binary.
pub fn apply_blocking(download_url: &str) -> Result<PathBuf, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("rhythm-server")
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let resp = client
        .get(download_url)
        .send()
        .map_err(|e| format!("Download failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Download returned {}", resp.status()));
    }

    let bytes = resp
        .bytes()
        .map_err(|e| format!("Failed to read download: {}", e))?;

    let current_exe =
        std::env::current_exe().map_err(|e| format!("Cannot determine exe path: {}", e))?;

    let new_path = current_exe.with_extension("new");
    let old_path = current_exe.with_extension("old");

    // Write new binary
    std::fs::write(&new_path, &bytes).map_err(|e| format!("Failed to write new binary: {}", e))?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Failed to set permissions: {}", e))?;
    }

    // Atomic swap: current -> old, new -> current
    if old_path.exists() {
        std::fs::remove_file(&old_path).ok();
    }
    std::fs::rename(&current_exe, &old_path)
        .map_err(|e| format!("Failed to backup current binary: {}", e))?;

    if let Err(e) = std::fs::rename(&new_path, &current_exe) {
        // Restore on failure
        let _ = std::fs::rename(&old_path, &current_exe);
        return Err(format!("Failed to install new binary: {}", e));
    }

    // Clean up old binary
    std::fs::remove_file(&old_path).ok();

    Ok(current_exe)
}
