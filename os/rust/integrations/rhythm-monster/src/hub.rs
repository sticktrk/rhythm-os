//! Linux appliance registration of Monster static lighting as a first-class
//! hub integration. Bluetooth work runs inside the shared `rhythm-ble`
//! adapter's admitted operations; LAN control runs on the server runtime.
//! Cloud calls never happen here: the app brokers them between stages.
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::{Context, Result};
use log::{info, warn};
use rhythm_ble::bluez::{BluezClient, BluezDriverId, DetachedBluezOutput};
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId, HubKey};
use rhythm_os::discovery::{DiscoveredDevice, DiscoveredRoom, HubDiscovery};
use rhythm_os::hub::{
    ActiveHub, ExternalLightHubIntegration, HubCredentials, HubDeviceProfileCapability, HubEvent,
    HubIntegrationCapability, HubProvider, HubType, DEVICE_ONBOARDING_METHOD_BLE_WIFI_NEARBY_SCAN,
};
use rhythm_os::pairing::{
    emit_pairing_progress, PairingRequestContext, PairingSession, PairingStage, PairingStatus,
    UnpairingCompletionScope, UnpairingResult,
};
use rhythm_os::registry::HubDeviceRegistry;
use rhythm_os::state::SharedState;
use uuid::Uuid;

use crate::ble::{self, LightWifiConfig, LightWifiSecurity};
use crate::bluez::LightBluezTransport;
use crate::controller::LightMonsterController;
use crate::lan::{LightLanClient, LightTransport};
use crate::pairing::{
    self, DiscoveredCandidate, LightPairingStage, MAX_CANDIDATES, STAGE_ADOPT, STAGE_DISCOVER,
    STAGE_PROVISION,
};
use crate::store::{now_epoch_secs, LightDeviceRecord, LightDeviceStore};
use crate::{LightError, LightProperty, LightSecret};

pub const HUB_TYPE: &str = pairing::HUB_TYPE;
pub const HUB_ADDRESS: &str = "local";
/// Advertised device profile for the bench-verified Neon Flow strip.
pub const NEON_FLOW_PROFILE_ID: &str = "monster.neon-flow.light.v1";
pub const NEON_FLOW_DISPLAY_NAME: &str = "Monster Neon Flow";
/// Edge function the app brokers commissioning through for this profile.
pub const CLOUD_BROKER_FUNCTION: &str = "monster-device";
const ADAPTER_ADMISSION_TIMEOUT: Duration = Duration::from_secs(5);
const PAIRING_SERVER_SLA: Duration = Duration::from_secs(150);
const ADOPT_READBACK_BUDGET: Duration = Duration::from_secs(20);

pub static INTEGRATION: MonsterIntegration = MonsterIntegration;

pub struct MonsterIntegration;

struct MonsterProvider;
static PROVIDER: MonsterProvider = MonsterProvider;

pub type SharedTransports = Arc<RwLock<HashMap<String, Arc<dyn LightTransport>>>>;

pub struct MonsterHubData {
    pub store: Arc<LightDeviceStore>,
    pub registry: Arc<Mutex<HubDeviceRegistry>>,
    pub transports: SharedTransports,
    _event_tx: Sender<HubEvent>,
}

pub fn hub_key() -> HubKey {
    HubKey::new(HubType::new(HUB_TYPE), HUB_ADDRESS)
}

fn data_dir(state: &SharedState) -> Result<String> {
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    if state.data_dir.trim().is_empty() {
        anyhow::bail!("data_dir not configured on AppState");
    }
    Ok(state.data_dir.clone())
}

pub fn get_hub_data(state: &SharedState) -> Result<Arc<MonsterHubData>> {
    let key = hub_key();
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    state
        .hubs
        .get(&key)
        .and_then(|hub| hub.data::<Arc<MonsterHubData>>())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Monster integration is not connected"))
}

