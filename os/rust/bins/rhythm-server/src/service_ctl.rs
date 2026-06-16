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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn unique_test_dir(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rhythm-service-ctl-{name}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn with_home<T>(home: &std::path::Path, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var("HOME").ok();
        std::env::set_var("HOME", home);
        let result = f();
        if let Some(previous) = previous {
            std::env::set_var("HOME", previous);
        } else {
            std::env::remove_var("HOME");
        }
        result
    }

    fn with_home_and_path<T>(
        home: &std::path::Path,
        path_prefix: &std::path::Path,
        f: impl FnOnce() -> T,
    ) -> T {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous_home = std::env::var("HOME").ok();
        let previous_path = std::env::var("PATH").ok();
        let previous_fail_start = std::env::var("SYSTEMCTL_FAIL_START").ok();
        let previous_fail_stop = std::env::var("SYSTEMCTL_FAIL_STOP").ok();
        let previous_active = std::env::var("SYSTEMCTL_ACTIVE").ok();
        let previous_pid = std::env::var("SYSTEMCTL_PID").ok();

        std::env::set_var("HOME", home);
        let path = previous_path
            .as_deref()
            .map(|path| format!("{}:{path}", path_prefix.display()))
            .unwrap_or_else(|| path_prefix.display().to_string());
        std::env::set_var("PATH", path);

        let result = f();

        if let Some(previous) = previous_home {
            std::env::set_var("HOME", previous);
        } else {
            std::env::remove_var("HOME");
        }
        if let Some(previous) = previous_path {
            std::env::set_var("PATH", previous);
        } else {
            std::env::remove_var("PATH");
        }
        if let Some(previous) = previous_fail_start {
            std::env::set_var("SYSTEMCTL_FAIL_START", previous);
        } else {
            std::env::remove_var("SYSTEMCTL_FAIL_START");
        }
        if let Some(previous) = previous_fail_stop {
            std::env::set_var("SYSTEMCTL_FAIL_STOP", previous);
        } else {
            std::env::remove_var("SYSTEMCTL_FAIL_STOP");
        }
        if let Some(previous) = previous_active {
            std::env::set_var("SYSTEMCTL_ACTIVE", previous);
        } else {
            std::env::remove_var("SYSTEMCTL_ACTIVE");
        }
        if let Some(previous) = previous_pid {
            std::env::set_var("SYSTEMCTL_PID", previous);
        } else {
            std::env::remove_var("SYSTEMCTL_PID");
        }

        result
    }

    #[cfg(unix)]
    fn install_fake_systemctl(bin_dir: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;

        let script = bin_dir.join("systemctl");
        std::fs::write(
            &script,
            r#"#!/bin/sh
if [ "$1" = "--user" ]; then
  shift
fi
case "$1" in
  start)
    if [ -n "$SYSTEMCTL_FAIL_START" ]; then
      echo "start failed" >&2
      exit 1
    fi
    exit 0
    ;;
  stop)
    if [ -n "$SYSTEMCTL_FAIL_STOP" ]; then
      echo "stop failed" >&2
      exit 1
    fi
    exit 0
    ;;
  disable|daemon-reload)
    exit 0
    ;;
  is-active)
    echo "${SYSTEMCTL_ACTIVE:-active}"
    exit 0
    ;;
  show)
    echo "${SYSTEMCTL_PID:-4242}"
    exit 0
    ;;
  *)
    echo "unexpected $1" >&2
    exit 9
    ;;
esac
"#,
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
    }

    #[test]
    fn gui_domain_and_target_use_current_uid_and_service_label() {
        let expected_domain = format!("gui/{}", unsafe { libc::getuid() });
        assert_eq!(gui_domain(), expected_domain);
        assert_eq!(gui_target(), format!("{}/{}", expected_domain, PLIST_LABEL));
    }

    #[test]
    fn plist_path_uses_home_and_requires_existing_launch_agent() {
        let home = unique_test_dir("plist");
        with_home(&home, || {
            assert_eq!(plist_path(), None);

            let launch_agents = home.join("Library").join("LaunchAgents");
            std::fs::create_dir_all(&launch_agents).unwrap();
            let plist = launch_agents.join(format!("{PLIST_LABEL}.plist"));
            std::fs::write(&plist, b"plist").unwrap();

            assert_eq!(plist_path(), Some(plist.display().to_string()));
        });
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn is_user_service_checks_home_systemd_unit() {
        let home = unique_test_dir("systemd");
        with_home(&home, || {
            assert!(!is_user_service());

            let user_dir = home.join(".config").join("systemd").join("user");
            std::fs::create_dir_all(&user_dir).unwrap();
            std::fs::write(user_dir.join(format!("{SYSTEMD_UNIT}.service")), b"unit").unwrap();

            assert!(is_user_service());
        });
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn remove_file_maybe_sudo_ignores_missing_and_removes_writable_file() {
        let root = unique_test_dir("remove-file");
        let missing = root.join("missing");
        assert!(remove_file_maybe_sudo(missing.to_str().unwrap()).is_ok());

        let file = root.join("service-file");
        std::fs::write(&file, b"service").unwrap();
        remove_file_maybe_sudo(file.to_str().unwrap()).unwrap();
        assert!(!file.exists());

        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn linux_service_commands_use_systemctl_output_and_error_paths() {
        let root = unique_test_dir("linux-systemctl");
        let home = root.join("home");
        let bin = root.join("bin");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        install_fake_systemctl(&bin);

        with_home_and_path(&home, &bin, || {
            assert!(linux_start().is_ok());
            assert!(linux_stop().is_ok());
            assert!(linux_status().is_ok());

            std::env::set_var("SYSTEMCTL_FAIL_START", "1");
            let err = linux_start().unwrap_err();
            assert!(err.contains("systemctl start failed"));
            assert!(err.contains("start failed"));
            std::env::remove_var("SYSTEMCTL_FAIL_START");

            std::env::set_var("SYSTEMCTL_FAIL_STOP", "1");
            let err = linux_stop().unwrap_err();
            assert!(err.contains("systemctl stop failed"));
            assert!(err.contains("stop failed"));
            std::env::remove_var("SYSTEMCTL_FAIL_STOP");

            std::env::set_var("SYSTEMCTL_ACTIVE", "inactive");
            std::env::set_var("SYSTEMCTL_PID", "0");
            assert!(linux_status().is_ok());
        });

        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn linux_uninstall_removes_user_service_and_binaries_without_data_prompt() {
        let root = unique_test_dir("linux-uninstall");
        let home = root.join("home");
        let bin = root.join("bin");
        let user_systemd = home.join(".config").join("systemd").join("user");
        let local_bin = home.join(".local").join("bin");
        std::fs::create_dir_all(&user_systemd).unwrap();
        std::fs::create_dir_all(&local_bin).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        install_fake_systemctl(&bin);

        let service = user_systemd.join(format!("{SYSTEMD_UNIT}.service"));
        let server = local_bin.join("rhythm-server");
        let cli = local_bin.join("rhythm-cli");
        std::fs::write(&service, b"unit").unwrap();
        std::fs::write(&server, b"server").unwrap();
        std::fs::write(&cli, b"cli").unwrap();

        with_home_and_path(&home, &bin, || {
            assert!(is_user_service());
            linux_uninstall().unwrap();
            assert!(!service.exists());
            assert!(!server.exists());
            assert!(!cli.exists());
        });

        std::fs::remove_dir_all(root).ok();
    }
}
