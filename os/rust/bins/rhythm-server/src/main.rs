//! Rhythm OS — macOS/Linux CLI server.
//!
//! Same API surface as the ESP32 firmware, running as a native process.
//! Connects to Hue bridges, Home Assistant, and future hubs via the
//! integration registry in `hub::INTEGRATIONS`.

mod http_server;
mod hub;
mod self_update;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap::Parser;
use log::{info, warn};
use rhythm_os::state::{AppState, SharedState};
use rhythm_os::storage::FileStorage;

/// Format tracing timestamps in local time instead of UTC.
struct LocalTimer;
impl tracing_subscriber::fmt::time::FormatTime for LocalTimer {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        write!(
            w,
            "{}",
            chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.6f")
        )
    }
}

/// Rhythm OS server for macOS/Linux.
#[derive(Parser, Debug)]
#[command(name = "rhythm-server", version, about)]
struct Args {
    /// HTTP server port.
    #[arg(short, long, default_value_t = 54448)]
    port: u16,

    /// Data directory for persistent storage.
    #[arg(short, long, default_value = "~/.rhythm")]
    data_dir: String,

    /// Log level (trace, debug, info, warn, error).
    #[arg(long, default_value = "info")]
    log_level: String,
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> Result<()> {
    let args = Args::parse();

    // Init logging — always suppress noisy mdns-sd AAAA errors
    let base_filter = std::env::var("RUST_LOG").unwrap_or_else(|_| args.log_level.clone());
    tracing_subscriber::fmt()
        .with_timer(LocalTimer)
        .with_env_filter(tracing_subscriber::EnvFilter::new(format!(
            "{},mdns_sd=off",
            base_filter
        )))
        .init();

    info!(target: "sys", "Rhythm Server v{} starting...", VERSION);

    // Expand ~ in data dir
    let data_dir = shellexpand(&args.data_dir);
    std::fs::create_dir_all(&data_dir)?;

    // Create storage backend
    let file_storage = FileStorage::new(&data_dir)?;

    // Build shared state
    let (event_tx, _) = tokio::sync::broadcast::channel(64);
    let state: SharedState = Arc::new(Mutex::new(AppState::default()));
    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.event_tx = Some(event_tx);
        s.firmware_version = Box::leak(VERSION.to_string().into_boxed_str());
        s.listen_port = Some(args.port);
        s.storage = Some(Box::new(file_storage));

        // Load persisted state
        rhythm_os::storage::load_persisted_state(&mut s);

        // Set integration-driven callbacks from the static registry
        let callbacks = rhythm_os::hub::integration_callbacks(hub::INTEGRATIONS);
        s.ensure_runtime_fn = Some(callbacks.ensure_runtime_fn);
        s.get_hub_provider_fn = Some(callbacks.get_hub_provider_fn);
        s.register_controller_fn = Some(callbacks.register_controller_fn);
        s.start_pairing_fn = Some(callbacks.start_pairing_fn);

        // Credentials interceptor: delegates to integrations
        s.hub_credentials_interceptor = Some(rhythm_os::hub::combined_credentials_interceptor(
            hub::INTEGRATIONS,
        ));
    }

    info!(target: "sys", "Data directory: {}", data_dir);

    // Connect to all configured hubs.
    // Must happen BEFORE the tokio runtime starts — reqwest::blocking::Client
    // cannot be created inside an async context.
    let hub_event_rxs = {
        let all_creds: Vec<(rhythm_os::canonical::identity::HubKey, String)> = {
            let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            s.hub_credentials
                .iter()
                .filter_map(|(key, creds)| {
                    creds
                        .hub_type
                        .as_ref()
                        .map(|ht| (key.clone(), ht.as_str().to_string()))
                })
                .collect()
        };

        if all_creds.is_empty() {
            info!(target: "sys", "No hub configured, waiting for credentials via HTTP");
        }

        // Refresh credentials before connecting (e.g., rotating tokens)
        for (key, hub_type_str) in &all_creds {
            if let Some(integration) = hub::find_integration(hub::INTEGRATIONS, hub_type_str) {
                integration.refresh_credentials(&state, key);
            }
        }

        let mut rxs = Vec::new();
        for (key, hub_type_str) in &all_creds {
            if let Some(integration) = hub::find_integration(hub::INTEGRATIONS, hub_type_str) {
                info!(target: "sys", "Connecting hub {} (type={})...", key, hub_type_str);
                match integration.connect_and_start(state.clone(), key) {
                    Ok(rx) => rxs.push(rx),
                    Err(e) => {
                        warn!(target: "sys", "Failed to connect hub {}: {} (will retry on credential push)", key, e);
                    }
                }
            } else {
                warn!(target: "sys", "No integration for hub type '{}', skipping {}", hub_type_str, key);
            }
        }
        rxs
    };

    // Auto-sync rooms from all connected hubs (catches changes made while stopped).
    // Must happen after connect_and_start so the ActiveHubs are available.
    {
        let has_hub = state.lock().map(|s| s.has_any_hub()).unwrap_or(false);
        if has_hub {
            if let Err(e) = rhythm_os::room_sync::sync_all_hubs(&state) {
                warn!(target: "sys", "Initial room sync failed: {}", e);
            }
            rhythm_os::room_sync::poll_initial_light_state(&state);

            // Run integration-specific post-connect for each hub
            let hub_entries: Vec<(String, rhythm_os::canonical::identity::HubKey)> = {
                let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
                s.hub_credentials
                    .iter()
                    .filter_map(|(key, creds)| {
                        creds
                            .hub_type
                            .as_ref()
                            .map(|ht| (ht.as_str().to_string(), key.clone()))
                    })
                    .collect()
            };
            for (ht, key) in &hub_entries {
                if let Some(i) = hub::find_integration(hub::INTEGRATIONS, ht) {
                    i.post_connect(&state, key);
                }
            }
        }
    }

    // Spawn event loop on a dedicated std::thread (not tokio)
    {
        let event_state = state.clone();
        std::thread::Builder::new()
            .name("event-loop".to_string())
            .spawn(move || {
                rhythm_os::event_loop::run_event_loop(event_state, hub_event_rxs);
            })
            .expect("Failed to spawn event loop thread");
    }

    // Spawn periodic updater on a dedicated std::thread
    {
        let periodic_state = state.clone();
        std::thread::Builder::new()
            .name("periodic".to_string())
            .spawn(move || {
                rhythm_os::periodic::run_periodic_loop(periodic_state, None::<fn()>);
            })
            .expect("Failed to spawn periodic thread");
    }

    // Start tokio runtime for the async HTTP server + mDNS
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run_server(state, args.port))
}

async fn run_server(state: SharedState, port: u16) -> Result<()> {
    // Start HTTP server
    let addr = format!("0.0.0.0:{}", port);
    info!(target: "sys", "Starting HTTP server on {}", addr);

    let server = http_server::create_router(state.clone());
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| {
            panic!(
                "Failed to bind to {} — is another process using this port? {}",
                addr, e
            )
        });
    info!(target: "sys", "Rhythm Server listening on http://{}", addr);

    // Register mDNS service for auto-discovery by clients
    let _mdns = rhythm_os::mdns::register_mdns_service(port, "server", VERSION, "rhythm-server");

    axum::serve(listener, server).await?;

    Ok(())
}

/// Expand ~ to home directory.
fn shellexpand(path: &str) -> String {
    if path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{}{}", home, &path[1..]);
        }
    }
    path.to_string()
}
