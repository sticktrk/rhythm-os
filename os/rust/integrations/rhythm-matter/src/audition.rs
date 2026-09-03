//! Bulb Audition scenario runner.
//!
//! Scenarios submit the controller's real endpoint plans and keep command
//! admission/outcome, direct readback, subscription evidence, and operator
//! observation as separate columns in the schema-v3 report.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rhythm_core::lighting::LightingCommand;
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::HubType;
use rhythm_os::state::SharedState;
use serde_json::{json, Value};

use crate::control_profile::{
    MatterColorRoute, MatterControlProfile, MatterHsWhitePoint, MatterLevelCommand,
    MatterMeasurementBasis, MatterProfileSource, MatterTurnOnStrategy,
};
use crate::controller::MatterLightController;
use crate::hub_state::MatterHubData;
use crate::transport::{
    MatterAttributeReport, MatterAttributeValue, MatterCommandOutcomeStatus, MatterCommandStep,
    MatterControllerEvent, MatterControllerEventCursor, MatterEndpointCommandPlan,
    MatterSubscriptionTarget, MatterTransport, DEFAULT_SUBSCRIPTION_MAX_INTERVAL_SECS,
    DEFAULT_SUBSCRIPTION_MIN_INTERVAL_SECS,
};

const OUTCOME_WAIT_MS: u64 = 2_000;
const SUBSCRIPTION_EVIDENCE_LOOKBACK_MS: u64 = 30_000;
const AUDITION_LIVENESS_MAX_INTERVAL_SECS: u16 = 2;

pub fn run_audition(state: &SharedState, params: &Value) -> Result<Value> {
    let device_id = params
        .get("device_id")
        .and_then(Value::as_str)
        .context("Missing device_id")?;
    let scenario = params
        .get("scenario")
        .or_else(|| params.get("test"))
        .and_then(Value::as_str)
        .context("Missing scenario")?;

    // One-release compatibility: old command-oriented names retain their
    // exact behavior behind both route families.
    if !is_audition_scenario(scenario) {
        return crate::bulb_test::run_bulb_test(state, params);
    }

    let native_id = resolve_matter_device_id(state, device_id)?;
    let (node_id, endpoint) = crate::lifecycle::parse_device_id(&native_id)
        .with_context(|| format!("Invalid Matter device ID: {native_id}"))?;
    let hub_data = get_hub_data(state)?;
    if scenario == "status" {
        let needs_audition = hub_data
            .needs_audition
            .lock()
            .map(|devices| devices.contains(&(node_id, endpoint)))
            .unwrap_or(false);
        return Ok(json!({
            "schema_version": 3,
            "status": "ok",
            "scenario": "status",
            "device_id": native_id,
            "needs_audition": needs_audition,
        }));
    }
    let transport = hub_data
        .transport
        .get()
        .cloned()
        .context("Matter transport not initialized")?;

    let device = transport.probe_light(node_id)?;
    crate::commissioning::store_device_metadata(&hub_data, &device, &native_id);
    let mut profile = hub_data
        .device_profiles
        .lock()
        .ok()
        .and_then(|profiles| profiles.get(&native_id).cloned())
        .unwrap_or_default();
    if scenario == "try_with" {
        apply_try_with_override(
            &mut profile,
            params
                .get("profile_override")
                .context("try_with requires profile_override")?,
        )?;
    }

    let capability_snapshot = transport
        .read_light_capability_snapshot(node_id, endpoint)
        .unwrap_or_else(|_| Value::Null);
    if scenario == "preflight" {
        return Ok(json!({
            "schema_version": 3,
            "status": "ok",
            "scenario": scenario,
            "device_id": native_id,
            "node_id": node_id,
            "endpoint": endpoint,
            "claimed_capabilities": capability_snapshot,
            "reported": readback_snapshot(&transport, node_id, endpoint),
            "observed": Value::Null,
            "profile_used": profile,
            "subscription": subscription_snapshot(&transport, node_id, endpoint, scenario),
        }));
    }

    if scenario == "identify" {
        let mut legacy_params = params.clone();
        legacy_params["test"] = Value::String("identify".to_string());
        let mut result = crate::bulb_test::run_bulb_test(state, &legacy_params)?;
        result["schema_version"] = json!(3);
        result["scenario"] = json!(scenario);
        result["reported"] = result.get("commands").cloned().unwrap_or_else(|| json!([]));
        result["observed"] = Value::Null;
        result["profile_used"] = serde_json::to_value(profile)?;
        return Ok(result);
    }

    let controller = MatterLightController::new(transport.clone(), hub_data.clone());
    let mut all_plans = Vec::new();
    for action in scenario_actions(scenario, params)? {
        let plans = match action {
            AuditionAction::Off => controller.turn_off_plans(std::slice::from_ref(&native_id)),
            AuditionAction::On(command) => controller.audition_turn_on_plans(
                std::slice::from_ref(&native_id),
                &command,
                &profile,
            ),
        }
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        all_plans.extend(plans);
    }

    let result = execute_scenario(
        &transport,
        &hub_data.attribute_report_history,
        scenario,
        node_id,
        endpoint,
        all_plans,
        &profile,
    )?;
    if result.get("needs_audition").and_then(Value::as_bool) == Some(true) {
        if let Ok(mut devices) = hub_data.needs_audition.lock() {
            devices.insert((node_id, endpoint));
        }
    }
    Ok(result)
}

