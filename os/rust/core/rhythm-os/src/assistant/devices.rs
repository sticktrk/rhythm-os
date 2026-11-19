//! Reviewed room, device, integration and appliance operations. Each preview
//! names a particular supported mutation. Callers never supply an HTTP route.
use super::{
    canonical_json_sha256, object_schema, operation, LightAssistantError as Error,
    LightAssistantOperationDto, OperationTier,
};
use crate::canonical::{
    identity::HubKey,
    triage::{TriageEntry, TriageKind, TriageStatus},
};
use crate::{
    commands, handlers,
    hub::HubType,
    pairing,
    state::{AppState, SharedState},
    topology::NodeControlKind,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeSet, VecDeque},
    future::Future,
    pin::Pin,
    sync::{Arc, MutexGuard},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Error>;
pub type PlatformFuture = Pin<Box<dyn Future<Output = Result<Value>> + Send>>;
pub type PlatformHandler = Arc<dyn Fn(SharedState, PlatformAction) -> PlatformFuture + Send + Sync>;

#[derive(Clone, Debug)]
pub enum PlatformAction {
    ReadHaSelection,
    SetHaSelection {
        entities: BTreeSet<String>,
        expected_entities: BTreeSet<String>,
        expected_revision: String,
    },
    ReadUpdateStatus,
    CheckUpdate,
    Update {
        expected_revision: String,
    },
    Restart,
}

#[derive(Default)]
pub struct DeviceSessions {
    plans: VecDeque<DevicePlan>,
    pub platform: Option<PlatformHandler>,
}

struct DevicePlan {
    id: String,
    hub: String,
    created: Instant,
    change: Change,
    before: Value,
}

#[derive(Clone)]
enum Change {
    CreateRoom(String),
    RenameRoom(String, String),
    MergeRooms(String, String),
    DeleteRoom(String),
    RenameDevice(String, String),
    RemoveDevice(String, Option<String>),
    ControlTargets(String, Vec<String>),
    Triage {
        entry: String,
        decision: String,
        target: Option<String>,
    },
    PairBridge {
        bridge: HubKey,
        serial: Option<String>,
    },
    PairNearbyHue,
    RetryBridge(HubKey),
    HaSelection {
        before: BTreeSet<String>,
        after: BTreeSet<String>,
        revision: String,
    },
    Update(Value),
    Restart,
}

fn invalid(message: &str) -> Error {
    Error::bad_request("invalid_device_request", message)
}
fn stale() -> Error {
    Error::conflict(
        "device_plan_changed",
        "Refresh the selected hub and review a new plan.",
    )
}
fn lock(state: &SharedState) -> Result<MutexGuard<'_, AppState>> {
    state
        .lock()
        .map_err(|_| Error::internal("state_unavailable", "The hub state is unavailable."))
}
fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 256 && !s.chars().any(char::is_control))
        .ok_or_else(|| invalid("Supply a nonempty ID or name from the operation's schema."))
}
fn strings(v: &Value, key: &str, limit: usize) -> Result<BTreeSet<String>> {
    let list = v[key]
        .as_array()
        .filter(|v| v.len() <= limit)
        .ok_or_else(|| invalid("The requested target list is invalid."))?;
    list.iter()
        .map(|v| {
            v.as_str()
                .filter(|s| !s.is_empty() && s.len() <= 256)
                .map(str::to_owned)
                .ok_or_else(|| invalid("Each target needs a valid ID."))
        })
        .collect()
}

