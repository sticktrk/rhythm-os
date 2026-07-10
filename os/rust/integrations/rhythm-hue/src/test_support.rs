//! Test support utilities for rhythm-hue.
//!
//! Provides `SpyHueTransport` — a recording implementation of `HueTransport`
//! for use in integration tests. Accessible cross-crate when `test-support`
//! is enabled.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::transport::HueTransport;

/// `HueTransport` impl for `Arc<SpyHueTransport>` — allows sharing the spy
/// between the controller (which takes ownership of the transport) and the test
/// (which inspects recorded calls).
impl HueTransport for Arc<SpyHueTransport> {
    fn test_connection(&self, username: &str) -> anyhow::Result<bool> {
        (**self).test_connection(username)
    }
    fn warmup_tls(&self) -> anyhow::Result<()> {
        (**self).warmup_tls()
    }
    fn set_grouped_light(
        &self,
        username: &str,
        grouped_light_id: &str,
        on: bool,
        brightness: Option<u8>,
        kelvin: Option<u16>,
        xy: Option<(f32, f32)>,
        fade_ms: Option<u16>,
    ) -> anyhow::Result<()> {
        (**self).set_grouped_light(
            username,
            grouped_light_id,
            on,
            brightness,
            kelvin,
            xy,
            fade_ms,
        )
    }
    fn set_light(
        &self,
        username: &str,
        light_id: &str,
        on: bool,
        brightness: Option<u8>,
        kelvin: Option<u16>,
        xy: Option<(f32, f32)>,
        fade_ms: Option<u16>,
    ) -> anyhow::Result<()> {
        (**self).set_light(username, light_id, on, brightness, kelvin, xy, fade_ms)
    }
    fn is_grouped_light_on(&self, username: &str, grouped_light_id: &str) -> anyhow::Result<bool> {
        (**self).is_grouped_light_on(username, grouped_light_id)
    }
    fn is_light_on(&self, username: &str, light_id: &str) -> anyhow::Result<bool> {
        (**self).is_light_on(username, light_id)
    }
    fn identify_light(&self, username: &str, light_id: &str) -> anyhow::Result<()> {
        (**self).identify_light(username, light_id)
    }
    fn get_resources(
        &self,
        username: &str,
        resource_type: &str,
    ) -> anyhow::Result<serde_json::Value> {
        (**self).get_resources(username, resource_type)
    }
}

/// A recorded call to a `SpyHueTransport`.
#[derive(Debug, Clone)]
pub enum HueTransportCall {
    SetGroupedLight {
        grouped_light_id: String,
        on: bool,
        brightness: Option<u8>,
        kelvin: Option<u16>,
        xy: Option<(f32, f32)>,
        fade_ms: Option<u16>,
    },
    SetLight {
        light_id: String,
        on: bool,
        brightness: Option<u8>,
        kelvin: Option<u16>,
        xy: Option<(f32, f32)>,
        fade_ms: Option<u16>,
    },
    IsGroupedLightOn {
        grouped_light_id: String,
    },
    IsLightOn {
        light_id: String,
    },
    IdentifyLight {
        light_id: String,
    },
    GetResources {
        resource_type: String,
    },
    TestConnection,
    WarmupTls,
}

/// A recording `HueTransport` implementation for testing.
///
/// Records all transport calls and returns configurable responses.
/// Thread-safe — all interior state is behind `Arc<Mutex<_>>` or atomics.
pub struct SpyHueTransport {
    calls: Arc<Mutex<Vec<HueTransportCall>>>,
    is_on: Arc<Mutex<bool>>,
    resources: Arc<Mutex<HashMap<String, serde_json::Value>>>,
    should_fail: Arc<AtomicBool>,
}

impl SpyHueTransport {
    /// Create a new spy transport and return shared handles for inspection.
    pub fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            is_on: Arc::new(Mutex::new(false)),
            resources: Arc::new(Mutex::new(HashMap::new())),
            should_fail: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Set whether `is_grouped_light_on` returns true or false.
    pub fn set_is_on(&self, on: bool) {
        *self.is_on.lock().unwrap() = on;
    }

    /// Set whether transport calls should return errors.
    pub fn set_should_fail(&self, fail: bool) {
        self.should_fail.store(fail, Ordering::Relaxed);
    }

    /// Configure the response returned by `get_resources` for a resource path.
    pub fn set_resource_response(&self, resource_type: &str, response: serde_json::Value) {
        self.resources
            .lock()
            .unwrap()
            .insert(resource_type.to_string(), response);
    }

    /// Get all recorded transport calls.
    pub fn calls(&self) -> Vec<HueTransportCall> {
        self.calls.lock().unwrap().clone()
    }