pub fn save_audition_report(state: &SharedState, report: &Value) -> Result<Value> {
    let mut report = report.clone();
    report["schema_version"] = json!(3);
    let saved = crate::bulb_test::save_bulb_test_report(state, &report)?;
    if saved.get("applied_local").and_then(Value::as_bool) == Some(true) {
        let native_id = report
            .pointer("/device/native_device_id")
            .or_else(|| report.get("device_id"))
            .and_then(Value::as_str);
        if let Some((node_id, endpoint)) = native_id.and_then(crate::lifecycle::parse_device_id) {
            if let Ok(hub_data) = get_hub_data(state) {
                if let Ok(mut devices) = hub_data.needs_audition.lock() {
                    devices.remove(&(node_id, endpoint));
                }
            }
        }
    }
    Ok(saved)
}

#[derive(Debug)]
enum AuditionAction {
    Off,
    On(LightingCommand),
}

fn is_audition_scenario(value: &str) -> bool {
    matches!(
        value,
        "status"
            | "preflight"
            | "identify"
            | "turn_on_from_off"
            | "tick_while_on"
            | "tick_while_off"
            | "adaptive_white_route"
            | "color_to_white_and_back"
            | "off_then_on_restore"
            | "power_cycle_then_tick"
            | "dim_floor"
            | "dim_ramp"
            | "brightness_range"
            | "subscription_establish"
            | "subscription_external_change"
            | "subscription_liveness"
            | "try_with"
    )
}

fn scenario_actions(scenario: &str, params: &Value) -> Result<Vec<AuditionAction>> {
    let warm = || LightingCommand::new(30, 2700);
    let cool = || LightingCommand::new(80, 6000);
    let color = || {
        LightingCommand::from_color(
            65,
            rhythm_core::Rgb::new(255, 0, 0),
            rhythm_core::XyColor { x: 0.64, y: 0.33 },
            None,
        )
    };
    Ok(match scenario {
        "turn_on_from_off" | "off_then_on_restore" => {
            vec![AuditionAction::Off, AuditionAction::On(warm())]
        }
        "tick_while_on" | "power_cycle_then_tick" => vec![AuditionAction::On(cool())],
        "tick_while_off" => vec![AuditionAction::Off],
        "adaptive_white_route" => [2200, 2700, 4000, 6500]
            .into_iter()
            .map(|kelvin| AuditionAction::On(LightingCommand::new(60, kelvin)))
            .collect(),
        "color_to_white_and_back" => vec![
            AuditionAction::On(color()),
            AuditionAction::On(warm()),
            AuditionAction::On(color()),
        ],
        "dim_floor" => vec![AuditionAction::On(LightingCommand::new(3, 4000))],
        "dim_ramp" => vec![
            AuditionAction::On(LightingCommand::new(85, 4000)),
            AuditionAction::On(LightingCommand::with_transition(10, 4000, 3000)),
        ],
        "brightness_range" => [20, 60, 100, 40]
            .into_iter()
            .map(|brightness| AuditionAction::On(LightingCommand::new(brightness, 4000)))
            .collect(),
        "subscription_establish" | "subscription_external_change" | "subscription_liveness" => {
            Vec::new()
        }
        "try_with" => {
            let base = params
                .get("base_scenario")
                .and_then(Value::as_str)
                .context("try_with requires base_scenario")?;
            if base == "try_with" || !is_audition_scenario(base) {
                anyhow::bail!("invalid try_with base_scenario");
            }
            return scenario_actions(base, params);
        }
        other => anyhow::bail!("Unknown Bulb Audition scenario: {other}"),
    })
}

