//! Service lifecycle: start, stop, restart, status, uninstall.
//!
//! Delegates to launchctl (macOS) or systemctl (Linux).

use std::io::{self, Write};
use std::process::Command;

const PLIST_LABEL: &str = "com.rhythm.lighting.server";
const SYSTEMD_UNIT: &str = "rhythm-server";

pub fn start() -> Result<(), String> {
    if cfg!(target_os = "macos") {
        macos_start()
    } else {
        linux_start()
    }
}

pub fn stop() -> Result<(), String> {
    if cfg!(target_os = "macos") {
        macos_stop()
    } else {
        linux_stop()
    }
}

pub fn restart() -> Result<(), String> {
    stop().ok();
    start()
}

pub fn status() -> Result<(), String> {
    if cfg!(target_os = "macos") {
        macos_status()
    } else {
        linux_status()
    }
}

pub fn uninstall() -> Result<(), String> {
    if cfg!(target_os = "macos") {
        macos_uninstall()
    } else {
        linux_uninstall()
    }
}

fn confirm(prompt: &str) -> bool {
    print!("{} [y/N] ", prompt);
    io::stdout().flush().ok();
    let mut input = String::new();
    io::stdin().read_line(&mut input).ok();
    input.trim().eq_ignore_ascii_case("y")
}

fn remove_file_maybe_sudo(path: &str) -> Result<(), String> {
    let p = std::path::Path::new(path);
    if !p.exists() {
        return Ok(());
    }
    // Try direct removal first, fall back to sudo
    if std::fs::remove_file(p).is_ok() {
        println!("Removed {}", path);
        return Ok(());
    }
    let output = Command::new("sudo")
        .args(["rm", path])
        .output()
        .map_err(|e| format!("Failed to remove {}: {}", path, e))?;
    if output.status.success() {
        println!("Removed {}", path);
        Ok(())
    } else {
        Err(format!("Failed to remove {}", path))
    }
}

// ---------------------------------------------------------------------------
// macOS — launchctl
// ---------------------------------------------------------------------------

fn gui_domain() -> String {
    let uid = unsafe { libc::getuid() };
    format!("gui/{}", uid)
}

fn gui_target() -> String {
    format!("{}/{}", gui_domain(), PLIST_LABEL)
}

fn plist_path() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let path = format!("{}/Library/LaunchAgents/{}.plist", home, PLIST_LABEL);
    if std::path::Path::new(&path).exists() {
        Some(path)
    } else {
        None
    }
}

fn macos_start() -> Result<(), String> {
    let plist = plist_path().ok_or_else(|| {
        format!(
            "Plist not found. Install first with: ./install/install.sh\n\
             Expected: ~/Library/LaunchAgents/{}.plist",
            PLIST_LABEL
        )
    })?;

    let output = Command::new("launchctl")
        .args(["bootstrap", &gui_domain(), &plist])
        .output()
        .map_err(|e| format!("Failed to run launchctl: {}", e))?;

    if output.status.success() {
        println!("Service started.");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("service already loaded") || stderr.contains("36:") {
            println!("Service is already running.");
            Ok(())
        } else {
            Err(format!("launchctl bootstrap failed: {}", stderr))
        }
    }
}

fn macos_stop() -> Result<(), String> {
    let output = Command::new("launchctl")
        .args(["bootout", &gui_target()])
        .output()
        .map_err(|e| format!("Failed to run launchctl: {}", e))?;

    if output.status.success() {
        println!("Service stopped.");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("could not find service") || stderr.contains("3:") {
            println!("Service is not running.");
            Ok(())
        } else {
            Err(format!("launchctl bootout failed: {}", stderr))
        }
    }
}

fn macos_status() -> Result<(), String> {
    let output = Command::new("launchctl")
        .args(["print", &gui_target()])
        .output()
        .map_err(|e| format!("Failed to run launchctl: {}", e))?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut pid = None;
        let mut state = "loaded";
        for line in stdout.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("pid = ") {
                pid = trimmed.strip_prefix("pid = ").map(|s| s.trim().to_string());
            }
            if trimmed.starts_with("state = ") {
                state = if trimmed.contains("running") {
                    "running"
                } else if trimmed.contains("waiting") {
                    "waiting"
                } else {
                    "loaded"
                };
            }
        }

        println!("Status:  {}", state);
        if let Some(p) = pid {
            println!("PID:     {}", p);
        }
        println!("Service: {}", PLIST_LABEL);
        if let Some(ref path) = plist_path() {
            println!("Plist:   {}", path);
        }
    } else {
        println!("Status:  not installed");
        println!("Install: ./install/install.sh");
    }

    println!("Version: {}", crate::VERSION);
    Ok(())
}

