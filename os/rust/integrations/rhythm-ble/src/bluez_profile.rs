//! Linux BlueZ transport client for local-BLE profiles.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use bluer::{Adapter, Device};
use rhythm_os::hub::HubEvent;
use rhythm_os::registry::HubDeviceRegistry;

use crate::bluez::{BluezClient, ScanObservation, ScannerHealth};
use crate::profile::{
    decode_profile_events, profile_by_id, profiles, BleAdvertisement, BleDeviceProfile,
    BleProfileAdmission, ValidatedBleSetup,
};
use crate::store::LocalBleDeviceStore;
use crate::transport::{
    accept_profile_advertisement, LocalBleAssociationDeadline, LocalBlePairingCandidate,
    LocalBleTransport,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const SERVICE_TIMEOUT: Duration = Duration::from_secs(30);
const CANDIDATE_CLEANUP_RESERVE: Duration = Duration::from_secs(2);
const ADDRESS_ROTATION_GRACE: Duration = Duration::from_secs(2);
const RETIRED_HINT_TTL: Duration = Duration::from_secs(10 * 60);
const RETIRED_HINT_LIMIT: usize = 8;

pub struct BluezLocalBleTransport {
    client: Arc<BluezClient>,
}

impl BluezLocalBleTransport {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: Arc::new(BluezClient::new("local_ble")?),
        })
    }

    async fn associate_inner(
        client: Arc<BluezClient>,
        adapter: Adapter,
        profile: &'static dyn BleDeviceProfile,
        setup: ValidatedBleSetup,
        deadline: LocalBleAssociationDeadline,
    ) -> Result<LocalBlePairingCandidate> {
        let mut observations = client.subscribe()?;
        let proof_deadline = tokio::time::Instant::from_std(deadline.proof_end());

        loop {
            let observation = tokio::time::timeout_at(proof_deadline, observations.recv())
                .await
                .map_err(|_| anyhow::anyhow!("local Bluetooth discovery timed out"))??;
            if observation.rssi.is_none() {
                continue;
            }
            let Ok(advertisement) = advertisement_snapshot(&observation) else {
                continue;
            };
            let Some(identity) = profile.stable_identity_from_advertisement(advertisement.view())
            else {
                continue;
            };
            if identity != setup.stable_identity() {
                continue;
            }

            match profile.admission() {
                BleProfileAdmission::AdvertisementOnly => {}
                admission @ BleProfileAdmission::GattServiceProof { .. } => {
                    let device = adapter.device(observation.address).map_err(|_| {
                        anyhow::anyhow!("local Bluetooth candidate could not be opened")
                    })?;
                    let was_connected = match tokio::time::timeout_at(
                        proof_deadline,
                        device.is_connected(),
                    )
                    .await
                    {
                        Ok(Ok(connected)) => connected,
                        Ok(Err(_)) => continue,
                        Err(_) => {
                            anyhow::bail!("local Bluetooth candidate proof timed out")
                        }
                    };
                    let validation =
                        Self::validate_candidate(admission, &device, was_connected, deadline).await;
                    if !was_connected {
                        // Once this path attempted a connection, status reads are not
                        // reliable enough to guard cleanup. Disconnect unconditionally.
                        Self::cleanup_candidate(&device, deadline).await?;
                    }
                    if validation.is_err() {
                        // Advertisement identity is insufficient when this
                        // profile declares a second GATT proof.
                        continue;
                    }
                }
            }
            let initial_replay = decode_profile_events(profile, advertisement.view())?
                .into_iter()
                .filter_map(|event| event.replay)
                .map(|replay| (replay.stream, replay.value))
                .collect::<BTreeMap<_, _>>();
            return Ok(LocalBlePairingCandidate {
                transport_hint: observation.address.to_string(),
                initial_replay,
            });
        }
    }

    async fn validate_candidate(
        admission: BleProfileAdmission,
        device: &Device,
        was_connected: bool,
        deadline: LocalBleAssociationDeadline,
    ) -> Result<()> {
        if !was_connected {
            let connect_deadline =
                tokio::time::Instant::from_std(deadline.stage_end(CONNECT_TIMEOUT)?);
            tokio::time::timeout_at(connect_deadline, device.connect())
                .await
                .map_err(|_| anyhow::anyhow!("local Bluetooth connection timed out"))?
                .map_err(|_| anyhow::anyhow!("local Bluetooth connection failed"))?;
        }
        let service_deadline = tokio::time::Instant::from_std(deadline.stage_end(SERVICE_TIMEOUT)?);
        let services = tokio::time::timeout_at(service_deadline, device.services())
            .await
            .map_err(|_| anyhow::anyhow!("local Bluetooth service validation timed out"))?
            .map_err(|_| anyhow::anyhow!("local Bluetooth service validation failed"))?;
        for service in services {
            let id = tokio::time::timeout_at(service_deadline, service.uuid())
                .await
                .map_err(|_| anyhow::anyhow!("local Bluetooth service validation timed out"))?
                .map_err(|_| anyhow::anyhow!("local Bluetooth service validation failed"))?;
            if admission.accepts_gatt_service(&id.to_string()) {
                return Ok(());
            }
        }
        anyhow::bail!("local Bluetooth profile evidence did not match")
    }

    async fn cleanup_candidate(
        device: &Device,
        deadline: LocalBleAssociationDeadline,
    ) -> Result<()> {
        let cleanup_deadline = tokio::time::Instant::from_std(deadline.cleanup_end());
        match tokio::time::timeout_at(cleanup_deadline, device.disconnect()).await {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(_)) => {}
            Err(_) => anyhow::bail!("local Bluetooth candidate cleanup timed out"),
        }

        let still_connected = tokio::time::timeout_at(cleanup_deadline, device.is_connected())
            .await
            .map_err(|_| anyhow::anyhow!("local Bluetooth candidate cleanup timed out"))?
            .map_err(|_| {
                anyhow::anyhow!("local Bluetooth candidate cleanup could not be confirmed")
            })?;
        if still_connected {
            anyhow::bail!("local Bluetooth candidate cleanup failed");
        }
        Ok(())
    }

    async fn monitor_supervisor(
        client: Arc<BluezClient>,
        store: Arc<LocalBleDeviceStore>,
        registry: Arc<Mutex<HubDeviceRegistry>>,
        event_tx: Sender<HubEvent>,
        shutdown: Arc<AtomicBool>,
    ) {
        let mut backoff = Duration::from_millis(250);
        while !shutdown.load(Ordering::Acquire) {
            let result = Self::monitor_once(
                client.clone(),
                store.clone(),
                registry.clone(),
                event_tx.clone(),
                shutdown.clone(),
            )
            .await;
            if shutdown.load(Ordering::Acquire) {
                break;
            }
            if result.is_err() {
                let _ = event_tx.send(HubEvent::Disconnected {
                    hub_key: None,
                    reason: "Local Bluetooth observer is recovering".to_string(),
                });
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(5));
            if result.is_ok() {
                backoff = Duration::from_millis(250);
            }
        }
    }

    async fn monitor_once(
        client: Arc<BluezClient>,
        store: Arc<LocalBleDeviceStore>,
        registry: Arc<Mutex<HubDeviceRegistry>>,
        event_tx: Sender<HubEvent>,
        shutdown: Arc<AtomicBool>,
    ) -> Result<()> {
        let (mut observations, mut scanner_health) = client.subscribe_broker()?;
        let mut routes = store
            .all()
            .into_iter()
            .map(|device| {
                (
                    identity_route_key(&device.profile_id, &device.stable_identity),
                    IdentityRoute::new(device.transport_hint),
                )
            })
            .collect::<HashMap<_, _>>();
        let mut connected = false;
        let initial_health = *scanner_health.borrow();
        publish_scanner_health(initial_health, &mut connected, &event_tx);

        while !shutdown.load(Ordering::Acquire) {
            let observation = tokio::select! {
                changed = scanner_health.changed() => {
                    changed.map_err(|_| anyhow::anyhow!("shared Bluetooth scanner health stream closed"))?;
                    let health = *scanner_health.borrow();
                    publish_scanner_health(health, &mut connected, &event_tx);
                    continue;
                }
                observation = observations.recv() => observation?,
                _ = tokio::time::sleep(Duration::from_millis(500)) => continue,
            };
            // Health and observations use separate channels. Active is
            // published before the first observation, but both may already be
            // ready when select polls them. Reconcile the watch's current
            // value here so channel ordering can never discard that first
            // valid event after startup or recovery.
            publish_scanner_health(*scanner_health.borrow(), &mut connected, &event_tx);
            if !connected {
                continue;
            }
            if observation.rssi.is_none() {
                continue;
            }
            let Ok(advertisement) = advertisement_snapshot(&observation) else {
                continue;
            };
            let matches = profiles()
                .iter()
                .filter_map(|profile| {
                    let profile = *profile;
                    let identity =
                        profile.stable_identity_from_advertisement(advertisement.view())?;
                    let device = store.get_by_identity(profile.descriptor().id, &identity)?;
                    Some((profile, identity, device))
                })
                .collect::<Vec<_>>();
            for (profile, identity, mut device) in matches {
                let route = routes
                    .entry(identity_route_key(profile.descriptor().id, &identity))
                    .or_insert_with(|| IdentityRoute::new(device.transport_hint.clone()));
                let observed_hint = observation.address.to_string();
                if !route.accept(&observed_hint, observation.observed_at) {
                    continue;
                }
                if !device.transport_hint.eq_ignore_ascii_case(&observed_hint) {
                    store.update_transport_hint(
                        profile.descriptor().id,
                        &identity,
                        &observed_hint,
                    )?;
                    device.transport_hint = observed_hint;
                }
                accept_profile_advertisement(
                    advertisement.view(),
                    &device,
                    &store,
                    &registry,
                    &event_tx,
                )?;
            }
        }
        Ok(())
    }
}