fn transport_for(record: &LightDeviceRecord) -> Option<Arc<dyn LightTransport>> {
    match LightLanClient::new(record.credentials()) {
        Ok(client) => Some(Arc::new(client)),
        Err(error) => {
            warn!(
                target: "sys",
                "Skipping Monster light {} with unusable stored credentials: {error}",
                record.native_id()
            );
            None
        }
    }
}

pub fn connect(state: &SharedState) -> Result<(ActiveHub, Receiver<HubEvent>)> {
    let store = Arc::new(LightDeviceStore::load(data_dir(state)?)?);
    let mut transports: HashMap<String, Arc<dyn LightTransport>> = HashMap::new();
    for record in store.all() {
        if let Some(transport) = transport_for(&record) {
            transports.insert(record.native_id(), transport);
        }
    }
    let active_ids: HashSet<String> = store
        .all()
        .iter()
        .map(LightDeviceRecord::native_id)
        .collect();
    let key = hub_key();
    let mut snapshot = state
        .lock()
        .ok()
        .and_then(|state| {
            state
                .storage
                .as_ref()?
                .load_hub_registry_for(&key)
                .ok()
                .flatten()
        })
        .and_then(|value| {
            serde_json::from_value::<rhythm_os::registry::RegistrySnapshot>(value).ok()
        });
    if let Some(registry) = snapshot.as_mut() {
        registry
            .devices
            .retain(|device| active_ids.contains(&device.id));
    }
    let transports = Arc::new(RwLock::new(transports));
    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let data_store = store.clone();
    let data_transports = transports.clone();
    let data_event_tx = event_tx.clone();
    let (mut hub, event_rx) = rhythm_os::lifecycle::connect_hub(
        state,
        HubType::new(HUB_TYPE),
        key,
        true,
        snapshot,
        move |registry| {
            Box::new(Arc::new(MonsterHubData {
                store: data_store,
                registry,
                transports: data_transports,
                _event_tx: data_event_tx,
            }))
        },
        move |_registry, _shutdown| {
            // LAN control needs no observer thread; announce readiness once so
            // the shared lifecycle marks the hub connected.
            let _ = event_tx.send(HubEvent::Connected { hub_key: None });
            event_rx
        },
    )?;
    hub.discovery = Some(Arc::new(MonsterDiscovery { store }));
    Ok((hub, event_rx))
}

pub struct MonsterDiscovery {
    store: Arc<LightDeviceStore>,
}

fn identity_for(record: &LightDeviceRecord) -> DiscoveredIdentity {
    DiscoveredIdentity {
        native_id: record.native_id(),
        room_id: None,
        room_name: None,
        name: record.name.clone(),
        device_type: DeviceType::Light,
        hardware_ids: vec![HardwareId::serial(&record.dsn)],
        manufacturer: Some(pairing::MANUFACTURER.to_string()),
        model: Some(
            record
                .model
                .clone()
                .unwrap_or_else(|| pairing::DEFAULT_MODEL.to_string()),
        ),
    }
}

impl HubDiscovery for MonsterDiscovery {
    fn discover_rooms(&self) -> Result<Vec<DiscoveredRoom>> {
        Ok(Vec::new())
    }

    fn discover_devices(&self) -> Result<Vec<DiscoveredDevice>> {
        Ok(Vec::new())
    }

    fn discover_identities(&self) -> Result<Vec<DiscoveredIdentity>> {
        Ok(self.store.all().iter().map(identity_for).collect())
    }
}

impl HubProvider for MonsterProvider {
    fn hub_type(&self) -> HubType {
        HubType::new(HUB_TYPE)
    }

    fn configure(&self, address: &str, credentials_json: &str, state: &SharedState) -> Result<()> {
        rhythm_os::lifecycle::configure_hub(
            state,
            address,
            credentials_json,
            |address, json| {
                let data = serde_json::from_str(json)
                    .unwrap_or_else(|_| serde_json::json!({ "adapter": "default" }));
                Ok(HubCredentials::new(HUB_TYPE, address, data))
            },
            |state, credentials| {
                let Some(key) = credentials.hub_key() else {
                    return false;
                };
                state.lock().ok().is_some_and(|state| {
                    state.hubs.contains_key(&key) && state.hub_credentials.contains_key(&key)
                })
            },
            connect,
        )
    }
}