pub(super) fn operations() -> Vec<LightAssistantOperationDto> {
    let id = json!({"type":"string","minLength":1,"maxLength":256});
    let ids = json!({"type":"array","items":id,"uniqueItems":true,"maxItems":4096});
    let mut ops = vec![];
    let mut add = |name, description, required: &[&str], properties: Value, tier| {
        ops.push(operation(
            name,
            description,
            tier,
            object_schema(required, properties),
            json!({"type":"object"}),
        ));
    };
    use OperationTier::{PersistentChange, Read};
    add(
        "read_device_review",
        "Inspect pending device matches, room binding proposals and unreachable-device reviews.",
        &[],
        json!({}),
        Read,
    );
    add("read_device_connections","List the current integration endpoints and physical room for one device. For a device with multiple connections, select a reviewed endpoint before removal.",&["device_id"],json!({"device_id":id}),Read);
    add("read_integrations","List configured bridges, connection status and supported onboarding methods. Use the returned bridge_id to select a known integration.",&[],json!({}),Read);
    add("guide_device_setup","Return the owner, phone or Home Assistant step required for commissioning, pairing secrets, enrollment, bridge credentials or archived devices. Refresh on the same hub afterward.", &["step"],json!({"step":{"enum":["pair","ha_matter","mobile_enrollment","bridge_credentials","bridge_authority","archived_device"]}}),Read);
    add("read_pairing_result","Inspect the status and failure stage of a known pairing attempt, including a pairing started from the phone.",&["session_id"],json!({"session_id":id}),Read);
    add(
        "plan_create_room",
        "Preview creating an empty Rhythm room.",
        &["name"],
        json!({"name":id}),
        Read,
    );
    add(
        "plan_rename_room",
        "Review renaming this room and any integration-managed room groups.",
        &["room_id", "name"],
        json!({"room_id":id,"name":id}),
        Read,
    );
    add("plan_merge_rooms","Review moving devices and bindings from the source room into the target and removing the source room.",&["source_room_id","target_room_id"],json!({"source_room_id":id,"target_room_id":id}),Read);
    add(
        "plan_delete_room",
        "Review removing the selected room, its source binding and resulting unassigned devices.",
        &["room_id"],
        json!({"room_id":id}),
        Read,
    );
    add("plan_rename_device","Review the physical device and replacement name. A manual rename transfers naming to the user.",&["device_id","name"],json!({"device_id":id,"name":id}),Read);
    add("plan_remove_device","Review removing a known device or a single connection on a multi-integration device. Read its connections to choose an endpoint. Review whether the operation unpairs hardware or only forgets its Rhythm record.",&["device_id"],json!({"device_id":id,"endpoint_id":id}),Read);
    add("plan_control_targets","Review all replacement control rooms for one sensor, button or switch. An empty list restores its inherited physical room.",&["device_id","room_ids"],json!({"device_id":id,"room_ids":ids}),Read);
    add("plan_device_review","Review resolving one pending device match or room binding. Select a listed candidate for merge or bind. For an unassigned device use the existing reviewed device-room move.",&["entry_id","decision"],json!({"entry_id":id,"decision":{"enum":["merge","bind","keep_separate","dismiss","snooze","still_installed"]},"target_id":id}),Read);
    add("plan_nearby_hue_lights", "Review pairing nearby Hue Bluetooth lights with this hub. Put only the intended bulbs in setup mode. Search may pair multiple nearby bulbs.", &[],json!({}),Read);
    add("plan_bridge_pairing","Review searching a configured Hue bridge. Provide a six-character printed serial for a light or omit it for a nearby button in pairing mode. Other commissioning methods return a physical setup handoff.",&["bridge_id","device_kind"],json!({"bridge_id":id,"device_kind":{"enum":["light","button"]},"serial":id}),Read);
    add(
        "plan_bridge_retry",
        "Review reconnecting one configured bridge using its stored authorization.",
        &["bridge_id"],
        json!({"bridge_id":id}),
        Read,
    );
    add("read_managed_ha_lights","Inspect HA's current reviewed lights and selectable registry identities on a Home Assistant add-on.",&[],json!({}),Read);
    add("plan_managed_ha_lights","Review replacing the complete set of HA lights delegated to Rhythm. Pause Rhythm before applying. Other entities stay under HA control.",&["entity_ids"],json!({"entity_ids":ids}),Read);
    add("read_appliance_update_status","Inspect the current OTA stage, version and last check without contacting the update feed. Home Assistant owns add-on updates.",&[],json!({}),Read);
    add("check_hub_update","Read the current version and check the configured update feed. Home Assistant owns add-on updates.",&[],json!({}),Read);
    add(
        "plan_hub_update",
        "Review the currently offered release and restart before applying an appliance update.",
        &[],
        json!({}),
        Read,
    );
    add("plan_hub_restart","Review restarting Rhythm on this selected hub. Active connections will drop until it is running again.",&[],json!({}),Read);
    add("apply_device_plan","Apply the exact room, device, integration or appliance proposal reviewed by the person. Reread state before retrying an uncertain operation.",&["plan_id","server_instance_id","confirmed"],json!({"plan_id":id,"server_instance_id":id,"confirmed":{"const":true,"type":"boolean"}}),PersistentChange);
    ops
}

fn bridge_id(key: &HubKey) -> String {
    canonical_json_sha256(&json!(key))
}
fn bridge(s: &AppState, id: &str) -> Result<HubKey> {
    s.hub_credentials
        .keys()
        .find(|k| bridge_id(k) == id)
        .cloned()
        .ok_or_else(|| invalid("Select a configured bridge from read_integrations."))
}
fn integrations(s: &AppState) -> Value {
    let mut configured:Vec<_>=s.hub_credentials.keys().map(|k|json!({"bridge_id":bridge_id(k),"integration":k.hub_type.as_str(),"connected":s.hub_is_connected(k),"configured":true,"room_automation_owners":s.topology.external_automation_rooms_for_hub(k)})).collect();
    configured.sort_by_key(|v| v["bridge_id"].as_str().map(str::to_owned));
    let supported:Vec<_>=s.hub_capabilities.iter().map(|c|json!({"integration":c.hub_type,"onboarding":c.device_onboarding_methods,"supports_removal":c.supports_unpairing})).collect();
    json!({"configured":configured,"supported":supported})
}

fn review_entry(e: &TriageEntry) -> Value {
    let rooms = e.room_binding.as_ref().map(|b|json!({"source":b.hub_room_name,"suggested_room":b.target_rhythm_room_id,"candidates":b.candidate_rooms}));
    json!({"entry_id":e.id,"kind":e.kind,"device_id":e.canonical_id,"name":e.discovered.name,"candidate_matches":e.candidate_matches,"room_binding":rooms,"integration":e.hub_key.hub_type.as_str()})
}
fn handoff(s: &AppState, surface: &str, actor: &str, instruction: &str) -> Value {
    json!({"status":"handoff","server_instance_id":s.server_instance_id,"actor":actor,"surface":surface,"instructions":instruction,"resume_operation":"get_topology_snapshot"})
}
fn ha_area(s: &AppState) -> Value {
    handoff(s,"home_assistant","ha_administrator","Manage areas and device membership in Home Assistant under Settings > Areas and Devices. Refresh this same hub afterward.")
}
fn setup(s: &AppState, step: &str) -> Result<Value> {
    Ok(match step {
        "ha_matter"=>handoff(s,"home_assistant","owner","Open Matter in the add-on panel to review HA-owned devices, scan a setup code, share a device or confirm removal."),
        "pair" if s.platform_context=="ha_addon"=>handoff(s,"home_assistant","resident","Open Add Device on your phone for this hub, scan the code and complete the Home Assistant pairing step."),
        "pair"=>handoff(s,"rhythm_app","resident","Open Add Device on the phone for this hub. Scan the pairing code or use the device's nearby pairing flow, then refresh discovery here."),
        "mobile_enrollment"=>handoff(s,"home_assistant_addon","owner","In the add-on panel, open Connect mobile app and scan the owner-issued enrollment code with the phone."),
        "bridge_authority"=>handoff(s,"rhythm_app","owner","Review the per-room Hue versus Rhythm automation owner and bridge topology sync in the Hue Bridge settings."),
        "bridge_credentials"=>handoff(s,"rhythm_app","owner","Open the hub's integration settings to connect or disconnect a bridge and authorize its credentials."),
        "archived_device"=>handoff(s,"rhythm_app","owner","Open Removed Devices and review any permanent deletion on the owner device."),
        _=>return Err(invalid("Unknown device setup step."))
    })
}

