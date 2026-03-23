//! Diagnostic logging system for ESP32.
//!
//! Provides an in-memory ring buffer of log entries, per-hub connection vitals,
//! and global command statistics. All data is accessible via HTTP endpoints
//! (`/api/diag/logs` and `/api/diag/vitals`) for remote monitoring.
//!
//! The custom `log::Log` implementation intercepts all `log!` macro calls and
//! routes them to both the ESP-IDF UART logger and the ring buffer. Log targets
//! (`target: "conn"`, `target: "cmd"`, etc.) are used to categorize entries.

use core::fmt::Write as FmtWrite;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

// ============================================================================
// Constants
// ============================================================================

const DIAG_BUF_SIZE: usize = 10;
const MSG_MAX_LEN: usize = 200;
const MAX_HUBS: usize = 4;

// Connection states (used in HubVitals.conn_state)
pub const CONN_DISCONNECTED: u8 = 0;
pub const CONN_CONNECTING: u8 = 1;
pub const CONN_CONNECTED: u8 = 2;

// ============================================================================
// DiagLevel / DiagCategory
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagLevel {
    Error,
    Warn,
    Info,
}

impl DiagLevel {
    fn from_log_level(level: log::Level) -> Option<Self> {
        match level {
            log::Level::Error => Some(DiagLevel::Error),
            log::Level::Warn => Some(DiagLevel::Warn),
            log::Level::Info => Some(DiagLevel::Info),
            _ => None, // Skip Debug/Trace
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagCategory {
    Conn,
    Hub,
    Cmd,
    Evt,
    Sys,
}

impl DiagCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            DiagCategory::Conn => "conn",
            DiagCategory::Hub => "hub",
            DiagCategory::Cmd => "cmd",
            DiagCategory::Evt => "evt",
            DiagCategory::Sys => "sys",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "conn" => Some(DiagCategory::Conn),
            "hub" => Some(DiagCategory::Hub),
            "cmd" => Some(DiagCategory::Cmd),
            "evt" => Some(DiagCategory::Evt),
            "sys" => Some(DiagCategory::Sys),
            _ => None,
        }
    }

    /// Infer category from a log record's target (module path).
    fn from_target(target: &str) -> Self {
        // Explicit targets set via `log!(target: "conn", ...)`
        if let Some(cat) = Self::from_str(target) {
            return cat;
        }

        // Module path fallback
        if target.contains("hue_sse") {
            DiagCategory::Conn
        } else if target.contains("hue_client") {
            DiagCategory::Hub
        } else if target.contains("hue_controller") || target.contains("http_server") {
            DiagCategory::Cmd
        } else if target.contains("hue_hub") {
            DiagCategory::Evt
        } else {
            DiagCategory::Sys
        }
    }
}

// ============================================================================
// FixedString — no-alloc message storage
// ============================================================================

#[derive(Clone)]
struct FixedString {
    buf: [u8; MSG_MAX_LEN],
    len: u8,
}

impl FixedString {
    const fn new() -> Self {
        Self {
            buf: [0u8; MSG_MAX_LEN],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        // Safety: we only write valid UTF-8 via fmt::Write
        unsafe { core::str::from_utf8_unchecked(&self.buf[..self.len as usize]) }
    }
}

impl FmtWrite for FixedString {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let remaining = MSG_MAX_LEN - self.len as usize;
        let bytes = s.as_bytes();
        let to_copy = bytes.len().min(remaining);
        if to_copy > 0 {
            self.buf[self.len as usize..self.len as usize + to_copy]
                .copy_from_slice(&bytes[..to_copy]);
            self.len += to_copy as u8;
        }
        Ok(())
    }
}