fn advertisement_snapshot(observation: &ScanObservation) -> Result<BleAdvertisement> {
    BleAdvertisement::try_from_parts(
        observation.service_uuids.iter().copied(),
        observation
            .service_data
            .iter()
            .map(|(service_uuid, payload)| (*service_uuid, payload.clone())),
        observation
            .manufacturer_data
            .iter()
            .map(|(manufacturer_id, payload)| (*manufacturer_id, payload.clone())),
    )
}

fn publish_scanner_health(
    health: ScannerHealth,
    connected: &mut bool,
    event_tx: &Sender<HubEvent>,
) {
    if health == ScannerHealth::Active {
        if !*connected {
            let _ = event_tx.send(HubEvent::Connected { hub_key: None });
            *connected = true;
        }
        return;
    }
    if *connected {
        let reason = match health {
            ScannerHealth::PausedForExternalOwner => {
                "Local Bluetooth observer paused for adapter commissioning"
            }
            ScannerHealth::Starting => "Local Bluetooth observer is starting",
            ScannerHealth::Recovering => "Local Bluetooth observer is recovering",
            ScannerHealth::Stopped => "Local Bluetooth observer stopped",
            ScannerHealth::Active => unreachable!(),
        };
        let _ = event_tx.send(HubEvent::Disconnected {
            hub_key: None,
            reason: reason.to_string(),
        });
        *connected = false;
    }
}

