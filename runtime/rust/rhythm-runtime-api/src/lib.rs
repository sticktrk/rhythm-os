//! Runtime contract shared by Rhythm OS hosts and external light runtimes.
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
    pub const fn light_runtime() -> Self {
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
        Self::light_runtime()
    }
}

/// Runtime-owned API and settings declaration.
///
/// The OS host owns HTTP routing, auth, persistence, logging, and plan
/// application. Runtime crates use this manifest to describe the extension
/// surface they want the host to mount under `/api/light-runtimes/{runtime_id}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeManifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub capabilities: RuntimeCapabilities,
    #[serde(default)]
    pub extension: RuntimeExtensionManifest,
}

impl RuntimeManifest {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            version: None,
            description: None,
            capabilities: RuntimeCapabilities::default(),
            extension: RuntimeExtensionManifest::default(),
        }
    }

    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_capabilities(mut self, capabilities: RuntimeCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    pub fn with_extension(mut self, extension: RuntimeExtensionManifest) -> Self {
        self.extension = extension;
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RuntimeExtensionManifest {
    #[serde(default)]
    pub settings: Option<RuntimeSettingsManifest>,
    #[serde(default)]
    pub endpoints: Vec<RuntimeEndpointSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSettingsManifest {
    pub version: u32,
    #[serde(default)]
    pub schema: serde_json::Value,
    #[serde(default)]
    pub defaults: serde_json::Value,
}

impl RuntimeSettingsManifest {
    pub fn new(version: u32) -> Self {
        Self {
            version,
            schema: serde_json::Value::Null,
            defaults: serde_json::Value::Null,
        }
    }

    pub fn with_schema(mut self, schema: serde_json::Value) -> Self {
        self.schema = schema;
        self
    }

    pub fn with_defaults(mut self, defaults: serde_json::Value) -> Self {
        self.defaults = defaults;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum RuntimeHttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeEndpointSpec {
    pub method: RuntimeHttpMethod,
    /// Runtime-relative path mounted below `/api/light-runtimes/{runtime_id}`.
    ///
    /// For example, `/settings` becomes
    /// `/api/light-runtimes/{runtime_id}/settings`.
    pub path: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub request_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub response_schema: Option<serde_json::Value>,
}

impl RuntimeEndpointSpec {
    pub fn new(method: RuntimeHttpMethod, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            description: None,
            request_schema: None,
            response_schema: None,
        }
    }

    pub fn get(path: impl Into<String>) -> Self {
        Self::new(RuntimeHttpMethod::Get, path)
    }

    pub fn post(path: impl Into<String>) -> Self {
        Self::new(RuntimeHttpMethod::Post, path)
    }

    pub fn put(path: impl Into<String>) -> Self {
        Self::new(RuntimeHttpMethod::Put, path)
    }

    pub fn patch(path: impl Into<String>) -> Self {
        Self::new(RuntimeHttpMethod::Patch, path)
    }

    pub fn delete(path: impl Into<String>) -> Self {
        Self::new(RuntimeHttpMethod::Delete, path)
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_request_schema(mut self, schema: serde_json::Value) -> Self {
        self.request_schema = Some(schema);
        self
    }

    pub fn with_response_schema(mut self, schema: serde_json::Value) -> Self {
        self.response_schema = Some(schema);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeExtensionRequest {
    pub method: RuntimeHttpMethod,
    /// Runtime-relative path after `/api/light-runtimes/{runtime_id}`.
    pub path: String,
    #[serde(default)]
    pub query: BTreeMap<String, String>,
    #[serde(default)]
    pub body: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeExtensionResponse {
    pub status: u16,
    #[serde(default)]
    pub body: serde_json::Value,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub plan: RuntimePlan,
}

impl RuntimeExtensionResponse {
    pub fn json(body: serde_json::Value) -> Self {
        Self {
            status: 200,
            body,
            headers: BTreeMap::new(),
            plan: RuntimePlan::noop(),
        }
    }

    pub fn empty(status: u16) -> Self {
        Self {
            status,
            body: serde_json::Value::Null,
            headers: BTreeMap::new(),
            plan: RuntimePlan::noop(),
        }
    }

    pub fn with_plan(mut self, plan: RuntimePlan) -> Self {
        self.plan = plan;
        self
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
    #[error("unsupported runtime extension endpoint: {0}")]
    UnsupportedExtension(String),
    #[error("invalid runtime extension request: {0}")]
    InvalidExtensionRequest(String),
    #[error("invalid runtime settings: {0}")]
    InvalidSettings(String),
    #[error("runtime error: {0}")]
    Runtime(String),
}

pub type RuntimeResult<T> = Result<T, RuntimeError>;

pub trait LightRuntime: Send {
    fn name(&self) -> &str;

    fn runtime_id(&self) -> &str {
        self.name()
    }

    fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities::default()
    }

    fn manifest(&self) -> RuntimeManifest {
        RuntimeManifest::new(self.runtime_id(), self.name()).with_capabilities(self.capabilities())
    }

    fn handle_event(
        &mut self,
        snapshot: &RuntimeSnapshot,
        event: RuntimeEvent,
    ) -> RuntimeResult<RuntimePlan>;

    fn handle_extension(
        &mut self,
        _snapshot: &RuntimeSnapshot,
        request: RuntimeExtensionRequest,
    ) -> RuntimeResult<RuntimeExtensionResponse> {
        Err(RuntimeError::UnsupportedExtension(format!(
            "{:?} {}",
            request.method, request.path
        )))
    }
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

    #[test]
    fn runtime_manifest_describes_namespaced_extension_surface() {
        let manifest = RuntimeManifest::new("example-runtime", "Example")
            .with_version("1")
            .with_extension(RuntimeExtensionManifest {
                settings: Some(
                    RuntimeSettingsManifest::new(1)
                        .with_schema(serde_json::json!({"type": "object"}))
                        .with_defaults(serde_json::json!({})),
                ),
                endpoints: vec![RuntimeEndpointSpec::put("/settings")
                    .with_description("Update runtime settings")],
            });

        assert_eq!(manifest.id, "example-runtime");
        assert_eq!(manifest.extension.endpoints[0].path, "/settings");
        assert_eq!(
            manifest.extension.endpoints[0].method,
            RuntimeHttpMethod::Put
        );
    }
}