fn execute_scenario(
    transport: &Arc<dyn MatterTransport>,
    attribute_report_history: &Arc<Mutex<VecDeque<MatterAttributeReport>>>,
    scenario: &str,
    node_id: u64,
    endpoint: u16,
    plans: Vec<MatterEndpointCommandPlan>,
    profile: &MatterControlProfile,
) -> Result<Value> {
    let started_at_unix_ms = now_unix_ms();
    let subscription = subscription_snapshot(transport, node_id, endpoint, scenario);
    let readback_before = readback_snapshot(transport, node_id, endpoint);
    let mut submissions = Vec::new();
    let mut outcomes = Vec::new();
    let mut cursor = None;
    let mut acknowledged_at_unix_ms = started_at_unix_ms;
    let mut acknowledgement_times = vec![started_at_unix_ms];
    let mut readback_after_immediate = readback_before.clone();
    let mut readback_after_500ms = readback_before.clone();
    let mut readback_after_1500ms = readback_before.clone();
    let mut plan_observations = Vec::new();
    let mut mismatch = None;
    // chipd intentionally coalesces multiple pending desired states for one
    // endpoint. Audition is a rehearsal, so preserve scenario order by
    // admitting and awaiting each runtime-built plan separately.
    for plan in &plans {
        let plan_readback_before = readback_snapshot(transport, node_id, endpoint);
        let plan_submissions = transport.submit_endpoint_plans(std::slice::from_ref(plan))?;
        if plan_submissions.len() != 1 || plan_submissions[0].command_id != plan.command_id {
            anyhow::bail!(
                "Matter controller returned a mismatched submission for audition command {}",
                plan.command_id
            );
        }
        let completed_inline = plan_submissions
            .iter()
            .all(|submission| submission.completed);
        submissions.extend(plan_submissions);
        let succeeded = if completed_inline {
            acknowledged_at_unix_ms = now_unix_ms();
            true
        } else {
            let outcome = wait_for_plan_outcome(transport, plan.command_id, &mut cursor);
            let succeeded = outcome
                .as_ref()
                .map(|outcome| outcome.status == MatterCommandOutcomeStatus::Succeeded)
                .unwrap_or(false);
            if let Some(outcome) = outcome {
                acknowledged_at_unix_ms = outcome.completed_at_unix_ms.unwrap_or_else(now_unix_ms);
                outcomes.push(outcome);
            }
            succeeded
        };
        acknowledgement_times.push(acknowledged_at_unix_ms);

        readback_after_immediate = readback_snapshot(transport, node_id, endpoint);
        sleep_ms(500);
        readback_after_500ms = readback_snapshot(transport, node_id, endpoint);
        sleep_ms(1000);
        readback_after_1500ms = readback_snapshot(transport, node_id, endpoint);
        let plan_mismatch = succeeded
            .then(|| {
                detect_ack_without_effect(
                    std::slice::from_ref(plan),
                    &plan_readback_before,
                    &readback_after_1500ms,
                )
            })
            .flatten();
        if let Some(field) = plan_mismatch {
            mismatch.get_or_insert(field);
            tracing::warn!(
                target: "cmd",
                event = "matter_command_ack_without_effect",
                scenario,
                node_id,
                endpoint,
                command_id = plan.command_id,
                mismatch_field = field,
                "Matter command completed but authoritative readback did not show the requested effect"
            );
        }
        plan_observations.push(json!({
            "command_id": plan.command_id,
            "acknowledged_at_unix_ms": acknowledged_at_unix_ms,
            "succeeded": succeeded,
            "mismatch_field": plan_mismatch,
            "reported": {
                "before": plan_readback_before,
                "after_immediate": readback_after_immediate,
                "after_500ms": readback_after_500ms,
                "after_1500ms": readback_after_1500ms,
            },
        }));
        if !succeeded {
            break;
        }
    }
    if plans.is_empty() {
        readback_after_immediate = readback_snapshot(transport, node_id, endpoint);
        sleep_ms(500);
        readback_after_500ms = readback_snapshot(transport, node_id, endpoint);
        sleep_ms(1000);
        readback_after_1500ms = readback_snapshot(transport, node_id, endpoint);
        if scenario == "subscription_liveness" {
            // Cross one additional negotiated maximum interval so the native
            // possibly-empty ReportData activity signal can prove silence did
            // not exceed the short audition ceiling.
            sleep_ms(u64::from(AUDITION_LIVENESS_MAX_INTERVAL_SECS) * 1_000);
        }
    }
    // chipd owns the native drain and publishes reports into the controller
    // event stream. Reading the shared lifecycle history keeps Audition on
    // that exact runtime path and avoids racing the background event loop.
    let evidence_start_unix_ms = match scenario {
        // The runtime subscription normally predates opening Audition. Its
        // latest retained values remain useful truth evidence when a duplicate
        // subscribe is correctly coalesced by the controller.
        "subscription_establish" => 0,
        "subscription_external_change" | "power_cycle_then_tick" => {
            started_at_unix_ms.saturating_sub(SUBSCRIPTION_EVIDENCE_LOOKBACK_MS)
        }
        // A liveness rehearsal replaces the long-lived runtime subscription.
        // Only activity from that replacement can prove the negotiated short
        // maximum interval; retained activity from the old subscription must
        // not make a quiet replacement look healthy.
        "subscription_liveness" => started_at_unix_ms,
        _ => started_at_unix_ms,
    };
    let raw_reports = attribute_report_history
        .lock()
        .map(|reports| {
            reports
                .iter()
                .filter(|report| {
                    report.node_id == node_id
                        && report.endpoint == endpoint
                        && report.received_at_unix_ms >= evidence_start_unix_ms
                })
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let report_latency_ms = raw_reports
        .iter()
        .filter(|report| !matches!(&report.value, MatterAttributeValue::SubscriptionAlive))
        .filter_map(|report| {
            acknowledgement_times
                .iter()
                .copied()
                .filter(|acknowledged_at| *acknowledged_at <= report.received_at_unix_ms)
                .max()
                .and_then(|acknowledged_at| report.received_at_unix_ms.checked_sub(acknowledged_at))
        })
        .min();
    let subscription_activity_times = raw_reports
        .iter()
        .filter(|report| matches!(&report.value, MatterAttributeValue::SubscriptionAlive))
        .map(|report| report.received_at_unix_ms)
        .collect::<Vec<_>>();
    let truth_results = raw_reports
        .iter()
        .enumerate()
        .filter(|(index, report)| {
            !raw_reports[index + 1..]
                .iter()
                .any(|later| later.cluster == report.cluster && later.attr_id == report.attr_id)
        })
        .filter_map(|(_, report)| report_matches_direct_read(report, &readback_after_1500ms))
        .collect::<Vec<_>>();
    let subscription_truth_matches_direct_read =
        (!truth_results.is_empty()).then(|| truth_results.iter().all(|value| *value));
    let reports = raw_reports
        .into_iter()
        .map(|report| {
            let received_at_unix_ms = if report.received_at_unix_ms == 0 {
                now_unix_ms()
            } else {
                report.received_at_unix_ms
            };
            let latency_from_ack_ms = acknowledgement_times
                .iter()
                .copied()
                .filter(|acknowledged_at| *acknowledged_at <= received_at_unix_ms)
                .max()
                .and_then(|acknowledged_at| received_at_unix_ms.checked_sub(acknowledged_at));
            json!({
                "received_at_unix_ms": received_at_unix_ms,
                "latency_from_ack_ms": latency_from_ack_ms,
                "matches_direct_read": report_matches_direct_read(&report, &readback_after_1500ms),
                "report": report,
            })
        })
        .collect::<Vec<_>>();
    let mut subscription_evidence = merge_subscription_evidence(
        subscription,
        report_latency_ms,
        subscription_truth_matches_direct_read,
        &subscription_activity_times,
        scenario,
    );
    if scenario == "subscription_liveness" {
        let restored = transport
            .refresh_light_state_subscription(
                &[MatterSubscriptionTarget { node_id, endpoint }],
                DEFAULT_SUBSCRIPTION_MIN_INTERVAL_SECS,
                DEFAULT_SUBSCRIPTION_MAX_INTERVAL_SECS,
            )
            .is_ok();
        if let Some(subscription) = subscription_evidence.as_object_mut() {
            subscription.insert("runtime_interval_restored".to_string(), json!(restored));
        }
    }

    Ok(json!({
        "schema_version": 3,
        "status": if mismatch.is_some() { "needs_audition" } else { "ok" },
        "scenario": scenario,
        "device_id": crate::lifecycle::format_device_id(node_id, endpoint),
        "node_id": node_id,
        "endpoint": endpoint,
        "started_at_unix_ms": started_at_unix_ms,
        "acknowledged_at_unix_ms": acknowledged_at_unix_ms,
        "plan_submitted": plans,
        "acknowledgements": {
            "submissions": submissions,
            "terminal_outcomes": outcomes,
        },
        "reported": {
            "before": readback_before,
            "after_immediate": readback_after_immediate,
            "after_500ms": readback_after_500ms,
            "after_1500ms": readback_after_1500ms,
        },
        "plan_observations": plan_observations,
        "observed": Value::Null,
        "subscription_reports": reports,
        "subscription": subscription_evidence,
        "profile_used": profile,
        "adaptive_white": if scenario == "adaptive_white_route" {
            json!({
                "kelvin_targets": [2200, 2700, 4000, 6500],
                "route_used": profile.color_route,
                "route_source": profile.source.color_route,
                "hs_white_curve_used": profile.hs_white_curve,
                "hs_white_curve_source": profile.source.hs_white_curve,
                "reference": "hue_neighbour_same_target",
            })
        } else {
            Value::Null
        },
        "needs_audition": mismatch.is_some(),
        "mismatch_field": mismatch,
    }))
}

fn wait_for_plan_outcome(
    transport: &Arc<dyn MatterTransport>,
    command_id: u64,
    cursor: &mut Option<MatterControllerEventCursor>,
) -> Option<crate::transport::MatterCommandOutcome> {
    let deadline = Instant::now() + Duration::from_millis(OUTCOME_WAIT_MS);
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        let batch = transport
            .wait_controller_events(cursor.as_ref(), remaining)
            .ok()?;
        let had_events = !batch.events.is_empty();
        if let Some(sequence) = batch.events.last().map(|event| event.sequence) {
            *cursor = Some(MatterControllerEventCursor {
                stream_id: batch.stream_id.clone(),
                sequence,
            });
        }
        for envelope in batch.events {
            if let MatterControllerEvent::CommandOutcome(outcome) = envelope.event {
                if outcome.command_id == command_id {
                    return Some(outcome);
                }
            }
        }
        if !had_events || remaining.is_zero() {
            break;
        }
    }
    None
}

fn subscription_snapshot(
    transport: &Arc<dyn MatterTransport>,
    node_id: u64,
    endpoint: u16,
    scenario: &str,
) -> Value {
    let started = std::time::Instant::now();
    let target = [MatterSubscriptionTarget { node_id, endpoint }];
    let (result, max_interval_secs) = if scenario == "subscription_liveness" {
        (
            transport.refresh_light_state_subscription(
                &target,
                DEFAULT_SUBSCRIPTION_MIN_INTERVAL_SECS,
                AUDITION_LIVENESS_MAX_INTERVAL_SECS,
            ),
            AUDITION_LIVENESS_MAX_INTERVAL_SECS,
        )
    } else {
        (
            transport.subscribe_light_state(
                &target,
                DEFAULT_SUBSCRIPTION_MIN_INTERVAL_SECS,
                DEFAULT_SUBSCRIPTION_MAX_INTERVAL_SECS,
            ),
            DEFAULT_SUBSCRIPTION_MAX_INTERVAL_SECS,
        )
    };
    json!({
        "works": result.is_ok(),
        "establish_latency_ms": started.elapsed().as_millis() as u64,
        "attributes": [
            "on_off",
            "current_level",
            "current_hue",
            "current_saturation",
            "current_x",
            "current_y",
            "color_temperature_mireds"
        ],
        "liveness_interval_s": max_interval_secs,
        "error_stage": if result.is_ok() { Value::Null } else { json!("subscribe") },
    })
}

fn merge_subscription_evidence(
    mut subscription: Value,
    report_latency_ms: Option<u64>,
    truth_matches_direct_read: Option<bool>,
    activity_times: &[u64],
    scenario: &str,
) -> Value {
    if let Some(subscription) = subscription.as_object_mut() {
        subscription.insert("report_latency_ms".to_string(), json!(report_latency_ms));
        subscription.insert(
            "truth_matches_direct_read".to_string(),
            json!(truth_matches_direct_read),
        );
        subscription.insert(
            "last_activity_at_unix_ms".to_string(),
            json!(activity_times.iter().max()),
        );
        subscription.insert(
            "activity_report_count".to_string(),
            json!(activity_times.len()),
        );
        if scenario == "power_cycle_then_tick" {
            subscription.insert(
                "resubscribe_after_power_cycle".to_string(),
                json!(truth_matches_direct_read.is_some()),
            );
        }
        if scenario == "subscription_external_change" {
            subscription.insert(
                "reports_external_changes".to_string(),
                json!(truth_matches_direct_read.is_some()),
            );
        }
        if scenario == "subscription_liveness" {
            let max_activity_gap_ms = activity_times
                .windows(2)
                .filter_map(|times| times[1].checked_sub(times[0]))
                .max();
            let observed_live = activity_times.len() >= 2
                && max_activity_gap_ms
                    .map(|gap| gap <= u64::from(AUDITION_LIVENESS_MAX_INTERVAL_SECS) * 1_500)
                    .unwrap_or(false);
            subscription.insert(
                "max_activity_gap_ms".to_string(),
                json!(max_activity_gap_ms),
            );
            subscription.insert("liveness_observed".to_string(), json!(observed_live));
            subscription.insert("went_quiet".to_string(), json!(!observed_live));
        }
    }
    subscription
}

fn report_matches_direct_read(report: &MatterAttributeReport, state: &Value) -> Option<bool> {
    const LEVEL_CONTROL_CLUSTER: u32 = 0x0008;
    const COLOR_CONTROL_CLUSTER: u32 = 0x0300;
    let field = match (report.cluster, report.attr_id) {
        (crate::clusters::CLUSTER_ON_OFF_U32, crate::clusters::ATTR_ON_OFF_U32) => "onoff",
        (LEVEL_CONTROL_CLUSTER, 0x0000) => "current_level",
        (COLOR_CONTROL_CLUSTER, 0x0000) => "current_hue",
        (COLOR_CONTROL_CLUSTER, 0x0001) => "current_saturation",
        (COLOR_CONTROL_CLUSTER, 0x0003) => "current_x",
        (COLOR_CONTROL_CLUSTER, 0x0004) => "current_y",
        (COLOR_CONTROL_CLUSTER, 0x0007) => "color_temperature_mireds",
        _ => return None,
    };
    let direct = state.get(field)?.get("value")?;
    Some(match &report.value {
        MatterAttributeValue::Bool(value) => direct.as_bool() == Some(*value),
        MatterAttributeValue::U8(value) => direct.as_u64() == Some(u64::from(*value)),
        MatterAttributeValue::U16(value) => direct.as_u64() == Some(u64::from(*value)),
        MatterAttributeValue::SubscriptionAlive => return None,
    })
}

fn readback_snapshot(transport: &Arc<dyn MatterTransport>, node_id: u64, endpoint: u16) -> Value {
    transport
        .read_light_state(node_id, endpoint)
        .or_else(|_| {
            transport.read_on_off(node_id, endpoint).map(|on| {
                json!({
                    "onoff": {"ok": true, "value": on},
                })
            })
        })
        .unwrap_or_else(|_| json!({"status": "unavailable"}))
}

fn detect_ack_without_effect(
    plans: &[MatterEndpointCommandPlan],
    before: &Value,
    after: &Value,
) -> Option<&'static str> {
    let final_steps = plans.last()?.steps.as_slice();
    for step in final_steps.iter().rev() {
        match step {
            MatterCommandStep::SetOnOff { on } => {
                if state_bool(after, "onoff") != Some(*on) {
                    return Some("on_off");
                }
            }
            MatterCommandStep::SetBrightness { level, .. }
            | MatterCommandStep::RunLevel {
                level_or_step: level,
                ..
            } => {
                if let Some(actual) = state_u64(after, "current_level") {
                    if actual.abs_diff(u64::from(*level)) > 8 {
                        return Some("level");
                    }
                }
            }
            MatterCommandStep::SetColorTemperature { .. }
            | MatterCommandStep::SetXy { .. }
            | MatterCommandStep::SetHueSaturation { .. } => {
                let keys = [
                    "color_temperature_mireds",
                    "current_x",
                    "current_y",
                    "current_hue",
                    "current_saturation",
                ];
                if keys
                    .iter()
                    .all(|key| state_value(before, key) == state_value(after, key))
                {
                    return Some("color");
                }
            }
            MatterCommandStep::Identify { .. } => {}
        }
    }
    None
}

