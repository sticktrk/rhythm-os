//! User-facing light activity history.
//!
//! This feed records intent-level light interactions: app/API controls,
//! physical buttons, mode changes, and explicit scene/profile actions. It
//! deliberately excludes autonomous periodic ticks so the history can answer
//! "when did a person or user-configured control modify the lights?"

use anyhow::Result;
use rhythm_core::ButtonAction;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

use crate::state::SharedState;

pub const LIGHT_ACTIVITY_HISTORY_LIMIT: usize = 2_000;

fn light_activity_schema_version() -> u32 {
    1
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LightActivityHistory {
    #[serde(default = "light_activity_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub activities: Vec<LightActivityEvent>,
}

impl Default for LightActivityHistory {
    fn default() -> Self {
        Self {
            schema_version: light_activity_schema_version(),
            activities: Vec::new(),
        }
    }
}

impl LightActivityHistory {
    pub fn normalized(mut self) -> Self {
        self.schema_version = light_activity_schema_version();
        self.activities.sort_by(|left, right| {
            right
                .epoch_ms
                .cmp(&left.epoch_ms)
                .then_with(|| right.id.cmp(&left.id))
        });
        self.activities.truncate(LIGHT_ACTIVITY_HISTORY_LIMIT);
        self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LightActivitySource {
    pub raw: String,
    pub kind: String,
    pub marks_touched: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LightValueChange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LightActivityTarget {
    #[serde(default)]
    pub node_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hub_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub rhythm_enabled: bool,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub lights_on: bool,
    #[serde(default)]
    pub brightness: u8,
    #[serde(default)]
    pub kelvin: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Value>,
    #[serde(default)]
    pub time_offset_minutes: f32,
    #[serde(default)]
    pub brightness_offset: f32,
    #[serde(default)]
    pub mood_enabled: bool,
    #[serde(default)]
    pub mood_active: bool,
    #[serde(default)]
    pub standby_enabled: bool,
    #[serde(default)]
    pub standby_active: bool,
    #[serde(default)]
    pub transitioning: bool,
    #[serde(default)]
    pub pending_dispatch: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve_modifier: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_settings: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LightActivityEvent {
    pub id: String,
    pub node_id: String,
    #[serde(alias = "action")]
    pub action_id: String,
    pub source: LightActivitySource,
    pub epoch_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_instance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_mode: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<LightActivityTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<LightValueChange>,
    #[serde(default = "default_count")]
    pub count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fanout_of: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kelvin: Option<u16>,
}

fn default_count() -> u32 {
    1
}

fn activity_event_id(epoch_ms: u64) -> String {
    format!(
        "{}-{}",
        crate::logging::next_command_id("activity"),
        epoch_ms
    )
}

impl LightActivityEvent {
    fn response_value(&self) -> Value {
        let ts = self.epoch_ms as f64 / 1000.0;
        json!({
            "id": &self.id,
            "event_id": &self.id,
            "node_id": &self.node_id,
            "area_id": &self.node_id,
            "action": &self.action_id,
            "action_id": &self.action_id,
            "source": &self.source,
            "source_kind": &self.source.kind,
            "source_entity": self
                .source
                .control_id
                .as_deref()
                .unwrap_or(self.source.raw.as_str()),
            "epoch_ms": self.epoch_ms,
            "ts": ts,
            "server_instance_id": &self.server_instance_id,
            "server_version": &self.server_version,
            "platform": &self.platform,
            "active_mode": &self.active_mode,
            "target": &self.target,
            "count": self.count,
            "correlation_id": &self.correlation_id,
            "fanout_of": &self.fanout_of,
            "change": &self.change,
            "payload": &self.payload,
            "brightness": self.brightness,
            "kelvin": self.kelvin,
        })
    }
}

#[derive(Clone, Debug)]
pub struct LightActivityRecord {
    pub node_id: String,
    pub action_id: String,
    pub source_kind: String,
    pub source_raw: String,
    pub source_control_id: Option<String>,
    pub marks_touched: bool,
    pub change: Option<LightValueChange>,
    pub payload: Option<Value>,
    pub correlation_id: Option<String>,
    pub fanout_of: Option<String>,
    pub brightness: Option<u8>,
    pub kelvin: Option<u16>,
}

impl LightActivityRecord {
    pub fn app(node_id: impl Into<String>, action_id: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            action_id: action_id.into(),
            source_kind: "app".to_string(),
            source_raw: "api".to_string(),
            source_control_id: None,
            marks_touched: true,
            change: None,
            payload: None,
            correlation_id: None,
            fanout_of: None,
            brightness: None,
            kelvin: None,
        }
    }

    pub fn physical_button(
        node_id: impl Into<String>,
        action: ButtonAction,
        control_id: Option<&str>,
    ) -> Self {
        let control = control_id.map(str::to_string);
        Self {
            node_id: node_id.into(),
            action_id: button_action_id(action).to_string(),
            source_kind: "switch".to_string(),
            source_raw: control.clone().unwrap_or_else(|| "button".to_string()),
            source_control_id: control,
            marks_touched: true,
            change: None,
            payload: None,
            correlation_id: None,
            fanout_of: None,
            brightness: None,
            kelvin: None,
        }
    }

    pub fn physical_input_binding(
        node_id: impl Into<String>,
        action_id: impl Into<String>,
        source_node_id: &str,
        control_id: Option<&str>,
    ) -> Self {
        let control = control_id
            .map(str::to_string)
            .or_else(|| (!source_node_id.is_empty()).then(|| source_node_id.to_string()));
        Self {
            node_id: node_id.into(),
            action_id: action_id.into(),
            source_kind: "switch".to_string(),
            source_raw: control.clone().unwrap_or_else(|| "button".to_string()),
            source_control_id: control,
            marks_touched: true,
            change: None,
            payload: None,
            correlation_id: None,
            fanout_of: None,
            brightness: None,
            kelvin: None,
        }
    }
}

#[derive(Default)]
pub struct LightActivityQuery {
    pub limit: Option<usize>,
    pub area: Option<String>,
    pub source: Option<String>,
    pub action: Option<String>,
}

pub fn button_action_id(action: ButtonAction) -> &'static str {
    match action {
        ButtonAction::OnPress => "turn_on",
        ButtonAction::Toggle => "toggle",
        ButtonAction::OffPress => "turn_off",
        ButtonAction::Reset => "reset",
        ButtonAction::UpPress => "dim_up",
        ButtonAction::DownPress => "dim_down",
        ButtonAction::UpHold => "step_up",
        ButtonAction::DownHold => "step_down",
        ButtonAction::Stop => "stop",
        ButtonAction::RhythmOn => "circadian_on",
        ButtonAction::RhythmOff => "circadian_off",
        ButtonAction::LightsOff => "lights_off",
        ButtonAction::SleepOn => "sleep_on",
        ButtonAction::SleepOff => "sleep_off",
    }
}

pub fn http_action_id(action: &str) -> String {
    match action {
        "on" | "on_press" => "turn_on",
        "off" | "off_press" => "turn_off",
        "rhythm_on" => "circadian_on",
        "rhythm_off" => "circadian_off",
        "rhythm_toggle" => "toggle",
        "lights_off" => "lights_off",
        "sleep_on" => "sleep_on",
        "sleep_off" => "sleep_off",
        other => other,
    }
    .to_string()
}

pub fn record_light_activity(state: &SharedState, mut record: LightActivityRecord) {
    record.action_id = http_action_id(record.action_id.as_str());
    let epoch_ms = crate::state::current_epoch_ms();
    let target = build_light_activity_target(state, &record.node_id);

    {
        let Ok(mut s) = state.lock() else { return };
        let event = LightActivityEvent {
            id: activity_event_id(epoch_ms),
            node_id: record.node_id,
            action_id: record.action_id,
            source: LightActivitySource {
                raw: record.source_raw,
                kind: record.source_kind,
                marks_touched: record.marks_touched,
                control_id: record.source_control_id,
            },
            epoch_ms,
            server_instance_id: Some(s.server_instance_id.clone()),
            server_version: Some(s.firmware_version.to_string()),
            platform: Some(s.platform_type.to_string()),
            active_mode: Some(json!(s.active_mode)),
            target,
            change: record.change,
            count: 1,
            correlation_id: record.correlation_id,
            fanout_of: record.fanout_of,
            payload: record.payload,
            brightness: record.brightness,
            kelvin: record.kelvin,
        };

        s.light_activity.insert(0, event);
        s.light_activity.truncate(LIGHT_ACTIVITY_HISTORY_LIMIT);
    }

    persist_light_activity_history(state);

    crate::activity_cloud::enqueue_light_activity_upload(state);
}

/// Persist the in-memory activity buffer to storage.
///
/// Persists OUTSIDE the state lock: this runs on every button press and the
/// write is fsync'd — on SD-card storage a slow flush would otherwise hold
/// the global lock and stall the periodic tick, event loop, and all HTTP.
pub(crate) fn persist_light_activity_history(state: &SharedState) {
    let persist = {
        let Ok(s) = state.lock() else { return };
        s.storage.as_ref().map(|storage| {
            (
                storage.clone(),
                LightActivityHistory {
                    schema_version: light_activity_schema_version(),
                    activities: s.light_activity.clone(),
                },
            )
        })
    };
    if let Some((storage, history)) = persist {
        if let Err(e) = storage.save_light_activity_history(&history) {
            log::warn!(target: "cmd", "Failed to save light activity history: {}", e);
        }
    }
}

/// Drain events that were successfully uploaded to the cloud from the buffer.
///
/// Matches by (id, epoch_ms) rather than id alone: pre-epoch-suffix builds
/// reused ids across reboots, and events recorded while the upload was in
/// flight must survive for the next batch.
pub(crate) fn remove_uploaded_light_activity(state: &SharedState, uploaded: &[LightActivityEvent]) {
    let uploaded_keys: std::collections::HashSet<(&str, u64)> = uploaded
        .iter()
        .map(|event| (event.id.as_str(), event.epoch_ms))
        .collect();
    let removed = {
        let Ok(mut s) = state.lock() else { return };
        let before = s.light_activity.len();
        s.light_activity
            .retain(|event| !uploaded_keys.contains(&(event.id.as_str(), event.epoch_ms)));
        s.light_activity.len() != before
    };
    if removed {
        persist_light_activity_history(state);
    }
}

fn build_light_activity_target(state: &SharedState, node_id: &str) -> Option<LightActivityTarget> {
    let node = crate::commands::build_node_state(state, node_id).ok()?;
    Some(LightActivityTarget {
        node_id: node.id,
        name: node.name,
        kind: serialized_string(&node.kind).unwrap_or_else(|| "unknown".to_string()),
        parent_id: node.parent_id,
        placement: node.placement.as_ref().and_then(serialized_string),
        hub_types: node.hub_types,
        manufacturer: node.manufacturer,
        model: node.model,
        state: serialized_string(&node.state).unwrap_or_else(|| "unknown".to_string()),
        rhythm_enabled: node.rhythm_enabled,
        disabled: node.disabled,
        lights_on: node.lights_on,
        brightness: node.brightness,
        kelvin: node.kelvin,
        color: node
            .color
            .and_then(|color| serde_json::to_value(color).ok()),
        time_offset_minutes: node.time_offset,
        brightness_offset: node.brightness_offset,
        mood_enabled: node.mood_enabled,
        mood_active: node.mood_active,
        standby_enabled: node.standby_enabled,
        standby_active: node.standby_active,
        transitioning: node.transitioning,
        pending_dispatch: node.pending_dispatch,
        curve_modifier: serde_json::to_value(node.curve_modifier).ok(),
        profile_settings: serde_json::to_value(node.profile_settings).ok(),
    })
}

fn serialized_string<T: Serialize>(value: &T) -> Option<String> {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
}

pub fn build_light_activity_history(
    state: &SharedState,
    query: LightActivityQuery,
) -> Result<String> {
    let limit = query.limit.unwrap_or(250).clamp(1, 5_000);
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let filtered: Vec<_> = s
        .light_activity
        .iter()
        .filter(|event| matches_query(event, &query))
        .take(limit)
        .cloned()
        .collect();
    let capped = s.light_activity.len() > filtered.len() && filtered.len() == limit;

    let mut sources = BTreeSet::new();
    let mut actions = BTreeSet::new();
    for event in &s.light_activity {
        sources.insert(event.source.kind.clone());
        actions.insert(event.action_id.clone());
    }

    let entries: Vec<Value> = filtered
        .iter()
        .map(LightActivityEvent::response_value)
        .collect();
    let body = json!({
        "schema_version": light_activity_schema_version(),
        "activities": entries.clone(),
        "entries": entries,
        "capped": capped,
        "facets": {
            "sources": sources.into_iter().collect::<Vec<_>>(),
            "actions": actions.into_iter().collect::<Vec<_>>(),
        }
    });
    serde_json::to_string(&body).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

fn matches_query(event: &LightActivityEvent, query: &LightActivityQuery) -> bool {
    if let Some(area) = query.area.as_deref() {
        if event.node_id != area {
            return false;
        }
    }
    if let Some(source) = query.source.as_deref() {
        if event.source.kind != source && event.source.raw != source {
            return false;
        }
    }
    if let Some(action) = query.action.as_deref() {
        if event.action_id != http_action_id(action) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};

    fn test_state() -> SharedState {
        Arc::new(Mutex::new(AppState::default()))
    }

    fn history_value(state: &SharedState, query: LightActivityQuery) -> Value {
        serde_json::from_str(&build_light_activity_history(state, query).unwrap()).unwrap()
    }

    #[test]
    fn records_and_filters_user_light_activity() {
        let state = test_state();

        let mut app_record = LightActivityRecord::app("bedroom", "on");
        app_record.brightness = Some(80);
        app_record.payload = Some(json!({"origin": "test"}));
        record_light_activity(&state, app_record);

        record_light_activity(
            &state,
            LightActivityRecord::physical_button("kitchen", ButtonAction::SleepOn, Some("wall-1")),
        );

        let all = history_value(&state, LightActivityQuery::default());
        let activities = all["activities"].as_array().unwrap();
        assert_eq!(activities.len(), 2);
        assert_eq!(all["entries"], all["activities"]);
        assert_eq!(activities[0]["action"], "sleep_on");
        assert_eq!(activities[0]["source_kind"], "switch");
        assert_eq!(activities[0]["source"]["control_id"], "wall-1");
        assert_eq!(activities[1]["action"], "turn_on");
        assert_eq!(activities[1]["source_kind"], "app");
        assert_eq!(activities[1]["brightness"], 80);

        let app_only = history_value(
            &state,
            LightActivityQuery {
                source: Some("app".to_string()),
                ..LightActivityQuery::default()
            },
        );
        assert_eq!(app_only["activities"].as_array().unwrap().len(), 1);
        assert_eq!(app_only["activities"][0]["node_id"], "bedroom");

        let area_only = history_value(
            &state,
            LightActivityQuery {
                area: Some("kitchen".to_string()),
                ..LightActivityQuery::default()
            },
        );
        assert_eq!(area_only["activities"].as_array().unwrap().len(), 1);
        assert_eq!(area_only["activities"][0]["action_id"], "sleep_on");

        let aliased_action = history_value(
            &state,
            LightActivityQuery {
                action: Some("on".to_string()),
                ..LightActivityQuery::default()
            },
        );
        assert_eq!(aliased_action["activities"].as_array().unwrap().len(), 1);
        assert_eq!(aliased_action["activities"][0]["action"], "turn_on");
    }

    #[test]
    fn record_light_activity_does_not_broadcast_server_events() {
        let state = test_state();
        let (tx, mut rx) = tokio::sync::broadcast::channel(4);
        state.lock().unwrap().event_tx = Some(tx);

        record_light_activity(&state, LightActivityRecord::app("bedroom", "on"));

        assert!(
            rx.try_recv().is_err(),
            "activity recording is analytics-only and must not emit SSE events"
        );
        let s = state.lock().unwrap();
        assert_eq!(s.light_activity.len(), 1);
        assert_eq!(s.light_activity[0].action_id, "turn_on");
        assert!(s.light_activity[0].server_instance_id.is_some());
    }

    #[test]
    fn activity_event_ids_include_epoch_component() {
        let id = activity_event_id(1_783_278_659_924);

        assert!(id.starts_with("activity-"));
        assert!(id.ends_with("-1783278659924"));
    }

    #[test]
    fn remove_uploaded_light_activity_keeps_events_recorded_mid_upload() {
        let state = test_state();
        record_light_activity(&state, LightActivityRecord::app("bedroom", "on"));
        let uploaded = state.lock().unwrap().light_activity.clone();
        record_light_activity(&state, LightActivityRecord::app("kitchen", "off"));

        remove_uploaded_light_activity(&state, &uploaded);

        let s = state.lock().unwrap();
        assert_eq!(s.light_activity.len(), 1);
        assert_eq!(s.light_activity[0].node_id, "kitchen");
    }
}