impl Serialize for FixedString {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

// ============================================================================
// DiagEntry
// ============================================================================

#[derive(Clone, Serialize)]
pub struct DiagEntry {
    pub ts: u32,
    pub level: DiagLevel,
    pub cat: DiagCategory,
    msg: FixedString,
}

impl DiagEntry {
    const fn empty() -> Self {
        Self {
            ts: 0,
            level: DiagLevel::Info,
            cat: DiagCategory::Sys,
            msg: FixedString::new(),
        }
    }
}

// ============================================================================
// Ring Buffer
// ============================================================================

struct DiagRingBuf {
    entries: [DiagEntry; DIAG_BUF_SIZE],
    head: usize,
    count: usize,
}

impl DiagRingBuf {
    const fn new() -> Self {
        // Use a const block to repeat DiagEntry::empty() without Copy
        const EMPTY: DiagEntry = DiagEntry::empty();
        Self {
            entries: [EMPTY; DIAG_BUF_SIZE],
            head: 0,
            count: 0,
        }
    }

    fn push(&mut self, entry: DiagEntry) {
        self.entries[self.head] = entry;
        self.head = (self.head + 1) % DIAG_BUF_SIZE;
        if self.count < DIAG_BUF_SIZE {
            self.count += 1;
        }
    }

    /// Get entries in reverse chronological order (newest first).
    /// Filters by optional category and since-timestamp.
    /// Returns at most `limit` entries.
    fn get_filtered(
        &self,
        category: Option<DiagCategory>,
        since: Option<u32>,
        limit: usize,
    ) -> Vec<DiagEntry> {
        let mut result = Vec::with_capacity(limit.min(self.count));
        let mut idx = if self.head == 0 {
            DIAG_BUF_SIZE - 1
        } else {
            self.head - 1
        };

        for _ in 0..self.count {
            let entry = &self.entries[idx];

            let cat_match = category.map_or(true, |c| entry.cat == c);
            let since_match = since.map_or(true, |s| entry.ts >= s);

            if cat_match && since_match {
                result.push(entry.clone());
                if result.len() >= limit {
                    break;
                }
            }

            if idx == 0 {
                idx = DIAG_BUF_SIZE - 1;
            } else {
                idx -= 1;
            }
        }

        result
    }
}

static DIAG_BUF: Mutex<DiagRingBuf> = Mutex::new(DiagRingBuf::new());

// ============================================================================
// System Vitals (global atomics)
// ============================================================================

struct SystemVitals {
    boot_time: AtomicU32,
    last_cmd_result: AtomicU8, // 0=none, 1=ok, 2=error
    last_cmd_ts: AtomicU32,
    cmd_ok_count: AtomicU32,
    cmd_err_count: AtomicU32,
}

impl SystemVitals {
    const fn new() -> Self {
        Self {
            boot_time: AtomicU32::new(0),
            last_cmd_result: AtomicU8::new(0),
            last_cmd_ts: AtomicU32::new(0),
            cmd_ok_count: AtomicU32::new(0),
            cmd_err_count: AtomicU32::new(0),
        }
    }
}

static VITALS: SystemVitals = SystemVitals::new();

// ============================================================================
// Boot / Crash Info (populated once at startup, cleared on user dismiss)
// ============================================================================

/// Crash and reset info populated at boot, served in vitals.
pub struct BootInfo {
    pub reset_reason: &'static str,
    pub crash_count: Option<u32>,
    pub last_crash_reset_reason: Option<&'static str>,
    pub last_panic_msg: Option<String>,
    pub last_crash_ts: Option<u32>,
}

impl BootInfo {
    const fn empty() -> Self {
        Self {
            reset_reason: "unknown",
            crash_count: None,
            last_crash_reset_reason: None,
            last_panic_msg: None,
            last_crash_ts: None,
        }
    }
}

static BOOT_INFO: Mutex<BootInfo> = Mutex::new(BootInfo::empty());

/// Populate boot crash info (call once at startup after loading NVS crash data).
pub fn set_boot_crash_info(
    reset_reason: &'static str,
    crash_count: u32,
    last_crash_reset_reason: Option<&'static str>,
    last_panic_msg: Option<String>,
    last_crash_ts: Option<u32>,
) {
    if let Ok(mut info) = BOOT_INFO.lock() {
        info.reset_reason = reset_reason;
        if crash_count > 0 {
            info.crash_count = Some(crash_count);
            info.last_crash_reset_reason = last_crash_reset_reason;
            info.last_panic_msg = last_panic_msg;
            info.last_crash_ts = last_crash_ts;
        }
    }
}

/// Clear boot crash info (called when user dismisses crash card).
pub fn clear_boot_crash_info() {
    if let Ok(mut info) = BOOT_INFO.lock() {
        info.crash_count = None;
        info.last_crash_reset_reason = None;
        info.last_panic_msg = None;
        info.last_crash_ts = None;
    }
}

/// Map an esp_reset_reason value to a human-readable string.
pub fn reset_reason_str(reason: u32) -> &'static str {
    // Values from esp_idf_svc::sys::esp_reset_reason_t
    match reason {
        1 => "power_on",
        3 => "software",
        4 => "panic",
        5 => "int_wdt",
        6 => "task_wdt",
        7 => "wdt",
        9 => "brownout",
        10 => "sdio",
        12 => "usb",
        13 => "jtag",
        15 => "power_glitch",
        16 => "efuse",
        _ => "unknown",
    }
}

