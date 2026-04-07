//! WS2812 RGB LED status indicator for ESP32-C6.
//!
//! Uses the RMT peripheral (legacy API) to drive the addressable RGB LED on GPIO8.

use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::Duration;

#[allow(deprecated)]
use esp_idf_svc::hal::gpio::OutputPin;
#[allow(deprecated)]
use esp_idf_svc::hal::rmt::{
    config::TransmitConfig, FixedLengthSignal, PinState, Pulse, RmtChannel, TxRmtDriver,
};
use log::info;

/// RGB color type.
#[derive(Debug, Clone, Copy, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// Predefined colors for status indication.
pub struct StatusColor;

impl StatusColor {
    pub const OFF: Rgb = Rgb::new(0, 0, 0);
    pub const BLUE: Rgb = Rgb::new(0, 0, 255);

    // Dimmer versions (easier on the eyes)
    pub const DIM_RED: Rgb = Rgb::new(40, 0, 0);
    pub const DIM_GREEN: Rgb = Rgb::new(0, 40, 0);
    pub const DIM_BLUE: Rgb = Rgb::new(0, 0, 40);
    pub const DIM_YELLOW: Rgb = Rgb::new(40, 30, 0);
    pub const DIM_CYAN: Rgb = Rgb::new(0, 40, 40);
    pub const DIM_PURPLE: Rgb = Rgb::new(30, 0, 40);
}

/// LED status states for visual feedback.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LedStatus {
    /// Device is initializing (blue pulse).
    Initializing,
    /// Connecting to WiFi (fast blue blink).
    WifiConnecting,
    /// WiFi connected (solid green briefly).
    WifiConnected,
    /// Syncing time with NTP (cyan blink).
    NtpSyncing,
    /// NTP sync complete (green flash).
    NtpSynced,
    /// HTTP server starting (yellow blink).
    ServerStarting,
    /// Device is ready (green for 2s then off).
    Ready,
    /// Error occurred (red blink).
    Error,
    /// BLE provisioning mode (slow purple pulse).
    BleProvisioning,
    /// BLE client connected (solid dim purple).
    BleConnected,
}

/// WS2812 RGB LED controller using RMT.
#[allow(deprecated)]
pub struct Ws2812Led<'d> {
    tx: TxRmtDriver<'d>,
}

#[allow(deprecated)]
impl<'d> Ws2812Led<'d> {
    /// Create a new WS2812 LED controller.
    pub fn new(channel: impl RmtChannel + 'd, pin: impl OutputPin + 'd) -> anyhow::Result<Self> {
        let config = TransmitConfig::new().clock_divider(1);
        let tx = TxRmtDriver::new(channel, pin, &config)?;
        info!("WS2812 RGB LED initialized");
        Ok(Self { tx })
    }

    /// Set the LED color.
    pub fn set_color(&mut self, rgb: Rgb) -> anyhow::Result<()> {
        // WS2812 expects GRB order, pack into u32
        let color: u32 = ((rgb.g as u32) << 16) | ((rgb.r as u32) << 8) | (rgb.b as u32);

        let ticks_hz = self.tx.counter_clock()?;

        // WS2812 timing
        let t0h = Pulse::new_with_duration(ticks_hz, PinState::High, &Duration::from_nanos(350))?;
        let t0l = Pulse::new_with_duration(ticks_hz, PinState::Low, &Duration::from_nanos(800))?;
        let t1h = Pulse::new_with_duration(ticks_hz, PinState::High, &Duration::from_nanos(700))?;
        let t1l = Pulse::new_with_duration(ticks_hz, PinState::Low, &Duration::from_nanos(600))?;

        let mut signal = FixedLengthSignal::<24>::new();
        for i in (0..24).rev() {
            let bit = (color >> i) & 1 != 0;
            let (high, low) = if bit { (t1h, t1l) } else { (t0h, t0l) };
            signal.set(23 - i as usize, &(high, low))?;
        }

        self.tx.start_blocking(&signal)?;
        Ok(())
    }

    /// Turn off the LED.
    pub fn off(&mut self) {
        let _ = self.set_color(StatusColor::OFF);
    }