fn state_value<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value
        .get(key)
        .and_then(|entry| entry.get("value").or(Some(entry)))
        .filter(|value| !value.is_null())
}

fn state_bool(value: &Value, key: &str) -> Option<bool> {
    state_value(value, key).and_then(Value::as_bool)
}

fn state_u64(value: &Value, key: &str) -> Option<u64> {
    state_value(value, key).and_then(Value::as_u64)
}

fn apply_try_with_override(profile: &mut MatterControlProfile, value: &Value) -> Result<()> {
    let object = value
        .as_object()
        .context("profile_override must be an object")?;
    if object.len() != 1 {
        anyhow::bail!("profile_override must change exactly one field");
    }
    let (field, value) = object.iter().next().expect("checked one override");
    match field.as_str() {
        "color_route" => {
            profile.color_route = serde_json::from_value::<MatterColorRoute>(value.clone())?;
            profile.source.color_route = MatterProfileSource::TryWith;
        }
        "hs_white_curve" => {
            let curve = serde_json::from_value::<Vec<MatterHsWhitePoint>>(value.clone())?;
            if !(2..=8).contains(&curve.len())
                || curve.iter().any(|point| {
                    !(1_000..=10_000).contains(&point.kelvin)
                        || point.hue > 254
                        || point.saturation > 254
                })
                || curve
                    .windows(2)
                    .any(|points| points[0].kelvin >= points[1].kelvin)
            {
                anyhow::bail!("HS white curve must contain 2-8 ordered valid Matter points");
            }
            profile.hs_white_curve = curve;
            profile.source.hs_white_curve = MatterProfileSource::TryWith;
        }
        "turn_on" => {
            profile.turn_on = serde_json::from_value::<MatterTurnOnStrategy>(value.clone())?;
            profile.source.turn_on = MatterProfileSource::TryWith;
        }
        "level_command" => {
            profile.level_command = serde_json::from_value::<MatterLevelCommand>(value.clone())?;
            profile.source.level_command = MatterProfileSource::TryWith;
        }
        "command_spacing_ms" => {
            let value_ms = value
                .as_u64()
                .context("command spacing must be milliseconds")?;
            if value_ms > 10_000 {
                anyhow::bail!("command spacing exceeds 10000 ms");
            }
            profile.command_spacing_ms.value_ms = value_ms as u32;
            profile.command_spacing_ms.basis = MatterMeasurementBasis::Measured;
            profile.command_spacing_ms.source = MatterProfileSource::TryWith;
        }
        "execute_if_off_honoured" => {
            profile.execute_if_off_honoured = value
                .as_bool()
                .context("execute_if_off_honoured must be boolean")?;
            profile.source.execute_if_off_honoured = MatterProfileSource::TryWith;
        }
        "supports_transition" => {
            profile.supports_transition = value
                .as_bool()
                .context("supports_transition must be boolean")?;
            profile.source.supports_transition = MatterProfileSource::TryWith;
        }
        _ => anyhow::bail!("unsupported try-with profile field: {field}"),
    }
    Ok(())
}