impl MonsterIntegration {
    fn ensure_connected(&self, state: &SharedState) -> Result<()> {
        let key = hub_key();
        if state
            .lock()
            .map_err(|_| anyhow::anyhow!("lock"))?
            .hubs
            .contains_key(&key)
        {
            return Ok(());
        }
        let has_credentials = state
            .lock()
            .map_err(|_| anyhow::anyhow!("lock"))?
            .hub_credentials
            .contains_key(&key);
        if !has_credentials {
            rhythm_os::commands::do_hub_credentials(
                state,
                HUB_TYPE,
                HUB_ADDRESS,
                &serde_json::json!({ "adapter": "default" }),
            )?;
            return Ok(());
        }
        let receiver = self.connect_and_start(state.clone(), &key)?;
        state
            .lock()
            .map_err(|_| anyhow::anyhow!("lock"))?
            .pending_hub_event_rxs
            .push(receiver);
        Ok(())
    }
}

fn progress(
    state: &SharedState,
    session_id: Option<&str>,
    status: PairingStatus,
    stage: PairingStage,
    message: &str,
) {
    emit_pairing_progress(
        state, HUB_TYPE, session_id, status, stage, message, None, None,
    );
}

fn fail(
    state: &SharedState,
    session_id: Option<&str>,
    stage: &str,
    message: String,
    uncertain: bool,
) -> PairingSession {
    emit_pairing_progress(
        state,
        HUB_TYPE,
        session_id,
        PairingStatus::Failed,
        PairingStage::Failed,
        message.clone(),
        None,
        Some(message.clone()),
    );
    pairing::failed_session(stage, message, uncertain)
}

fn discover(
    state: &SharedState,
    session_id: Option<&str>,
    scan_secs: u64,
) -> Result<PairingSession> {
    progress(
        state,
        session_id,
        PairingStatus::Searching,
        PairingStage::Searching,
        "Looking for a nearby Monster strip in setup mode",
    );
    let client = BluezClient::new(BluezDriverId::Monster)?;
    let mut observations = client.subscribe()?;
    let service = Uuid::parse_str(ble::ID_SERVICE).context("Ayla service UUID")?;
    let scan = Duration::from_secs(scan_secs);
    let addresses = client
        .run_adapter_operation_for(
            "monster-discover",
            ADAPTER_ADMISSION_TIMEOUT,
            scan + Duration::from_secs(5),
            move |_, _| async move {
                let deadline = tokio::time::Instant::now() + scan;
                let mut seen = BTreeSet::new();
                loop {
                    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                    if remaining.is_zero() || seen.len() >= MAX_CANDIDATES {
                        break;
                    }
                    match tokio::time::timeout(remaining, observations.recv()).await {
                        Ok(Ok(observation)) => {
                            if observation.service_uuids.contains(&service) {
                                seen.insert(observation.address.to_string().to_ascii_uppercase());
                            }
                        }
                        Ok(Err(_)) | Err(_) => break,
                    }
                }
                DetachedBluezOutput::from_value(seen.into_iter().collect::<Vec<String>>())
            },
        )?
        .into_value()?;
    if addresses.is_empty() {
        return Ok(fail(
            state,
            session_id,
            STAGE_DISCOVER,
            "No Monster strip in setup mode was found nearby. Hold its button until it blinks, then try again.".into(),
            false,
        ));
    }
    progress(
        state,
        session_id,
        PairingStatus::Found,
        PairingStage::Connecting,
        "Reading the strip's serial number",
    );
    let mut candidates = Vec::new();
    for address in addresses {
        let transport = match LightBluezTransport::new(&address) {
            Ok(transport) => transport,
            Err(_) => continue,
        };
        match transport.identify_blocking() {
            Ok(dsn) => candidates.push(DiscoveredCandidate { dsn, address }),
            Err(error) => {
                info!(target: "pair", "Skipped an Ayla advertiser that did not identify: {error}");
            }
        }
    }
    if candidates.is_empty() {
        return Ok(fail(
            state,
            session_id,
            STAGE_DISCOVER,
            "A nearby strip advertised setup mode but did not answer. Move the Rhythm Box closer and try again.".into(),
            false,
        ));
    }
    let count = candidates.len();
    emit_pairing_progress(
        state,
        HUB_TYPE,
        session_id,
        PairingStatus::Complete,
        PairingStage::Complete,
        format!(
            "Found {count} Monster strip{}",
            if count == 1 { "" } else { "s" }
        ),
        None,
        None,
    );
    Ok(pairing::discover_session(&candidates))
}