    /// Blink the LED with a specific color.
    pub fn blink(&mut self, color: Rgb, count: u8, on_ms: u64, off_ms: u64) {
        for _ in 0..count {
            let _ = self.set_color(color);
            thread::sleep(Duration::from_millis(on_ms));
            self.off();
            if off_ms > 0 {
                thread::sleep(Duration::from_millis(off_ms));
            }
        }
    }

    /// Pulse the LED (fade in and out).
    pub fn pulse(&mut self, color: Rgb, duration_ms: u64) {
        let steps = 20u64;
        let step_time = duration_ms / (steps * 2);

        // Fade in
        for i in 0..steps {
            let brightness = i as f32 / steps as f32;
            let c = Rgb::new(
                (color.r as f32 * brightness) as u8,
                (color.g as f32 * brightness) as u8,
                (color.b as f32 * brightness) as u8,
            );
            let _ = self.set_color(c);
            thread::sleep(Duration::from_millis(step_time));
        }

        // Fade out
        for i in (0..steps).rev() {
            let brightness = i as f32 / steps as f32;
            let c = Rgb::new(
                (color.r as f32 * brightness) as u8,
                (color.g as f32 * brightness) as u8,
                (color.b as f32 * brightness) as u8,
            );
            let _ = self.set_color(c);
            thread::sleep(Duration::from_millis(step_time));
        }

        self.off();
    }

    /// Show a status pattern.
    pub fn show(&mut self, status: LedStatus) {
        match status {
            LedStatus::Initializing => {
                self.pulse(StatusColor::BLUE, 1000);
                self.pulse(StatusColor::BLUE, 1000);
            }
            LedStatus::WifiConnecting => {
                self.blink(StatusColor::DIM_BLUE, 5, 100, 100);
            }
            LedStatus::WifiConnected => {
                let _ = self.set_color(StatusColor::DIM_GREEN);
                thread::sleep(Duration::from_millis(500));
                self.off();
            }
            LedStatus::NtpSyncing => {
                self.blink(StatusColor::DIM_CYAN, 2, 100, 100);
                thread::sleep(Duration::from_millis(300));
            }
            LedStatus::NtpSynced => {
                self.blink(StatusColor::DIM_GREEN, 1, 200, 0);
            }
            LedStatus::ServerStarting => {
                self.blink(StatusColor::DIM_YELLOW, 3, 100, 100);
            }
            LedStatus::Ready => {
                let _ = self.set_color(StatusColor::DIM_GREEN);
                thread::sleep(Duration::from_millis(2000));
                self.off();
            }
            LedStatus::Error => {
                self.blink(StatusColor::DIM_RED, 10, 50, 50);
            }
            LedStatus::BleProvisioning => {
                self.pulse(StatusColor::DIM_PURPLE, 2000);
            }
            LedStatus::BleConnected => {
                let _ = self.set_color(StatusColor::DIM_PURPLE);
                thread::sleep(Duration::from_millis(500));
                self.off();
            }
        }
    }
}

// Global pending flash for activity indicators from other threads
static PENDING_FLASH: AtomicU32 = AtomicU32::new(0);

/// Request an activity flash from any thread (white).
pub fn request_activity_flash() {
    let packed = 0x1E1E1E_u32; // dim white
    PENDING_FLASH.store(packed, Ordering::Relaxed);
}

/// Request a periodic update flash from any thread (cyan).
pub fn request_periodic_flash() {
    let packed = 0x002828_u32; // dim cyan
    PENDING_FLASH.store(packed, Ordering::Relaxed);
}

/// Check if there's a pending flash and return the color.
pub fn take_pending_flash() -> Option<Rgb> {
    let packed = PENDING_FLASH.swap(0, Ordering::Relaxed);
    if packed == 0 {
        None
    } else {
        Some(Rgb::new(
            ((packed >> 16) & 0xFF) as u8,
            ((packed >> 8) & 0xFF) as u8,
            (packed & 0xFF) as u8,
        ))
    }
}

// Convenience functions for existing code
pub fn led_activity() {
    request_activity_flash();
}

pub fn led_periodic() {
    request_periodic_flash();
}