fn get_hub_data(state: &SharedState) -> Result<Arc<MatterHubData>> {
    let hub_key = HubKey::new(HubType::new("matter"), "local");
    state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .hubs
        .get(&hub_key)
        .ok_or_else(|| anyhow::anyhow!("Matter hub not connected"))?
        .data::<Arc<MatterHubData>>()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Matter hub data missing"))
}

fn resolve_matter_device_id(state: &SharedState, device_id: &str) -> Result<String> {
    if crate::lifecycle::parse_device_id(device_id).is_some() {
        return Ok(device_id.to_string());
    }
    let matter_hub_key = HubKey::new(HubType::new("matter"), "local");
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let device = state
        .canonical_registry
        .get(device_id)
        .ok_or_else(|| anyhow::anyhow!("Invalid Matter device ID: {device_id}"))?;
    let endpoint_id = device
        .active_endpoints()
        .find(|endpoint| {
            endpoint.hub_key == matter_hub_key
                && crate::lifecycle::parse_device_id(&endpoint.native_id).is_some()
        })
        .map(|endpoint| endpoint.native_id.clone())
        .ok_or_else(|| anyhow::anyhow!("Device '{device_id}' has no Matter endpoint"))?;
    Ok(endpoint_id)
}

#[cfg(not(test))]
fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