impl LocalBleTransport for BluezLocalBleTransport {
    fn is_available(&self) -> Result<bool> {
        self.client
            .run_adapter_operation(
                |_session, adapter| async move { Ok(adapter.is_powered().await?) },
            )
    }

    fn is_available_until(&self, deadline: Instant) -> Result<bool> {
        self.client
            .run_adapter_operation_until(deadline, |_session, adapter| async move {
                Ok(adapter.is_powered().await?)
            })
    }

    fn associate(
        &self,
        profile_id: &str,
        setup: &ValidatedBleSetup,
        timeout: Duration,
    ) -> Result<LocalBlePairingCandidate> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| anyhow::anyhow!("local Bluetooth association timeout overflow"))?;
        self.associate_until(profile_id, setup, deadline)
    }

    fn associate_until(
        &self,
        profile_id: &str,
        setup: &ValidatedBleSetup,
        deadline: Instant,
    ) -> Result<LocalBlePairingCandidate> {
        let profile = profile_by_id(profile_id)
            .ok_or_else(|| anyhow::anyhow!("unsupported local Bluetooth profile"))?;
        let deadline = LocalBleAssociationDeadline::new(deadline, CANDIDATE_CLEANUP_RESERVE)?;
        let client = self.client.clone();
        let setup = setup.clone();
        self.client
            .run_adapter_operation_until(deadline.cleanup_end(), move |_session, adapter| {
                Self::associate_inner(client, adapter, profile, setup, deadline)
            })
    }

    fn start_monitor(
        &self,
        store: Arc<LocalBleDeviceStore>,
        registry: Arc<Mutex<HubDeviceRegistry>>,
        event_tx: Sender<HubEvent>,
        shutdown: Arc<AtomicBool>,
    ) -> Result<JoinHandle<()>> {
        let client = self.client.clone();
        std::thread::Builder::new()
            .name("local-ble-observer".to_string())
            .spawn(move || {
                let future =
                    Self::monitor_supervisor(client.clone(), store, registry, event_tx, shutdown);
                if let Err(error) = client.run_broker_observer(future) {
                    tracing::warn!(
                        target: "ble",
                        "Local Bluetooth observer exited at bounded runtime stage"
                    );
                    let _ = error;
                }
            })
            .context("spawning local Bluetooth observer")
    }

    fn quiesce(&self) -> Result<()> {
        self.client.quiesce()
    }
}