fn load_wifi(state: &SharedState) -> Result<LightWifiConfig> {
    let wifi = rhythm_os::provisioning::load_accessory_wifi_credentials(state)?
        .ok_or_else(|| anyhow::anyhow!("No saved appliance Wi-Fi credentials are available"))?;
    let security = if wifi.password.is_empty() {
        LightWifiSecurity::Open
    } else {
        LightWifiSecurity::Wpa2
    };
    Ok(LightWifiConfig {
        ssid: LightSecret::new(wifi.ssid),
        password: LightSecret::new(wifi.password),
        security,
    })
}

fn provision(
    state: &SharedState,
    session_id: Option<&str>,
    dsn: &str,
    address: &str,
    setup_token: &LightSecret,
) -> Result<PairingSession> {
    let wifi = match load_wifi(state) {
        Ok(wifi) => wifi,
        Err(error) => {
            return Ok(fail(
                state,
                session_id,
                STAGE_PROVISION,
                format!("{error:#}"),
                false,
            ))
        }
    };
    if let Err(error) = wifi.encode() {
        return Ok(fail(
            state,
            session_id,
            STAGE_PROVISION,
            format!("The appliance Wi-Fi credentials cannot be sent to a Monster strip: {error}"),
            false,
        ));
    }
    progress(
        state,
        session_id,
        PairingStatus::Commissioning,
        PairingStage::Commissioning,
        "Sending Wi-Fi details to the strip",
    );
    let transport = LightBluezTransport::new(address)?;
    match transport.provision_blocking(dsn, setup_token, &wifi) {
        Ok(()) => {
            emit_pairing_progress(
                state,
                HUB_TYPE,
                session_id,
                PairingStatus::Complete,
                PairingStage::Complete,
                "The strip joined Wi-Fi",
                None,
                None,
            );
            Ok(pairing::provision_session(dsn))
        }
        Err(error) => {
            let uncertain = !matches!(
                error,
                LightError::Unavailable | LightError::Identity | LightError::InvalidInput
            );
            let message = match error {
                LightError::Identity => "That strip is not the one Rhythm expected. Start over from the scan.".to_string(),
                LightError::Wifi => "The strip could not join Wi-Fi. Check the network and try again with a fresh scan.".to_string(),
                LightError::Unavailable => "The Rhythm Box could not reach the strip over Bluetooth.".to_string(),
                other => format!("Wi-Fi setup did not complete: {other}"),
            };
            Ok(fail(state, session_id, STAGE_PROVISION, message, uncertain))
        }
    }
}