fn room(s: &AppState, id: &str) -> Result<Value> {
    let r = s
        .topology
        .get(id)
        .ok_or_else(|| invalid("Select an existing Rhythm room."))?;
    let devices: Vec<_> = r.devices.iter().map(|d| &d.device_id).collect();
    let bindings:Vec<_>=r.hub_room_bindings.iter().map(|b|json!({"bridge_id":bridge_id(&b.hub_key),"integration":b.hub_key.hub_type.as_str(),"room_id":b.hub_room_id})).collect();
    Ok(json!({"room_id":r.id,"name":r.name,"devices":devices,"integration_rooms":bindings}))
}
fn endpoint_id(ep: &crate::canonical::identity::IntegrationEndpoint) -> String {
    canonical_json_sha256(&json!([
        ep.hub_key,
        ep.native_id,
        ep.capabilities.as_ref().and_then(|c| c.get("ha_identity"))
    ]))
}
fn selected_endpoint<'a>(
    d: &'a crate::canonical::identity::CanonicalDevice,
    selected: Option<&str>,
) -> Result<Option<&'a crate::canonical::identity::IntegrationEndpoint>> {
    match selected {
        Some(id) => d
            .active_endpoints()
            .find(|ep| endpoint_id(ep) == id)
            .map(Some)
            .ok_or_else(|| invalid("Select a current endpoint from read_device_connections.")),
        None if d.endpoints.len() <= 1 => Ok(d.active_endpoints().next()),
        None => Err(invalid(
            "This device has multiple connections. Read its connections and select an endpoint_id.",
        )),
    }
}
fn device(s: &AppState, id: &str) -> Result<Value> {
    let d = s
        .canonical_registry
        .get(id)
        .filter(|d| !d.is_removed() && s.topology.has_public_node(id))
        .ok_or_else(|| invalid("Select an active discovered device."))?;
    let endpoints:Vec<_>=d.active_endpoints().map(|ep|json!({"bridge_id":bridge_id(&ep.hub_key),"integration":ep.hub_key.hub_type.as_str(),"endpoint_id":endpoint_id(ep)})).collect();
    Ok(
        json!({"device_id":d.id,"name":d.name,"kind":d.device_type,"parent_id":s.topology.device_parent_room_id(id),"endpoints":endpoints}),
    )
}
fn platform_handoff(s: &AppState, op: &str) -> Option<Value> {
    let addon = s.platform_context == "ha_addon";
    match op {
        "plan_create_room"|"plan_rename_room"|"plan_merge_rooms"|"plan_delete_room"|"plan_rename_device"|"plan_remove_device" if addon =>Some(ha_area(s)),
        "plan_bridge_pairing"|"plan_nearby_hue_lights" if addon=>setup(s,"pair").ok(),
        "read_appliance_update_status"|"check_hub_update"|"plan_hub_restart"|"plan_hub_update" if addon=>Some(handoff(s,"home_assistant","ha_administrator","Open Settings > Apps > Rhythm to review the update or restart the add-on.")),
        "read_managed_ha_lights"|"plan_managed_ha_lights" if !addon=>Some(handoff(s,"home_assistant","ha_administrator","Light delegation is managed on the Home Assistant add-on. Select that hub to review its lights.")),
        _=>None
    }
}

async fn platform(state: &SharedState, action: PlatformAction) -> Result<Value> {
    let handler = lock(state)?
        .assistant_devices
        .platform
        .clone()
        .ok_or_else(|| invalid("This platform does not provide this operation."))?;
    handler(state.clone(), action).await
}

pub(super) async fn call(state: &SharedState, id: &str, args: Value) -> Result<Value> {
    {
        let s = lock(state)?;
        if let Some(next) = platform_handoff(&s, id) {
            return Ok(next);
        }
        match id {
            "read_device_connections" => return device(&s, field(&args, "device_id")?),
            "guide_device_setup" => return setup(&s, field(&args, "step")?),
            "read_integrations" => return Ok(integrations(&s)),
            "read_device_review" => {
                let entries: Vec<_> = s
                    .canonical_registry
                    .triage()
                    .pending()
                    .into_iter()
                    .map(review_entry)
                    .collect();
                let attention:Vec<_>=s.device_health.visible_records().into_iter().map(|r|json!({"entry_id":r.entry_id,"device_id":r.canonical_id,"status":r.status,"failure_count":r.failure_count})).collect();
                return Ok(json!({"entries":entries,"attention":attention}));
            }
            _ => {}
        }
    }
    match id {
        "apply_device_plan" => apply(state, &args).await,
        "read_pairing_result" => {
            let session = field(&args, "session_id")?;
            pairing::validate_pairing_session_id(session).map_err(invalid)?;
            let record = pairing::lookup_pairing_result(state, session)
                .map_err(|_| invalid("Pairing evidence is unavailable."))?;
            Ok(match record {
                None => {
                    json!({"status":"not_found","session_id":session,"next_check":"Refresh discovery before retrying pairing."})
                }
                Some(record) => {
                    json!({"status":record.state,"session_id":session,"integration":record.hub_type,"pairing_status":record.result.as_ref().map(|r|&r.status),"failure_stage":record.result.as_ref().and_then(|r|r.failure_stage.as_ref())})
                }
            })
        }
        "read_managed_ha_lights" => platform(state, PlatformAction::ReadHaSelection).await,
        "read_appliance_update_status" => platform(state, PlatformAction::ReadUpdateStatus).await,
        "check_hub_update" => platform(state, PlatformAction::CheckUpdate).await,
        _ => preview(state, id, &args).await,
    }
}