struct IdentityRoute {
    current: String,
    last_current_seen: Instant,
    pending: Option<(String, u8)>,
    retired: VecDeque<RetiredHint>,
}

struct RetiredHint {
    address: String,
    retired_at: Instant,
}

impl IdentityRoute {
    fn new(current: String) -> Self {
        Self::new_at(current, Instant::now())
    }

    fn new_at(current: String, now: Instant) -> Self {
        Self {
            current,
            last_current_seen: now,
            pending: None,
            retired: VecDeque::new(),
        }
    }

    fn accept(&mut self, observed: &str, now: Instant) -> bool {
        self.prune_retired(now);
        if self.current.eq_ignore_ascii_case(observed) {
            self.last_current_seen = now;
            self.pending = None;
            return true;
        }
        if self
            .retired
            .iter()
            .any(|hint| hint.address.eq_ignore_ascii_case(observed))
            || now.saturating_duration_since(self.last_current_seen) < ADDRESS_ROTATION_GRACE
        {
            return false;
        }
        let confirmations = match self.pending.as_mut() {
            Some((address, confirmations)) if address.eq_ignore_ascii_case(observed) => {
                *confirmations = confirmations.saturating_add(1);
                *confirmations
            }
            _ => {
                self.pending = Some((observed.to_string(), 1));
                1
            }
        };
        if confirmations < 2 {
            return false;
        }
        self.retired.push_back(RetiredHint {
            address: self.current.clone(),
            retired_at: now,
        });
        while self.retired.len() > RETIRED_HINT_LIMIT {
            self.retired.pop_front();
        }
        self.current = observed.to_string();
        self.last_current_seen = now;
        self.pending = None;
        true
    }

    fn prune_retired(&mut self, now: Instant) {
        while self
            .retired
            .front()
            .is_some_and(|hint| now.saturating_duration_since(hint.retired_at) > RETIRED_HINT_TTL)
        {
            self.retired.pop_front();
        }
    }
}