/// Log the reset reason as a sys-level diagnostic entry.
pub fn log_reset_reason(reason_str: &str) {
    log::info!(target: "sys", "Reset reason: {}", reason_str);
}

/// Returns true if the given esp_reset_reason indicates a crash.
pub fn is_crash_reason(reason: u32) -> bool {
    matches!(reason, 4 | 5 | 6 | 7 | 9)
}

// ============================================================================
// Per-Hub Connection Vitals
// ============================================================================

struct HubVitals {
    hub_type: AtomicU8,          // 0=unused, 1=hue, 2=ha, ...
    conn_state: AtomicU8,        // CONN_DISCONNECTED/CONNECTING/CONNECTED
    last_heartbeat_ts: AtomicU32,
    reconnect_count: AtomicU32,
}

impl HubVitals {
    const fn new() -> Self {
        Self {
            hub_type: AtomicU8::new(0),
            conn_state: AtomicU8::new(CONN_DISCONNECTED),
            last_heartbeat_ts: AtomicU32::new(0),
            reconnect_count: AtomicU32::new(0),
        }
    }
}

const fn new_hub_vitals_array() -> [HubVitals; MAX_HUBS] {
    [
        HubVitals::new(),
        HubVitals::new(),
        HubVitals::new(),
        HubVitals::new(),
    ]
}

static HUB_VITALS: [HubVitals; MAX_HUBS] = new_hub_vitals_array();

/// Map HubType to a u8 slot key (1-indexed, 0 = unused).
fn hub_type_to_u8(hub_type: crate::hub::HubType) -> u8 {
    match hub_type.as_str() {
        crate::hub::HubType::HUE => 1,
        _ => 255,
    }
}

fn hub_type_str(val: u8) -> &'static str {
    match val {
        1 => "hue",
        _ => "unknown",
    }
}

/// Find the slot index for a given hub type, or None.
fn find_hub_slot(hub_type: crate::hub::HubType) -> Option<usize> {
    let key = hub_type_to_u8(hub_type);
    for (i, slot) in HUB_VITALS.iter().enumerate() {
        if slot.hub_type.load(Ordering::Relaxed) == key {
            return Some(i);
        }
    }
    None
}

// ============================================================================
// Public Vitals API
// ============================================================================

/// Register a hub in the vitals system (claim a slot).
pub fn vitals_hub_register(hub_type: crate::hub::HubType) {
    let key = hub_type_to_u8(hub_type);
    // Try to claim an unused slot
    for slot in HUB_VITALS.iter() {
        if slot
            .hub_type
            .compare_exchange(0, key, Ordering::SeqCst, Ordering::Relaxed)
            .is_ok()
        {
            slot.conn_state.store(CONN_DISCONNECTED, Ordering::Relaxed);
            slot.last_heartbeat_ts.store(0, Ordering::Relaxed);
            slot.reconnect_count.store(0, Ordering::Relaxed);
            return;
        }
        // Already registered for this hub type
        if slot.hub_type.load(Ordering::Relaxed) == key {
            return;
        }
    }
}

