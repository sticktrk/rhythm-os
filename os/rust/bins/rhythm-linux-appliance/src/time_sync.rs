//! Appliance wall-clock synchronization helpers.
//!
//! Buildroot already installs `S48sntp`, but it runs during early boot before
//! first-boot Wi-Fi or BLE provisioning has a chance to bring the network up.
//! This module retries an explicit one-shot sync once the appliance actually
//! has connectivity, and provides a coarse "clock sane" check so startup can
//! gate time-sensitive behavior.

use std::fs::OpenOptions;
use std::process::{Command, ExitStatus};

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use log::{info, warn};

const SNTP_INIT_SCRIPT: &str = "/etc/init.d/S48sntp";
const SNTP_BIN: &str = "/usr/bin/sntp";
const SNTP_KEY_CACHE: &str = "/tmp/kod";
const SNTP_SERVERS: &[&str] = &["pool.ntp.org"];
const SNTP_ARGS: &[&str] = &["-Ss", "-M", "128"];

// Treat anything before 2024-01-01 UTC as an unsynchronized cold-boot clock.
const MIN_SANE_CLOCK_UNIX_EPOCH_SECS: i64 = 1_704_067_200;

fn min_sane_clock_utc() -> DateTime<Utc> {
    Utc.timestamp_opt(MIN_SANE_CLOCK_UNIX_EPOCH_SECS, 0)
        .single()
        .expect("valid sane clock threshold")
}

pub fn clock_is_sane_at(now_utc: DateTime<Utc>) -> bool {
    now_utc >= min_sane_clock_utc()
}

pub fn system_clock_is_sane() -> bool {
    clock_is_sane_at(Utc::now())
}

fn run_sntp_script() -> Result<ExitStatus> {
    Command::new(SNTP_INIT_SCRIPT)
        .arg("start")
        .status()
        .with_context(|| format!("running {} start", SNTP_INIT_SCRIPT))
}

fn ensure_sntp_key_cache() -> Result<()> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(SNTP_KEY_CACHE)
        .with_context(|| format!("creating {}", SNTP_KEY_CACHE))?;
    Ok(())
}

fn run_direct_sntp() -> Result<ExitStatus> {
    ensure_sntp_key_cache()?;

    let mut cmd = Command::new(SNTP_BIN);
    cmd.args(SNTP_ARGS);
    cmd.arg("-K").arg(SNTP_KEY_CACHE);
    cmd.args(SNTP_SERVERS);
    cmd.status()
        .with_context(|| format!("running direct {}", SNTP_BIN))
}

/// Attempt a one-shot wall-clock sync using the image's Buildroot NTP wiring.
///
/// Returns whether the clock is sane after the attempt completes.
pub fn sync_system_clock(reason: &str) -> Result<bool> {
    let before = Utc::now();
    if clock_is_sane_at(before) {
        info!(
            target: "sys",
            "Wall clock already sane; skipping explicit sync (reason={}, current_utc={})",
            reason,
            before.to_rfc3339()
        );
        return Ok(true);
    }

    info!(
        target: "sys",
        "Attempting wall-clock sync (reason={}, before_utc={})",
        reason,
        before.to_rfc3339()
    );

    let status = match run_sntp_script() {
        Ok(status) => status,
        Err(error) => {
            warn!(
                target: "sys",
                "Failed to invoke {} start: {:#}; falling back to direct sntp",
                SNTP_INIT_SCRIPT,
                error
            );
            run_direct_sntp()?
        }
    };

    let after = Utc::now();
    let sane = clock_is_sane_at(after);

    if status.success() {
        if sane {
            info!(
                target: "sys",
                "Wall-clock sync completed (reason={}, before_utc={}, after_utc={})",
                reason,
                before.to_rfc3339(),
                after.to_rfc3339()
            );
        } else {
            warn!(
                target: "sys",
                "Wall-clock sync command exited successfully but clock is still unsynchronized (reason={}, before_utc={}, after_utc={})",
                reason,
                before.to_rfc3339(),
                after.to_rfc3339()
            );
        }
    } else {
        warn!(
            target: "sys",
            "Wall-clock sync command failed with status {:?} (reason={}, before_utc={}, after_utc={}, sane={})",
            status.code(),
            reason,
            before.to_rfc3339(),
            after.to_rfc3339(),
            sane
        );
    }

    Ok(sane)
}

#[cfg(test)]
mod tests {
    use super::{clock_is_sane_at, min_sane_clock_utc};
    use chrono::{TimeZone, Utc};

    #[test]
    fn clock_sanity_rejects_epoch_start() {
        let epoch = Utc.timestamp_opt(0, 0).single().unwrap();
        assert!(!clock_is_sane_at(epoch));
    }

    #[test]
    fn clock_sanity_accepts_threshold_and_newer_times() {
        let threshold = min_sane_clock_utc();
        assert!(clock_is_sane_at(threshold));

        let newer = Utc
            .with_ymd_and_hms(2026, 4, 23, 19, 41, 2)
            .single()
            .unwrap();
        assert!(clock_is_sane_at(newer));
    }
}