    /// Get only `SetGroupedLight` calls.
    pub fn set_grouped_light_calls(&self) -> Vec<HueTransportCall> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| matches!(c, HueTransportCall::SetGroupedLight { .. }))
            .cloned()
            .collect()
    }

    /// Get only `SetLight` calls.
    pub fn set_light_calls(&self) -> Vec<HueTransportCall> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| matches!(c, HueTransportCall::SetLight { .. }))
            .cloned()
            .collect()
    }

    /// Count `SetGroupedLight` calls.
    pub fn set_grouped_light_count(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| matches!(c, HueTransportCall::SetGroupedLight { .. }))
            .count()
    }

    /// Clear all recorded calls.
    pub fn reset(&self) {
        self.calls.lock().unwrap().clear();
    }
}

impl Default for SpyHueTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HueTransport for SpyHueTransport {
    fn test_connection(&self, _username: &str) -> anyhow::Result<bool> {
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::TestConnection);
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: connection failed");
        }
        Ok(true)
    }

    fn warmup_tls(&self) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push(HueTransportCall::WarmupTls);
        Ok(())
    }

    fn set_grouped_light(
        &self,
        _username: &str,
        grouped_light_id: &str,
        on: bool,
        brightness: Option<u8>,
        kelvin: Option<u16>,
        xy: Option<(f32, f32)>,
        fade_ms: Option<u16>,
    ) -> anyhow::Result<()> {
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: set_grouped_light failed");
        }
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::SetGroupedLight {
                grouped_light_id: grouped_light_id.to_string(),
                on,
                brightness,
                kelvin,
                xy,
                fade_ms,
            });
        Ok(())
    }

    fn set_light(
        &self,
        _username: &str,
        light_id: &str,
        on: bool,
        brightness: Option<u8>,
        kelvin: Option<u16>,
        xy: Option<(f32, f32)>,
        fade_ms: Option<u16>,
    ) -> anyhow::Result<()> {
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: set_light failed");
        }
        self.calls.lock().unwrap().push(HueTransportCall::SetLight {
            light_id: light_id.to_string(),
            on,
            brightness,
            kelvin,
            xy,
            fade_ms,
        });
        Ok(())
    }

    fn is_grouped_light_on(&self, _username: &str, grouped_light_id: &str) -> anyhow::Result<bool> {
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::IsGroupedLightOn {
                grouped_light_id: grouped_light_id.to_string(),
            });
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: is_grouped_light_on failed");
        }
        Ok(*self.is_on.lock().unwrap())
    }

    fn is_light_on(&self, _username: &str, light_id: &str) -> anyhow::Result<bool> {
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::IsLightOn {
                light_id: light_id.to_string(),
            });
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: is_light_on failed");
        }
        Ok(*self.is_on.lock().unwrap())
    }

    fn identify_light(&self, _username: &str, light_id: &str) -> anyhow::Result<()> {
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: identify_light failed");
        }
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::IdentifyLight {
                light_id: light_id.to_string(),
            });
        Ok(())
    }

    fn get_resources(
        &self,
        _username: &str,
        resource_type: &str,
    ) -> anyhow::Result<serde_json::Value> {
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::GetResources {
                resource_type: resource_type.to_string(),
            });
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: get_resources failed");
        }
        Ok(self
            .resources
            .lock()
            .unwrap()
            .get(resource_type)
            .cloned()
            .unwrap_or_else(|| serde_json::json!({"data": []})))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_set_grouped_light() {
        let spy = SpyHueTransport::new();
        spy.set_grouped_light("user", "gl1", true, Some(80), Some(4000), None, Some(500))
            .unwrap();

        let calls = spy.set_grouped_light_calls();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            HueTransportCall::SetGroupedLight {
                grouped_light_id,
                on,
                brightness,
                kelvin,
                xy: _,
                fade_ms,
            } => {
                assert_eq!(grouped_light_id, "gl1");
                assert!(*on);
                assert_eq!(*brightness, Some(80));
                assert_eq!(*kelvin, Some(4000));
                assert_eq!(*fade_ms, Some(500));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn should_fail_returns_error() {
        let spy = SpyHueTransport::new();
        spy.set_should_fail(true);
        assert!(spy
            .set_grouped_light("user", "gl1", true, None, None, None, None)
            .is_err());
        assert!(spy
            .set_light("user", "light1", true, None, None, None, None)
            .is_err());
    }

    #[test]
    fn is_on_configurable() {
        let spy = SpyHueTransport::new();
        assert!(!spy.is_grouped_light_on("user", "gl1").unwrap());
        assert!(!spy.is_light_on("user", "light1").unwrap());
        spy.set_is_on(true);
        assert!(spy.is_grouped_light_on("user", "gl1").unwrap());
        assert!(spy.is_light_on("user", "light1").unwrap());
    }

    #[test]
    fn reset_clears_calls() {
        let spy = SpyHueTransport::new();
        spy.set_grouped_light("user", "gl1", true, None, None, None, None)
            .unwrap();
        assert_eq!(spy.set_grouped_light_count(), 1);
        spy.reset();
        assert_eq!(spy.set_grouped_light_count(), 0);
    }
}
