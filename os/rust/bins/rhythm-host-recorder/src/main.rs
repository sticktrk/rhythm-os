use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rhythm_host_recorder::{
    boot_capture, build_synthesis, collect_boot_id, collect_detail, collect_escalation,
    collect_summary, recorder_root, CollectorPaths, EarlyBootSnapshot, RingConfig, RingWriter,
    TriggerState, EARLY_BOOT_FILE,
};

static STOP: AtomicBool = AtomicBool::new(false);
const VERSION: &str = match option_env!("RHYTHM_BUILD_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Debug)]
struct Options {
    command: String,
    paths: CollectorPaths,
    once: bool,
    config: RingConfig,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("rhythm-host-recorder: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if requests_version(&arguments) {
        println!("rhythm-host-recorder {VERSION}");
        return Ok(());
    }
    let options = parse_args(arguments.into_iter())?;
    match options.command.as_str() {
        "boot-capture" => {
            boot_capture(&options.paths, false).map_err(|error| error.to_string())?;
        }
        "synthesize" => {
            let synthesis =
                build_synthesis(&options.paths.data_dir).map_err(|error| error.to_string())?;
            print!("{synthesis}");
        }
        "run" => run_daemon(options)?,
        command => return Err(format!("unknown command: {command}")),
    }
    Ok(())
}

fn requests_version(arguments: &[String]) -> bool {
    matches!(arguments, [argument] if argument == "--version" || argument == "-V")
}

fn run_daemon(options: Options) -> Result<(), String> {
    install_signal_handlers();
    let boot_id = collect_boot_id(&options.paths)
        .value
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let early_boot_path = recorder_root(&options.paths.data_dir).join(EARLY_BOOT_FILE);
    let early_boot = std::fs::read(&early_boot_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<EarlyBootSnapshot>(&bytes).ok());
    if early_boot
        .as_ref()
        .and_then(|snapshot| snapshot.boot_id.value.as_deref())
        != Some(boot_id.as_str())
    {
        boot_capture(&options.paths, true).map_err(|error| error.to_string())?;
    }

    let mut writer = RingWriter::open(&options.paths.data_dir, &boot_id, options.config.clone())
        .map_err(|error| error.to_string())?;
    let mut triggers = TriggerState::default();
    let summary_interval = Duration::from_secs(options.config.summary_interval_secs.max(1));
    let detail_every = options
        .config
        .detail_interval_secs
        .max(options.config.summary_interval_secs)
        .div_ceil(options.config.summary_interval_secs.max(1));
    let sync_every = options
        .config
        .sync_interval_secs
        .max(options.config.summary_interval_secs)
        .div_ceil(options.config.summary_interval_secs.max(1));
    let mut cycle = 0_u64;
    let mut next_cycle = Instant::now();

    while !STOP.load(Ordering::Relaxed) {
        let cycle_started = Instant::now();
        let (monotonic_ms, sample) = collect_summary(&options.paths, writer.health());
        writer.health_mut().truncated_sources = sample.recorder_health.truncated_sources;
        writer.health_mut().timed_out_sources = sample.recorder_health.timed_out_sources;
        if cycle_started > next_cycle + Duration::from_secs(2) {
            let late_cycles = writer.health().late_cycles.saturating_add(1);
            writer.health_mut().late_cycles = late_cycles;
        }
        if let Err(error) = writer.append(&boot_id, monotonic_ms, "summary", &sample, false) {
            eprintln!("rhythm-host-recorder: summary append failed: {error}");
        }

        if cycle.is_multiple_of(detail_every.max(1)) {
            let detail = collect_detail(&options.paths, false);
            if let Err(error) = writer.append(&boot_id, monotonic_ms, "detail", &detail, false) {
                eprintln!("rhythm-host-recorder: detail append failed: {error}");
            }
        }

        let reasons = triggers.evaluate(
            &sample,
            monotonic_ms,
            options.config.summary_interval_secs.saturating_mul(1000),
        );
        if !reasons.is_empty() {
            let escalation = collect_escalation(&options.paths, reasons);
            if let Err(error) =
                writer.append(&boot_id, monotonic_ms, "escalation", &escalation, true)
            {
                eprintln!("rhythm-host-recorder: escalation append failed: {error}");
            }
        } else if cycle.is_multiple_of(sync_every.max(1)) {
            if let Err(error) = writer.sync() {
                eprintln!("rhythm-host-recorder: sync failed: {error}");
            }
        }

        cycle = cycle.saturating_add(1);
        if options.once {
            break;
        }
        next_cycle += summary_interval;
        while !STOP.load(Ordering::Relaxed) && Instant::now() < next_cycle {
            let remaining = next_cycle
                .checked_duration_since(Instant::now())
                .unwrap_or_default();
            std::thread::sleep(remaining.min(Duration::from_millis(250)));
        }
    }
    writer.sync().map_err(|error| error.to_string())
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut args = args.peekable();
    let command = args
        .peek()
        .filter(|value| !value.starts_with('-'))
        .cloned()
        .unwrap_or_else(|| "run".to_string());
    if args.peek() == Some(&command) {
        args.next();
    }
    let mut paths = CollectorPaths::appliance(PathBuf::from("/data"));
    let mut once = false;
    let mut config = RingConfig::default();
    while let Some(argument) = args.next() {
        let value = |args: &mut std::iter::Peekable<_>, name: &str| {
            args.next()
                .ok_or_else(|| format!("{name} requires a value"))
        };
        match argument.as_str() {
            "--data-dir" => paths.data_dir = PathBuf::from(value(&mut args, "--data-dir")?),
            "--proc-root" => paths.proc_root = PathBuf::from(value(&mut args, "--proc-root")?),
            "--sys-root" => paths.sys_root = PathBuf::from(value(&mut args, "--sys-root")?),
            "--run-root" => paths.run_root = PathBuf::from(value(&mut args, "--run-root")?),
            "--dmesg-command" => {
                paths.dmesg_command = PathBuf::from(value(&mut args, "--dmesg-command")?)
            }
            "--summary-secs" => {
                config.summary_interval_secs = parse_u64(value(&mut args, "--summary-secs")?)?
            }
            "--detail-secs" => {
                config.detail_interval_secs = parse_u64(value(&mut args, "--detail-secs")?)?
            }
            "--sync-secs" => {
                config.sync_interval_secs = parse_u64(value(&mut args, "--sync-secs")?)?
            }
            "--once" => once = true,
            "-h" | "--help" => {
                println!(
                    "Usage: rhythm-host-recorder [run|boot-capture|synthesize] [--data-dir PATH] [--once]\n       rhythm-host-recorder --version"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Options {
        command,
        paths,
        once,
        config,
    })
}

fn parse_u64(value: String) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("invalid positive integer: {value}"))
        .and_then(|value| {
            if value == 0 {
                Err("interval must be greater than zero".to_string())
            } else {
                Ok(value)
            }
        })
}

#[cfg(unix)]
fn install_signal_handlers() {
    extern "C" fn stop(_: libc::c_int) {
        STOP.store(true, Ordering::Relaxed);
    }
    // SAFETY: the handler only performs an async-signal-safe atomic store.
    unsafe {
        libc::signal(libc::SIGTERM, stop as libc::sighandler_t);
        libc::signal(libc::SIGINT, stop as libc::sighandler_t);
        libc::signal(libc::SIGHUP, stop as libc::sighandler_t);
    }
}

#[cfg(not(unix))]
fn install_signal_handlers() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_flag_matches_ota_component_probe_contract() {
        assert!(requests_version(&["--version".to_string()]));
        assert!(requests_version(&["-V".to_string()]));
        assert!(!requests_version(&["run".to_string()]));
        assert!(VERSION.split('.').count() >= 3);
    }
}