fn register_canonical_identity(state: &SharedState, record: &LightDeviceRecord) -> Result<()> {
    let identity = identity_for(record);
    let key = hub_key();
    let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    state
        .canonical_registry
        .resolve(&identity, &key, now_epoch_secs());
    let canonical_id = state
        .canonical_registry
        .find_by_native_id(&key, &identity.native_id)
        .map(|device| device.id.clone())
        .ok_or_else(|| anyhow::anyhow!("Canonical Monster endpoint was not registered"))?;
    if let Some(endpoint) = state
        .canonical_registry
        .get_mut(&canonical_id)
        .and_then(|device| {
            device.endpoints.iter_mut().find(|endpoint| {
                endpoint.hub_key == key && endpoint.native_id == identity.native_id
            })
        })
    {
        endpoint.capabilities = Some(serde_json::json!({
            "light_capabilities": { "color_temperature": null, "individual_profile_overrides": null },
            "automatic_naming": { "color_kind": "color" },
        }));
    }
    rhythm_os::commands::save_authority_state(&state).context("persisting Monster endpoint")?;
    Ok(())
}

fn materialize_unassigned(state: &SharedState, native_id: &str) -> Result<()> {
    let (canonical_id, assigned) = {
        let guard = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        guard
            .canonical_registry
            .find_by_native_id(&hub_key(), native_id)
            .map(|device| (device.id.clone(), device.room_id.is_some()))
            .ok_or_else(|| anyhow::anyhow!("Canonical Monster device was not registered"))?
    };
    if assigned {
        rhythm_os::commands::reconcile_runtime_from_state(state)
    } else {
        rhythm_os::commands::do_canonical_assign_room(state, &canonical_id, None)
    }
}

fn adopt(
    state: &SharedState,
    session_id: Option<&str>,
    credentials: crate::LightCredentials,
    name: Option<String>,
    model: Option<String>,
) -> Result<PairingSession> {
    progress(
        state,
        session_id,
        PairingStatus::Commissioning,
        PairingStage::Finalizing,
        "Checking the strip on your network",
    );
    let client = match LightLanClient::new(credentials.clone()) {
        Ok(client) => Arc::new(client),
        Err(error) => {
            return Ok(fail(
                state,
                session_id,
                STAGE_ADOPT,
                format!("Invalid LAN credentials: {error}"),
                false,
            ))
        }
    };
    // The pairing handler is synchronous; give the LAN client its own small
    // runtime for the one authoritative readback.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting Monster readback runtime")?;
    let readback = runtime.block_on(async {
        tokio::time::timeout(ADOPT_READBACK_BUDGET, client.read(LightProperty::Power))
            .await
            .map_err(|_| LightError::Timeout)
            .and_then(|result| result)
    });
    if let Err(error) = readback {
        let message = match error {
            LightError::Authentication | LightError::Integrity => {
                "The strip rejected its LAN key. Ask the cloud for a fresh key and try again.".to_string()
            }
            LightError::Unavailable | LightError::Timeout => {
                "The strip is not reachable on the Rhythm Box's network yet. Wait a moment and try again.".to_string()
            }
            other => format!("LAN readback failed: {other}"),
        };
        return Ok(fail(state, session_id, STAGE_ADOPT, message, false));
    }
    let record = pairing::record_for_adoption(
        &credentials,
        name.as_deref(),
        model.as_deref(),
        now_epoch_secs(),
    );
    let data = get_hub_data(state)?;
    data.store.upsert(record.clone())?;
    if let Ok(mut transports) = data.transports.write() {
        transports.insert(record.native_id(), client);
    }
    register_canonical_identity(state, &record)?;
    materialize_unassigned(state, &record.native_id())?;
    let info = pairing::paired_device_info(&record);
    emit_pairing_progress(
        state,
        HUB_TYPE,
        session_id,
        PairingStatus::Complete,
        PairingStage::Complete,
        format!("{} was added", record.name),
        Some(info),
        None,
    );
    Ok(pairing::adopted_session(&record))
}

pub fn resolve_native_id(state: &SharedState, requested_id: &str) -> Result<String> {
    if requested_id.starts_with("monster-") {
        return Ok(requested_id.to_string());
    }
    state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .canonical_registry
        .get(requested_id)
        .and_then(|device| {
            device
                .endpoints
                .iter()
                .find(|endpoint| endpoint.hub_key == hub_key())
                .map(|endpoint| endpoint.native_id.clone())
        })
        .ok_or_else(|| anyhow::anyhow!("device has no Monster endpoint"))
}

