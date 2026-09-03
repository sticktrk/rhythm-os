//! Shared Matter commissioning orchestration.

use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use log::{error, info, warn};
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_os::canonical::identity::{HardwareId, HubKey};
use rhythm_os::hub::HubType;
use rhythm_os::pairing::{
    PairedDeviceInfo, PairingRequestContext, PairingSession, PairingStage, PairingStatus,
};
use rhythm_os::state::SharedState;
use serde_json::Value;

use crate::hub_state::MatterHubData;
use crate::setup_recovery::SetupPayloadTarget;
use crate::transport::{
    CommissionedDevice, MatterCommissionRequest, MatterCommissioningNetwork,
    MatterCommissioningRendezvous, MatterCommissioningWifiCredentials, MatterTransport,
};

const RECOVERY_ACTION_CONNECTION_RECOVERED: &str = "existing_connection_recovered";
const RECOVERY_ACTION_NODE_RECOMMISSIONED: &str = "existing_node_recommissioned";
const RECOVERY_ACTION_NODE_RECOMMISSION_FAILED: &str = "existing_node_recommission_failed";
const SETUP_PAYLOAD_PERSISTENCE_WARNING: &str = "The light was paired, but Rhythm could not save its Matter setup code for recovery. Keep using the light normally. Do not reset or pair it again; contact Rhythm Support if its connection needs recovery.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PairingCompletion {
    New,
    ExistingConnectionRecovered,
    ExistingNodeRecommissioned,
}

impl PairingCompletion {
    fn recovery_action(self) -> Option<&'static str> {
        match self {
            Self::New => None,
            Self::ExistingConnectionRecovered => Some(RECOVERY_ACTION_CONNECTION_RECOVERED),
            Self::ExistingNodeRecommissioned => Some(RECOVERY_ACTION_NODE_RECOMMISSIONED),
        }
    }
}

/// Parsed Matter pairing request owned by `rhythm-matter`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatterPairingParams {
    /// Raw Matter setup payload. May be an `MT:` QR payload or a manual code.
    pub setup_payload: String,
    /// Optional client-generated pairing session ID for SSE correlation.
    pub session_id: Option<String>,
    /// Matter network being commissioned.
    pub network: MatterCommissioningNetwork,
    /// How the commissioner reaches the device.
    pub rendezvous: MatterCommissioningRendezvous,
}

impl MatterPairingParams {
    /// Parse the integration-specific request payload.
    ///
    /// When clients omit `rendezvous`, manual setup codes default to
    /// on-network commissioning while QR payloads keep the existing auto
    /// behavior.
    pub fn from_value(params: &Value) -> Result<Self> {
        let setup_payload = params
            .get("setup_payload")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Missing 'setup_payload' in pairing params"))?;
        let session_id = params
            .get("session_id")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);

        let network = match params.get("network").and_then(|value| value.as_str()) {
            None | Some("wifi") => MatterCommissioningNetwork::Wifi,
            Some(other) => anyhow::bail!(
                "Unsupported Matter network '{}'; only 'wifi' is currently supported",
                other
            ),
        };

        let rendezvous = match params.get("rendezvous").and_then(|value| value.as_str()) {
            None => default_rendezvous_for_payload(setup_payload),
            Some("auto") => MatterCommissioningRendezvous::Auto,
            Some("ble") => MatterCommissioningRendezvous::Ble,
            Some("on_network") => MatterCommissioningRendezvous::OnNetwork,
            Some(other) => anyhow::bail!(
                "Unsupported Matter rendezvous '{}'; expected 'auto', 'ble', or 'on_network'",
                other
            ),
        };

        Ok(Self {
            setup_payload: setup_payload.to_string(),
            session_id,
            network,
            rendezvous,
        })
    }

    /// Build the transport-facing request for a specific local node ID.
    pub fn to_commission_request(
        &self,
        node_id: u64,
        wifi_credentials: MatterCommissioningWifiCredentials,
    ) -> MatterCommissionRequest {
        MatterCommissionRequest {
            setup_payload: self.setup_payload.clone(),
            node_id,
            network: self.network,
            rendezvous: self.rendezvous,
            wifi_credentials,
        }
    }
}

fn default_rendezvous_for_payload(setup_payload: &str) -> MatterCommissioningRendezvous {
    if is_qr_setup_payload(setup_payload) {
        MatterCommissioningRendezvous::Auto
    } else {
        MatterCommissioningRendezvous::OnNetwork
    }
}

fn is_qr_setup_payload(setup_payload: &str) -> bool {
    setup_payload
        .get(..3)
        .map(|prefix| prefix.eq_ignore_ascii_case("MT:"))
        .unwrap_or(false)
}

/// Ensure the local Matter hub exists before commissioning.
pub fn ensure_matter_hub_connected(state: &SharedState) -> Result<()> {
    let hub_key = HubKey::new(HubType::new("matter"), "local");
    let already_connected = state
        .lock()
        .map(|state| state.hubs.contains_key(&hub_key))
        .unwrap_or(false);

    if !already_connected {
        info!(target: "pair", "Matter hub not connected, auto-bootstrapping...");
        let credentials = serde_json::json!({ "fabric_id": "default" });
        rhythm_os::commands::do_hub_credentials(state, "matter", "local", &credentials)?;
    }

    Ok(())
}

/// Commission a Matter-over-WiFi light using the shared transport.
pub fn pair_device(
    state: &SharedState,
    transport: Arc<dyn MatterTransport>,
    hub_data: Arc<MatterHubData>,
    request: &MatterPairingParams,
) -> Result<PairingSession> {
    pair_device_with_context(
        state,
        transport,
        hub_data,
        request,
        &PairingRequestContext::accepted_now(),
    )
}