async fn preview(state: &SharedState, id: &str, args: &Value) -> Result<Value> {
    let change = match id {
        "plan_create_room" => Change::CreateRoom(field(args, "name")?.to_owned()),
        "plan_rename_room" => Change::RenameRoom(
            field(args, "room_id")?.to_owned(),
            field(args, "name")?.to_owned(),
        ),
        "plan_merge_rooms" => Change::MergeRooms(
            field(args, "source_room_id")?.to_owned(),
            field(args, "target_room_id")?.to_owned(),
        ),
        "plan_delete_room" => Change::DeleteRoom(field(args, "room_id")?.to_owned()),
        "plan_rename_device" => Change::RenameDevice(
            field(args, "device_id")?.to_owned(),
            field(args, "name")?.to_owned(),
        ),
        "plan_remove_device" => Change::RemoveDevice(
            field(args, "device_id")?.to_owned(),
            args.get("endpoint_id")
                .map(|_| field(args, "endpoint_id").map(str::to_owned))
                .transpose()?,
        ),
        "plan_control_targets" => Change::ControlTargets(
            field(args, "device_id")?.to_owned(),
            strings(args, "room_ids", 256)?.into_iter().collect(),
        ),
        "plan_device_review" => Change::Triage {
            entry: field(args, "entry_id")?.to_owned(),
            decision: field(args, "decision")?.to_owned(),
            target: args
                .get("target_id")
                .map(|_| field(args, "target_id").map(str::to_owned))
                .transpose()?,
        },
        "plan_nearby_hue_lights" => Change::PairNearbyHue,
        "plan_bridge_retry" | "plan_bridge_pairing" => {
            let s = lock(state)?;
            let key = bridge(&s, field(args, "bridge_id")?)?;
            if id == "plan_bridge_retry" {
                Change::RetryBridge(key)
            } else {
                if key.hub_type.as_str() != HubType::HUE {
                    return setup(&s, "pair");
                }
                let serial = match field(args, "device_kind")? {
                    "button" if args.get("serial").is_none() => None,
                    "light" => Some(field(args, "serial")?.to_ascii_uppercase()),
                    _ => return Err(invalid(
                        "Select a light with a six-character serial or a button without a serial.",
                    )),
                };
                if serial
                    .as_ref()
                    .is_some_and(|s| s.len() != 6 || !s.bytes().all(|c| c.is_ascii_hexdigit()))
                {
                    return Err(invalid("Use the six-character Hue light serial."));
                }
                Change::PairBridge {
                    bridge: key,
                    serial,
                }
            }
        }
        "plan_managed_ha_lights" => {
            let catalog = platform(state, PlatformAction::ReadHaSelection).await?;
            let before = strings(&catalog, "entities", 4096)?;
            let after = strings(args, "entity_ids", 4096)?;
            let available = catalog["available"].as_array().ok_or_else(stale)?;
            if lock(state)?.light_breaker_enabled {
                return Err(invalid(
                    "Pause Rhythm before reviewing a managed-light change.",
                ));
            }
            let removing_only = after.is_subset(&before);
            if !after.iter().all(|id| {
                available.iter().any(|item| {
                    item["entity_id"] == *id
                        && (item["reviewable"] == true
                            || (removing_only && item["reviewed_identity_current"] == true))
                })
            }) {
                return Err(invalid(
                    "Select only currently reviewable Home Assistant lights.",
                ));
            }
            Change::HaSelection {
                before,
                after,
                revision: field(&catalog, "snapshot_revision")?.to_owned(),
            }
        }
        "plan_hub_update" => {
            let update = platform(state, PlatformAction::CheckUpdate).await?;
            if update["update_available"] != true {
                return Ok(json!({"status":"up_to_date","update":update}));
            }
            Change::Update(update)
        }
        "plan_hub_restart" => Change::Restart,
        _ => return Err(invalid("Unknown device operation.")),
    };
    let mut s = lock(state)?;
    let before = review(&s, &change)?;
    if before["status"] == "handoff" {
        return Ok(before);
    }
    let plan_id = uuid::Uuid::new_v4().to_string();
    let hub = s.server_instance_id.clone();
    let result = json!({"status":"ready","server_instance_id":hub,"plan_id":plan_id,"operation":"apply_device_plan","change":id,"before":before,"requested":args,"requires_confirmation":true,"expires_in_seconds":300});
    let plans = &mut s.assistant_devices.plans;
    plans.retain(|p| p.created.elapsed() < Duration::from_secs(300));
    if plans.len() >= 64 {
        plans.pop_front();
    }
    plans.push_back(DevicePlan {
        id: plan_id,
        hub,
        created: Instant::now(),
        change,
        before,
    });
    Ok(result)
}

