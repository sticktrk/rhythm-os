//! Read-side operations with explicitly bounded results and resumable handoffs.
use super::*;

const STEPS: &[(&str, &str, &str)] = &[
    ("first_wifi", "resident", "Open Add Light Box in the Rhythm phone app near the box to provision Wi-Fi over Bluetooth."),
    ("pairing_code", "resident", "Scan the device's setup code in Add Device in the Rhythm phone app."),
    ("phone_location", "resident", "Open location settings in the Rhythm phone app and approve the location prompt."),
    ("apple_health", "resident", "Open Health sync in the iOS app and review the requested permissions."),
    ("ha_integration", "ha_administrator", "Open Settings > Devices & services in Home Assistant and set up the device integration."),
    ("ha_area", "ha_administrator", "Review the device area in Home Assistant, then refresh managed devices in Rhythm."),
    ("ha_location", "ha_administrator", "Set the installation location under Home Assistant Settings > System > General."),
    ("ha_update", "ha_administrator", "Open the Rhythm app in Home Assistant Settings > Apps to review and install an update."),
    ("cloud_account", "resident", "Open the account and home settings in the Rhythm app."),
    ("owner_access", "owner", "Open this hub's settings in the Rhythm app to review ownership, credentials or remote access."),
    ("restore", "owner", "Review the backup and destination in Backup & Restore in the Rhythm app."),
    ("factory_reset", "owner", "Open the selected hub's system settings and review the factory reset."),
];

pub(super) fn operations() -> Vec<LightAssistantOperationDto> {
    let empty = || object_schema(&[], json!({}));
    let result = || json!({"type":"object"});
    vec![
        operation("read_hub_status", "Read this installation's identity, platform, version, light breaker and integration connectivity before diagnosing unavailable lights.", OperationTier::Read, empty(), result()),
        operation("read_lighting_state", "Read the current canonical room and light states. Check each light's evidence before claiming a physical change.", OperationTier::Read, empty(), result()),
        operation("read_lighting_history", "Read the latest lighting actions for one selected node. These are action records, not proof that a light changed.", OperationTier::Read, object_schema(&["node_id"], json!({"node_id":string_schema("Canonical room or light ID"),"limit":{"type":"integer","minimum":1,"maximum":50}})), result()),
        operation("get_handoff", "Explain a step that needs a resident, owner, phone, cloud service or Home Assistant administrator. Resume on the same hub after the person finishes.", OperationTier::Read, object_schema(&["step"], json!({"step":{"enum":STEPS.iter().map(|step|step.0).collect::<Vec<_>>()}})), result()),
    ]
}

pub(super) fn call(
    state: &SharedState,
    id: &str,
    args: Value,
) -> Result<Value, LightAssistantError> {
    if id == "read_lighting_state" {
        return commands::build_nodes_state(state)
            .ok()
            .and_then(|json| serde_json::from_str(&json).ok())
            .ok_or_else(|| {
                LightAssistantError::internal(
                    "state_unavailable",
                    "Could not read current light state",
                )
            });
    }
    let s = state
        .lock()
        .map_err(|_| LightAssistantError::internal("state_unavailable", "Hub state unavailable"))?;
    match id {
        "read_hub_status" => Ok(
            json!({"server_instance_id":s.server_instance_id,"server_version":s.firmware_version,"platform":s.platform_context,"integration_connected":s.has_any_connected_hub(),"light_breaker_enabled":s.light_breaker_enabled,"managed_ha_light_count":s.managed_ha_lights.as_ref().map(|lights|lights.len())}),
        ),
        "read_lighting_history" => {
            let node = validate_id(args["node_id"].as_str().unwrap_or_default(), "node_id")?;
            let limit = match args.get("limit") {
                None => 10,
                Some(value) => {
                    value
                        .as_u64()
                        .filter(|n| (1..=50).contains(n))
                        .ok_or_else(|| {
                            LightAssistantError::bad_request(
                                "invalid_limit",
                                "Choose 1 to 50 entries",
                            )
                        })?
                }
            };
            let entries: Vec<_> = s.light_activity.iter().filter(|event|event.node_id == node).take(limit as usize)
                .map(|event|json!({"at_epoch_ms":event.epoch_ms,"action":event.action_id,"source_kind":event.source.kind,"brightness":event.brightness,"kelvin":event.kelvin})).collect();
            Ok(json!({"node_id":node,"entries":entries}))
        }
        "get_handoff" => {
            let step = STEPS
                .iter()
                .find(|step| args["step"] == step.0)
                .ok_or_else(|| {
                    LightAssistantError::bad_request(
                        "unknown_step",
                        "Choose a step from the contract",
                    )
                })?;
            Ok(
                json!({"status":"handoff","step":step.0,"actor":step.1,"instructions":step.2,"resume_server_instance_id":s.server_instance_id}),
            )
        }
        _ => Err(LightAssistantError::bad_request(
            "unknown_operation",
            "Unknown information operation",
        )),
    }
}
