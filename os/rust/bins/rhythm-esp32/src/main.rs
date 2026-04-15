//! Rhythm OS ESP32-C6 Firmware
//!
//! Multi-room adaptive lighting controller using esp-idf.
//! Uses rhythm-core for calculations, scheduling, and event handling.
//!
//! ## Architecture
//!
//! The ESP32 is the autonomous executor. The companion app is the configuration
//! tool. Room/device config is pushed via HTTP REST; the ESP32 persists it
//! to NVS and runs the rhythm tick independently.
//!
//! Boot flow:
//! 1. Init ESP-IDF, LED, NVS, WiFi, NTP
//! 2. Load from NVS: credentials, config, location, rooms, device registry
//! 3. Start HTTP server (REST API for config + state)
//! 4. If credentials exist → connect_sse() (SSE starts, light controller ready)
//! 5. If rooms exist in NVS → ensure_runtime() (tick starts immediately)
//! 6. ESP32 runs autonomously from NVS state
//! 7. App connects via HTTP → GET /api/state, pushes config via PUT endpoints

use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::gpio::{PinDriver, Pull};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::mdns::EspMdns;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sntp::{EspSntp, SntpConf};
use log::{info, warn};

use rhythm_os::event_loop::MotionTimerState;
use rhythm_os::hub::HubEvent;
use rhythm_os::provisioning::{ProvisioningConnectResult, WifiCredentials};
use rhythm_os::state::{AppState, SharedState, WorkItem};

/// Firmware version from Cargo.toml, used in mDNS, BLE, HTTP API, and OTA.
pub const FIRMWARE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// NVS partition clone held for the panic hook. Set early in main(), read by the hook.
/// If NVS isn't initialized yet when a panic fires, `get()` returns `None` and
/// the hook silently skips — `esp_reset_reason()` on next boot still detects the crash.
static NVS_FOR_PANIC: OnceLock<EspDefaultNvsPartition> = OnceLock::new();

/// Install a panic hook that persists the panic message to NVS before the
/// ESP-IDF abort handler reboots the chip.
///
/// Uses only stack buffers — no Rust heap allocation (the allocator may be corrupt).
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        // Format panic message into a stack buffer (no heap alloc)
        let mut buf = [0u8; 200];
        let msg = {
            use core::fmt::Write;
            struct StackWriter<'a> {
                buf: &'a mut [u8],
                pos: usize,
            }
            impl<'a> Write for StackWriter<'a> {
                fn write_str(&mut self, s: &str) -> core::fmt::Result {
                    let bytes = s.as_bytes();
                    let remaining = self.buf.len() - self.pos;
                    let to_copy = bytes.len().min(remaining);
                    if to_copy > 0 {
                        self.buf[self.pos..self.pos + to_copy].copy_from_slice(&bytes[..to_copy]);
                        self.pos += to_copy;
                    }
                    Ok(())
                }
            }

            let mut writer = StackWriter {
                buf: &mut buf,
                pos: 0,
            };
            let _ = write!(writer, "{}", info);
            let len = writer.pos;
            // Safety: we only wrote valid UTF-8 via fmt::Write
            unsafe { core::str::from_utf8_unchecked(&buf[..len]) }
        };

        // Print to UART before attempting NVS (in case NVS write hangs)
        let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("PANIC: {}\n", msg));

        // Persist to NVS if available. Reason 0xFF = sentinel (real reason unknown yet).
        if let Some(nvs) = NVS_FOR_PANIC.get() {
            let _ = storage::save_crash_info(nvs, 0xFF, Some(msg));
        }
    }));
}

mod ble_prov;
pub mod commands;
pub mod diag;
mod http_server;
pub mod hub;
mod led;
mod ota;
pub mod platform;
mod storage;
mod wifi;

// MotionTimerState, handle_hub_event, check_motion_timers, process_work_item
// are now in rhythm_os::event_loop (shared across ESP32, server, etc.)