fn control_kind(s: &AppState, id: &str) -> Result<NodeControlKind> {
    s.canonical_registry
        .get(id)
        .and_then(|d| NodeControlKind::default_for_device_type(&d.device_type))
        .ok_or_else(|| invalid("Select a motion sensor, switch or button."))
}
fn review(s: &AppState, change: &Change) -> Result<Value> {
    match change {
        Change::CreateRoom(_) => {
            Ok(json!({"rooms":s.topology.room_count(),"effect":"create_empty_room"}))
        }
        Change::RenameRoom(id, _) | Change::DeleteRoom(id) => room(s, id),
        Change::MergeRooms(a, b) => {
            if a == b {
                return Err(invalid("Select two different rooms."));
            }
            Ok(json!({"source":room(s,a)?,"target":room(s,b)?}))
        }
        Change::RenameDevice(id, _) => device(s, id),
        Change::RemoveDevice(id, selected) => {
            let mut info = device(s, id)?;
            let d = s.canonical_registry.get(id).ok_or_else(stale)?;
            let ep = selected_endpoint(d, selected.as_deref())?;
            let physical = ep.or_else(|| d.endpoints.first());
            let kind = physical.map(|ep| ep.hub_key.hub_type.as_str());
            if matches!(kind, Some(HubType::HA | "homeassistant")) {
                return Ok(handoff(s,"home_assistant","ha_administrator","Review the device and its integration in Home Assistant under Settings > Devices & services."));
            }
            let native = matches!(
                kind,
                Some(HubType::MATTER | HubType::HUE | HubType::HUE_BLE | HubType::LOCAL_BLE)
            );
            let unpairable = ep.is_some_and(|ep| {
                s.hub_capabilities.iter().any(|cap| {
                    let device_type = info["kind"].as_str().unwrap_or("");
                    cap.hub_type == ep.hub_key.hub_type.as_str()
                        && cap.supports_unpairing
                        && (cap
                            .unpairable_device_types
                            .iter()
                            .any(|kind| kind == device_type)
                            || (cap.hub_type == HubType::HUE
                                && cap.unpairable_device_types.is_empty()
                                && device_type == "light"))
                })
            });
            if !unpairable && (native || d.endpoints.len() > 1) {
                return Ok(handoff(s,"integration_settings","owner","Review this endpoint in its integration settings. Refresh the hub after removing it there."));
            }
            info["effect"] = json!(if unpairable {
                "unpair_endpoint"
            } else {
                "forget_rhythm_record"
            });
            info["selected_endpoint_id"] = json!(ep.map(endpoint_id));
            Ok(info)
        }
        Change::ControlTargets(id, rooms) => {
            let source = device(s, id)?;
            let kind = control_kind(s, id)?;
            let reachable = s.rhythm_automation_allowed_for_node(id)
                && rooms.iter().all(|id| {
                    s.topology.get(id).is_some() && s.rhythm_automation_allowed_for_node(id)
                });
            if !reachable {
                return Err(invalid(
                    "Select rooms and a source under Rhythm automation authority.",
                ));
            }
            Ok(
                json!({"source":source,"kind":kind,"current_targets":s.topology.effective_control_targets(id,&kind),"requested_rooms":rooms.iter().map(|id|room(s,id)).collect::<Result<Vec<_>>>()?}),
            )
        }
        Change::Triage {
            entry,
            decision,
            target,
        } => {
            if matches!(decision.as_str(), "snooze" | "still_installed") {
                let record = s
                    .device_health
                    .visible_records()
                    .into_iter()
                    .find(|r| r.entry_id.as_ref() == Some(entry))
                    .ok_or_else(stale)?;
                return Ok(
                    json!({"device_id":record.canonical_id,"status":record.status,"journey":record.review_correlation_id}),
                );
            }
            let e = s
                .canonical_registry
                .triage()
                .get(entry)
                .filter(|e| e.status == TriageStatus::Pending)
                .ok_or_else(stale)?;
            let valid = match (e.kind.clone(), decision.as_str(), target.as_deref()) {
                (TriageKind::DeviceMerge, "merge", Some(id)) => {
                    e.candidate_matches.iter().any(|m| m.canonical_id == id)
                }
                (TriageKind::RoomBinding, "bind", Some(id)) => {
                    e.room_binding.as_ref().is_some_and(|b| {
                        b.target_rhythm_room_id == id || b.candidate_rooms.iter().any(|r| r.0 == id)
                    })
                }
                (
                    TriageKind::UnassignedDevice
                    | TriageKind::RoomBinding
                    | TriageKind::DeviceMerge,
                    "keep_separate",
                    None,
                ) => true,
                (_, "dismiss", None) => true,
                _ => false,
            };
            if !valid {
                return Err(invalid(
                    "Choose an action and candidate offered for this pending review.",
                ));
            }
            if s.platform_context == "ha_addon" && !matches!(decision.as_str(), "bind" | "dismiss")
            {
                return Ok(ha_area(s));
            }
            Ok(review_entry(e))
        }
        Change::PairNearbyHue => {
            let supported = s.hub_capabilities.iter().any(|cap| {
                cap.hub_type == HubType::HUE_BLE
                    && cap.device_onboarding_methods.iter().any(|method| {
                        method == crate::hub::DEVICE_ONBOARDING_METHOD_HUE_BLE_NEARBY_SCAN
                    })
            });
            if !supported {
                return setup(s, "pair");
            }
            Ok(
                json!({"integration":HubType::HUE_BLE,"scope":"nearby_lights_in_setup_mode","effect":"search_and_pair","physical_step":"Place only the intended bulbs in Bluetooth setup mode."}),
            )
        }
        Change::PairBridge { bridge: key, .. } | Change::RetryBridge(key) => {
            let cred = s
                .hub_credentials
                .get(key)
                .filter(|c| c.can_connect())
                .ok_or_else(stale)?;
            let capability = s
                .hub_capabilities
                .iter()
                .find(|c| c.hub_type == key.hub_type.as_str())
                .ok_or_else(stale)?;
            if let Change::PairBridge { serial, .. } = change {
                let method = if serial.is_some() {
                    crate::hub::DEVICE_ONBOARDING_METHOD_HUE_BRIDGE_SERIAL_SEARCH
                } else {
                    crate::hub::DEVICE_ONBOARDING_METHOD_HUE_BRIDGE_BUTTON_SEARCH
                };
                if !capability
                    .device_onboarding_methods
                    .iter()
                    .any(|v| v == method)
                {
                    return Err(invalid(
                        "This integration does not support this pairing method.",
                    ));
                }
            }
            Ok(
                json!({"bridge_id":bridge_id(key),"integration":cred.hub_type,"connected":s.hub_is_connected(key),"methods":capability.device_onboarding_methods}),
            )
        }
        Change::HaSelection {
            before,
            after,
            revision,
        } => Ok(
            json!({"current_entities":before,"requested_entities":after,"snapshot_revision":revision,"light_breaker_enabled":s.light_breaker_enabled}),
        ),
        Change::Update(update) => Ok(update.clone()),
        Change::Restart => Ok(
            json!({"version":s.firmware_version,"platform":s.platform_context,"effect":"disconnect_and_restart"}),
        ),
    }
}