/// Release a hub's vitals slot.
pub fn vitals_hub_deregister(hub_type: crate::hub::HubType) {
    if let Some(i) = find_hub_slot(hub_type) {
        HUB_VITALS[i].hub_type.store(0, Ordering::Relaxed);
        HUB_VITALS[i]
            .conn_state
            .store(CONN_DISCONNECTED, Ordering::Relaxed);
    }
}

/// Update hub connection state.
pub fn vitals_hub_conn_state(hub_type: crate::hub::HubType, state: u8) {
    if let Some(i) = find_hub_slot(hub_type) {
        HUB_VITALS[i].conn_state.store(state, Ordering::Relaxed);
    }
}

/// Record a heartbeat for a hub.
pub fn vitals_hub_heartbeat(hub_type: crate::hub::HubType) {
    if let Some(i) = find_hub_slot(hub_type) {
        HUB_VITALS[i]
            .last_heartbeat_ts
            .store(now_epoch(), Ordering::Relaxed);
    }
}

/// Increment reconnect counter for a hub.
pub fn vitals_hub_reconnect(hub_type: crate::hub::HubType) {
    if let Some(i) = find_hub_slot(hub_type) {
        HUB_VITALS[i]
            .reconnect_count
            .fetch_add(1, Ordering::Relaxed);
    }
}

/// Record a light command result.
pub fn vitals_cmd_result(ok: bool) {
    let now = now_epoch();
    VITALS.last_cmd_ts.store(now, Ordering::Relaxed);
    if ok {
        VITALS.last_cmd_result.store(1, Ordering::Relaxed);
        VITALS.cmd_ok_count.fetch_add(1, Ordering::Relaxed);
    } else {
        VITALS.last_cmd_result.store(2, Ordering::Relaxed);
        VITALS.cmd_err_count.fetch_add(1, Ordering::Relaxed);
    }
}

/// Set boot time (call once after NTP sync).
pub fn vitals_set_boot_time(ts: u32) {
    VITALS.boot_time.store(ts, Ordering::Relaxed);
}

// ============================================================================
// Snapshot Types for HTTP Responses
// ============================================================================

#[derive(Serialize)]
pub struct VitalsSnapshot {
    pub uptime_secs: u32,
    pub free_heap_bytes: u32,
    pub reset_reason: &'static str,
    pub min_free_heap_bytes: u32,
    pub hubs: Vec<HubVitalsSnapshot>,
    pub last_cmd_result: &'static str,
    pub last_cmd_secs_ago: Option<u32>,
    pub cmd_ok_count: u32,
    pub cmd_err_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crash_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_crash_reset_reason: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_panic_msg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_crash_ts: Option<u32>,
}

#[derive(Serialize)]
pub struct HubVitalsSnapshot {
    #[serde(rename = "type")]
    pub hub_type: &'static str,
    pub conn_state: &'static str,
    pub last_heartbeat_secs_ago: Option<u32>,
    pub reconnect_count: u32,
}

fn conn_state_str(state: u8) -> &'static str {
    match state {
        CONN_DISCONNECTED => "disconnected",
        CONN_CONNECTING => "connecting",
        CONN_CONNECTED => "connected",
        _ => "unknown",
    }
}