pub fn pair_device_with_context(
    state: &SharedState,
    transport: Arc<dyn MatterTransport>,
    hub_data: Arc<MatterHubData>,
    request: &MatterPairingParams,
    request_context: &PairingRequestContext,
) -> Result<PairingSession> {
    ensure_pairing_request_active(request_context)?;
    let recovery_targets = match registered_recovery_targets(
        state,
        &hub_data,
        &request.setup_payload,
    ) {
        Ok(targets) => targets,
        Err(error) => {
            tracing::error!(
                target: "pair",
                event = "matter_repeat_pair_lookup_unavailable",
                error = %format!("{error:#}"),
                "Matter recovery metadata could not be checked; refusing to risk a duplicate pairing"
            );
            return Ok(PairingSession {
                hub_type: "matter".to_string(),
                status: PairingStatus::Failed,
                device: None,
                devices: Vec::new(),
                error: Some(
                    "Rhythm could not safely check whether this Matter setup code is already registered, so pairing was not started. Restart the server and try again."
                        .to_string(),
                ),
                failure_stage: None,
                warnings: Vec::new(),
                details: None,
            });
        }
    };
    if recovery_targets.len() > 1 {
        tracing::warn!(
            target: "pair",
            event = "matter_repeat_pair_ambiguous",
            match_count = recovery_targets.len(),
            "Matter repeat-pair payload matched multiple registered endpoints; refusing to guess"
        );
        return Ok(PairingSession {
            hub_type: "matter".to_string(),
            status: PairingStatus::Failed,
            device: None,
            devices: Vec::new(),
            error: Some(
                "This setup code matches more than one saved Matter device, so Rhythm did not change either one. Remove the stale duplicate before trying again."
                    .to_string(),
            ),
            failure_stage: None,
            warnings: Vec::new(),
            details: None,
        });
    }

    if let Some(target) = recovery_targets.first().copied() {
        rhythm_os::pairing::emit_pairing_progress(
            state,
            "matter",
            request.session_id.as_deref(),
            PairingStatus::Searching,
            PairingStage::Searching,
            "Checking saved Matter device connection",
            None,
            None,
        );
        match transport.recover_light_connection_with_context(
            target.node_id,
            target.endpoint,
            request_context,
        ) {
            Ok(device) => {
                ensure_pairing_request_active(request_context)?;
                tracing::info!(
                    target: "pair",
                    event = "matter_repeat_pair_connection_recovered",
                    "Matter repeat-pair recovered the existing operational connection"
                );
                hub_data.record_node_proof_of_life(target.node_id);
                return build_success_session(
                    state,
                    &hub_data,
                    device,
                    &request.setup_payload,
                    request.session_id.as_deref(),
                    PairingCompletion::ExistingConnectionRecovered,
                );
            }
            Err(error) => {
                ensure_pairing_request_active(request_context)?;
                tracing::warn!(
                    target: "pair",
                    event = "matter_repeat_pair_probe_failed",
                    error = %format!("{error:#}"),
                    "Saved Matter device was unreachable; trying same-node recommissioning"
                );
            }
        }

        let wifi_credentials = match load_commissioning_wifi_credentials(state) {
            Ok(credentials) => credentials,
            Err(error) => {
                return Ok(failed_recovery_session(
                    &error,
                    request.rendezvous,
                    "Rhythm found this saved Matter device, but cannot recommission it until the appliance Wi-Fi credentials are available.",
                ));
            }
        };
        ensure_pairing_request_active(request_context)?;
        let commission_request = request.to_commission_request(target.node_id, wifi_credentials);
        rhythm_os::pairing::emit_pairing_progress(
            state,
            "matter",
            request.session_id.as_deref(),
            PairingStatus::Commissioning,
            PairingStage::Commissioning,
            "Recommissioning saved Matter device",
            None,
            None,
        );

        return match transport.commission_light_with_context(&commission_request, request_context) {
            Ok(device) => {
                ensure_pairing_request_active(request_context)?;
                build_success_session(
                    state,
                    &hub_data,
                    device,
                    &request.setup_payload,
                    request.session_id.as_deref(),
                    PairingCompletion::ExistingNodeRecommissioned,
                )
            }
            Err(error) => {
                tracing::error!(
                    target: "pair",
                    event = "matter_repeat_pair_recommission_failed",
                    error = %format!("{error:#}"),
                    "Matter same-node recommissioning failed"
                );
                Ok(failed_recovery_session(
                    &error,
                    request.rendezvous,
                    "Rhythm found this saved Matter device, but could not restore its connection or recommission it. Put the light in Matter pairing mode, keep it powered, and try again.",
                ))
            }
        };
    }

    ensure_pairing_request_active(request_context)?;
    let wifi_credentials = load_commissioning_wifi_credentials(state)?;
    let node_id = hub_data.reserve_node_id();
    let commission_request = request.to_commission_request(node_id, wifi_credentials);
    rhythm_os::pairing::emit_pairing_progress(
        state,
        "matter",
        request.session_id.as_deref(),
        PairingStatus::Commissioning,
        PairingStage::Commissioning,
        "Commissioning Matter device",
        None,
        None,
    );

    match transport.commission_light_with_context(&commission_request, request_context) {
        Ok(device) => {
            ensure_pairing_request_active(request_context)?;
            build_success_session(
                state,
                &hub_data,
                device,
                &request.setup_payload,
                request.session_id.as_deref(),
                PairingCompletion::New,
            )
        }
        Err(error) => {
            error!(target: "pair", "Matter commissioning error: {:#}", error);
            Ok(PairingSession {
                hub_type: "matter".to_string(),
                status: PairingStatus::Failed,
                device: None,
                devices: Vec::new(),
                error: Some(summarize_commissioning_error_for_rendezvous(
                    &error,
                    request.rendezvous,
                )),
                failure_stage: None,
                warnings: Vec::new(),
                details: None,
            })
        }
    }
}

fn ensure_pairing_request_active(context: &PairingRequestContext) -> Result<()> {
    if context.is_cancelled() {
        anyhow::bail!("Matter pairing request was cancelled");
    }
    Ok(())
}

fn registered_recovery_targets(
    state: &SharedState,
    hub_data: &MatterHubData,
    setup_payload: &str,
) -> Result<Vec<SetupPayloadTarget>> {
    let targets = crate::setup_recovery::find_setup_payload_targets(
        state,
        &hub_data.fabric_id,
        setup_payload,
    )?;
    if targets.is_empty() {
        return Ok(targets);
    }

    let matter_hub_key = HubKey::new(HubType::new("matter"), "local");
    let state = state
        .lock()
        .map_err(|_| anyhow::anyhow!("Matter canonical registry lock poisoned"))?;
    Ok(targets
        .into_iter()
        .filter(|target| {
            let native_id = crate::lifecycle::format_device_id(target.node_id, target.endpoint);
            state
                .canonical_registry
                .find_by_native_id(&matter_hub_key, &native_id)
                .is_some()
        })
        .collect())
}

fn failed_recovery_session(
    error: &anyhow::Error,
    rendezvous: MatterCommissioningRendezvous,
    message: &str,
) -> PairingSession {
    PairingSession {
        hub_type: "matter".to_string(),
        status: PairingStatus::Failed,
        device: None,
        devices: Vec::new(),
        error: Some(format!(
            "{} {}",
            message,
            summarize_commissioning_error_for_rendezvous(error, rendezvous)
        )),
        failure_stage: None,
        warnings: Vec::new(),
        details: Some(serde_json::json!({
            "recovery_action": RECOVERY_ACTION_NODE_RECOMMISSION_FAILED,
        })),
    }
}

#[cfg(test)]
fn summarize_commissioning_error(error: &anyhow::Error) -> String {
    summarize_commissioning_error_for_rendezvous(error, MatterCommissioningRendezvous::Auto)
}

fn summarize_commissioning_error_for_rendezvous(
    error: &anyhow::Error,
    rendezvous: MatterCommissioningRendezvous,
) -> String {
    let detail = format!("{:#}", error);
    let lower = detail.to_ascii_lowercase();

    if lower.contains("matter pairing request was cancelled") {
        return "Matter pairing stopped because the initiating app request ended. Reopen the device's pairing window and try again.".to_string();
    }

    if lower.contains("matter pairing request exceeded its server deadline") {
        return "Matter pairing stopped at the server deadline and released its resources. Reopen the device's pairing window and try again.".to_string();
    }

    if rendezvous == MatterCommissioningRendezvous::OnNetwork
        && (lower.contains("network unreachable")
            || lower.contains("os error 0x02000065")
            || (lower.contains("discovery timed out")
                && !lower.contains("operational discovery failed")))
    {
        return "On-network Matter pairing could not reach the device over local IPv6/IP. Keep its multi-admin pairing window open and verify the Rhythm Box accepts the Thread border router's IPv6 route; Bluetooth proximity or a factory reset will not repair a missing route.".to_string();
    }

    if lower.contains("addressresolve") || lower.contains("operational discovery failed") {
        return "The light joined the Wi-Fi network, but Rhythm could not discover it over mDNS afterwards. Rhythm reset its Matter controller to recover; wait a few seconds and retry pairing without factory-resetting the light.".to_string();
    }

    if lower.contains("gatt write characteristic operation failed") {
        return "Matter BLE commissioning reached the bulb, but macOS CoreBluetooth failed the GATT write. This matches the current official Matter controller behavior on this host. Try Linux/BlueZ or the appliance target for real commissioning.".to_string();
    }

    if lower.contains("pasesession.cpp") {
        return "Matter BLE pairing reached the device, but the BLE connection was lost during secure setup. Rhythm reset the Matter controller; wait a few seconds, keep the light close, and retry pairing.".to_string();
    }

    if lower.contains("connectiondelegate timeout")
        || lower.contains("discovery timed out")
        || lower.contains("blemanagerimpl.cpp")
        || lower.contains("chip error 0x00000032: timeout")
    {
        return "Matter BLE commissioning timed out while discovering the bulb from this host. Factory-reset the bulb, keep it close to the machine, and if it still fails, try Linux/BlueZ or the appliance target.".to_string();
    }

    if is_linux_ble_stack_error(&lower)
        || lower.contains("chip error 0x000000ac")
        || lower.contains("ble device doesn't seem to support chip")
    {
        return "Pairing lost its Bluetooth connection before setup finished. Rhythm reset the Matter controller; put the light back in pairing mode, keep it near the Rhythm Box, wait a few seconds, and try again.".to_string();
    }

    if lower.contains("matter wi-fi commissioning requires stored appliance wi-fi credentials") {
        return "Matter pairing needs stored appliance Wi-Fi credentials on the server before a light can be commissioned.".to_string();
    }

    detail
}