fn changed(message: &str) -> Error {
    Error {
        status: 503,
        code: "device_outcome_uncertain",
        message: message.into(),
        retryable: false,
        mutation_may_have_applied: true,
    }
}
fn result_body(response: handlers::ApiResponse) -> Result<Value> {
    if !(200..300).contains(&response.status) {
        return Err(changed(
            "The integration could not confirm the request. Refresh devices before retrying.",
        ));
    }
    if response.body.is_empty() {
        Ok(Value::Null)
    } else {
        serde_json::from_str(&response.body)
            .map_err(|_| changed("The integration returned an unreadable receipt."))
    }
}

async fn apply(state: &SharedState, args: &Value) -> Result<Value> {
    let id = field(args, "plan_id")?;
    let hub = field(args, "server_instance_id")?;
    if args["confirmed"] != true {
        return Err(invalid(
            "Review and explicitly confirm this proposal first.",
        ));
    }
    let plan = {
        let mut s = lock(state)?;
        if hub != s.server_instance_id {
            return Err(stale());
        }
        let plans = &mut s.assistant_devices.plans;
        let index = plans
            .iter()
            .position(|p| {
                p.id == id && p.hub == hub && p.created.elapsed() < Duration::from_secs(300)
            })
            .ok_or_else(stale)?;
        plans.remove(index).ok_or_else(stale)?
    };
    let check = |s: &AppState| -> anyhow::Result<()> {
        anyhow::ensure!(
            s.server_instance_id == plan.hub
                && review(s, &plan.change).ok().as_ref() == Some(&plan.before),
            "device_plan_changed"
        );
        Ok(())
    };
    {
        let s = lock(state)?;
        check(&s).map_err(|_| stale())?;
    }
    let map_write = |e: anyhow::Error| {
        if e.to_string() == "device_plan_changed" {
            stale()
        } else {
            changed("The change could not be verified. Reread this device or room before retrying.")
        }
    };
    let result = match &plan.change {
        Change::CreateRoom(name) => {
            let output = commands::do_topology_create_room_reviewed(state, name, &check)
                .map_err(map_write)?;
            serde_json::from_str(&output).map_err(|_| changed("Refresh the list of rooms."))?
        }
        Change::RenameRoom(room, name) => {
            commands::do_topology_rename_room_reviewed(state, room, name, &check)
                .map_err(map_write)?;
            json!({"room_id":room})
        }
        Change::DeleteRoom(room) => {
            commands::do_topology_delete_room_reviewed(state, room, &check).map_err(map_write)?;
            json!({"removed_room_id":room})
        }
        Change::MergeRooms(source, target) => {
            commands::do_topology_merge_rooms_reviewed(state, target, source, &check)
                .map_err(map_write)?;
            json!({"target_room_id":target})
        }
        Change::RenameDevice(device, name) => {
            commands::do_canonical_rename_device_reviewed(state, device, name, &check)
                .map_err(map_write)?;
            json!({"device_id":device})
        }
        Change::ControlTargets(source, rooms) => {
            let gate = lock(state)?.external_topology_transaction_lock.clone();
            let _guard = gate.lock().map_err(|_| stale())?;
            let mut s = lock(state)?;
            check(&s).map_err(map_write)?;
            let kind = control_kind(&s, source)?;
            commands::set_topology_control_targets_locked(&mut s, source, kind.clone(), rooms)
                .map_err(map_write)?;
            json!({"device_id":source,"control_targets":s.topology.effective_control_targets(source,&kind)})
        }
        Change::RemoveDevice(id, _) if plan.before["effect"] == "forget_rhythm_record" => {
            commands::do_active_device_forget_reviewed(state, id, &check).map_err(map_write)?;
            json!({"removed_device_id":id,"scope":"rhythm_record"})
        }
        Change::RemoveDevice(id, selected) => {
            let endpoint = {
                let s = lock(state)?;
                check(&s).map_err(map_write)?;
                let d = s.canonical_registry.get(id).ok_or_else(stale)?;
                selected_endpoint(d, selected.as_deref())?
                    .cloned()
                    .ok_or_else(stale)?
            };
            let response = handlers::handle_unpair_device_reviewed(
                state,
                &pairing::UnpairingRequest {
                    hub_type: endpoint.hub_key.hub_type.as_str().into(),
                    params: json!({"device_id":endpoint.native_id,"hub_address":endpoint.hub_key.address,"force":false}),
                },
                &check,
            );
            if response.status == 409 {
                return Err(stale());
            }
            let result = result_body(response)?;
            if result["status"] != "complete" {
                return Err(changed(
                    "Removal was not verified. Reread the integration before retrying.",
                ));
            }
            json!({"device_id":id,"scope":result["completion_scope"],"integration_status":result["status"]})
        }
        Change::Triage {
            entry,
            decision,
            target,
        } => {
            match decision.as_str() {
                "merge" => commands::do_triage_merge_reviewed(
                    state,
                    entry,
                    target.as_deref().ok_or_else(stale)?,
                    &check,
                )
                .map_err(map_write)?,
                "bind" => commands::do_triage_bind_room_to_reviewed(
                    state,
                    entry,
                    target.as_deref(),
                    &check,
                )
                .map_err(map_write)?,
                "dismiss" => {
                    commands::do_triage_dismiss_reviewed(state, entry, &check).map_err(map_write)?
                }
                "keep_separate" => {
                    commands::do_triage_new_device_reviewed(state, entry, &check)
                        .map_err(map_write)?;
                }
                "snooze" | "still_installed" => {
                    let action = if decision == "snooze" {
                        commands::do_device_attention_snooze
                    } else {
                        commands::do_device_attention_still_installed
                    };
                    action(state, entry, plan.before["journey"].as_str()).map_err(|_| stale())?;
                }
                _ => return Err(invalid("Unknown device review action.")),
            }
            json!({"entry_id":entry,"decision":decision})
        }
        Change::RetryBridge(key) => {
            commands::do_retry_hub_connect_reviewed(
                state,
                key.hub_type.as_str(),
                &key.address,
                &check,
            )
            .map_err(map_write)?;
            let s = lock(state)?;
            json!({"bridge_id":bridge_id(key),"connected":s.hub_is_connected(key)})
        }
        Change::PairBridge { .. } | Change::PairNearbyHue => {
            let (integration, params) = match &plan.change {
                Change::PairBridge { bridge, serial } => {
                    let mut params = json!({"hub_address":bridge.address,"device_kind":if serial.is_some(){"light"}else{"button"}});
                    if let Some(serial) = serial {
                        params["serial"] = json!(serial);
                    }
                    (bridge.hub_type.as_str(), params)
                }
                _ => (HubType::HUE_BLE, json!({})),
            };
            let req = pairing::PairingRequest {
                hub_type: integration.into(),
                session_id: Some(plan.id.clone()),
                params,
            };
            let response = handlers::handle_pair_device_reviewed(
                state,
                &req,
                pairing::PairingRequestContext::accepted_now(),
                &check,
            );
            if response.status == 409 {
                return Err(stale());
            }
            let value = result_body(response)?;
            // Pairing results can contain vendor recovery secrets. Return only typed status.
            if value["status"] == "failed" {
                return Err(changed("The bridge pairing attempt failed. Inspect discovery before starting another attempt."));
            }
            json!({"pairing_status":value["status"],"session_id":plan.id,"next_check":"Refresh this hub's topology to verify the discovered device."})
        }
        Change::HaSelection {
            before,
            after,
            revision,
        } => {
            platform(
                state,
                PlatformAction::SetHaSelection {
                    entities: after.clone(),
                    expected_entities: before.clone(),
                    expected_revision: revision.clone(),
                },
            )
            .await?
        }
        Change::Update(info) => {
            platform(
                state,
                PlatformAction::Update {
                    expected_revision: field(info, "review_revision")?.into(),
                },
            )
            .await?
        }
        Change::Restart => platform(state, PlatformAction::Restart).await?,
    };
    Ok(
        json!({"status":"acknowledged","server_instance_id":hub,"plan_id":id,"result":result,"verification":"reread_resource"}),
    )
}

