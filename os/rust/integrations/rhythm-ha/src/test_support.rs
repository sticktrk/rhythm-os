//! Test support utilities for rhythm-ha.
//!
//! Provides `SpyHaTransport` — a recording implementation of `HaTransport`
//! for use in integration tests. Accessible cross-crate when `test-support`
//! is enabled.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde_json::Value;

use std::sync::Arc;

use crate::transport::{EntityState, HaTransport};

/// `HaTransport` impl for `Arc<SpyHaTransport>` — allows sharing the spy
/// between the controller (which takes ownership) and the test (which inspects).
impl HaTransport for Arc<SpyHaTransport> {
    fn call_service(&self, domain: &str, service: &str, data: &Value) -> anyhow::Result<()> {
        (**self).call_service(domain, service, data)
    }
    fn get_states(&self) -> anyhow::Result<Vec<EntityState>> {
        (**self).get_states()
    }
    fn get_state(&self, entity_id: &str) -> anyhow::Result<EntityState> {
        (**self).get_state(entity_id)
    }
    fn test_connection(&self) -> anyhow::Result<bool> {
        (**self).test_connection()
    }
    fn get_config(&self) -> anyhow::Result<Value> {
        (**self).get_config()
    }
}

/// A recorded service call to a `SpyHaTransport`.
#[derive(Debug, Clone)]
pub struct HaServiceCall {
    pub domain: String,
    pub service: String,
    pub data: Value,
}

/// A recording `HaTransport` implementation for testing.
///
/// Records all service calls and returns configurable entity states.
/// Thread-safe — all interior state is behind `Mutex` or atomics.
pub struct SpyHaTransport {
    calls: Mutex<Vec<HaServiceCall>>,
    entity_states: Mutex<HashMap<String, String>>,
    should_fail: AtomicBool,
}

impl SpyHaTransport {
    pub fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            entity_states: Mutex::new(HashMap::new()),
            should_fail: AtomicBool::new(false),
        }
    }

    /// Set the state of an entity (e.g., "light.kitchen_1" -> "on").
    pub fn set_entity_state(&self, entity_id: &str, state: &str) {
        self.entity_states
            .lock()
            .unwrap()
            .insert(entity_id.to_string(), state.to_string());
    }

    /// Set whether transport calls should return errors.
    pub fn set_should_fail(&self, fail: bool) {
        self.should_fail.store(fail, Ordering::Relaxed);
    }

    /// Get all recorded service calls.
    pub fn calls(&self) -> Vec<HaServiceCall> {
        self.calls.lock().unwrap().clone()
    }

    /// Get only service calls matching a specific service name.
    pub fn calls_for_service(&self, service: &str) -> Vec<HaServiceCall> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.service == service)
            .cloned()
            .collect()
    }

    /// Count total service calls.
    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    /// Clear all recorded calls.
    pub fn reset(&self) {
        self.calls.lock().unwrap().clear();
    }
}

impl Default for SpyHaTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HaTransport for SpyHaTransport {
    fn call_service(&self, domain: &str, service: &str, data: &Value) -> anyhow::Result<()> {
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: call_service failed");
        }
        self.calls.lock().unwrap().push(HaServiceCall {
            domain: domain.to_string(),
            service: service.to_string(),
            data: data.clone(),
        });
        Ok(())
    }

    fn get_states(&self) -> anyhow::Result<Vec<EntityState>> {
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: get_states failed");
        }
        let states = self.entity_states.lock().unwrap();
        Ok(states
            .iter()
            .map(|(id, state)| EntityState {
                entity_id: id.clone(),
                state: state.clone(),
                attributes: Value::Null,
            })
            .collect())
    }

    fn get_state(&self, entity_id: &str) -> anyhow::Result<EntityState> {
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: get_state failed");
        }
        let states = self.entity_states.lock().unwrap();
        let state = states
            .get(entity_id)
            .cloned()
            .unwrap_or_else(|| "off".to_string());
        Ok(EntityState {
            entity_id: entity_id.to_string(),
            state,
            attributes: Value::Null,
        })
    }

    fn test_connection(&self) -> anyhow::Result<bool> {
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: test_connection failed");
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_service_calls() {
        let spy = SpyHaTransport::new();
        let data = serde_json::json!({"area_id": "kitchen", "brightness_pct": 80});
        spy.call_service("light", "turn_on", &data).unwrap();

        let calls = spy.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].domain, "light");
        assert_eq!(calls[0].service, "turn_on");
        assert_eq!(calls[0].data["area_id"], "kitchen");
    }

    #[test]
    fn should_fail_returns_error() {
        let spy = SpyHaTransport::new();
        spy.set_should_fail(true);
        assert!(spy
            .call_service("light", "turn_on", &serde_json::json!({}))
            .is_err());
    }

    #[test]
    fn entity_state_configurable() {
        let spy = SpyHaTransport::new();
        assert_eq!(spy.get_state("light.kitchen").unwrap().state, "off");

        spy.set_entity_state("light.kitchen", "on");
        assert_eq!(spy.get_state("light.kitchen").unwrap().state, "on");
    }

    #[test]
    fn calls_for_service_filters() {
        let spy = SpyHaTransport::new();
        spy.call_service("light", "turn_on", &serde_json::json!({}))
            .unwrap();
        spy.call_service("light", "turn_off", &serde_json::json!({}))
            .unwrap();
        spy.call_service("light", "turn_on", &serde_json::json!({}))
            .unwrap();

        assert_eq!(spy.calls_for_service("turn_on").len(), 2);
        assert_eq!(spy.calls_for_service("turn_off").len(), 1);
    }

    #[test]
    fn reset_clears_calls() {
        let spy = SpyHaTransport::new();
        spy.call_service("light", "turn_on", &serde_json::json!({}))
            .unwrap();
        spy.reset();
        assert_eq!(spy.call_count(), 0);
    }
}