fn main() -> Result<()> {
    // =========================================================================
    // Phase 1: Basic ESP-IDF init (minimal)
    // =========================================================================
    esp_idf_svc::sys::link_patches();
    diag::init();
    install_panic_hook();

    info!("Rhythm OS ESP32-C6 starting...");

    // =========================================================================
    // Phase 2: Take peripherals and initialize basic services
    // =========================================================================
    let peripherals = Peripherals::take()?;
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // Make NVS available to the panic hook for crash persistence
    let _ = NVS_FOR_PANIC.set(nvs.clone());

    // ---- Boot crash detection ----
    // Check esp_reset_reason() and persist crash info if this was a crash reboot.
    {
        let reset_reason_raw = unsafe { esp_idf_svc::sys::esp_reset_reason() } as u32;
        let reset_reason_s = diag::reset_reason_str(reset_reason_raw);
        diag::log_reset_reason(reset_reason_s);

        // Load any crash info saved by a previous panic hook
        let crash_info = storage::load_crash_info(&nvs);

        if diag::is_crash_reason(reset_reason_raw) {
            if let Some(ref ci) = crash_info {
                if ci.last_reset_reason == 0xFF {
                    // Panic hook fired but didn't know the real reason — update it now
                    let _ = storage::update_crash_reset_reason(&nvs, reset_reason_raw as u8);
                }
            } else {
                // Hardware crash (WDT, brownout) — panic hook didn't fire
                let _ = storage::save_crash_info(&nvs, reset_reason_raw as u8, None);
            }
        }

        // Reload (may have been updated) and populate diag BootInfo
        let crash_info = storage::load_crash_info(&nvs);
        if let Some(ci) = crash_info {
            let crash_reason_str = if ci.last_reset_reason == 0xFF {
                "panic"
            } else {
                diag::reset_reason_str(ci.last_reset_reason as u32)
            };
            diag::set_boot_crash_info(
                reset_reason_s,
                ci.crash_count,
                Some(crash_reason_str),
                ci.last_panic,
                if ci.last_crash_ts > 0 {
                    Some(ci.last_crash_ts)
                } else {
                    None
                },
            );
        } else {
            diag::set_boot_crash_info(reset_reason_s, 0, None, None, None);
        }
    }

    // Initialize WS2812 RGB LED on GPIO8 using RMT channel 0
    #[allow(deprecated)]
    let mut led = led::Ws2812Led::new(peripherals.rmt.channel0, peripherals.pins.gpio8)?;
    led.show(led::LedStatus::Initializing);

    // BOOT button on GPIO9 (active-low with internal pull-up)
    let boot_btn = PinDriver::input(peripherals.pins.gpio9, Pull::Up)?;

    // Initialize shared state with NvsStorage backend
    let nvs_storage = storage::NvsStorage::new(nvs.clone());
    let mut app_state = AppState::default();
    app_state.storage = Some(Box::new(nvs_storage));
    app_state.firmware_version = FIRMWARE_VERSION;
    app_state.platform_type = "embedded";
    app_state.platform_context = "esp32";
    app_state.platform = rhythm_os::state::PlatformConfig::embedded();
    rhythm_os::storage::load_persisted_state(&mut app_state);

    // Set platform-specific callbacks
    app_state.ensure_runtime_fn = Some(Arc::new(|state: &SharedState| {
        let bridge_ip = {
            let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            s.hub_credentials
                .values()
                .next()
                .map(|c| c.address.clone())
                .ok_or_else(|| anyhow::anyhow!("No hub credentials configured"))?
        };
        let transport = platform::hue::client::HueClient::new(bridge_ip);
        rhythm_hue::embedded_lifecycle::ensure_runtime(state, transport)
    }));
    app_state.get_hub_provider_fn = Some(Arc::new(|hub_type| hub::get_hub_provider(hub_type)));
    app_state.on_hub_heartbeat = Some(Arc::new(|| {
        diag::vitals_hub_heartbeat(rhythm_os::hub::HubType::new(rhythm_os::hub::HubType::HUE));
    }));
    app_state.on_hub_disconnect = Some(Arc::new(|| {
        diag::vitals_hub_conn_state(
            rhythm_os::hub::HubType::new(rhythm_os::hub::HubType::HUE),
            diag::CONN_DISCONNECTED,
        );
    }));

    let state = Arc::new(Mutex::new(app_state));

    // =========================================================================
    // Phase 3: Connect WiFi (compile-time → NVS → BLE provisioning)
    // =========================================================================

    // Strategy 1: Compile-time env vars (dev fallback)
    let compile_ssid = option_env!("WIFI_SSID");
    let compile_pass = option_env!("WIFI_PASS");

    // Strategy 2: NVS-persisted credentials
    let nvs_creds = storage::load_wifi_credentials(&nvs);

    let (wifi_ssid, wifi_pass, modem) = if let (Some(ssid), Some(pass)) =
        (compile_ssid, compile_pass)
    {
        // Dev mode: use compile-time credentials, skip BLE entirely
        info!("Using compile-time WiFi credentials: {}", ssid);
        (ssid.to_string(), pass.to_string(), peripherals.modem)
    } else if let Some((ssid, pass)) = nvs_creds {
        info!("Using NVS WiFi credentials: {}", ssid);
        (ssid, pass, peripherals.modem)
    } else {
        // No credentials available — enter BLE provisioning with BLE+WiFi coexistence.
        //
        // Split the modem so BLE and WiFi can run simultaneously on ESP32-C6.
        // The WiFi thread connects to verify credentials and report the IP back
        // over BLE. After BLE teardown, the main thread reconnects permanently.
        info!("No WiFi credentials found, entering BLE provisioning...");

        let (wifi_modem, _thread_modem, bt_modem) = peripherals.modem.split();
        drop(wifi_modem); // WiFi thread will steal its own WifiModem

        // Channels: BLE → WiFi thread (credentials), WiFi thread → BLE (result)
        let (cred_tx, cred_rx) = std::sync::mpsc::channel::<WifiCredentials>();
        let (wifi_tx, wifi_rx) = std::sync::mpsc::channel::<ProvisioningConnectResult>();

        // Spawn WiFi provisioning thread
        let prov_sysloop = sysloop.clone();
        let prov_nvs = nvs.clone();
        if let Err(e) = thread::Builder::new()
            .name("wifi-prov".to_string())
            .stack_size(8 * 1024)
            .spawn(move || {
                // Wait for credentials from BLE
                while let Ok(creds) = cred_rx.recv() {
                    info!("WiFi thread: attempting connection to '{}'", creds.ssid);

                    // Steal a WifiModem — safe: ZST marker, BLE holds BluetoothModem
                    let modem = unsafe { esp_idf_svc::hal::modem::WifiModem::steal() };

                    match wifi::connect_wifi(
                        modem,
                        prov_sysloop.clone(),
                        prov_nvs.clone(),
                        &creds.ssid,
                        &creds.password,
                    ) {
                        Ok(wifi) => {
                            let ip = wifi
                                .wifi()
                                .sta_netif()
                                .get_ip_info()
                                .map(|info| format!("{}", info.ip))
                                .unwrap_or_else(|_| "unknown".to_string());

                            let _ = wifi_tx.send(ProvisioningConnectResult::Connected { ip });

                            // Drop the temporary WiFi — main thread will reconnect permanently
                            drop(wifi);
                            return;
                        }
                        Err(e) => {
                            warn!("WiFi thread: connection failed: {:?}", e);
                            let _ = wifi_tx.send(ProvisioningConnectResult::Failed {
                                error: format!("{}", e),
                            });
                            // Loop to wait for retry credentials
                        }
                    }
                }
            })
        {
            warn!("Failed to spawn wifi-prov thread: {}", e);
            return Err(anyhow::anyhow!("WiFi provisioning thread spawn failed"));
        }

        let creds = ble_prov::run_provisioning(bt_modem, nvs.clone(), &mut led, cred_tx, wifi_rx)?;

        // Save to NVS for next boot
        if let Err(e) = storage::save_wifi_credentials(&nvs, &creds.ssid, &creds.password) {
            warn!("Failed to save WiFi credentials to NVS: {:?}", e);
        }

        // Recreate full modem for permanent WiFi connection after BLE teardown
        let modem = unsafe { esp_idf_svc::hal::modem::Modem::steal() };
        (creds.ssid, creds.password, modem)
    };

    // Release Bluetooth controller memory back to the heap (~60-70KB).
    // BLE is either never started (NVS/compile-time creds) or already torn down.
    unsafe {
        let ret =
            esp_idf_svc::sys::esp_bt_mem_release(esp_idf_svc::sys::esp_bt_mode_t_ESP_BT_MODE_BTDM);
        if ret == esp_idf_svc::sys::ESP_OK {
            let free = esp_idf_svc::sys::esp_get_free_heap_size();
            info!("Released BT memory, free heap now: {}KB", free / 1024);
        } else {
            warn!("esp_bt_mem_release failed: {}", ret);
        }
    }

    info!("Connecting to WiFi: {}", &wifi_ssid);
    led.show(led::LedStatus::WifiConnecting);

    let _wifi =
        match wifi::connect_wifi(modem, sysloop.clone(), nvs.clone(), &wifi_ssid, &wifi_pass) {
            Ok(wifi) => {
                led.show(led::LedStatus::WifiConnected);
                wifi
            }
            Err(e) => {
                warn!("WiFi connection failed: {:?}", e);

                // If we had NVS credentials that failed, clear them and restart
                // to re-enter BLE provisioning
                if compile_ssid.is_none() {
                    info!("Clearing failed NVS credentials and restarting...");
                    let _ = storage::clear_wifi_credentials(&nvs);
                    led.show(led::LedStatus::Error);
                    thread::sleep(Duration::from_secs(2));
                    unsafe { esp_idf_svc::sys::esp_restart() };
                }

                led.show(led::LedStatus::Error);
                return Err(e);
            }
        };

    info!("WiFi connected!");

    // Persist compile-time credentials to NVS so they survive OTA updates.
    // BLE provisioning already saves to NVS; this covers the dev env-var path.
    if compile_ssid.is_some() {
        if let Err(e) = storage::save_wifi_credentials(&nvs, &wifi_ssid, &wifi_pass) {
            warn!("Failed to persist WiFi credentials to NVS: {:?}", e);
        }
    }

    // Start mDNS so the device is discoverable as rhythm-XXYY.local
    let _mdns = {
        let mac = ble_prov::get_mac_address();
        let hostname = format!(
            "{}{:02x}{:02x}",
            rhythm_os::mdns::MDNS_HOSTNAME_PREFIX,
            mac[4],
            mac[5]
        );

        match EspMdns::take() {
            Ok(mut mdns) => {
                if let Err(e) = mdns.set_hostname(&hostname) {
                    warn!("mDNS: failed to set hostname: {:?}", e);
                }
                if let Err(e) = mdns.set_instance_name(rhythm_os::mdns::MDNS_INSTANCE_NAME) {
                    warn!("mDNS: failed to set instance name: {:?}", e);
                }
                if let Err(e) = mdns.add_service(
                    Some(rhythm_os::mdns::MDNS_INSTANCE_NAME),
                    rhythm_os::mdns::MDNS_SERVICE_TYPE,
                    rhythm_os::mdns::MDNS_SERVICE_PROTO,
                    80,
                    &[
                        (rhythm_os::mdns::MDNS_TXT_VERSION, FIRMWARE_VERSION),
                        (rhythm_os::mdns::MDNS_TXT_TYPE, "rhythm-esp32"),
                    ],
                ) {
                    warn!("mDNS: failed to register HTTP service: {:?}", e);
                }
                info!("mDNS: advertising as {}.local", hostname);
                Some(mdns)
            }
            Err(e) => {
                warn!("mDNS: failed to initialize: {:?}", e);
                None
            }
        }
    };

    // =========================================================================
    // Phase 4: NTP, HTTP server, main loop
    // =========================================================================
    info!("Initializing NTP time sync...");
    led.show(led::LedStatus::NtpSyncing);
    let sntp_conf = SntpConf::default();
    let _sntp = EspSntp::new(&sntp_conf)?;

    // Wait for time sync (with timeout)
    let mut sync_attempts = 0;
    while !is_time_synced() && sync_attempts < 30 {
        thread::sleep(Duration::from_secs(1));
        sync_attempts += 1;
    }

    if is_time_synced() {
        info!("NTP time synchronized");
        led.show(led::LedStatus::NtpSynced);

        // Record boot time for uptime calculation
        if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            diag::vitals_set_boot_time(d.as_secs() as u32);
        }
    } else {
        warn!("NTP time sync timed out, using default time");
        led.show(led::LedStatus::Error);
    }

    // Start HTTP server for configuration API
    info!("Starting HTTP server...");
    led.show(led::LedStatus::ServerStarting);
    let server_state = state.clone();
    let _server = http_server::start_server(server_state, nvs.clone())?;

    info!("HTTP server started on port 80");

    // Mark the running OTA slot as valid now that WiFi + NTP + HTTP are confirmed.
    // If rollback is enabled and this is a new OTA firmware, this prevents the
    // bootloader from reverting to the previous partition on next reboot.
    {
        let mut ota = esp_idf_svc::ota::EspOta::new()
            .map_err(|e| anyhow::anyhow!("OTA init failed: {}", e))?;
        if let Err(e) = ota.mark_running_slot_valid() {
            warn!("OTA: Failed to mark slot valid (not an OTA boot?): {:?}", e);
        } else {
            info!("OTA: Running slot marked valid");
        }
    }

    // Show ready status
    led.show(led::LedStatus::Ready);

    info!("Rhythm OS ready!");

    // Suppress noisy WiFi ADDBA negotiation logs during normal runtime
    // (keep them at INFO during boot for diagnostics)
    unsafe {
        esp_idf_svc::sys::esp_log_level_set(
            b"wifi\0".as_ptr() as *const _,
            esp_idf_svc::sys::esp_log_level_t_ESP_LOG_WARN,
        );
    }
    info!("Endpoints:");
    info!("  GET    /health - Health check");
    info!("  GET    /api/state - Full state snapshot");
    info!("  GET    /api/rooms/state - Room state for polling");
    info!("  PUT    /api/rooms - Upsert room(s)");
    info!("  PUT    /api/config - Save light profile config");
    info!("  GET    /api/ota/version - Firmware version");
    info!("  POST   /api/ota/upload - OTA firmware upload");

    // Spawn cmd-worker thread for:
    // - ButtonAction: hub-originated actions (SSE events, button presses)
    // - DeferredPersist: NVS save after inline button processing
    // - DeferredPersistState: batched persistence work
    //
    // Physical button/motion events are processed inline on the main thread
    // (16KB stack, same as worker) for zero queue delay.
    let (work_tx, work_rx) = std::sync::mpsc::sync_channel::<WorkItem>(16);
    let (periodic_tx, periodic_rx) = std::sync::mpsc::sync_channel::<WorkItem>(16);
    match state.lock() {
        Ok(mut s) => {
            s.work_tx = Some(work_tx);
            s.periodic_work_tx = Some(periodic_tx);
        }
        Err(_) => {
            log::error!("State mutex poisoned during work_tx init — restarting");
            unsafe { esp_idf_svc::sys::esp_restart() };
        }
    }

    let worker_state = state.clone();
    if let Err(e) = thread::Builder::new()
        .name("cmd-worker".to_string())
        .stack_size(16 * 1024)
        .spawn(move || {
            info!(target: "sys", "cmd-worker started");
            loop {
                match work_rx.recv() {
                    Ok(item) => rhythm_os::event_loop::process_work_item(&worker_state, item),
                    Err(_) => {
                        warn!(target: "sys", "cmd-worker: channel disconnected");
                        return;
                    }
                }
            }
        })
    {
        log::error!("Failed to spawn cmd-worker thread: {} — restarting", e);
        unsafe { esp_idf_svc::sys::esp_restart() };
    }

    let periodic_state = state.clone();
    if let Err(e) = thread::Builder::new()
        .name("periodic-worker".to_string())
        .stack_size(16 * 1024)
        .spawn(move || {
            info!(target: "sys", "periodic-worker started");
            loop {
                match periodic_rx.recv() {
                    Ok(item) => rhythm_os::event_loop::process_work_item(&periodic_state, item),
                    Err(_) => {
                        warn!(target: "sys", "periodic-worker: channel disconnected");
                        return;
                    }
                }
            }
        })
    {
        log::error!("Failed to spawn periodic-worker thread: {} — restarting", e);
        unsafe { esp_idf_svc::sys::esp_restart() };
    }

    // =========================================================================
    // Phase 5: Initialize Hub (if configured)
    // =========================================================================
    let mut hub_event_rx: Option<std::sync::mpsc::Receiver<HubEvent>> = {
        let is_configured = state
            .lock()
            .map(|s| s.hub_credentials.values().any(|c| c.is_configured()))
            .unwrap_or(false);

        if is_configured {
            info!("Hub configured, connecting SSE...");
            diag::vitals_hub_register(rhythm_os::hub::HubType::new(rhythm_os::hub::HubType::HUE));

            // Run SSE connect + runtime init in one thread (saves a 16KB stack allocation)
            let init_state = state.clone();
            let event_state = state.clone();
            let init_result = match thread::Builder::new()
                .name("hub-init".to_string())
                .stack_size(16 * 1024)
                .spawn(move || -> Result<std::sync::mpsc::Receiver<HubEvent>> {
                    rhythm_hue::embedded_lifecycle::connect_and_start(
                        init_state,
                        |ip| Ok(platform::hue::client::HueClient::new(ip.to_string())),
                        |config, registry, shutdown| {
                            platform::hue::start_event_stream(
                                config,
                                registry,
                                shutdown,
                                event_state,
                            )
                        },
                    )
                }) {
                Ok(handle) => handle
                    .join()
                    .map_err(|_| anyhow::anyhow!("Hub init thread panicked"))?,
                Err(e) => {
                    warn!(
                        "Failed to spawn hub init thread: {} — continuing without hub",
                        e
                    );
                    Err(anyhow::anyhow!("spawn failed"))
                }
            };

            match init_result {
                Ok(event_rx) => {
                    // Poll initial light state so the first API response shows
                    // correct on/off status instead of defaulting to all-off.
                    // Runs on a dedicated thread (TLS calls need 16KB stack).
                    let poll_state = state.clone();
                    if let Err(e) = thread::Builder::new()
                        .name("init-poll".to_string())
                        .stack_size(16 * 1024)
                        .spawn(move || {
                            rhythm_os::room_sync::poll_initial_light_state(&poll_state);
                        })
                    {
                        warn!(
                            "Failed to spawn init-poll thread: {} — skipping initial poll",
                            e
                        );
                    }
                    Some(event_rx)
                }
                Err(e) => {
                    warn!(
                        "Failed to connect SSE: {} (will retry when app pushes credentials)",
                        e
                    );
                    None
                }
            }
        } else {
            info!("No hub configured, waiting for app to push credentials via HTTP");
            None
        }
    };

    // Spawn periodic update thread with larger stack for float formatting
    let periodic_state = state.clone();
    if let Err(e) = thread::Builder::new()
        .name("periodic".to_string())
        .stack_size(16 * 1024) // 16KB stack: periodic_tick makes HTTPS calls to Hue bridge
        .spawn(move || {
            rhythm_os::periodic::run_periodic_loop(periodic_state, Some(|| led::led_periodic()));
        })
    {
        log::error!("Failed to spawn periodic thread: {} — restarting", e);
        unsafe { esp_idf_svc::sys::esp_restart() };
    }

    // Button state for BOOT button LED color cycling + factory reset
    let mut btn_was_pressed = false;
    let mut btn_hold_ticks: u32 = 0;
    let mut led_color_index: u8 = 0;

    // Motion timer state (hub-agnostic)
    let mut motion_state = MotionTimerState::new();
    let mut motion_tick: u32 = 0;
    let mut motion_dirty = false;

    // Main loop - keep the server alive, handle LED flashes, process events
    loop {
        // BOOT button: active-low (pressed = low)
        // Each tick = 50ms, so 40 ticks = 2s (warning), 100 ticks = 5s (reset)
        let btn_pressed = boot_btn.is_low();
        if btn_pressed {
            btn_hold_ticks += 1;

            if !btn_was_pressed {
                // Press edge: cycle LED color
                led_color_index = (led_color_index + 1) % 6;
                let color = match led_color_index {
                    0 => led::StatusColor::DIM_RED,
                    1 => led::StatusColor::DIM_GREEN,
                    2 => led::StatusColor::DIM_BLUE,
                    3 => led::StatusColor::DIM_YELLOW,
                    4 => led::StatusColor::DIM_CYAN,
                    5 => led::StatusColor::DIM_PURPLE,
                    _ => led::StatusColor::OFF,
                };
                led.set_color(color).ok();
            } else if btn_hold_ticks == 40 {
                // 2s hold: solid red warning
                log::warn!("Factory reset warning - keep holding for 3 more seconds");
                led.set_color(led::StatusColor::DIM_RED).ok();
            } else if btn_hold_ticks >= 100 {
                // 5s hold: factory reset
                log::warn!("Factory reset triggered! Clearing NVS and restarting...");
                led.blink(led::StatusColor::DIM_RED, 10, 50, 50);
                if let Err(e) = storage::clear_config(&nvs) {
                    log::error!("Failed to clear NVS: {}", e);
                }
                unsafe { esp_idf_svc::sys::esp_restart() };
            }
        } else if btn_was_pressed {
            // Released: reset hold counter, turn LED off
            btn_hold_ticks = 0;
            led.set_color(led::StatusColor::OFF).ok();
        }
        btn_was_pressed = btn_pressed;

        // Check for pending LED activity flashes from other threads
        if let Some(color) = led::take_pending_flash() {
            led.blink(color, 1, 30, 0);
        }

        // Pick up new hub event receiver(s) from reconfiguration
        if let Ok(mut s) = state.lock() {
            if let Some(new_rx) = s.pending_hub_event_rxs.pop() {
                hub_event_rx = Some(new_rx);
                // Clear stale motion state from old hub
                motion_state.sensors.clear();
                motion_state.motion_owned.clear();
                motion_state.warning_active.clear();
            }
        }

        // Process hub events (hub-agnostic)
        if let Some(ref event_rx) = hub_event_rx {
            loop {
                match event_rx.try_recv() {
                    Ok(event) => {
                        rhythm_os::event_loop::handle_hub_event(&state, event, &mut motion_state);
                        motion_dirty = true;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        warn!("Hub event channel disconnected — dropping dead receiver");
                        hub_event_rx = None;
                        break;
                    }
                }
            }
        }

        // Periodic motion timer check (every ~30s = 600 * 50ms)
        motion_tick += 1;
        if motion_tick >= 600 {
            motion_tick = 0;
            if !motion_state.sensors.is_empty() {
                rhythm_os::event_loop::check_motion_timers(&state, &mut motion_state);
                motion_dirty = true;
            }
        }

        // Sync motion snapshots to AppState when dirty
        if motion_dirty {
            motion_dirty = false;
            rhythm_os::event_loop::sync_motion_snapshots(&state, &motion_state);
        }

        thread::sleep(Duration::from_millis(50));
    }
}

// run_periodic_updates and check_solar_midnight are now in rhythm_os::periodic

/// Check if NTP time has been synchronized.
fn is_time_synced() -> bool {
    use std::time::{SystemTime, UNIX_EPOCH};

    if let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) {
        let secs = duration.as_secs();
        return secs > 1577836800;
    }
    false
}

/// Get current hour as float (0-24) in UTC.
pub fn get_current_hour_utc() -> f32 {
    use std::time::{SystemTime, UNIX_EPOCH};

    if let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) {
        let secs = duration.as_secs();
        let seconds_in_day = secs % 86400;
        let hour = seconds_in_day / 3600;
        let minute = (seconds_in_day % 3600) / 60;
        return hour as f32 + minute as f32 / 60.0;
    }
    12.0
}

/// Get current hour as float (0-24) in local time.
pub fn get_current_hour_local(utc_offset_hours: f32) -> f32 {
    let utc_hour = get_current_hour_utc();
    let local_hour = utc_hour + utc_offset_hours;
    if local_hour < 0.0 {
        local_hour + 24.0
    } else if local_hour >= 24.0 {
        local_hour - 24.0
    } else {
        local_hour
    }
}