pub fn unpair(state: &SharedState, requested_id: &str) -> Result<UnpairingResult> {
    let native_id = resolve_native_id(state, requested_id)?;
    let (store, transports) = match get_hub_data(state) {
        Ok(data) => (data.store.clone(), Some(data.transports.clone())),
        Err(_) => (Arc::new(LightDeviceStore::load(data_dir(state)?)?), None),
    };
    let removed = store
        .get_by_native_id(&native_id)
        .map(|record| store.remove(&record.dsn))
        .transpose()?
        .flatten();
    if let Some(transports) = transports {
        if let Ok(mut transports) = transports.write() {
            transports.remove(&native_id);
        }
    }
    let has_active_hub = state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .hubs
        .contains_key(&hub_key());
    if has_active_hub {
        rhythm_os::commands::do_device_remove(state, &native_id, &hub_key())?;
    } else {
        rhythm_os::commands::do_device_endpoint_remove(state, &native_id, &hub_key())?;
    }
    Ok(UnpairingResult {
        hub_type: HUB_TYPE.to_string(),
        hub_address: Some(HUB_ADDRESS.to_string()),
        status: PairingStatus::Complete,
        device_id: Some(native_id),
        error: None,
        completion_scope: Some(if removed.is_some() {
            UnpairingCompletionScope::LocalStateOnly
        } else {
            UnpairingCompletionScope::AlreadyAbsent
        }),
        warning: None,
    })
}