/// Decode the platform's existing typed response inside its adapter. The
/// adapter selects public fields before returning a result to the agent.
pub async fn platform_receipt(response: axum::response::Response) -> Result<Value> {
    let status = response.status();
    if !status.is_success() {
        return Err(if status.as_u16() == 409 || status.as_u16() == 412 {
            stale()
        } else {
            Error {
                status: status.as_u16(),
                code: "platform_action_unavailable",
                message: "Check this hub's status before repeating the platform operation.".into(),
                retryable: false,
                mutation_may_have_applied: status.is_server_error(),
            }
        });
    }
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .map_err(|_| changed("Read the hub state to verify the platform result."))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| changed("Read the hub state to verify the platform result."))
}

pub fn unsupported_platform() -> Error {
    invalid("This operation is unavailable on the selected platform.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn state() -> SharedState {
        Arc::new(Mutex::new(AppState::default()))
    }

    fn confirm(plan: &Value) -> Value {
        json!({"confirmed":true,"plan_id":plan["plan_id"],"server_instance_id":plan["server_instance_id"]})
    }

    #[tokio::test]
    async fn reviewed_room_changes_require_confirmation_current_scope_and_one_application() {
        let state = state();
        let name = "Jo’s \"Music\" room";
        let preview = call(&state, "plan_create_room", json!({"name":name}))
            .await
            .unwrap();
        assert_eq!(preview["operation"], "apply_device_plan");
        assert_eq!(preview["requested"]["name"], name);
        let args = confirm(&preview);
        let mut wrong = args.clone();
        wrong["server_instance_id"] = json!("different-hub");
        assert_eq!(
            call(&state, "apply_device_plan", wrong)
                .await
                .unwrap_err()
                .status,
            409
        );
        let mut declined = args.clone();
        declined["confirmed"] = json!(false);
        assert_eq!(
            call(&state, "apply_device_plan", declined)
                .await
                .unwrap_err()
                .status,
            400
        );
        assert_eq!(lock(&state).unwrap().topology.room_count(), 0);
        let receipt = call(&state, "apply_device_plan", args.clone())
            .await
            .unwrap();
        let room_id = receipt["result"]["id"].as_str().unwrap();
        assert_eq!(room(&lock(&state).unwrap(), room_id).unwrap()["name"], name);
        assert_eq!(
            call(&state, "apply_device_plan", args)
                .await
                .unwrap_err()
                .status,
            409
        );
        let preview = call(&state, "plan_delete_room", json!({"room_id":room_id}))
            .await
            .unwrap();
        assert_eq!(preview["before"]["name"], name);
        lock(&state)
            .unwrap()
            .topology
            .rename_room(room_id, "Other name");
        assert_eq!(
            call(&state, "apply_device_plan", confirm(&preview))
                .await
                .unwrap_err()
                .status,
            409
        );
        assert_eq!(
            room(&lock(&state).unwrap(), room_id).unwrap()["name"],
            "Other name"
        );
    }

    #[tokio::test]
    async fn hardware_removal_checks_the_target_kind_and_delegates_ha_ownership() {
        use crate::canonical::{identity::DiscoveredIdentity, registry::ResolveResult};
        use rhythm_core::DeviceType;
        let state = state();
        let mut ids = Vec::new();
        {
            let mut s = lock(&state).unwrap();
            s.hub_capabilities.push(crate::hub::HubIntegrationCapability {
                hub_type: HubType::HUE.into(),
                configurable: true,
                device_onboarding_methods: vec![],
                device_profiles: vec![],
                supports_unpairing: true,
                unpairable_device_types: vec!["light".into()],
                supports_roomless_devices: true,
                blocks_room_readiness: false,
            });
            for (integration, kind, name) in [
                (HubType::HUE, DeviceType::Motion, "motion"),
                (HubType::HUE, DeviceType::Light, "lamp"),
                ("homeassistant", DeviceType::Contact, "door"),
            ] {
                let key = HubKey::new(HubType::new(integration), "test-bridge");
                let identity = DiscoveredIdentity {
                    native_id: name.into(),
                    name: name.into(),
                    device_type: kind,
                    room_id: None,
                    room_name: None,
                    hardware_ids: vec![],
                    manufacturer: None,
                    model: None,
                };
                let ResolveResult::Created { canonical_id } =
                    s.canonical_registry.resolve(&identity, &key, 1)
                else {
                    panic!("expected a new device")
                };
                s.topology.ensure_standalone_device(&canonical_id);
                ids.push(canonical_id);
            }
        }
        let args = |i: usize| json!({"device_id":ids[i]});
        assert_eq!(
            call(&state, "plan_remove_device", args(0)).await.unwrap()["status"],
            "handoff"
        );
        let endpoints = call(&state, "read_device_connections", args(1))
            .await
            .unwrap();
        let plan = call(
            &state,
            "plan_remove_device",
            json!({"device_id":ids[1],"endpoint_id":endpoints["endpoints"][0]["endpoint_id"]}),
        )
        .await
        .unwrap();
        assert_eq!(plan["before"]["effect"], "unpair_endpoint");
        assert_eq!(
            call(&state, "plan_remove_device", args(2)).await.unwrap()["surface"],
            "home_assistant"
        );
        lock(&state).unwrap().hub_capabilities.clear();
        assert_eq!(
            call(&state, "apply_device_plan", confirm(&plan))
                .await
                .unwrap_err()
                .status,
            409
        );
        assert_eq!(lock(&state).unwrap().canonical_registry.device_count(), 3);
    }

    #[tokio::test]
    async fn ha_area_and_enrollment_handoffs_stay_on_the_selected_hub() {
        let state = state();
        lock(&state).unwrap().platform_context = "ha_addon";
        let request = call(&state, "plan_create_room", json!({"name":"New area"}))
            .await
            .unwrap();
        assert_eq!(request["actor"], "ha_administrator");
        assert!(request.get("plan_id").is_none());
        let owner = call(
            &state,
            "guide_device_setup",
            json!({"step":"mobile_enrollment"}),
        )
        .await
        .unwrap();
        assert_eq!(request["server_instance_id"], owner["server_instance_id"]);
        assert_eq!(owner["actor"], "owner");
        assert_eq!(lock(&state).unwrap().topology.room_count(), 0);
    }

    #[tokio::test]
    async fn managed_ha_selection_is_bound_to_the_reviewed_identity_revision() {
        let state = state();
        let writes = Arc::new(Mutex::new(0usize));
        let applied = writes.clone();
        {
            let mut s = lock(&state).unwrap();
            s.platform_context = "ha_addon";
            s.light_breaker_enabled = false;
            s.assistant_devices.platform = Some(Arc::new(move |_, action| {
                let applied = applied.clone();
                Box::pin(async move {
                    match action {
                        PlatformAction::ReadHaSelection => Ok(
                            json!({"entities":["light.old"],"snapshot_revision":"inventory:42","available":[{"entity_id":"light.lamp","reviewable":true}]}),
                        ),
                        PlatformAction::SetHaSelection {
                            entities,
                            expected_entities,
                            expected_revision,
                        } => {
                            assert_eq!(entities, BTreeSet::from(["light.lamp".into()]));
                            assert_eq!(expected_entities, BTreeSet::from(["light.old".into()]));
                            assert_eq!(expected_revision, "inventory:42");
                            *applied.lock().unwrap() += 1;
                            Ok(json!({"entities":entities}))
                        }
                        _ => panic!("unexpected platform operation"),
                    }
                })
            }));
        }
        assert!(call(
            &state,
            "plan_managed_ha_lights",
            json!({"entity_ids":["switch.unknown"]})
        )
        .await
        .is_err());
        let args = json!({"entity_ids":["light.lamp"]});
        let first = call(&state, "plan_managed_ha_lights", args.clone())
            .await
            .unwrap();
        lock(&state).unwrap().light_breaker_enabled = true;
        assert_eq!(
            call(&state, "apply_device_plan", confirm(&first))
                .await
                .unwrap_err()
                .status,
            409
        );
        assert!(call(&state, "plan_managed_ha_lights", args.clone())
            .await
            .is_err());
        assert_eq!(*writes.lock().unwrap(), 0);
        lock(&state).unwrap().light_breaker_enabled = false;
        let next = call(&state, "plan_managed_ha_lights", args).await.unwrap();
        let receipt = call(&state, "apply_device_plan", confirm(&next))
            .await
            .unwrap();
        assert_eq!(receipt["result"]["entities"], json!(["light.lamp"]));
        assert_eq!(*writes.lock().unwrap(), 1);
    }
}
