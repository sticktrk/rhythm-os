//! Runtime contract shared by Rhythm OS hosts and external lighting runtimes.
//!
//! This crate deliberately contains data shapes and traits only. It does not
//! own hub topology, persistence, auth, HTTP routing, or any concrete lighting
//! algorithm. Runtime crates consume snapshots/events and return plans; the OS
//! host remains responsible for executing those plans against real devices.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type RuntimeId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeNodeKind {
    Home,
    Zone,
    Area,
    Section,
    Device,
    Input,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeNode {
    pub id: RuntimeId,
    pub name: String,
    pub kind: RuntimeNodeKind,
    pub parent_id: Option<RuntimeId>,
    #[serde(default)]
    pub properties: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    #[serde(default)]
    pub nodes: Vec<RuntimeNode>,
    #[serde(default)]
    pub state: BTreeMap<RuntimeId, NodeState>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeState {
    pub power_on: bool,
    pub rhythm_enabled: bool,
    #[serde(default)]
    pub brightness: Option<u8>,
    #[serde(default)]
    pub kelvin: Option<u16>,
    #[serde(default)]
    pub properties: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DispatchTarget {
    Node {
        node_id: RuntimeId,
    },
    Group {
        hub_id: String,
        control_id: String,
    },
    Devices {
        hub_id: String,
        native_ids: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LightingCommand {
    pub brightness: u8,
    pub kelvin: u16,
    #[serde(default)]
    pub transition_ms: Option<u32>,
    #[serde(default)]
    pub purpose: Option<String>,
}

impl LightingCommand {
    pub fn new(brightness: u8, kelvin: u16) -> Self {
        Self {
            brightness,
            kelvin,
            transition_ms: None,
            purpose: None,
        }
    }

    pub fn with_transition(mut self, transition_ms: u32) -> Self {
        self.transition_ms = Some(transition_ms);
        self
    }

    pub fn with_purpose(mut self, purpose: impl Into<String>) -> Self {
        self.purpose = Some(purpose.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DispatchCommand {
    TurnOn {
        target: DispatchTarget,
        command: LightingCommand,
    },
    TurnOff {
        target: DispatchTarget,
        transition_ms: Option<u32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputAction {
    On,
    Off,
    Toggle,
    Reset,
    BrightnessUp,
    BrightnessDown,
    StepUp,
    StepDown,
    Stop,
    Named(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeInputEvent {
    pub source_id: RuntimeId,
    pub target_id: RuntimeId,
    pub action: InputAction,
    #[serde(default)]
    pub epoch_ms: Option<i64>,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TickContext {
    pub node_id: RuntimeId,
    pub hour: f64,
    #[serde(default)]
    pub epoch_ms: Option<i64>,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    Input(RuntimeInputEvent),
    PeriodicTick(TickContext),
    HostStateChanged { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateWrite {
    pub node_id: RuntimeId,
    pub key: String,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeDiagnostic {
    pub level: DiagnosticLevel,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RuntimePlan {
    #[serde(default)]
    pub dispatch: Vec<DispatchCommand>,
    #[serde(default)]
    pub state_writes: Vec<StateWrite>,
    #[serde(default)]
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

impl RuntimePlan {
    pub fn noop() -> Self {
        Self::default()
    }

    pub fn dispatch(command: DispatchCommand) -> Self {
        Self {
            dispatch: vec![command],
            ..Self::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.dispatch.is_empty() && self.state_writes.is_empty() && self.diagnostics.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCapabilities {
    pub input_events: bool,
    pub periodic_ticks: bool,
    pub host_state_changes: bool,
    pub state_writes: bool,
}

impl RuntimeCapabilities {
    pub const fn lighting_app() -> Self {
        Self {
            input_events: true,
            periodic_ticks: true,
            host_state_changes: true,
            state_writes: true,
        }
    }
}

impl Default for RuntimeCapabilities {
    fn default() -> Self {
        Self::lighting_app()
    }
}

#[async_trait]
pub trait LightDispatcher: Send + Sync {
    async fn dispatch(&self, command: DispatchCommand) -> Result<(), RuntimeHostError>;
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeHostError {
    #[error("dispatch failed: {0}")]
    DispatchFailed(String),
    #[error("host error: {0}")]
    Host(String),
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("unknown node: {0}")]
    UnknownNode(String),
    #[error("invalid event: {0}")]
    InvalidEvent(String),
    #[error("runtime error: {0}")]
    Runtime(String),
}

pub type RuntimeResult<T> = Result<T, RuntimeError>;

pub trait LightingRuntime: Send {
    fn name(&self) -> &str;

    fn runtime_id(&self) -> &str {
        self.name()
    }

    fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities::default()
    }

    fn handle_event(
        &mut self,
        snapshot: &RuntimeSnapshot,
        event: RuntimeEvent,
    ) -> RuntimeResult<RuntimePlan>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_plan_empty_tracks_all_payloads() {
        assert!(RuntimePlan::noop().is_empty());
        assert!(!RuntimePlan::dispatch(DispatchCommand::TurnOff {
            target: DispatchTarget::Node {
                node_id: "area".to_string()
            },
            transition_ms: None,
        })
        .is_empty());
    }
}