impl ExternalLightHubIntegration for MonsterIntegration {
    fn hub_type(&self) -> &'static str {
        HUB_TYPE
    }

    fn provider(&self) -> &'static dyn HubProvider {
        &PROVIDER
    }

    fn api_capabilities(&self) -> HubIntegrationCapability {
        HubIntegrationCapability {
            hub_type: HUB_TYPE.to_string(),
            configurable: false,
            device_onboarding_methods: vec![
                DEVICE_ONBOARDING_METHOD_BLE_WIFI_NEARBY_SCAN.to_string()
            ],
            // The profile is what the app renders and scans for; nothing
            // vendor-specific is hardcoded on the phone.
            device_profiles: vec![HubDeviceProfileCapability {
                id: NEON_FLOW_PROFILE_ID.to_string(),
                compatible_profile_ids: Vec::new(),
                device_type: "light".to_string(),
                display_name: NEON_FLOW_DISPLAY_NAME.to_string(),
                input_only: false,
                onboarding_methods: vec![DEVICE_ONBOARDING_METHOD_BLE_WIFI_NEARBY_SCAN.to_string()],
                nearby_service_uuids: vec![ble::ID_SERVICE.to_string()],
                cloud_broker: Some(CLOUD_BROKER_FUNCTION.to_string()),
                phone_provisioning_protocol: Some("ayla_v1".to_string()),
            }],
            supports_unpairing: true,
            unpairable_device_types: vec!["light".to_string()],
            supports_roomless_devices: true,
            // The store restores every light locally and LAN control needs no
            // cloud session, so Rooms never wait on this hub.
            blocks_room_readiness: false,
        }
    }

    fn connect_and_start(&self, state: SharedState, _key: &HubKey) -> Result<Receiver<HubEvent>> {
        let (hub, event_rx) = connect(&state)?;
        {
            let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            let key = hub.hub_key.clone();
            if state.hubs.contains_key(&key) {
                drop(state);
                drop(hub);
                drop(event_rx);
                let (_closed_tx, closed_rx) = std::sync::mpsc::channel();
                return Ok(closed_rx);
            }
            state.hubs.insert(key.clone(), hub);
            state.set_hub_connected(&key, false);
        }
        Ok(event_rx)
    }

    fn ensure_runtime(&self, _state: &SharedState) -> Result<()> {
        Ok(())
    }

    fn create_controller(
        &self,
        state: &SharedState,
        _key: &HubKey,
    ) -> Result<Arc<dyn rhythm_core::HubLightController>> {
        let data = get_hub_data(state)?;
        Ok(Arc::new(LightMonsterController::with_shared_devices(
            data.registry.clone(),
            data.transports.clone(),
        )))
    }

    fn start_pairing(
        &self,
        state: &SharedState,
        params: &serde_json::Value,
    ) -> Result<PairingSession> {
        self.start_pairing_with_context(state, params, PairingRequestContext::accepted_now())
    }

    fn start_pairing_with_context(
        &self,
        state: &SharedState,
        params: &serde_json::Value,
        context: PairingRequestContext,
    ) -> Result<PairingSession> {
        let deadline = context.deadline_after(PAIRING_SERVER_SLA);
        if deadline <= std::time::Instant::now() {
            anyhow::bail!("Monster pairing deadline expired before request admission");
        }
        let session_id = params.get("session_id").and_then(serde_json::Value::as_str);
        let stage = match pairing::parse_stage(params) {
            Ok(stage) => stage,
            Err(error) => {
                return Ok(fail(
                    state,
                    session_id,
                    "request",
                    format!("{error:#}"),
                    false,
                ))
            }
        };
        progress(
            state,
            session_id,
            PairingStatus::Searching,
            PairingStage::HubConnecting,
            "Preparing the Rhythm Box",
        );
        if let Err(error) = self.ensure_connected(state) {
            return Ok(fail(
                state,
                session_id,
                stage.name(),
                format!("{error:#}"),
                false,
            ));
        }
        match stage {
            LightPairingStage::Discover { scan_secs } => discover(state, session_id, scan_secs),
            LightPairingStage::Provision {
                dsn,
                address,
                setup_token,
            } => provision(state, session_id, &dsn, &address, &setup_token),
            LightPairingStage::Adopt {
                credentials,
                name,
                model,
            } => adopt(state, session_id, credentials, name, model),
        }
    }

    fn start_unpairing(
        &self,
        state: &SharedState,
        params: &serde_json::Value,
    ) -> Result<UnpairingResult> {
        let device_id = params
            .get("device_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Missing 'device_id' in Monster unpair request"))?;
        unpair(state, device_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_advertises_staged_nearby_scan_without_blocking_rooms() {
        let capabilities = INTEGRATION.api_capabilities();
        assert_eq!(capabilities.hub_type, "monster");
        assert_eq!(
            capabilities.device_onboarding_methods,
            vec![DEVICE_ONBOARDING_METHOD_BLE_WIFI_NEARBY_SCAN]
        );
        assert!(capabilities.supports_unpairing);
        assert!(capabilities.supports_roomless_devices);
        assert!(!capabilities.blocks_room_readiness);
        let profile = &capabilities.device_profiles[0];
        assert_eq!(profile.id, NEON_FLOW_PROFILE_ID);
        assert_eq!(profile.nearby_service_uuids, vec![ble::ID_SERVICE]);
        assert_eq!(
            profile.phone_provisioning_protocol.as_deref(),
            Some("ayla_v1")
        );
        assert_eq!(profile.cloud_broker.as_deref(), Some(CLOUD_BROKER_FUNCTION));
        assert!(rhythm_os::hub::is_valid_device_profile_id(
            NEON_FLOW_PROFILE_ID
        ));
    }

    #[test]
    fn malformed_stage_fails_visibly_without_touching_the_adapter() {
        let state = Arc::new(Mutex::new(rhythm_os::state::AppState::default()));
        let session = INTEGRATION
            .start_pairing(&state, &serde_json::json!({"stage": "reset"}))
            .unwrap();
        assert_eq!(session.status, PairingStatus::Failed);
        assert_eq!(session.details.unwrap()["stage"], "request");
    }
}