fn is_linux_ble_stack_error(lower_detail: &str) -> bool {
    [
        "matter ble commissioning failed",
        "blemanagerimpl.cpp",
        "bluezendpoint.cpp",
        "bluezobjectmanager.cpp",
        "pasesession.cpp",
        "chipoble",
        "ble adapter unavailable",
        "d-bus system bus",
        "operation was cancelled",
    ]
    .iter()
    .any(|needle| lower_detail.contains(needle))
}

fn load_commissioning_wifi_credentials(
    state: &SharedState,
) -> Result<MatterCommissioningWifiCredentials> {
    let wifi = match load_stored_commissioning_wifi_credentials(state)? {
        Some(wifi) => wifi,
        None => load_platform_commissioning_wifi_credentials(state)?.ok_or_else(|| {
            anyhow::anyhow!(
                "Matter Wi-Fi commissioning requires stored appliance Wi-Fi credentials; provision the appliance over Wi-Fi before pairing Matter lights"
            )
        })?,
    };

    Ok(MatterCommissioningWifiCredentials {
        ssid: wifi.ssid,
        password: wifi.password,
    })
}

fn load_stored_commissioning_wifi_credentials(
    state: &SharedState,
) -> Result<Option<rhythm_os::provisioning::WifiCredentials>> {
    let state = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
    let Some(storage) = state.storage.as_ref() else {
        return Ok(None);
    };

    storage
        .load_commissioning_wifi_credentials()
        .context("loading stored appliance Wi-Fi credentials")
}

fn load_platform_commissioning_wifi_credentials(
    state: &SharedState,
) -> Result<Option<rhythm_os::provisioning::WifiCredentials>> {
    let provider = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
        .commissioning_wifi_credentials_provider
        .clone();
    let Some(provider) = provider else {
        return Ok(None);
    };

    let creds = provider().context("loading platform appliance Wi-Fi credentials")?;
    if let Some(creds) = creds.as_ref() {
        persist_commissioning_wifi_credentials(state, creds);
        info!(
            target: "pair",
            "Recovered Matter commissioning Wi-Fi credentials from platform network config for SSID '{}'",
            creds.ssid
        );
    }
    Ok(creds)
}

fn persist_commissioning_wifi_credentials(
    state: &SharedState,
    creds: &rhythm_os::provisioning::WifiCredentials,
) {
    let result = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))
        .and_then(|state| {
            let storage = state
                .storage
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("storage not configured"))?;
            storage.save_commissioning_wifi_credentials(creds)
        });

    if let Err(error) = result {
        warn!(
            target: "pair",
            "Failed to persist recovered Matter commissioning Wi-Fi credentials: {:#}",
            error
        );
    }
}

fn build_success_session(
    state: &SharedState,
    hub_data: &Arc<MatterHubData>,
    device: CommissionedDevice,
    setup_payload: &str,
    session_id: Option<&str>,
    completion: PairingCompletion,
) -> Result<PairingSession> {
    rhythm_os::pairing::emit_pairing_progress(
        state,
        "matter",
        session_id,
        PairingStatus::Commissioning,
        PairingStage::Finalizing,
        "Finalizing paired Matter device",
        None,
        None,
    );

    let device_id = crate::lifecycle::format_device_id(device.node_id, device.light_endpoint);
    let device_name = format!("{} {}", device.vendor_name, device.product_name);
    let hub_key = HubKey::new(HubType::new("matter"), "local");

    if let Some(recovery_action) = completion.recovery_action() {
        tracing::info!(
            target: "pair",
            event = "matter_repeat_pair_completed",
            recovery_action,
            "Matter repeat-pair completed against the existing identity"
        );
    } else {
        info!(
            target: "sys",
            "Matter: paired {} (node {}, id={})",
            device_name,
            device.node_id,
            device_id
        );
    }

    hub_data.record_commissioned_device(&device);
    store_device_metadata(hub_data, &device, &device_id);
    if let Err(error) = crate::capture::persist_device_capture(hub_data, &device, "pair") {
        warn!(
            target: "sys",
            "Matter: failed to persist pair capture for {}: {}",
            device_id,
            error
        );
    }
    register_canonical_identity(state, &hub_key, &device, &device_id, &device_name)?;
    materialize_unassigned_canonical_device(state, &hub_key, &device_id)?;

    let mut warnings = Vec::new();
    if let Err(error) = crate::setup_recovery::save_setup_payload(
        state,
        &hub_data.fabric_id,
        device.node_id,
        device.light_endpoint,
        setup_payload,
    ) {
        warn!(
            target: "pair",
            "Matter setup recovery material was not persisted: {}",
            error
        );
        warnings.push(SETUP_PAYLOAD_PERSISTENCE_WARNING.to_string());
    } else {
        info!(target: "pair", "Matter setup recovery material persisted");
    }

    let _ = hub_data.event_tx.send(
        crate::events::device_paired_event(
            device.node_id,
            &device.vendor_name,
            &device.product_name,
        )
        .with_hub_key(hub_key),
    );

    Ok(PairingSession {
        hub_type: "matter".to_string(),
        status: PairingStatus::Complete,
        device: Some(PairedDeviceInfo {
            device_id,
            name: device_name,
            device_type: DeviceType::Light,
            manufacturer: Some(device.vendor_name),
            model: Some(device.product_name),
        }),
        devices: Vec::new(),
        error: None,
        failure_stage: None,
        warnings,
        details: completion
            .recovery_action()
            .map(|recovery_action| serde_json::json!({ "recovery_action": recovery_action })),
    })
}