/// Get a snapshot of all vitals for the HTTP response.
pub fn get_vitals() -> VitalsSnapshot {
    let now = now_epoch();
    let boot = VITALS.boot_time.load(Ordering::Relaxed);
    let uptime = if boot > 0 && now > boot {
        now - boot
    } else {
        0
    };

    let free_heap = unsafe { esp_idf_svc::sys::esp_get_free_heap_size() } as u32;
    let min_free_heap = unsafe { esp_idf_svc::sys::esp_get_minimum_free_heap_size() } as u32;

    let last_cmd_ts = VITALS.last_cmd_ts.load(Ordering::Relaxed);
    let last_cmd_result = match VITALS.last_cmd_result.load(Ordering::Relaxed) {
        1 => "ok",
        2 => "error",
        _ => "none",
    };
    let last_cmd_secs_ago = if last_cmd_ts > 0 && now > last_cmd_ts {
        Some(now - last_cmd_ts)
    } else {
        None
    };

    let mut hubs = Vec::new();
    for slot in HUB_VITALS.iter() {
        let ht = slot.hub_type.load(Ordering::Relaxed);
        if ht == 0 {
            continue;
        }
        let hb_ts = slot.last_heartbeat_ts.load(Ordering::Relaxed);
        let hb_ago = if hb_ts > 0 && now > hb_ts {
            Some(now - hb_ts)
        } else {
            None
        };
        hubs.push(HubVitalsSnapshot {
            hub_type: hub_type_str(ht),
            conn_state: conn_state_str(slot.conn_state.load(Ordering::Relaxed)),
            last_heartbeat_secs_ago: hb_ago,
            reconnect_count: slot.reconnect_count.load(Ordering::Relaxed),
        });
    }

    // Read boot/crash info
    let (reset_reason, crash_count, last_crash_reset_reason, last_panic_msg, last_crash_ts) =
        if let Ok(info) = BOOT_INFO.lock() {
            (
                info.reset_reason,
                info.crash_count,
                info.last_crash_reset_reason,
                info.last_panic_msg.clone(),
                info.last_crash_ts,
            )
        } else {
            ("unknown", None, None, None, None)
        };

    VitalsSnapshot {
        uptime_secs: uptime,
        free_heap_bytes: free_heap,
        reset_reason,
        min_free_heap_bytes: min_free_heap,
        hubs,
        last_cmd_result,
        last_cmd_secs_ago,
        cmd_ok_count: VITALS.cmd_ok_count.load(Ordering::Relaxed),
        cmd_err_count: VITALS.cmd_err_count.load(Ordering::Relaxed),
        crash_count,
        last_crash_reset_reason,
        last_panic_msg,
        last_crash_ts,
    }
}

/// Get filtered log entries for the HTTP response.
pub fn get_logs(
    category: Option<DiagCategory>,
    limit: usize,
    since: Option<u32>,
) -> Vec<DiagEntry> {
    if let Ok(buf) = DIAG_BUF.lock() {
        buf.get_filtered(category, since, limit)
    } else {
        Vec::new()
    }
}

// ============================================================================
// Custom Logger
// ============================================================================

struct DiagLogger;

impl log::Log for DiagLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        // Forward to UART via stderr (avoids println! panic during early boot)
        let level_char = match record.level() {
            log::Level::Error => "E",
            log::Level::Warn => "W",
            log::Level::Info => "I",
            log::Level::Debug => "D",
            log::Level::Trace => "V",
        };
        // Use writeln! to stderr instead of println! — println! panics if
        // stdout isn't ready (common early in ESP32 boot).
        let _ = std::io::Write::write_fmt(
            &mut std::io::stderr(),
            format_args!("{} ({}): {}\n", level_char, record.target(), record.args()),
        );

        // Only buffer Info/Warn/Error
        let Some(level) = DiagLevel::from_log_level(record.level()) else {
            return;
        };

        let cat = DiagCategory::from_target(record.target());
        let ts = now_epoch();

        let mut msg = FixedString::new();
        let _ = write!(msg, "{}", record.args());

        let entry = DiagEntry {
            ts,
            level,
            cat,
            msg,
        };

        if let Ok(mut buf) = DIAG_BUF.lock() {
            buf.push(entry);
        }
    }

    fn flush(&self) {}
}

static LOGGER: DiagLogger = DiagLogger;

/// Initialize the diagnostic logging system.
///
/// Replaces `EspLogger::initialize_default()`. Sets up the custom logger
/// that forwards to both UART and the ring buffer.
pub fn init() {
    // Set our custom logger as the global logger.
    // UART output is handled via stderr write (ESP-IDF routes to serial).
    log::set_logger(&LOGGER).ok();
    log::set_max_level(log::LevelFilter::Info);
}

// ============================================================================
// Helpers
// ============================================================================

fn now_epoch() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0)
}