#[cfg(test)]
fn sleep_ms(_ms: u64) {}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_with_requires_exactly_one_bounded_typed_override() {
        let mut profile = MatterControlProfile::default();
        apply_try_with_override(&mut profile, &json!({"color_route": "xy"})).unwrap();
        assert_eq!(profile.color_route, MatterColorRoute::Xy);
        assert_eq!(profile.source.color_route, MatterProfileSource::TryWith);

        assert!(apply_try_with_override(
            &mut profile,
            &json!({"color_route": "xy", "turn_on": "explicit_on_first"})
        )
        .is_err());
        assert!(
            apply_try_with_override(&mut profile, &json!({"command_spacing_ms": 10_001})).is_err()
        );
    }

    #[test]
    fn readback_detects_success_without_color_effect() {
        let plan = MatterEndpointCommandPlan {
            command_id: 1,
            node_id: 7,
            endpoint: 1,
            steps: vec![MatterCommandStep::SetXy {
                x: 0.64,
                y: 0.33,
                transition_ms: None,
            }],
            inter_step_delay_ms: None,
        };
        let state = json!({
            "current_x": {"ok": true, "value": 100},
            "current_y": {"ok": true, "value": 200},
        });
        assert_eq!(
            detect_ack_without_effect(&[plan], &state, &state),
            Some("color")
        );
    }

    #[test]
    fn liveness_requires_repeated_activity_within_the_short_interval() {
        let evidence = merge_subscription_evidence(
            json!({"works": true, "liveness_interval_s": 2}),
            None,
            None,
            &[1_000, 3_000],
            "subscription_liveness",
        );
        assert_eq!(evidence["max_activity_gap_ms"], json!(2_000));
        assert_eq!(evidence["liveness_observed"], json!(true));
        assert_eq!(evidence["went_quiet"], json!(false));

        let quiet = merge_subscription_evidence(
            json!({"works": true}),
            None,
            None,
            &[1_000, 5_000],
            "subscription_liveness",
        );
        assert_eq!(quiet["liveness_observed"], json!(false));
        assert_eq!(quiet["went_quiet"], json!(true));
    }
}