pub(crate) fn store_device_metadata(
    hub_data: &Arc<MatterHubData>,
    device: &CommissionedDevice,
    device_id: &str,
) {
    let cloud_profiles = hub_data
        .cloud_profiles
        .lock()
        .map(|profiles| profiles.clone())
        .unwrap_or_default();
    let local_overrides = hub_data
        .local_overrides
        .lock()
        .map(|overrides| overrides.clone())
        .unwrap_or_default();
    let resolved = resolve_device_metadata(device, device_id, &cloud_profiles, &local_overrides);

    if let Ok(mut device_caps) = hub_data.device_caps.lock() {
        device_caps.insert(device_id.to_string(), resolved.capabilities);
        info!(
            target: "sys",
            "Matter: stored caps for {} ({} devices tracked)",
            device_id,
            device_caps.len()
        );
    }

    if let Ok(mut device_quirks) = hub_data.device_quirks.lock() {
        device_quirks.insert(device_id.to_string(), resolved.quirks);
    }
    if let Ok(mut device_profiles) = hub_data.device_profiles.lock() {
        device_profiles.insert(device_id.to_string(), resolved.control_profile);
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedMatterDeviceMetadata {
    pub capabilities: rhythm_devices::LightCapabilities,
    pub quirks: Vec<rhythm_devices::DeviceQuirk>,
    pub control_profile: crate::control_profile::MatterControlProfile,
}

/// The single metadata/profile resolver used by startup, pairing, discovery,
/// on-demand probes, and Bulb Audition.
pub(crate) fn resolve_device_metadata(
    device: &CommissionedDevice,
    device_id: &str,
    cloud_profiles: &crate::cloud_profiles::CloudMatterProfileCatalog,
    local_overrides: &crate::local_quirks::LocalMatterOverrides,
) -> ResolvedMatterDeviceMetadata {
    let builtin_capabilities = build_device_capabilities(device);
    let builtin_quirks = build_device_quirks(device);
    let mut capabilities = builtin_capabilities.clone();
    let mut quirks = builtin_quirks.clone();

    cloud_profiles.apply_to_device(device, &mut capabilities, &mut quirks);

    if let Some(override_caps) = local_overrides.capabilities.get(device_id) {
        crate::local_quirks::apply_capability_override(&mut capabilities, override_caps);
    }
    if let Some(local_quirks) = local_overrides.quirks.get(device_id) {
        quirks = crate::local_quirks::apply_quirk_override(&quirks, local_quirks);
    }
    let cloud_profile = cloud_profiles.control_profile_for_device(device);
    let legacy_local_profile = if local_overrides.capabilities.contains_key(device_id)
        || local_overrides.quirks.contains_key(device_id)
    {
        let local_capabilities = local_overrides.capabilities.get(device_id);
        Some(crate::control_profile::profile_overlay_from_legacy_local(
            local_overrides
                .quirks
                .get(device_id)
                .map(Vec::as_slice)
                .unwrap_or_default(),
            local_capabilities.and_then(|value| value.min_brightness),
            local_capabilities.and_then(|value| value.supports_transition),
        ))
    } else {
        None
    };
    let mut control_profile = crate::control_profile::resolve_control_profile(
        &builtin_capabilities,
        &builtin_quirks,
        cloud_profile.as_ref(),
        legacy_local_profile.as_ref(),
    );
    if let Some(typed_local_profile) = local_overrides.control_profiles.get(device_id) {
        crate::control_profile::overlay_sourced_audition_profile(
            &mut control_profile,
            typed_local_profile,
        );
    }

    ResolvedMatterDeviceMetadata {
        capabilities,
        quirks,
        control_profile,
    }
}

pub(crate) fn fallback_device_capabilities() -> rhythm_devices::LightCapabilities {
    rhythm_devices::LightCapabilities {
        color_modes: vec![
            rhythm_devices::ColorMode::HueSaturation,
            rhythm_devices::ColorMode::ColorTemperature,
        ],
        ..rhythm_devices::LightCapabilities::defaults_for(rhythm_devices::LightType::ExtendedColor)
    }
}

pub(crate) fn store_fallback_device_metadata(hub_data: &Arc<MatterHubData>, device_id: &str) {
    if let Ok(mut device_caps) = hub_data.device_caps.lock() {
        if !device_caps.contains_key(device_id) {
            device_caps.insert(device_id.to_string(), fallback_device_capabilities());
            info!(
                target: "sys",
                "Matter: stored fallback caps for {} ({} devices tracked)",
                device_id,
                device_caps.len()
            );
        }
    }

    if let Ok(mut device_quirks) = hub_data.device_quirks.lock() {
        device_quirks.entry(device_id.to_string()).or_default();
    }
    if let Ok(mut device_profiles) = hub_data.device_profiles.lock() {
        device_profiles
            .entry(device_id.to_string())
            .or_insert_with(|| {
                crate::control_profile::profile_from_legacy(
                    &fallback_device_capabilities(),
                    &[],
                    crate::control_profile::MatterProfileSource::SafeDefault,
                )
            });
    }
}

pub(crate) fn build_device_capabilities(
    device: &CommissionedDevice,
) -> rhythm_devices::LightCapabilities {
    let mut caps = crate::capabilities::capabilities_from_commissioned(device);
    crate::capabilities::enrich_from_db(&mut caps, device, rhythm_devices::builtin_db());
    caps
}

pub(crate) fn build_device_quirks(device: &CommissionedDevice) -> Vec<rhythm_devices::DeviceQuirk> {
    crate::capabilities::quirks_from_db(device, rhythm_devices::builtin_db())
}

fn register_canonical_identity(
    state: &SharedState,
    hub_key: &HubKey,
    device: &CommissionedDevice,
    device_id: &str,
    device_name: &str,
) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let identity = rhythm_os::canonical::identity::DiscoveredIdentity {
        native_id: device_id.to_string(),
        room_id: None,
        room_name: None,
        name: device_name.to_string(),
        device_type: DeviceType::Light,
        hardware_ids: vec![HardwareId::matter(&device.node_id.to_string())],
        manufacturer: Some(device.vendor_name.clone()),
        model: Some(device.product_name.clone()),
    };

    let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    state.canonical_registry.resolve(&identity, hub_key, now);
    let canonical_id = state
        .canonical_registry
        .find_by_native_id(hub_key, device_id)
        .map(|device| device.id.clone())
        .ok_or_else(|| anyhow::anyhow!("Canonical Matter endpoint was not registered"))?;
    if let Some(normalized) =
        crate::lifecycle::normalized_endpoint_capabilities(&build_device_capabilities(device))
    {
        let endpoint = state
            .canonical_registry
            .get_mut(&canonical_id)
            .and_then(|device| {
                device.endpoints.iter_mut().find(|endpoint| {
                    endpoint.hub_key == *hub_key && endpoint.native_id == device_id
                })
            })
            .ok_or_else(|| anyhow::anyhow!("Canonical Matter endpoint disappeared"))?;
        endpoint.capabilities = Some(normalized);
        rhythm_os::commands::save_authority_state(&state)
            .context("persisting Matter endpoint capabilities")?;
    }
    Ok(())
}

fn materialize_unassigned_canonical_device(
    state: &SharedState,
    hub_key: &HubKey,
    device_id: &str,
) -> Result<()> {
    let state_guard = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let (canonical_id, already_assigned) = state_guard
        .canonical_registry
        .find_by_native_id(hub_key, device_id)
        .map(|device| (device.id.clone(), device.room_id.is_some()))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Canonical device missing after Matter pairing: {}",
                device_id
            )
        })?;

    drop(state_guard);

    if already_assigned {
        rhythm_os::commands::reconcile_runtime_from_state(state)?;
    } else {
        rhythm_os::commands::do_canonical_assign_room(state, &canonical_id, None)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    use rhythm_os::hub::HubEvent;
    use rhythm_os::provisioning::WifiCredentials;
    use rhythm_os::storage::{FileStorage, Storage};

    use crate::cloud_profiles::CloudMatterProfileCatalog;
    use crate::controller::MatterDeviceRegistry;
    use crate::transport::{
        MatterColorMode, MatterDeviceInfo, MatterGroup, MatterGroupMember,
        MatterLevelCommandVariant, MatterLevelStepMode, MatterSubscriptionTarget,
    };

    #[derive(Default)]
    struct FakeMatterTransport {
        commission_requests: Mutex<Vec<MatterCommissionRequest>>,
        commission_error: Mutex<Option<String>>,
        recovery_requests: Mutex<Vec<(u64, u16)>>,
        recovery_error: Mutex<Option<String>>,
        cancel_during_recovery: Mutex<Option<PairingRequestContext>>,
        subscribe_calls: AtomicUsize,
    }

    impl FakeMatterTransport {
        fn with_commission_error(error: impl Into<String>) -> Self {
            Self {
                commission_error: Mutex::new(Some(error.into())),
                ..Self::default()
            }
        }

        fn with_recovery_error(error: impl Into<String>) -> Self {
            Self {
                recovery_error: Mutex::new(Some(error.into())),
                ..Self::default()
            }
        }

        fn with_recovery_and_commission_error(
            recovery_error: impl Into<String>,
            commission_error: impl Into<String>,
        ) -> Self {
            Self {
                recovery_error: Mutex::new(Some(recovery_error.into())),
                commission_error: Mutex::new(Some(commission_error.into())),
                ..Self::default()
            }
        }
    }

    impl MatterTransport for FakeMatterTransport {
        fn commission_light(
            &self,
            request: &MatterCommissionRequest,
        ) -> Result<CommissionedDevice> {
            self.commission_requests
                .lock()
                .unwrap()
                .push(request.clone());
            if let Some(error) = self.commission_error.lock().unwrap().clone() {
                anyhow::bail!(error);
            }
            Ok(commissioned_device(request.node_id))
        }

        fn decommission_device(&self, _node_id: u64, _force: bool) -> Result<()> {
            Ok(())
        }

        fn list_devices(&self) -> Result<Vec<MatterDeviceInfo>> {
            Ok(Vec::new())
        }

        fn probe_light(&self, node_id: u64) -> Result<CommissionedDevice> {
            Ok(commissioned_device(node_id))
        }

        fn recover_light_connection(
            &self,
            node_id: u64,
            expected_endpoint: u16,
        ) -> Result<CommissionedDevice> {
            self.recovery_requests
                .lock()
                .unwrap()
                .push((node_id, expected_endpoint));
            if let Some(context) = self.cancel_during_recovery.lock().unwrap().take() {
                context.cancel();
            }
            if let Some(error) = self.recovery_error.lock().unwrap().clone() {
                anyhow::bail!(error);
            }
            Ok(commissioned_device(node_id))
        }

        fn set_on_off(&self, _node_id: u64, _endpoint: u16, _on: bool) -> Result<()> {
            Ok(())
        }

        fn configure_group(&self, _group: &MatterGroup) -> Result<()> {
            Ok(())
        }

        fn remove_group(&self, _group_id: u16, _members: &[MatterGroupMember]) -> Result<()> {
            Ok(())
        }

        fn identify_light(&self, _node_id: u64, _endpoint: u16, _duration_secs: u16) -> Result<()> {
            Ok(())
        }

        fn set_brightness(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _level: u8,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn run_level_command(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _command: MatterLevelCommandVariant,
            _level_or_step: u8,
            _step_mode: Option<MatterLevelStepMode>,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn set_color_temperature(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _kelvin: u16,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn set_xy(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _x: f32,
            _y: f32,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn set_hue_saturation(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _hue: u8,
            _saturation: u8,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn read_on_off(&self, _node_id: u64, _endpoint: u16) -> Result<bool> {
            Ok(false)
        }

        fn subscribe_on_off(
            &self,
            _targets: &[MatterSubscriptionTarget],
            _min_interval_secs: u16,
            _max_interval_secs: u16,
        ) -> Result<()> {
            self.subscribe_calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("automatic subscriptions are disabled")
        }
    }

    fn state() -> SharedState {
        Arc::new(std::sync::Mutex::new(rhythm_os::state::AppState::default()))
    }

    fn unique_data_dir(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rhythm-matter-commissioning-{name}-{nanos}"))
    }

    fn state_with_storage(name: &str) -> (SharedState, std::path::PathBuf) {
        let path = unique_data_dir(name);
        let storage = FileStorage::new(path.to_str().unwrap()).unwrap();
        let state = state();
        state.lock().unwrap().storage = Some(std::sync::Arc::new(storage));
        (state, path)
    }

    fn wifi(ssid: &str, password: &str) -> WifiCredentials {
        WifiCredentials {
            ssid: ssid.to_string(),
            password: password.to_string(),
        }
    }

    fn save_wifi(path: &std::path::Path, creds: &WifiCredentials) {
        let storage = FileStorage::new(path.to_str().unwrap()).unwrap();
        storage.save_commissioning_wifi_credentials(creds).unwrap();
    }

    fn commissioned_device(node_id: u64) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: "Acme".to_string(),
            product_name: "Color Lamp".to_string(),
            vendor_id: 1,
            product_id: 2,
            serial_number: Some(format!("serial-{node_id}")),
            light_endpoint: 2,
            color_modes: vec![
                MatterColorMode::ColorTemperature,
                MatterColorMode::Xy,
                MatterColorMode::HueSaturation,
            ],
            min_kelvin: Some(2200),
            max_kelvin: Some(6500),
        }
    }

    fn hub_data() -> (Arc<MatterHubData>, std::sync::mpsc::Receiver<HubEvent>) {
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        (
            Arc::new(MatterHubData {
                transport: OnceLock::new(),
                capture_dir: OnceLock::new(),
                registry: Arc::new(Mutex::new(MatterDeviceRegistry::new())),
                fabric_id: "default".to_string(),
                commissioned: Mutex::new(Vec::new()),
                next_node_id: AtomicU64::new(10),
                device_caps: Mutex::new(HashMap::new()),
                device_quirks: Mutex::new(HashMap::new()),
                device_profiles: Mutex::new(HashMap::new()),
                pending_turn_on_plans: Arc::new(Mutex::new(HashMap::new())),
                needs_audition: Arc::new(Mutex::new(HashSet::new())),
                readback: Arc::new(crate::hub_state::MatterReadbackCoordinator::default()),
                local_overrides: Mutex::new(crate::local_quirks::LocalMatterOverrides::default()),
                cloud_profiles: Mutex::new(CloudMatterProfileCatalog::default()),
                decommissioning: Mutex::new(HashSet::new()),
                recently_decommissioned: Mutex::new(HashMap::new()),
                node_proof_of_life: Arc::new(Mutex::new(HashMap::new())),
                on_off_observations: Arc::new(Mutex::new(HashMap::new())),
                attribute_report_history: Arc::new(Mutex::new(std::collections::VecDeque::new())),
                event_tx,
            }),
            event_rx,
        )
    }

    fn install_transport(
        hub_data: &Arc<MatterHubData>,
        transport: Arc<FakeMatterTransport>,
    ) -> Arc<FakeMatterTransport> {
        let transport_dyn: Arc<dyn MatterTransport> = transport.clone();
        assert!(hub_data.transport.set(transport_dyn).is_ok());
        transport
    }

    fn pairing_request() -> MatterPairingParams {
        MatterPairingParams::from_value(&serde_json::json!({
            "setup_payload": "MT:Y.K908OC16750648G00",
            "session_id": "pair-1",
            "rendezvous": "ble"
        }))
        .unwrap()
    }

    fn seed_saved_device(
        state: &SharedState,
        hub_data: &Arc<MatterHubData>,
        node_id: u64,
        setup_payload: &str,
    ) -> String {
        let device = commissioned_device(node_id);
        let device_id = crate::lifecycle::format_device_id(node_id, device.light_endpoint);
        let device_name = format!("{} {}", device.vendor_name, device.product_name);
        let hub_key = HubKey::new(HubType::new("matter"), "local");
        hub_data.record_commissioned_device(&device);
        store_device_metadata(hub_data, &device, &device_id);
        register_canonical_identity(state, &hub_key, &device, &device_id, &device_name).unwrap();
        materialize_unassigned_canonical_device(state, &hub_key, &device_id).unwrap();
        crate::setup_recovery::save_setup_payload(
            state,
            &hub_data.fabric_id,
            node_id,
            device.light_endpoint,
            setup_payload,
        )
        .unwrap();
        state
            .lock()
            .unwrap()
            .canonical_registry
            .find_by_native_id(&hub_key, &device_id)
            .unwrap()
            .id
            .clone()
    }

    fn string_error<T>(result: Result<T>) -> String {
        match result {
            Ok(_) => panic!("expected error"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn pairing_params_accept_raw_mt_payload() {
        let params = serde_json::json!({
            "setup_payload": "MT:Y.K908OC16750648G00",
            "network": "wifi",
            "rendezvous": "ble"
        });

        let parsed = MatterPairingParams::from_value(&params).unwrap();
        let request = parsed.to_commission_request(
            123,
            MatterCommissioningWifiCredentials {
                ssid: "wifi".to_string(),
                password: "secret".to_string(),
            },
        );

        assert_eq!(request.setup_payload, "MT:Y.K908OC16750648G00");
        assert_eq!(request.node_id, 123);
        assert_eq!(request.rendezvous, MatterCommissioningRendezvous::Ble);
    }

    #[test]
    fn pairing_params_trim_validate_and_default_manual_codes_to_on_network() {
        let parsed = MatterPairingParams::from_value(&serde_json::json!({
            "setup_payload": " 12345678901 ",
            "session_id": "   ",
        }))
        .unwrap();

        assert_eq!(parsed.setup_payload, "12345678901");
        assert_eq!(parsed.session_id, None);
        assert_eq!(parsed.network, MatterCommissioningNetwork::Wifi);
        assert_eq!(parsed.rendezvous, MatterCommissioningRendezvous::OnNetwork);

        assert_eq!(
            string_error(MatterPairingParams::from_value(&serde_json::json!({}))),
            "Missing 'setup_payload' in pairing params"
        );
        assert_eq!(
            string_error(MatterPairingParams::from_value(&serde_json::json!({
                "setup_payload": " ",
            }))),
            "Missing 'setup_payload' in pairing params"
        );
        assert_eq!(
            string_error(MatterPairingParams::from_value(&serde_json::json!({
                "setup_payload": "MT:payload",
                "network": "thread",
            }))),
            "Unsupported Matter network 'thread'; only 'wifi' is currently supported"
        );
        assert_eq!(
            string_error(MatterPairingParams::from_value(&serde_json::json!({
                "setup_payload": "MT:payload",
                "rendezvous": "nfc",
            }))),
            "Unsupported Matter rendezvous 'nfc'; expected 'auto', 'ble', or 'on_network'"
        );
    }

    #[test]
    fn pairing_params_default_to_auto_rendezvous() {
        let params = serde_json::json!({
            "setup_payload": "MT:Y.K908OC16750648G00",
        });

        let parsed = MatterPairingParams::from_value(&params).unwrap();
        assert_eq!(parsed.rendezvous, MatterCommissioningRendezvous::Auto);
    }

    #[test]
    fn pairing_params_accept_session_id_for_progress_correlation() {
        let params = serde_json::json!({
            "setup_payload": "MT:Y.K908OC16750648G00",
            "session_id": "pair-1",
        });

        let parsed = MatterPairingParams::from_value(&params).unwrap();
        assert_eq!(parsed.session_id.as_deref(), Some("pair-1"));
    }

    #[test]
    fn commissioning_error_summarizes_darwin_gatt_failures() {
        let error = anyhow::anyhow!(
            "commissioning Matter light: Ble Error 0x00000407: GATT write characteristic operation failed"
        );

        let message = summarize_commissioning_error(&error);

        assert!(message.contains("CoreBluetooth failed the GATT write"));
        assert!(message.contains("Linux/BlueZ"));
    }

    #[test]
    fn commissioning_error_summarizes_ble_timeouts() {
        let error = anyhow::anyhow!("commissioning Matter light: ConnectionDelegate timeout");

        let message = summarize_commissioning_error(&error);

        assert!(message.contains("timed out while discovering the bulb"));
    }

    #[test]
    fn on_network_route_failure_does_not_recommend_ble_recovery() {
        for detail in [
            "commissioning Matter light: OS Error 0x02000065: Network unreachable",
            "commissioning Matter light: Discovery timed out",
        ] {
            let message = summarize_commissioning_error_for_rendezvous(
                &anyhow::anyhow!(detail),
                MatterCommissioningRendezvous::OnNetwork,
            );

            assert!(message.contains("local IPv6/IP"));
            assert!(message.contains("Thread border router's IPv6 route"));
            assert!(message.contains("Bluetooth proximity"));
            assert!(!message.contains("keep it close"));
        }
    }

    #[test]
    fn stopped_pairing_reports_terminal_server_ownership() {
        let cancelled = summarize_commissioning_error_for_rendezvous(
            &anyhow::anyhow!("Matter pairing request was cancelled"),
            MatterCommissioningRendezvous::OnNetwork,
        );
        assert!(cancelled.contains("initiating app request ended"));

        let deadline = summarize_commissioning_error_for_rendezvous(
            &anyhow::anyhow!("Matter pairing request exceeded its server deadline"),
            MatterCommissioningRendezvous::OnNetwork,
        );
        assert!(deadline.contains("released its resources"));
    }

    /// Regression for issue #117: operational-discovery timeouts were
    /// summarized with the BLE-discovery message telling the user to
    /// factory-reset the bulb and keep it close — advice that cannot help
    /// when the host's mDNS resolution is what actually failed.
    #[test]
    fn commissioning_error_summarizes_operational_discovery_timeouts() {
        let error = anyhow::anyhow!(
            "commissioning Matter light: src/lib/address_resolve/AddressResolve_DefaultImpl.cpp:124: CHIP Error 0x00000032: Timeout"
        )
        .context("Matter operational discovery failed; reset CHIP sidecar before next attempt");

        let message = summarize_commissioning_error(&error);

        assert!(message.contains("could not discover it over mDNS"));
        assert!(message.contains("retry pairing"));
        assert!(!message.contains("Factory-reset the bulb"));
        assert!(!message.contains("AddressResolve_DefaultImpl.cpp"));
    }

    #[test]
    fn commissioning_error_summarizes_linux_chip_timeouts() {
        let error = anyhow::anyhow!(
            "commissioning Matter light: src/protocols/secure_channel/PASESession.cpp:310: CHIP Error 0x00000032: Timeout"
        );

        let message = summarize_commissioning_error(&error);

        assert!(message.contains("reached the device"));
        assert!(message.contains("lost during secure setup"));
        assert!(!message.contains("PASESession.cpp"));
    }

    #[test]
    fn commissioning_error_summarizes_linux_ble_stack_failures() {
        for detail in [
            "Matter BLE commissioning failed; reset CHIP sidecar before next attempt: commissioning Matter light: src/platform/Linux/bluez/BluezEndpoint.cpp:623: CHIP Error 0x000000AC: Internal error",
            "Matter BLE commissioning failed; reset CHIP sidecar before next attempt: FAIL: Get D-Bus system bus: Could not connect: Connection refused",
            "Matter BLE commissioning failed; reset CHIP sidecar before next attempt: commissioning Matter light: src/platform/Linux/bluez/BluezObjectManager.cpp:118: CHIP Error 0x000000AC: Internal error",
            "Matter BLE commissioning failed; reset CHIP sidecar before next attempt: commissioning Matter light: src/platform/Linux/bluez/BluezEndpoint.cpp:493: Operation was cancelled",
        ] {
            let error = anyhow::anyhow!(detail);

            let message = summarize_commissioning_error(&error);

            assert!(message.contains("lost its Bluetooth connection"));
            assert!(message.contains("put the light back in pairing mode"));
            assert!(!message.contains("BlueZ"));
            assert!(!message.contains("CHIP"));
            assert!(message.contains("try again"));
        }
    }

    #[test]
    fn commissioning_error_summarizes_bluez_internal_errors() {
        let error = anyhow::anyhow!(
            "commissioning Matter light: src/platform/Linux/bluez/BluezEndpoint.cpp:623: CHIP Error 0x000000AC: Internal error"
        );

        let message = summarize_commissioning_error(&error);

        assert!(message.contains("lost its Bluetooth connection"));
        assert!(!message.contains("BlueZ"));
        assert!(!message.contains("CHIP"));
        assert!(!message.contains("BluezEndpoint.cpp"));
    }

    #[test]
    fn wifi_credentials_load_from_storage_or_platform_and_persist_recovered_values() {
        let state = state();
        assert_eq!(
            string_error(load_commissioning_wifi_credentials(&state)),
            "Matter Wi-Fi commissioning requires stored appliance Wi-Fi credentials; provision the appliance over Wi-Fi before pairing Matter lights"
        );

        let stored_wifi = wifi("StoredNet", "stored-secret");
        let (stored_state, stored_path) = state_with_storage("stored");
        save_wifi(&stored_path, &stored_wifi);

        let loaded = load_commissioning_wifi_credentials(&stored_state).unwrap();
        assert_eq!(loaded.ssid, stored_wifi.ssid);
        assert_eq!(loaded.password, stored_wifi.password);
        std::fs::remove_dir_all(stored_path).ok();

        let recovered_wifi = wifi("PlatformNet", "platform-secret");
        let (platform_state, platform_path) = state_with_storage("platform");
        platform_state
            .lock()
            .unwrap()
            .commissioning_wifi_credentials_provider = Some(Arc::new({
            let recovered_wifi = recovered_wifi.clone();
            move || Ok(Some(recovered_wifi.clone()))
        }));

        let loaded = load_commissioning_wifi_credentials(&platform_state).unwrap();
        assert_eq!(loaded.ssid, recovered_wifi.ssid);
        assert_eq!(loaded.password, recovered_wifi.password);

        let storage = FileStorage::new(platform_path.to_str().unwrap()).unwrap();
        assert_eq!(
            storage.load_commissioning_wifi_credentials().unwrap(),
            Some(recovered_wifi)
        );
        std::fs::remove_dir_all(platform_path).ok();
    }

    #[test]
    fn ensure_matter_hub_connected_reports_missing_provider_when_not_bootstrapped() {
        let state = state();
        assert_eq!(
            string_error(ensure_matter_hub_connected(&state)),
            "No hub provider registered"
        );
    }

    #[test]
    fn pair_device_success_records_fabric_state_metadata_and_event_without_subscription() {
        let (state, path) = state_with_storage("pair-success");
        save_wifi(&path, &wifi("PairNet", "pair-secret"));
        let (hub_data, event_rx) = hub_data();
        let transport = install_transport(&hub_data, Arc::new(FakeMatterTransport::default()));

        let session = pair_device(
            &state,
            transport.clone() as Arc<dyn MatterTransport>,
            hub_data.clone(),
            &pairing_request(),
        )
        .unwrap();

        assert_eq!(session.status, PairingStatus::Complete);
        let paired = session.device.unwrap();
        assert_eq!(paired.device_id, "matter-10-2");
        assert_eq!(paired.name, "Acme Color Lamp");
        assert_eq!(paired.device_type, DeviceType::Light);

        let requests = transport.commission_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].node_id, 10);
        assert_eq!(requests[0].wifi_credentials.ssid, "PairNet");
        assert_eq!(requests[0].wifi_credentials.password, "pair-secret");
        assert_eq!(requests[0].rendezvous, MatterCommissioningRendezvous::Ble);
        drop(requests);

        assert_eq!(transport.subscribe_calls.load(Ordering::SeqCst), 0);
        assert_eq!(hub_data.commissioned.lock().unwrap()[0].node_id, 10);
        assert!(hub_data
            .device_caps
            .lock()
            .unwrap()
            .contains_key("matter-10-2"));
        assert!(hub_data
            .device_quirks
            .lock()
            .unwrap()
            .contains_key("matter-10-2"));

        let matter_key = HubKey::new(HubType::new("matter"), "local");
        let state_guard = state.lock().unwrap();
        let canonical = state_guard
            .canonical_registry
            .find_by_native_id(&matter_key, "matter-10-2")
            .expect("paired device should be in canonical registry");
        assert_eq!(
            canonical
                .endpoint_by_native_id("matter-10-2")
                .and_then(|endpoint| endpoint.capabilities.as_ref())
                .and_then(|value| value.pointer("/light_capabilities/color_temperature")),
            Some(&serde_json::json!({
                "min_kelvin": 2200,
                "max_kelvin": 6500,
            }))
        );
        assert!(state_guard
            .topology
            .get_device_node(&canonical.id)
            .is_some());
        drop(state_guard);
        let recovery =
            crate::setup_recovery::load_setup_payload(&state, &hub_data.fabric_id, "matter-10-2")
                .unwrap()
                .expect("successful commission should retain setup recovery");
        assert_eq!(recovery.payload_kind, "qr_code");
        assert_eq!(recovery.setup_payload, pairing_request().setup_payload);

        match event_rx.try_recv().unwrap() {
            HubEvent::DevicePaired {
                hub_key,
                device_id,
                name,
                device_type,
            } => {
                assert_eq!(hub_key, Some(matter_key));
                assert_eq!(device_id, "matter-10");
                assert_eq!(name, "Acme Color Lamp");
                assert_eq!(device_type, DeviceType::Light);
            }
            other => panic!("expected device paired event, got {other:?}"),
        }

        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn setup_payload_persistence_failure_does_not_recommend_or_reserve_a_new_pairing() {
        let state = state();
        let (hub_data, _event_rx) = hub_data();
        hub_data.next_node_id.store(43, Ordering::SeqCst);
        let next_node_id = hub_data.next_node_id.load(Ordering::SeqCst);

        let session = build_success_session(
            &state,
            &hub_data,
            commissioned_device(42),
            &pairing_request().setup_payload,
            Some("pair-persistence-failure"),
            PairingCompletion::New,
        )
        .unwrap();

        assert_eq!(session.status, PairingStatus::Complete);
        assert_eq!(
            session.warnings,
            vec![SETUP_PAYLOAD_PERSISTENCE_WARNING.to_string()]
        );
        assert!(!session.warnings[0].contains("Pair it again"));
        assert!(session.warnings[0].contains("Do not reset or pair it again"));
        assert_eq!(
            hub_data.next_node_id.load(Ordering::SeqCst),
            next_node_id,
            "non-mutating recovery guidance must not reserve a replacement node"
        );
        assert_eq!(
            state.lock().unwrap().canonical_registry.devices().count(),
            1
        );
    }

    #[test]
    fn repeat_pair_recovers_registered_connection_without_reserving_or_commissioning() {
        let (state, path) = state_with_storage("repeat-connection");
        let (hub_data, _event_rx) = hub_data();
        let original_canonical_id =
            seed_saved_device(&state, &hub_data, 42, &pairing_request().setup_payload);
        let room = rhythm_os::commands::do_topology_create_room(&state, "Bedroom").unwrap();
        let room_id = serde_json::from_str::<serde_json::Value>(&room).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        rhythm_os::commands::do_canonical_assign_room(
            &state,
            &original_canonical_id,
            Some(&room_id),
        )
        .unwrap();
        let next_node_id = hub_data.next_node_id.load(Ordering::SeqCst);
        let transport = Arc::new(FakeMatterTransport::default());

        let session = pair_device(
            &state,
            transport.clone() as Arc<dyn MatterTransport>,
            hub_data.clone(),
            &pairing_request(),
        )
        .unwrap();

        assert_eq!(session.status, PairingStatus::Complete);
        assert_eq!(session.device.unwrap().device_id, "matter-42-2");
        assert_eq!(
            session.details,
            Some(serde_json::json!({
                "recovery_action": RECOVERY_ACTION_CONNECTION_RECOVERED,
            }))
        );
        assert_eq!(
            transport.recovery_requests.lock().unwrap().as_slice(),
            &[(42, 2)]
        );
        assert!(transport.commission_requests.lock().unwrap().is_empty());
        assert_eq!(hub_data.next_node_id.load(Ordering::SeqCst), next_node_id);
        let matter_key = HubKey::new(HubType::new("matter"), "local");
        let state_guard = state.lock().unwrap();
        assert_eq!(state_guard.canonical_registry.devices().count(), 1);
        let recovered = state_guard
            .canonical_registry
            .find_by_native_id(&matter_key, "matter-42-2")
            .unwrap();
        assert_eq!(recovered.id, original_canonical_id);
        assert_eq!(recovered.room_id.as_deref(), Some(room_id.as_str()));
        drop(state_guard);

        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn cancelled_repeat_pair_recovery_cannot_finalize_device_state() {
        let (state, path) = state_with_storage("repeat-cancelled");
        let (hub_data, event_rx) = hub_data();
        seed_saved_device(&state, &hub_data, 42, &pairing_request().setup_payload);
        while event_rx.try_recv().is_ok() {}
        let context = PairingRequestContext::accepted_now();
        let transport = Arc::new(FakeMatterTransport::default());
        *transport.cancel_during_recovery.lock().unwrap() = Some(context.clone());

        let error = pair_device_with_context(
            &state,
            transport.clone() as Arc<dyn MatterTransport>,
            hub_data,
            &pairing_request(),
            &context,
        )
        .expect_err("cancelled recovery must not produce a successful pairing session");

        assert!(error.to_string().contains("request was cancelled"));
        assert_eq!(
            transport.recovery_requests.lock().unwrap().as_slice(),
            &[(42, 2)]
        );
        assert!(transport.commission_requests.lock().unwrap().is_empty());
        assert!(
            event_rx.try_recv().is_err(),
            "cancelled recovery must not emit a late device-paired event"
        );

        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn repeat_pair_recommissions_the_same_node_and_preserves_canonical_identity() {
        let (state, path) = state_with_storage("repeat-recommission");
        save_wifi(&path, &wifi("PairNet", "pair-secret"));
        let (hub_data, _event_rx) = hub_data();
        let original_canonical_id =
            seed_saved_device(&state, &hub_data, 42, &pairing_request().setup_payload);
        let room = rhythm_os::commands::do_topology_create_room(&state, "Bedroom").unwrap();
        let room_id = serde_json::from_str::<serde_json::Value>(&room).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        rhythm_os::commands::do_canonical_assign_room(
            &state,
            &original_canonical_id,
            Some(&room_id),
        )
        .unwrap();
        let next_node_id = hub_data.next_node_id.load(Ordering::SeqCst);
        let transport = Arc::new(FakeMatterTransport::with_recovery_error(
            "operational node unreachable",
        ));

        let session = pair_device(
            &state,
            transport.clone() as Arc<dyn MatterTransport>,
            hub_data.clone(),
            &pairing_request(),
        )
        .unwrap();

        assert_eq!(session.status, PairingStatus::Complete);
        assert_eq!(
            session.details,
            Some(serde_json::json!({
                "recovery_action": RECOVERY_ACTION_NODE_RECOMMISSIONED,
            }))
        );
        let requests = transport.commission_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].node_id, 42);
        drop(requests);
        assert_eq!(
            hub_data.next_node_id.load(Ordering::SeqCst),
            next_node_id,
            "same-node recovery must not reserve a replacement identity"
        );
        let matter_key = HubKey::new(HubType::new("matter"), "local");
        let state_guard = state.lock().unwrap();
        assert_eq!(state_guard.canonical_registry.devices().count(), 1);
        let recovered = state_guard
            .canonical_registry
            .find_by_native_id(&matter_key, "matter-42-2")
            .unwrap();
        assert_eq!(recovered.id, original_canonical_id);
        assert_eq!(recovered.room_id.as_deref(), Some(room_id.as_str()));
        drop(state_guard);

        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn failed_repeat_pair_keeps_existing_identity_and_recovery_material() {
        let (state, path) = state_with_storage("repeat-failed");
        save_wifi(&path, &wifi("PairNet", "pair-secret"));
        let (hub_data, _event_rx) = hub_data();
        let original_canonical_id =
            seed_saved_device(&state, &hub_data, 42, &pairing_request().setup_payload);
        let next_node_id = hub_data.next_node_id.load(Ordering::SeqCst);
        let transport = Arc::new(FakeMatterTransport::with_recovery_and_commission_error(
            "operational node unreachable",
            "ConnectionDelegate timeout",
        ));

        let session = pair_device(
            &state,
            transport.clone() as Arc<dyn MatterTransport>,
            hub_data.clone(),
            &pairing_request(),
        )
        .unwrap();

        assert_eq!(session.status, PairingStatus::Failed);
        assert_eq!(
            session.details,
            Some(serde_json::json!({
                "recovery_action": RECOVERY_ACTION_NODE_RECOMMISSION_FAILED,
            }))
        );
        assert_eq!(transport.commission_requests.lock().unwrap()[0].node_id, 42);
        assert_eq!(hub_data.next_node_id.load(Ordering::SeqCst), next_node_id);
        assert_eq!(hub_data.commissioned.lock().unwrap()[0].node_id, 42);
        let recovery =
            crate::setup_recovery::load_setup_payload(&state, &hub_data.fabric_id, "matter-42-2")
                .unwrap()
                .expect("failed recovery must retain saved setup material");
        assert_eq!(recovery.setup_payload, pairing_request().setup_payload);
        let matter_key = HubKey::new(HubType::new("matter"), "local");
        assert_eq!(
            state
                .lock()
                .unwrap()
                .canonical_registry
                .find_by_native_id(&matter_key, "matter-42-2")
                .unwrap()
                .id,
            original_canonical_id
        );

        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn ambiguous_repeat_pair_does_not_probe_reserve_or_commission() {
        let (state, path) = state_with_storage("repeat-ambiguous");
        let (hub_data, _event_rx) = hub_data();
        seed_saved_device(&state, &hub_data, 42, "12345678901");
        seed_saved_device(&state, &hub_data, 43, "123-456-78901");
        let next_node_id = hub_data.next_node_id.load(Ordering::SeqCst);
        let transport = Arc::new(FakeMatterTransport::default());
        let request = MatterPairingParams::from_value(&serde_json::json!({
            "setup_payload": "123 456 78901",
            "session_id": "pair-ambiguous",
        }))
        .unwrap();

        let session = pair_device(
            &state,
            transport.clone() as Arc<dyn MatterTransport>,
            hub_data.clone(),
            &request,
        )
        .unwrap();

        assert_eq!(session.status, PairingStatus::Failed);
        assert!(session.error.unwrap().contains("more than one saved"));
        assert!(transport.recovery_requests.lock().unwrap().is_empty());
        assert!(transport.commission_requests.lock().unwrap().is_empty());
        assert_eq!(hub_data.next_node_id.load(Ordering::SeqCst), next_node_id);
        assert_eq!(
            state.lock().unwrap().canonical_registry.devices().count(),
            2
        );

        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn unreadable_recovery_metadata_fails_before_reserving_or_commissioning() {
        let (state, path) = state_with_storage("repeat-unreadable");
        save_wifi(&path, &wifi("PairNet", "pair-secret"));
        let storage = state.lock().unwrap().storage.clone().unwrap();
        storage
            .save_integration_state_file("matter/setup-payloads.json", "{")
            .unwrap();
        let (hub_data, _event_rx) = hub_data();
        let next_node_id = hub_data.next_node_id.load(Ordering::SeqCst);
        let transport = Arc::new(FakeMatterTransport::default());

        let session = pair_device(
            &state,
            transport.clone() as Arc<dyn MatterTransport>,
            hub_data.clone(),
            &pairing_request(),
        )
        .unwrap();

        assert_eq!(session.status, PairingStatus::Failed);
        assert!(session.error.unwrap().contains("could not safely check"));
        assert!(transport.recovery_requests.lock().unwrap().is_empty());
        assert!(transport.commission_requests.lock().unwrap().is_empty());
        assert_eq!(hub_data.next_node_id.load(Ordering::SeqCst), next_node_id);

        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn pair_device_failure_reports_summarized_error_without_recording_device() {
        let (state, path) = state_with_storage("pair-failure");
        save_wifi(&path, &wifi("PairNet", "pair-secret"));
        let (hub_data, _event_rx) = hub_data();
        let transport = Arc::new(FakeMatterTransport::with_commission_error(
            "ConnectionDelegate timeout",
        ));

        let session = pair_device(
            &state,
            transport.clone() as Arc<dyn MatterTransport>,
            hub_data.clone(),
            &pairing_request(),
        )
        .unwrap();

        assert_eq!(session.status, PairingStatus::Failed);
        assert!(session.device.is_none());
        assert!(session
            .error
            .as_deref()
            .unwrap()
            .contains("timed out while discovering the bulb"));
        assert_eq!(transport.commission_requests.lock().unwrap().len(), 1);
        assert!(hub_data.commissioned.lock().unwrap().is_empty());

        std::fs::remove_dir_all(path).ok();
    }
}