fn identity_route_key(profile_id: &str, stable_identity: &str) -> (String, String) {
    (profile_id.to_string(), stable_identity.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{ValidatedBleSetup, OREIN_OC02001_PROFILE_ID};
    use std::collections::BTreeMap;

    #[test]
    fn scanner_health_drives_connected_state_without_false_startup_success() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut connected = false;

        publish_scanner_health(ScannerHealth::Starting, &mut connected, &tx);
        publish_scanner_health(ScannerHealth::PausedForExternalOwner, &mut connected, &tx);
        assert!(rx.try_recv().is_err());

        publish_scanner_health(ScannerHealth::Active, &mut connected, &tx);
        assert!(matches!(rx.recv().unwrap(), HubEvent::Connected { .. }));
        publish_scanner_health(ScannerHealth::Recovering, &mut connected, &tx);
        assert!(matches!(rx.recv().unwrap(), HubEvent::Disconnected { .. }));

        publish_scanner_health(ScannerHealth::Active, &mut connected, &tx);
        assert!(matches!(rx.recv().unwrap(), HubEvent::Connected { .. }));
    }

    #[test]
    fn current_health_is_reconciled_before_a_ready_observation() {
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let (health_tx, health_rx) = tokio::sync::watch::channel(ScannerHealth::Starting);
        let mut connected = false;

        // The Active transition is intentionally left unread, matching the
        // select race where an observation branch wins while health is ready.
        health_tx.send_replace(ScannerHealth::Active);
        publish_scanner_health(*health_rx.borrow(), &mut connected, &event_tx);

        assert!(connected);
        assert!(matches!(
            event_rx.recv().unwrap(),
            HubEvent::Connected { .. }
        ));
    }

    #[test]
    fn proven_address_rotation_requires_stability_and_never_flips_to_retired_hint() {
        let start = Instant::now();
        let mut route = IdentityRoute::new_at("02:00:00:00:00:01".to_string(), start);

        assert!(!route.accept("02:00:00:00:00:02", start + Duration::from_secs(1)));
        assert!(!route.accept("02:00:00:00:00:02", start + Duration::from_secs(3)));
        assert!(route.accept("02:00:00:00:00:02", start + Duration::from_secs(4)));
        assert!(!route.accept("02:00:00:00:00:01", start + Duration::from_secs(7)));
        assert!(route.accept("02:00:00:00:00:02", start + Duration::from_secs(8)));
    }

    #[test]
    fn every_connection_opened_for_validation_is_disconnect_eligible() {
        // The live BlueZ path snapshots `was_connected`, performs validation,
        // and disconnects unconditionally when this flow owned the connection.
        // A second status read must not be able to leak the connection.
        let should_disconnect = |was_connected: bool| !was_connected;
        assert!(should_disconnect(false));
        assert!(!should_disconnect(true));
    }

    #[test]
    fn alternating_retired_and_rotated_addresses_cannot_replay_counters() {
        use crate::profile::BleReplayToken;
        use crate::store::{LocalBleDevice, ReplayDisposition};

        let root = std::env::temp_dir().join(format!(
            "rhythm-local-ble-address-counter-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = LocalBleDeviceStore::load(&root).unwrap();
        let device = LocalBleDevice::from_setup(
            OREIN_OC02001_PROFILE_ID,
            &ValidatedBleSetup {
                stable_identity: "A1B2C3D4E5F6".to_string(),
                metadata: BTreeMap::from([
                    ("serial".to_string(), "sanitized-s".to_string()),
                    ("model".to_string(), "sanitized-m".to_string()),
                ]),
            },
            "02:00:00:00:00:01".to_string(),
            BTreeMap::from([("press".to_string(), vec![7])]),
            false,
            1,
        );
        store.upsert(device).unwrap();
        let start = Instant::now();
        let mut route = IdentityRoute::new_at("02:00:00:00:00:01".to_string(), start);

        assert!(!route.accept("02:00:00:00:00:02", start + Duration::from_secs(3)));
        assert!(route.accept("02:00:00:00:00:02", start + Duration::from_secs(4)));
        store
            .update_transport_hint(
                OREIN_OC02001_PROFILE_ID,
                "A1B2C3D4E5F6",
                "02:00:00:00:00:02",
            )
            .unwrap();
        assert_eq!(
            store
                .accept_replay(
                    OREIN_OC02001_PROFILE_ID,
                    "A1B2C3D4E5F6",
                    Some(&BleReplayToken {
                        stream: "press".to_string(),
                        value: vec![8],
                    }),
                )
                .unwrap(),
            ReplayDisposition::New
        );
        assert!(!route.accept("02:00:00:00:00:01", start + Duration::from_secs(8)));
        assert_eq!(
            store
                .get_by_identity(OREIN_OC02001_PROFILE_ID, "A1B2C3D4E5F6")
                .unwrap()
                .replay_state
                .get("press"),
            Some(&vec![8])
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn routes_are_scoped_by_profile_even_when_private_identity_strings_collide() {
        assert_ne!(
            identity_route_key("example.button.v1", "A1B2"),
            identity_route_key("example.motion.v1", "A1B2")
        );
        assert_ne!(
            identity_route_key("example.motion.v1", "a1b2"),
            identity_route_key("example.motion.v1", "A1B2")
        );
    }

    #[test]
    fn retired_address_memory_is_aged_and_bounded() {
        let start = Instant::now();
        let mut route = IdentityRoute::new_at("address-01".to_string(), start);
        let mut now = start;
        for index in 2..=20 {
            now += ADDRESS_ROTATION_GRACE + Duration::from_secs(1);
            let address = format!("address-{index:02}");
            assert!(!route.accept(&address, now));
            now += Duration::from_secs(1);
            assert!(route.accept(&address, now));
        }

        assert_eq!(route.retired.len(), RETIRED_HINT_LIMIT);
        assert!(!route.accept(
            "address-19",
            now + ADDRESS_ROTATION_GRACE + Duration::from_secs(1)
        ));
        route.prune_retired(now + RETIRED_HINT_TTL + Duration::from_secs(1));
        assert!(route.retired.is_empty());
    }
}