fn macos_uninstall() -> Result<(), String> {
    // Stop service
    macos_stop().ok();

    // Remove plist
    if let Some(path) = plist_path() {
        std::fs::remove_file(&path).ok();
        println!("Removed {}", path);
    }

    // Remove binaries
    remove_file_maybe_sudo("/usr/local/bin/rhythm-server")?;
    remove_file_maybe_sudo("/usr/local/bin/rhythm-cli")?;

    // Remove logs
    let home = std::env::var("HOME").unwrap_or_default();
    let log_dir = format!("{}/Library/Logs/Rhythm", home);
    if std::path::Path::new(&log_dir).exists() {
        std::fs::remove_dir_all(&log_dir).ok();
        println!("Removed {}", log_dir);
    }

    // Prompt for data
    let data_dir = format!("{}/.rhythm", home);
    if std::path::Path::new(&data_dir).exists()
        && confirm(&format!("Remove data directory {}?", data_dir))
    {
        std::fs::remove_dir_all(&data_dir).ok();
        println!("Removed {}", data_dir);
    }

    println!("\nrhythm-server uninstalled.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Linux — systemctl
// ---------------------------------------------------------------------------

fn is_user_service() -> bool {
    let home = std::env::var("HOME").unwrap_or_default();
    let user_unit = format!("{}/.config/systemd/user/{}.service", home, SYSTEMD_UNIT);
    std::path::Path::new(&user_unit).exists()
}

fn systemctl(args: &[&str]) -> Result<std::process::Output, String> {
    let mut cmd = Command::new("systemctl");
    if is_user_service() {
        cmd.arg("--user");
    }
    cmd.args(args)
        .output()
        .map_err(|e| format!("Failed to run systemctl: {}", e))
}

fn linux_start() -> Result<(), String> {
    let output = systemctl(&["start", SYSTEMD_UNIT])?;
    if output.status.success() {
        println!("Service started.");
        Ok(())
    } else {
        Err(format!(
            "systemctl start failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn linux_stop() -> Result<(), String> {
    let output = systemctl(&["stop", SYSTEMD_UNIT])?;
    if output.status.success() {
        println!("Service stopped.");
        Ok(())
    } else {
        Err(format!(
            "systemctl stop failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn linux_status() -> Result<(), String> {
    let user = is_user_service();
    let output = systemctl(&["is-active", SYSTEMD_UNIT])?;
    let active = String::from_utf8_lossy(&output.stdout).trim().to_string();

    let pid_output = systemctl(&["show", SYSTEMD_UNIT, "--property=MainPID", "--value"])?;
    let pid = String::from_utf8_lossy(&pid_output.stdout)
        .trim()
        .to_string();

    println!("Status:  {}", active);
    if active == "active" && pid != "0" {
        println!("PID:     {}", pid);
    }
    println!(
        "Service: {}.service{}",
        SYSTEMD_UNIT,
        if user { " (user)" } else { "" }
    );
    println!("Version: {}", crate::VERSION);
    Ok(())
}

fn linux_uninstall() -> Result<(), String> {
    let user = is_user_service();

    // Stop and disable
    let _ = systemctl(&["stop", SYSTEMD_UNIT]);
    let _ = systemctl(&["disable", SYSTEMD_UNIT]);

    // Remove service file
    let service_path = if user {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{}/.config/systemd/user/{}.service", home, SYSTEMD_UNIT)
    } else {
        format!("/etc/systemd/system/{}.service", SYSTEMD_UNIT)
    };
    if std::path::Path::new(&service_path).exists() {
        remove_file_maybe_sudo(&service_path)?;
        let _ = systemctl(&["daemon-reload"]);
    }

    // Remove binaries
    let bin_dir = if user {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{}/.local/bin", home)
    } else {
        "/usr/local/bin".to_string()
    };
    remove_file_maybe_sudo(&format!("{}/rhythm-server", bin_dir))?;
    remove_file_maybe_sudo(&format!("{}/rhythm-cli", bin_dir))?;

    // Prompt for data
    let data_dir = if user {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{}/.rhythm", home)
    } else {
        "/var/lib/rhythm".to_string()
    };
    if std::path::Path::new(&data_dir).exists()
        && confirm(&format!("Remove data directory {}?", data_dir))
    {
        if std::fs::remove_dir_all(&data_dir).is_err() {
            let _ = Command::new("sudo").args(["rm", "-rf", &data_dir]).output();
        }
        println!("Removed {}", data_dir);
    }

    println!("\nrhythm-server uninstalled.");
    Ok(())
}
