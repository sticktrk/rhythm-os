//! One process-wide BlueZ central-role runtime and discovery broker.
//!
//! Integration drivers are clients of this module. They must not open their
//! own D-Bus session, Tokio runtime, or discovery stream.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock, RwLockReadGuard, TryLockError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use bluer::{
    Adapter, AdapterEvent, Address, DeviceEvent, DeviceProperty, DiscoveryFilter,
    DiscoveryTransport, ErrorKind, Session,
};
use futures::{Stream, StreamExt};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::runtime::Runtime;
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio_stream::StreamMap;
use uuid::Uuid;

pub use crate::coordination::BluezOperationDeadlineExceeded;
use crate::coordination::{
    run_validated_device_operation_batch, AdapterOperationGate, ClientOperationCoordinator,
};
use crate::transport::LocalBleAssociationDiagnostics;

/// Closed identities for integrations admitted to the shared BlueZ runtime.
///
/// Using a driver ID rather than a caller-selected string guarantees that two
/// handles for one integration share the same operation coordinator and
/// cannot create extra lanes by choosing another name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BluezDriverId {
    LocalProfiles,
    Hue,
}

impl BluezDriverId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalProfiles => "local_ble",
            Self::Hue => "hue_ble",
        }
    }
}

/// Private implementation boundary for values allowed to leave an admitted
/// BlueZ operation. Downstream drivers cannot add a wrapper containing a raw
/// `bluer` handle and mark it safe themselves.
pub(crate) mod operation_output_sealed {
    pub trait Sealed {}
}

/// Sealed marker for values that cannot retain a live BlueZ handle after an
/// operation releases its admission guards.
pub trait BluezOperationOutput: operation_output_sealed::Sealed + Send + 'static {}

impl operation_output_sealed::Sealed for () {}
impl operation_output_sealed::Sealed for bool {}
impl operation_output_sealed::Sealed for String {}
impl operation_output_sealed::Sealed for Instant {}
impl BluezOperationOutput for () {}
impl BluezOperationOutput for bool {}
impl BluezOperationOutput for String {}
impl BluezOperationOutput for Instant {}

impl<T: BluezOperationOutput> operation_output_sealed::Sealed for Option<T> {}
impl<T: BluezOperationOutput> operation_output_sealed::Sealed for Vec<T> {}
impl<T: BluezOperationOutput> BluezOperationOutput for Option<T> {}
impl<T: BluezOperationOutput> BluezOperationOutput for Vec<T> {}

/// Byte-detached operation result for data-transfer objects owned by another
/// integration crate. Serializing inside the admitted closure and rebuilding
/// outside it makes retaining a BlueZ session/device/GATT handle structurally
/// impossible through the return value.
pub struct DetachedBluezOutput<T> {
    bytes: Vec<u8>,
    value_type: PhantomData<fn() -> T>,
}

impl<T> DetachedBluezOutput<T>
where
    T: Serialize,
{
    pub fn from_value(value: T) -> Result<Self> {
        Ok(Self {
            bytes: serde_json::to_vec(&value).context("detaching BlueZ operation output")?,
            value_type: PhantomData,
        })
    }
}

impl<T> DetachedBluezOutput<T>
where
    T: DeserializeOwned,
{
    pub fn into_value(self) -> Result<T> {
        serde_json::from_slice(&self.bytes).context("rebuilding detached BlueZ operation output")
    }
}

impl<T: Send + 'static> operation_output_sealed::Sealed for DetachedBluezOutput<T> {}
impl<T: Send + 'static> BluezOperationOutput for DetachedBluezOutput<T> {}

const SCAN_CHANNEL_CAPACITY: usize = 1024;
const SCAN_RESTART_MIN_BACKOFF: Duration = Duration::from_millis(250);
const SCAN_RESTART_MAX_BACKOFF: Duration = Duration::from_secs(5);
const OBSERVATION_CACHE_TTL: Duration = Duration::from_secs(120);
const OBSERVATION_CACHE_LIMIT: usize = 512;
const DEVICE_SUBSCRIPTION_LIMIT: usize = 512;
const DEVICE_SUBSCRIPTION_ROTATION_INTERVAL: Duration = Duration::from_secs(30);
const RESERVATION_ACK_TIMEOUT: Duration = Duration::from_secs(5);
const DISCOVERY_STOP_RETRY_INTERVAL: Duration = Duration::from_millis(25);
const DISCOVERY_STOP_SAFETY_MARGIN: Duration = Duration::from_millis(250);
const CLIENT_ADMISSION_TIMEOUT: Duration = Duration::from_secs(5);
const CLIENT_OPERATION_TIMEOUT: Duration = Duration::from_secs(120);
const CLIENT_QUIESCE_TIMEOUT: Duration = Duration::from_secs(125);
/// Normal clients take one permit, so independent bounded GATT work does not
/// globally serialize. An external commissioner takes the whole pool to drain
/// every in-flight operation before it receives the adapter lease.
const ADAPTER_OPERATION_PERMITS: u32 = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanObservation {
    pub address: Address,
    pub rssi: Option<i16>,
    pub service_uuids: HashSet<Uuid>,
    pub service_data: HashMap<Uuid, Vec<u8>>,
    pub manufacturer_data: HashMap<u16, Vec<u8>>,
    pub sequence: u64,
    pub observed_at: Instant,
}

/// Lossless state for consumers that need to distinguish a running broker
/// from a subscription that is merely waiting for adapter recovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScannerHealth {
    Starting,
    Active,
    PausedForExternalOwner,
    Recovering,
    Stopped,
}

impl ScannerHealth {
    fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Active => "active",
            Self::PausedForExternalOwner => "paused_for_external_owner",
            Self::Recovering => "recovering",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Clone)]
struct BluezContext {
    session: Session,
    adapter: Adapter,
}

#[derive(Default)]
struct ScanControlState {
    starting: bool,
    active: bool,
    stop_failed: bool,
}

impl ScanControlState {
    fn begin(&mut self, externally_reserved: bool) -> bool {
        if externally_reserved || self.starting || self.active {
            return false;
        }
        self.starting = true;
        self.stop_failed = false;
        true
    }

    fn mark_active(&mut self) {
        self.starting = false;
        self.active = true;
    }

    fn finish(&mut self, stop_acknowledged: bool) {
        self.starting = false;
        self.active = false;
        self.stop_failed = !stop_acknowledged;
    }

    fn busy(&self) -> bool {
        self.starting || self.active
    }
}

/// Bluer begins every discovery stream by replaying all addresses already
/// known to BlueZ. Their cached RSSI/service properties are not evidence of a
/// live advertisement, so suppress exactly that first replay. A newly added
/// address, or the next property-change event for a known address, is fresh.
struct InitialKnownDeviceFilter {
    pending: HashSet<Address>,
}

impl InitialKnownDeviceFilter {
    fn new(known: impl IntoIterator<Item = Address>) -> Self {
        Self {
            pending: known.into_iter().collect(),
        }
    }

    fn accepts(&mut self, address: Address) -> bool {
        !self.pending.remove(&address)
    }

    fn removed(&mut self, address: Address) {
        self.pending.remove(&address);
    }
}

/// Avoid `bluer::Adapter::discover_devices_with_changes`, which collapses
/// every device property change into `DeviceAdded`, including connection and
/// GATT state. The raw adapter stream plus per-device events preserve which
/// property changed, so only advertisement evidence refreshes an observation.
/// RSSI means the device is currently in range; service/manufacturer data are
/// advertisement payloads.
fn is_fresh_advertisement_property(property: &DeviceProperty) -> bool {
    matches!(
        property,
        DeviceProperty::Rssi(_)
            | DeviceProperty::ServiceData(_)
            | DeviceProperty::ManufacturerData(_)
    )
}

#[derive(Clone, Debug)]
struct DeviceSubscriptionGeneration(Arc<()>);

/// Fence property streams to the lifetime of the BlueZ device object that
/// created them. Bluer closes a device stream when the object is removed, but
/// its unbounded receiver may still yield already-queued properties. An opaque
/// allocation identity avoids both counter wraparound and accepting an old
/// stream after the same Bluetooth address is re-added.
struct DeviceSubscriptionGenerations {
    current: HashMap<Address, DeviceSubscriptionGeneration>,
    limit: usize,
}

impl DeviceSubscriptionGenerations {
    fn new(limit: usize) -> Self {
        Self {
            current: HashMap::with_capacity(limit),
            limit,
        }
    }

    fn begin(&mut self, address: Address) -> DeviceSubscriptionAdmission {
        if self.current.contains_key(&address) {
            return DeviceSubscriptionAdmission::AlreadySubscribed;
        }
        if self.current.len() >= self.limit {
            return DeviceSubscriptionAdmission::AtCapacity;
        }
        let generation = DeviceSubscriptionGeneration(Arc::new(()));
        self.current.insert(address, generation.clone());
        DeviceSubscriptionAdmission::Started(generation)
    }

    fn remove(&mut self, address: Address) {
        self.current.remove(&address);
    }

    fn remove_if_current(
        &mut self,
        address: Address,
        generation: &DeviceSubscriptionGeneration,
    ) -> bool {
        if !self.is_current(address, generation) {
            return false;
        }
        self.current.remove(&address);
        true
    }

    fn invalidate_all(&mut self) {
        self.current.clear();
    }

    fn is_current(&self, address: Address, generation: &DeviceSubscriptionGeneration) -> bool {
        self.current
            .get(&address)
            .is_some_and(|current| Arc::ptr_eq(&current.0, &generation.0))
    }
}

impl Default for DeviceSubscriptionGenerations {
    fn default() -> Self {
        Self::new(DEVICE_SUBSCRIPTION_LIMIT)
    }
}

enum DeviceSubscriptionAdmission {
    AlreadySubscribed,
    AtCapacity,
    Started(DeviceSubscriptionGeneration),
}

#[derive(Debug, PartialEq, Eq)]
enum DeviceSubscriptionOutcome {
    AlreadySubscribed,
    AtCapacity,
    Subscribed,
}

enum DeviceSubscriptionChange {
    Event(DeviceSubscriptionGeneration, DeviceEvent),
    Closed(DeviceSubscriptionGeneration),
}

type DeviceChangeStream = Pin<Box<dyn Stream<Item = DeviceSubscriptionChange> + Send>>;

/// Removable multiplexing is important here: `SelectAll` cannot prune a
/// removed device until every queued property has drained. This collection
/// invalidates the generation first and drops the receiver immediately, so a
/// later object at the same address cannot receive a stale property.
struct DeviceSubscriptions {
    generations: DeviceSubscriptionGenerations,
    streams: StreamMap<Address, DeviceChangeStream>,
}

impl DeviceSubscriptions {
    fn new(limit: usize) -> Self {
        Self {
            generations: DeviceSubscriptionGenerations::new(limit),
            streams: StreamMap::with_capacity(limit),
        }
    }

    async fn subscribe(
        &mut self,
        adapter: &Adapter,
        address: Address,
    ) -> Result<DeviceSubscriptionOutcome> {
        let generation = match self.generations.begin(address) {
            DeviceSubscriptionAdmission::AlreadySubscribed => {
                return Ok(DeviceSubscriptionOutcome::AlreadySubscribed);
            }
            DeviceSubscriptionAdmission::AtCapacity => {
                return Ok(DeviceSubscriptionOutcome::AtCapacity);
            }
            DeviceSubscriptionAdmission::Started(generation) => generation,
        };

        let result = async {
            let device = adapter
                .device(address)
                .with_context(|| format!("opening discovered Bluetooth device {address}"))?;
            let events = device
                .events()
                .await
                .with_context(|| format!("subscribing to Bluetooth device {address}"))?;
            let event_generation = generation.clone();
            let closed_generation = generation.clone();
            let changes = events
                .map(move |event| DeviceSubscriptionChange::Event(event_generation.clone(), event))
                .chain(futures::stream::once(async move {
                    DeviceSubscriptionChange::Closed(closed_generation)
                }));
            Ok::<DeviceChangeStream, anyhow::Error>(Box::pin(changes))
        }
        .await;

        let stream = match result {
            Ok(stream) => stream,
            Err(error) => {
                self.generations.remove_if_current(address, &generation);
                return Err(error);
            }
        };
        let replaced = self.streams.insert(address, stream);
        debug_assert!(replaced.is_none());
        debug_assert_eq!(self.streams.len(), self.generations.current.len());
        Ok(DeviceSubscriptionOutcome::Subscribed)
    }

    fn remove(&mut self, address: Address) {
        // Fence first. Even if dropping a receiver wakes another task, no
        // queued value from it remains authorized after this point.
        self.generations.remove(address);
        self.streams.remove(&address);
    }

    fn close_if_current(&mut self, address: Address, generation: &DeviceSubscriptionGeneration) {
        if self.generations.remove_if_current(address, generation) {
            self.streams.remove(&address);
        }
    }

    fn invalidate_all(&mut self) {
        self.generations.invalidate_all();
        self.streams.clear();
    }
}

#[derive(Default)]
struct DeviceSubscriptionRotation {
    resume_after: Option<Address>,
}

struct DeviceSubscriptionWindow {
    addresses: Vec<Address>,
    overflowed: bool,
}

impl DeviceSubscriptionRotation {
    /// Select consecutive circular windows from a stable address ordering.
    /// For any finite known-device set, every address is selected within
    /// `ceil(addresses / limit)` rotations even when the set exceeds the hard
    /// live-subscription limit.
    fn next_window(&mut self, known: &[Address], limit: usize) -> DeviceSubscriptionWindow {
        let mut ordered = known.to_vec();
        ordered.sort_unstable();
        ordered.dedup();
        let overflowed = ordered.len() > limit;
        if ordered.is_empty() || limit == 0 {
            return DeviceSubscriptionWindow {
                addresses: Vec::new(),
                overflowed,
            };
        }

        let start = self
            .resume_after
            .and_then(|cursor| ordered.iter().position(|address| *address > cursor))
            .unwrap_or(0);
        let addresses = (0..ordered.len().min(limit))
            .map(|offset| ordered[(start + offset) % ordered.len()])
            .collect::<Vec<_>>();
        self.resume_after = addresses.last().copied();
        DeviceSubscriptionWindow {
            addresses,
            overflowed,
        }
    }
}

struct RuntimeCore {
    runtime: Runtime,
    context: tokio::sync::Mutex<Option<BluezContext>>,
    observations: RwLock<HashMap<Address, ScanObservation>>,
    observation_tx: broadcast::Sender<ScanObservation>,
    scan_started: AtomicBool,
    scan_handle: Mutex<Option<JoinHandle<()>>>,
    scan_shutdown_tx: watch::Sender<bool>,
    scanner_health_tx: watch::Sender<ScannerHealth>,
    external_adapter_reserved: AtomicBool,
    reservation_tx: watch::Sender<bool>,
    scan_control: Mutex<ScanControlState>,
    scan_stopped: Condvar,
    quiescing: AtomicBool,
    operation_barrier: RwLock<()>,
    adapter_operation: AdapterOperationGate,
    // Driver coordinators live for the process lifetime. The driver set is
    // closed and tiny, while command-uncertainty fences must survive a
    // controller handle being dropped and recreated around the same BlueZ
    // session.
    client_operations: Mutex<HashMap<BluezDriverId, Arc<ClientOperationCoordinator>>>,
    external_operation_permit: Mutex<Option<OwnedSemaphorePermit>>,
    next_sequence: AtomicU64,
    supervisor_restarts: AtomicU64,
    subscriber_lagged_observations: Arc<AtomicU64>,
    last_failure_stage: Mutex<Option<&'static str>>,
    last_local_association: Mutex<Option<LocalBleAssociationDiagnostics>>,
}

impl RuntimeCore {
    fn build() -> Result<Arc<Self>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("rhythm-ble")
            .enable_all()
            .build()
            .context("building shared Bluetooth runtime")?;
        let (observation_tx, _) = broadcast::channel(SCAN_CHANNEL_CAPACITY);
        let (scan_shutdown_tx, _) = watch::channel(false);
        let (scanner_health_tx, _) = watch::channel(ScannerHealth::Stopped);
        let (reservation_tx, _) = watch::channel(false);
        Ok(Arc::new(Self {
            runtime,
            context: tokio::sync::Mutex::new(None),
            observations: RwLock::new(HashMap::new()),
            observation_tx,
            scan_started: AtomicBool::new(false),
            scan_handle: Mutex::new(None),
            scan_shutdown_tx,
            scanner_health_tx,
            external_adapter_reserved: AtomicBool::new(false),
            reservation_tx,
            scan_control: Mutex::new(ScanControlState::default()),
            scan_stopped: Condvar::new(),
            quiescing: AtomicBool::new(false),
            operation_barrier: RwLock::new(()),
            adapter_operation: AdapterOperationGate::new(ADAPTER_OPERATION_PERMITS),
            client_operations: Mutex::new(HashMap::new()),
            external_operation_permit: Mutex::new(None),
            next_sequence: AtomicU64::new(1),
            supervisor_restarts: AtomicU64::new(0),
            subscriber_lagged_observations: Arc::new(AtomicU64::new(0)),
            last_failure_stage: Mutex::new(None),
            last_local_association: Mutex::new(None),
        }))
    }

    fn client_operations(&self, driver: BluezDriverId) -> Result<Arc<ClientOperationCoordinator>> {
        let mut clients = self
            .client_operations
            .lock()
            .map_err(|_| anyhow::anyhow!("shared Bluetooth client coordinator map poisoned"))?;
        if let Some(coordinator) = clients.get(&driver) {
            return Ok(Arc::clone(coordinator));
        }
        let coordinator = Arc::new(ClientOperationCoordinator::new());
        clients.insert(driver, Arc::clone(&coordinator));
        Ok(coordinator)
    }

    fn admit_operation_barrier(&self, deadline: Instant) -> Result<RwLockReadGuard<'_, ()>> {
        loop {
            match self.operation_barrier.try_read() {
                Ok(guard) => return Ok(guard),
                Err(TryLockError::Poisoned(_)) => {
                    anyhow::bail!("shared Bluetooth operation barrier poisoned")
                }
                Err(TryLockError::WouldBlock) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        anyhow::bail!("timed out waiting for shared Bluetooth runtime admission");
                    }
                    std::thread::sleep(Duration::from_millis(1).min(remaining));
                }
            }
        }
    }

    async fn session_adapter(&self) -> Result<(Session, Adapter)> {
        if self.quiescing.load(Ordering::Acquire) {
            anyhow::bail!("shared Bluetooth runtime is quiesced for appliance shutdown");
        }
        let mut context = self.context.lock().await;
        if let Some(cached) = context.as_ref().cloned() {
            // A bluer Session does not reconnect after bluetoothd disappears.
            // Validate the cached proxy on every admitted operation so a
            // Hue-only appliance (which may never start discovery) recovers on
            // its next observer poll instead of retaining a dead proxy forever.
            match cached.adapter.is_powered().await {
                Ok(true) => return Ok((cached.session, cached.adapter)),
                Ok(false) => {
                    if cached.adapter.set_powered(true).await.is_ok() {
                        return Ok((cached.session, cached.adapter));
                    }
                }
                Err(_) => {}
            }
            tracing::debug!(
                target: "ble",
                "Recreating shared BlueZ context after adapter health validation failed"
            );
            *context = None;
        }
        let session = Session::new()
            .await
            .context("opening shared BlueZ session")?;
        let adapter = session
            .default_adapter()
            .await
            .context("finding shared Bluetooth adapter")?;
        adapter
            .set_powered(true)
            .await
            .context("powering shared Bluetooth adapter")?;
        *context = Some(BluezContext {
            session: session.clone(),
            adapter: adapter.clone(),
        });
        Ok((session, adapter))
    }

    async fn invalidate_current_context(&self) {
        *self.context.lock().await = None;
    }

    fn begin_scan(&self) -> Result<bool> {
        let mut state = self
            .scan_control
            .lock()
            .map_err(|_| anyhow::anyhow!("shared Bluetooth scan state poisoned"))?;
        let began = state.begin(self.external_adapter_reserved.load(Ordering::Acquire));
        if began {
            self.scanner_health_tx.send_replace(ScannerHealth::Starting);
        }
        Ok(began)
    }

    fn mark_scan_active(&self) {
        if let Ok(mut state) = self.scan_control.lock() {
            state.mark_active();
        }
        self.scanner_health_tx.send_replace(ScannerHealth::Active);
    }

    fn finish_scan(&self, stop_acknowledged: bool) {
        if let Ok(mut state) = self.scan_control.lock() {
            state.finish(stop_acknowledged);
            self.scan_stopped.notify_all();
        }
        let health = if self.quiescing.load(Ordering::Acquire) {
            ScannerHealth::Stopped
        } else if self.external_adapter_reserved.load(Ordering::Acquire) {
            ScannerHealth::PausedForExternalOwner
        } else {
            ScannerHealth::Recovering
        };
        self.scanner_health_tx.send_replace(health);
    }

    fn ensure_scanner_started(self: &Arc<Self>) -> Result<()> {
        if self.quiescing.load(Ordering::Acquire) {
            anyhow::bail!("shared Bluetooth runtime is quiesced for appliance shutdown");
        }
        if self
            .scan_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(());
        }
        self.scanner_health_tx.send_replace(ScannerHealth::Starting);
        let core = self.clone();
        let handle = self.runtime.spawn(async move {
            core.scan_supervisor().await;
        });
        *self
            .scan_handle
            .lock()
            .map_err(|_| anyhow::anyhow!("shared Bluetooth scanner lock poisoned"))? = Some(handle);
        Ok(())
    }

    async fn scan_supervisor(self: Arc<Self>) {
        let mut backoff = SCAN_RESTART_MIN_BACKOFF;
        let mut reservation_rx = self.reservation_tx.subscribe();
        let mut shutdown_rx = self.scan_shutdown_tx.subscribe();
        let mut subscription_rotation = DeviceSubscriptionRotation::default();
        'supervisor: while !self.quiescing.load(Ordering::Acquire) {
            while self.external_adapter_reserved.load(Ordering::Acquire)
                && !self.quiescing.load(Ordering::Acquire)
            {
                self.scanner_health_tx
                    .send_replace(ScannerHealth::PausedForExternalOwner);
                tokio::select! {
                    changed = reservation_rx.changed() => {
                        if changed.is_err() { break 'supervisor; }
                    }
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() { break 'supervisor; }
                    }
                }
            }
            let result = self
                .scan_once(
                    &mut reservation_rx,
                    &mut shutdown_rx,
                    &mut subscription_rotation,
                )
                .await;
            if self.quiescing.load(Ordering::Acquire) {
                break;
            }
            if self.external_adapter_reserved.load(Ordering::Acquire) {
                backoff = SCAN_RESTART_MIN_BACKOFF;
                continue;
            }
            if result.is_err() {
                // Any scan-stage error may represent adapter or D-Bus loss.
                // Never retry indefinitely through a cached dead proxy.
                self.invalidate_current_context().await;
            }
            self.scanner_health_tx
                .send_replace(ScannerHealth::Recovering);
            self.supervisor_restarts.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut stage) = self.last_failure_stage.lock() {
                *stage = Some(if result.is_err() {
                    "scan"
                } else {
                    "stream_ended"
                });
            }
            if let Err(error) = result {
                tracing::warn!(
                    target: "ble",
                    "Shared Bluetooth scanner restarting after bounded scan failure at scan stage"
                );
                let _ = error;
            }
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                changed = reservation_rx.changed() => {
                    if changed.is_err() { break; }
                }
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() { break; }
                }
            }
            backoff = (backoff * 2).min(SCAN_RESTART_MAX_BACKOFF);
        }
        self.scanner_health_tx.send_replace(ScannerHealth::Stopped);
    }

    async fn scan_once(
        &self,
        reservation_rx: &mut watch::Receiver<bool>,
        shutdown_rx: &mut watch::Receiver<bool>,
        subscription_rotation: &mut DeviceSubscriptionRotation,
    ) -> Result<()> {
        if !self.begin_scan()? {
            return Ok(());
        }
        let (_session, adapter) = match self.session_adapter().await {
            Ok(context) => context,
            Err(error) => {
                self.finish_scan(true);
                return Err(error);
            }
        };
        if self.scan_interrupted() {
            self.finish_scan(true);
            return Ok(());
        }
        if let Err(error) = adapter
            .set_discovery_filter(broad_discovery_filter())
            .await
            .context("setting shared broad LE discovery filter")
        {
            self.finish_scan(true);
            return Err(error);
        }
        if self.scan_interrupted() {
            self.finish_scan(true);
            return Ok(());
        }
        let known_addresses = match adapter.device_addresses().await {
            Ok(addresses) => addresses,
            Err(error) => {
                self.finish_scan(true);
                return Err(error).context("snapshotting known Bluetooth devices before discovery");
            }
        };
        let initial_subscription_window =
            subscription_rotation.next_window(&known_addresses, DEVICE_SUBSCRIPTION_LIMIT);
        let loop_result = {
            let events = match adapter
                .discover_devices()
                .await
                .context("starting shared LE discovery")
            {
                Ok(events) => events,
                Err(error) => {
                    self.finish_scan(true);
                    return Err(error);
                }
            };
            self.mark_scan_active();
            let mut initial_known = InitialKnownDeviceFilter::new(known_addresses);
            let selected_initial_addresses = initial_subscription_window
                .addresses
                .into_iter()
                .collect::<HashSet<_>>();
            let mut subscription_rotation_needed = initial_subscription_window.overflowed;
            let mut subscriptions = DeviceSubscriptions::new(DEVICE_SUBSCRIPTION_LIMIT);
            let mut rotation_interval =
                tokio::time::interval(DEVICE_SUBSCRIPTION_ROTATION_INTERVAL);
            rotation_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // Tokio intervals fire immediately once. Consume that tick so an
            // oversized known-device set gets a useful observation window
            // before its subscriptions rotate.
            rotation_interval.tick().await;

            if self.scan_interrupted() {
                Ok(())
            } else {
                tokio::pin!(events);
                'scan: loop {
                    tokio::select! {
                        changed = shutdown_rx.changed() => {
                            if changed.is_err() || *shutdown_rx.borrow() {
                                break Ok(());
                            }
                        }
                        changed = reservation_rx.changed() => {
                            if changed.is_err() || *reservation_rx.borrow() {
                                break Ok(());
                            }
                        }
                        event = events.next() => {
                            match event {
                                Some(AdapterEvent::DeviceAdded(address)) => {
                                    let is_fresh = initial_known.accepts(address);
                                    if is_fresh || selected_initial_addresses.contains(&address) {
                                        match subscriptions.subscribe(&adapter, address).await {
                                            Ok(DeviceSubscriptionOutcome::AtCapacity) => {
                                                subscription_rotation_needed = true;
                                            }
                                            Ok(DeviceSubscriptionOutcome::AlreadySubscribed | DeviceSubscriptionOutcome::Subscribed) => {}
                                            Err(error) => break Err(error),
                                        }
                                    }
                                    if !is_fresh {
                                        continue;
                                    }
                                    if let Some(observation) = self.snapshot(&adapter, address).await {
                                        self.cache_observation(observation.clone());
                                        let _ = self.observation_tx.send(observation);
                                    }
                                }
                                Some(AdapterEvent::DeviceRemoved(address)) => {
                                    initial_known.removed(address);
                                    subscriptions.remove(address);
                                    if let Ok(mut observations) = self.observations.write() {
                                        observations.remove(&address);
                                    }
                                }
                                Some(AdapterEvent::PropertyChanged(_)) => {}
                                None => break Err(anyhow::anyhow!("shared BlueZ discovery stream ended")),
                            }
                        }
                        change = subscriptions.streams.next(), if !subscriptions.streams.is_empty() => {
                            let Some((address, change)) = change else {
                                continue;
                            };
                            let (generation, event) = match change {
                                DeviceSubscriptionChange::Event(generation, event) => (generation, event),
                                DeviceSubscriptionChange::Closed(generation) => {
                                    subscriptions.close_if_current(address, &generation);
                                    continue;
                                }
                            };
                            if !subscriptions.generations.is_current(address, &generation) {
                                continue;
                            }
                            let DeviceEvent::PropertyChanged(property) = event;
                            if !is_fresh_advertisement_property(&property) {
                                continue;
                            }
                            if let Some(observation) = self.snapshot(&adapter, address).await {
                                self.cache_observation(observation.clone());
                                let _ = self.observation_tx.send(observation);
                            }
                        }
                        _ = rotation_interval.tick(), if subscription_rotation_needed => {
                            let known_addresses = match adapter.device_addresses().await {
                                Ok(addresses) => addresses,
                                Err(error) => break Err(error).context(
                                    "snapshotting known Bluetooth devices for subscription rotation"
                                ),
                            };
                            let window = subscription_rotation
                                .next_window(&known_addresses, DEVICE_SUBSCRIPTION_LIMIT);

                            // Invalidate before dropping receivers. Any
                            // already-queued property is unauthorized before
                            // an address can acquire its next generation.
                            subscriptions.invalidate_all();
                            for address in window.addresses {
                                match subscriptions.subscribe(&adapter, address).await {
                                    Ok(DeviceSubscriptionOutcome::AlreadySubscribed | DeviceSubscriptionOutcome::Subscribed) => {}
                                    Ok(DeviceSubscriptionOutcome::AtCapacity) => {
                                        break 'scan Err(anyhow::anyhow!(
                                            "bounded Bluetooth subscription window exceeded its limit"
                                        ));
                                    }
                                    Err(error) => break 'scan Err(error),
                                }
                            }
                            subscription_rotation_needed = window.overflowed;
                        }
                    }
                }
            }
        };

        // `discover_devices` forwards through an internal bluer
        // task which owns the actual discovery token. Dropping our receiver
        // wakes that task asynchronously, so a first SetDiscoveryFilter may
        // legitimately observe DiscoveryActive before StopDiscovery starts.
        // Retry that one state until bluer has dropped the token. Bluer's
        // internal teardown deliberately discards the StopDiscovery D-Bus
        // result, so changing the filter alone is not an acknowledgement:
        // explicitly require BlueZ's Discovering property to become false.
        let stop_acknowledged = wait_for_discovery_stop(
            RESERVATION_ACK_TIMEOUT.saturating_sub(DISCOVERY_STOP_SAFETY_MARGIN),
            DISCOVERY_STOP_RETRY_INTERVAL,
            || async {
                match adapter.set_discovery_filter(broad_discovery_filter()).await {
                    Ok(()) => match adapter.is_discovering().await {
                        Ok(false) => DiscoveryStopAttempt::Complete,
                        Ok(true) => DiscoveryStopAttempt::StillActive,
                        Err(_) => DiscoveryStopAttempt::Failed,
                    },
                    Err(error) if error.kind == ErrorKind::DiscoveryActive => {
                        DiscoveryStopAttempt::StillActive
                    }
                    Err(_) => DiscoveryStopAttempt::Failed,
                }
            },
        )
        .await;
        self.finish_scan(stop_acknowledged);
        if !stop_acknowledged {
            anyhow::bail!("shared Bluetooth discovery stop was not acknowledged");
        }
        loop_result
    }

    fn scan_interrupted(&self) -> bool {
        self.quiescing.load(Ordering::Acquire)
            || self.external_adapter_reserved.load(Ordering::Acquire)
    }

    fn cache_observation(&self, observation: ScanObservation) {
        if let Ok(mut observations) = self.observations.write() {
            prune_observations(&mut observations, observation.observed_at);
            observations.insert(observation.address, observation);
            if observations.len() > OBSERVATION_CACHE_LIMIT {
                let mut oldest = observations
                    .iter()
                    .map(|(address, observation)| (*address, observation.observed_at))
                    .collect::<Vec<_>>();
                oldest.sort_by_key(|(_, observed_at)| *observed_at);
                for (address, _) in oldest
                    .into_iter()
                    .take(observations.len() - OBSERVATION_CACHE_LIMIT)
                {
                    observations.remove(&address);
                }
            }
        }
    }

    async fn snapshot(&self, adapter: &Adapter, address: Address) -> Option<ScanObservation> {
        let device = adapter.device(address).ok()?;
        let rssi = device.rssi().await.ok().flatten();
        let service_uuids = device.uuids().await.ok().flatten().unwrap_or_default();
        let service_data = device
            .service_data()
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        let manufacturer_data = device
            .manufacturer_data()
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        Some(ScanObservation {
            address,
            rssi,
            service_uuids,
            service_data,
            manufacturer_data,
            sequence: self.next_sequence.fetch_add(1, Ordering::Relaxed),
            observed_at: Instant::now(),
        })
    }

    fn subscribe(self: &Arc<Self>) -> Result<ScanSubscription> {
        // Subscriptions are deliberately fresh-only. Cached observations are
        // diagnostics, not proof that a pairing candidate is still nearby.
        let receiver = self.observation_tx.subscribe();
        let mut observations = self
            .observations
            .write()
            .map_err(|_| anyhow::anyhow!("shared Bluetooth observation cache poisoned"))?;
        prune_observations(&mut observations, Instant::now());
        drop(observations);
        self.ensure_scanner_started()?;
        Ok(ScanSubscription {
            receiver,
            lagged_observations: self.subscriber_lagged_observations.clone(),
        })
    }

    fn scanner_health(&self) -> watch::Receiver<ScannerHealth> {
        self.scanner_health_tx.subscribe()
    }

    fn diagnostics(&self) -> SharedBluezDiagnostics {
        SharedBluezDiagnostics {
            quiescing: self.quiescing.load(Ordering::Acquire),
            external_adapter_reserved: self.external_adapter_reserved.load(Ordering::Acquire),
            scanner_started: self.scan_started.load(Ordering::Acquire),
            supervisor_restarts: self.supervisor_restarts.load(Ordering::Relaxed),
            subscriber_lagged_observations: self
                .subscriber_lagged_observations
                .load(Ordering::Relaxed),
            cached_observations: self
                .observations
                .read()
                .map(|observations| observations.len())
                .unwrap_or_default(),
            scanner_health: self.scanner_health_tx.borrow().as_str(),
            last_failure_stage: self.last_failure_stage.lock().ok().and_then(|stage| *stage),
            last_local_association: self
                .last_local_association
                .lock()
                .ok()
                .and_then(|diagnostics| diagnostics.clone()),
        }
    }

    fn set_external_reservation(&self, reserved: bool) -> Result<()> {
        if !reserved {
            self.clear_external_reservation();
            return Ok(());
        }

        if self.external_adapter_reserved.swap(true, Ordering::SeqCst) {
            return self
                .external_operation_permit
                .lock()
                .map_err(|_| anyhow::anyhow!("shared Bluetooth reservation lock poisoned"))?
                .as_ref()
                .map(|_| ())
                .ok_or_else(|| anyhow::anyhow!("Bluetooth adapter reservation is still pending"));
        }
        self.reservation_tx.send_replace(true);

        let state = self
            .scan_control
            .lock()
            .map_err(|_| anyhow::anyhow!("shared Bluetooth scan state poisoned"))?;
        let (state, timeout) = self
            .scan_stopped
            .wait_timeout_while(state, RESERVATION_ACK_TIMEOUT, |state| state.busy())
            .map_err(|_| anyhow::anyhow!("shared Bluetooth scan state poisoned"))?;
        if timeout.timed_out() && state.busy() {
            drop(state);
            self.clear_external_reservation();
            anyhow::bail!("timed out reserving the Bluetooth adapter");
        }
        if state.stop_failed {
            drop(state);
            self.clear_external_reservation();
            anyhow::bail!("Bluetooth discovery did not stop cleanly");
        }
        drop(state);

        let permit = match self.runtime.block_on(async {
            tokio::time::timeout(
                RESERVATION_ACK_TIMEOUT,
                self.adapter_operation.acquire_exclusive(),
            )
            .await
            .context("timed out draining Bluetooth client operations")?
            .map_err(|_| anyhow::anyhow!("shared Bluetooth operation gate closed"))
        }) {
            Ok(permit) => permit,
            Err(error) => {
                self.clear_external_reservation();
                return Err(error);
            }
        };
        *self
            .external_operation_permit
            .lock()
            .map_err(|_| anyhow::anyhow!("shared Bluetooth reservation lock poisoned"))? =
            Some(permit);
        Ok(())
    }

    fn clear_external_reservation(&self) {
        if let Ok(mut permit) = self.external_operation_permit.lock() {
            permit.take();
        }
        self.external_adapter_reserved
            .store(false, Ordering::SeqCst);
        self.reservation_tx.send_replace(false);
    }

    fn quiesce_and_join(&self) -> Result<()> {
        self.quiescing.store(true, Ordering::SeqCst);
        self.scanner_health_tx.send_replace(ScannerHealth::Stopped);
        self.scan_shutdown_tx.send_replace(true);
        let _exclusive = self
            .operation_barrier
            .write()
            .map_err(|_| anyhow::anyhow!("shared Bluetooth operation barrier poisoned"))?;
        let handle = self
            .scan_handle
            .lock()
            .map_err(|_| anyhow::anyhow!("shared Bluetooth scanner lock poisoned"))?
            .take();
        if let Some(handle) = handle {
            self.runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(5), handle)
                    .await
                    .context("timed out joining shared Bluetooth scanner")?
                    .context("shared Bluetooth scanner task failed")
            })?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiscoveryStopAttempt {
    Complete,
    StillActive,
    Failed,
}

async fn wait_for_discovery_stop<F, Fut>(
    timeout: Duration,
    retry_delay: Duration,
    mut attempt: F,
) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = DiscoveryStopAttempt>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        match tokio::time::timeout(remaining, attempt()).await {
            Ok(DiscoveryStopAttempt::Complete) => return true,
            Ok(DiscoveryStopAttempt::StillActive) => {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    return false;
                }
                tokio::time::sleep(retry_delay.min(remaining)).await;
            }
            Ok(DiscoveryStopAttempt::Failed) | Err(_) => return false,
        }
    }
}

fn broad_discovery_filter() -> DiscoveryFilter {
    DiscoveryFilter {
        transport: DiscoveryTransport::Le,
        duplicate_data: true,
        ..Default::default()
    }
}

fn prune_observations(observations: &mut HashMap<Address, ScanObservation>, now: Instant) {
    observations.retain(|_, observation| {
        now.saturating_duration_since(observation.observed_at) <= OBSERVATION_CACHE_TTL
    });
}

static SHARED_RUNTIME: OnceLock<Arc<RuntimeCore>> = OnceLock::new();

fn shared_runtime() -> Result<Arc<RuntimeCore>> {
    if let Some(runtime) = SHARED_RUNTIME.get() {
        return Ok(runtime.clone());
    }
    let candidate = RuntimeCore::build()?;
    let _ = SHARED_RUNTIME.set(candidate);
    Ok(SHARED_RUNTIME
        .get()
        .expect("shared Bluetooth runtime initialized")
        .clone())
}

/// Per-driver handle into the one shared adapter owner.
pub struct BluezClient {
    driver: BluezDriverId,
    core: Arc<RuntimeCore>,
    operations: Arc<ClientOperationCoordinator>,
    quiescing: AtomicBool,
}

impl BluezClient {
    pub fn new(driver: BluezDriverId) -> Result<Self> {
        let core = shared_runtime()?;
        let operations = core.client_operations(driver)?;
        Ok(Self {
            driver,
            core,
            operations,
            quiescing: AtomicBool::new(false),
        })
    }

    /// Replace the bounded local-profile association snapshot used by support
    /// bundles. Only the local-profile driver writes this slot.
    pub(crate) fn record_local_association(&self, diagnostics: LocalBleAssociationDiagnostics) {
        debug_assert_eq!(self.driver, BluezDriverId::LocalProfiles);
        if let Ok(mut latest) = self.core.last_local_association.lock() {
            *latest = Some(diagnostics);
        }
    }

    /// Execute one complete adapter/GATT operation while holding this client's
    /// exclusive scope and the shared external-owner gate. Raw handles are
    /// created inside the admitted scope instead of being handed out by an
    /// independently callable accessor. The closed set of registered drivers
    /// is a trusted boundary: operation closures must not clone a handle into
    /// captured external state. The sealed output contract separately makes
    /// handle escape through the return value impossible.
    pub fn run_adapter_operation<T, F, Fut>(&self, operation: F) -> Result<T>
    where
        T: BluezOperationOutput,
        F: FnOnce(Session, Adapter) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.run_adapter_operation_bounded(
            CLIENT_ADMISSION_TIMEOUT,
            CLIENT_OPERATION_TIMEOUT,
            operation,
        )
    }

    /// Execute a driver-wide operation with explicit admission and execution
    /// budgets. Pairing scans may use a larger execution budget than ordinary
    /// per-device GATT work without making the owner itself unbounded.
    pub fn run_adapter_operation_bounded<T, F, Fut>(
        &self,
        admission_timeout: Duration,
        operation_timeout: Duration,
        operation: F,
    ) -> Result<T>
    where
        T: BluezOperationOutput,
        F: FnOnce(Session, Adapter) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.run_adapter_operation_inner(None, admission_timeout, operation_timeout, operation)
    }

    /// Execute a driver-wide operation under one absolute deadline. Runtime,
    /// client-scope, and adapter admission plus session recovery and the
    /// operation future all consume the same budget.
    pub fn run_adapter_operation_until<T, F, Fut>(
        &self,
        deadline: Instant,
        operation: F,
    ) -> Result<T>
    where
        T: BluezOperationOutput,
        F: FnOnce(Session, Adapter) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.ensure_active()?;
        let _shared = self.core.admit_operation_barrier(deadline)?;
        if deadline <= Instant::now() {
            anyhow::bail!("Bluetooth operation deadline expired during runtime admission");
        }
        self.core.runtime.block_on(async {
            let deadline = tokio::time::Instant::from_std(deadline);
            let _scope = self.operations.admit_global(deadline).await?;
            let _permit = tokio::time::timeout_at(deadline, self.core.adapter_operation.acquire())
                .await
                .context("Bluetooth operation deadline expired during adapter admission")?
                .map_err(|_| anyhow::anyhow!("shared Bluetooth operation gate closed"))?;
            self.ensure_active()?;
            tokio::time::timeout_at(deadline, async {
                let (session, adapter) = self.core.session_adapter().await?;
                operation(session, adapter).await
            })
            .await
            .context("Bluetooth operation deadline expired")?
        })
    }

    /// Execute one operation in a stable device lane. Calls for the same key
    /// are ordered, while a stalled device cannot hold unrelated device lanes.
    /// Both lane/gate admission and the complete BlueZ future are bounded.
    pub fn run_adapter_operation_for<T, F, Fut>(
        &self,
        lane_key: &str,
        admission_timeout: Duration,
        operation_timeout: Duration,
        operation: F,
    ) -> Result<T>
    where
        T: BluezOperationOutput,
        F: FnOnce(Session, Adapter) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.run_adapter_operation_inner(
            Some(lane_key),
            admission_timeout,
            operation_timeout,
            operation,
        )
    }

    /// Execute a set of stable-device operations under one absolute deadline.
    /// The common BlueZ context is validated once, then all device-lane and
    /// adapter-permit admissions are polled together. Results stay keyed and
    /// ordered like the input so one failed bulb cannot erase sibling
    /// outcomes.
    pub fn run_adapter_operations_for_until<I, T, F, Fut>(
        &self,
        deadline: Instant,
        operations: Vec<(String, I)>,
        operation: F,
    ) -> Result<Vec<(String, Result<T>)>>
    where
        T: BluezOperationOutput,
        F: Fn(I, Session, Adapter) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.ensure_active()?;
        let _shared = self.core.admit_operation_barrier(deadline)?;
        if deadline <= Instant::now() {
            anyhow::bail!("Bluetooth device batch deadline expired during runtime admission");
        }
        if operations.is_empty() {
            return Ok(Vec::new());
        }
        self.core.runtime.block_on(async {
            let deadline = tokio::time::Instant::from_std(deadline);
            let operation = &operation;
            run_validated_device_operation_batch(
                &self.operations,
                &self.core.adapter_operation,
                deadline,
                || async {
                    self.ensure_active()?;
                    self.core.session_adapter().await
                },
                operations,
                move |input, (session, adapter)| async move {
                    self.ensure_active()?;
                    operation(input, session, adapter).await
                },
            )
            .await
        })
    }

    /// Fence one stable device after an ambiguous daemon-side operation. The
    /// marker is shared by every same-driver client and must be cleared only
    /// after a later operation proves the device terminal again.
    pub fn mark_device_operation_uncertain(&self, lane_key: &str) -> Result<()> {
        self.operations.mark_device_uncertain(lane_key)
    }

    pub fn clear_device_operation_uncertain(&self, lane_key: &str) -> Result<()> {
        self.operations.clear_device_uncertain(lane_key)
    }

    pub fn device_operation_is_uncertain(&self, lane_key: &str) -> Result<bool> {
        self.operations.device_is_uncertain(lane_key)
    }

    fn run_adapter_operation_inner<T, F, Fut>(
        &self,
        lane_key: Option<&str>,
        admission_timeout: Duration,
        operation_timeout: Duration,
        operation: F,
    ) -> Result<T>
    where
        T: BluezOperationOutput,
        F: FnOnce(Session, Adapter) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.ensure_active()?;
        let admission_deadline = Instant::now() + admission_timeout;
        let _shared = self.core.admit_operation_barrier(admission_deadline)?;
        let admission_remaining = admission_deadline.saturating_duration_since(Instant::now());
        if admission_remaining.is_zero() {
            anyhow::bail!("timed out waiting for shared Bluetooth runtime admission");
        }
        self.core.runtime.block_on(async {
            let admission_deadline = tokio::time::Instant::now() + admission_remaining;
            let _scope = if let Some(lane_key) = lane_key {
                self.operations
                    .admit_device(lane_key, admission_deadline)
                    .await?
            } else {
                self.operations.admit_global(admission_deadline).await?
            };
            let _permit =
                tokio::time::timeout_at(admission_deadline, self.core.adapter_operation.acquire())
                    .await
                    .context("timed out waiting for shared Bluetooth adapter admission")?
                    .map_err(|_| anyhow::anyhow!("shared Bluetooth operation gate closed"))?;
            self.ensure_active()?;
            tokio::time::timeout(operation_timeout, async {
                let (session, adapter) = self.core.session_adapter().await?;
                operation(session, adapter).await
            })
            .await
            .context("Bluetooth device operation timed out")?
        })
    }

    /// Non-blocking admission for opportunistic reads. `None` means another
    /// client operation or external owner currently holds the gate.
    pub fn try_run_adapter_operation<T, F, Fut>(&self, operation: F) -> Result<Option<T>>
    where
        T: BluezOperationOutput,
        F: FnOnce(Session, Adapter) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.try_run_adapter_operation_inner(None, CLIENT_OPERATION_TIMEOUT, operation)
    }

    /// Opportunistically execute one bounded per-device operation. `None`
    /// means its lane, the shared gate, or an external owner is busy.
    pub fn try_run_adapter_operation_for<T, F, Fut>(
        &self,
        lane_key: &str,
        operation_timeout: Duration,
        operation: F,
    ) -> Result<Option<T>>
    where
        T: BluezOperationOutput,
        F: FnOnce(Session, Adapter) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.try_run_adapter_operation_inner(Some(lane_key), operation_timeout, operation)
    }

    fn try_run_adapter_operation_inner<T, F, Fut>(
        &self,
        lane_key: Option<&str>,
        operation_timeout: Duration,
        operation: F,
    ) -> Result<Option<T>>
    where
        T: BluezOperationOutput,
        F: FnOnce(Session, Adapter) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.ensure_not_quiescing()?;
        // An opportunistic observer cannot distinguish an adapter reserved by
        // an out-of-process commissioner from ordinary local contention by
        // probing BlueZ. Report both as busy (`None`); only shutdown is a hard
        // error for this non-blocking path.
        if self.core.external_adapter_reserved.load(Ordering::Acquire) {
            return Ok(None);
        }
        let Ok(_shared) = self.core.operation_barrier.try_read() else {
            return Ok(None);
        };
        let _scope = if let Some(lane_key) = lane_key {
            let Some(scope) = self.operations.try_admit_device(lane_key)? else {
                return Ok(None);
            };
            scope
        } else {
            let Some(scope) = self.operations.try_admit_global()? else {
                return Ok(None);
            };
            scope
        };
        let Ok(_permit) = self.core.adapter_operation.try_acquire() else {
            return Ok(None);
        };
        // Close the race with a reservation that arrived during admission.
        // Drop our permit as busy so the external owner can finish draining
        // without creating a false adapter-health transition.
        self.ensure_not_quiescing()?;
        if self.core.external_adapter_reserved.load(Ordering::Acquire) {
            return Ok(None);
        }
        self.core.runtime.block_on(async {
            tokio::time::timeout(operation_timeout, async {
                let (session, adapter) = self.core.session_adapter().await?;
                operation(session, adapter).await
            })
            .await
            .context("Bluetooth device operation timed out")
            .and_then(|result| result)
            .map(Some)
        })
    }

    /// Run the profile host's broker-only observer. This intentionally ignores
    /// an external reservation because it owns no Adapter or GATT handle; its
    /// observation stream simply pauses until the scanner resumes. Keeping the
    /// method crate-private prevents a driver from using this path to bypass
    /// adapter-operation admission.
    pub(crate) fn run_broker_observer<T>(&self, future: impl Future<Output = T>) -> Result<T> {
        self.ensure_not_quiescing()?;
        Ok(self.core.runtime.block_on(future))
    }

    pub fn subscribe(&self) -> Result<ScanSubscription> {
        self.ensure_active()?;
        self.core.subscribe()
    }

    pub(crate) fn subscribe_broker(
        &self,
    ) -> Result<(ScanSubscription, watch::Receiver<ScannerHealth>)> {
        self.ensure_not_quiescing()?;
        Ok((self.core.subscribe()?, self.core.scanner_health()))
    }

    /// Permanently stop this driver's new work and wait for its synchronous
    /// operation in flight. Other clients (for example Hue during reset
    /// handoff) remain usable.
    pub fn quiesce(&self) -> Result<()> {
        self.quiescing.store(true, Ordering::SeqCst);
        self.core
            .runtime
            .block_on(self.operations.drain(CLIENT_QUIESCE_TIMEOUT))
    }

    pub fn driver(&self) -> BluezDriverId {
        self.driver
    }

    fn ensure_active(&self) -> Result<()> {
        self.ensure_not_quiescing()?;
        if self.core.external_adapter_reserved.load(Ordering::Acquire) {
            anyhow::bail!("Bluetooth adapter is reserved by another commissioner");
        }
        Ok(())
    }

    fn ensure_not_quiescing(&self) -> Result<()> {
        if self.quiescing.load(Ordering::Acquire) {
            anyhow::bail!("Bluetooth client is quiesced for appliance shutdown");
        }
        if self.core.quiescing.load(Ordering::Acquire) {
            anyhow::bail!("shared Bluetooth runtime is quiesced for appliance shutdown");
        }
        Ok(())
    }
}

pub struct ScanSubscription {
    receiver: broadcast::Receiver<ScanObservation>,
    lagged_observations: Arc<AtomicU64>,
}

impl ScanSubscription {
    pub async fn recv(&mut self) -> Result<ScanObservation> {
        match self.receiver.recv().await {
            Ok(observation) => Ok(observation),
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                self.lagged_observations
                    .fetch_add(skipped, Ordering::Relaxed);
                anyhow::bail!(
                    "shared Bluetooth observation subscriber lagged by {skipped} observations"
                )
            }
            Err(broadcast::error::RecvError::Closed) => {
                anyhow::bail!("shared Bluetooth observation stream closed")
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SharedBluezDiagnostics {
    pub quiescing: bool,
    pub external_adapter_reserved: bool,
    pub scanner_started: bool,
    pub supervisor_restarts: u64,
    pub subscriber_lagged_observations: u64,
    pub cached_observations: usize,
    pub scanner_health: &'static str,
    pub last_failure_stage: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_local_association: Option<LocalBleAssociationDiagnostics>,
}

pub fn diagnostics() -> Result<SharedBluezDiagnostics> {
    Ok(shared_runtime()?.diagnostics())
}

fn diagnostics_from_initialized(
    runtime: &OnceLock<Arc<RuntimeCore>>,
) -> Option<SharedBluezDiagnostics> {
    runtime.get().map(|runtime| runtime.diagnostics())
}

/// Return a privacy-bounded runtime snapshot only when Bluetooth has already
/// been used by this process. Debug-bundle collection must not open D-Bus,
/// power an adapter, or otherwise initialize Bluetooth as a side effect.
pub fn diagnostics_if_initialized() -> Option<SharedBluezDiagnostics> {
    diagnostics_from_initialized(&SHARED_RUNTIME)
}

/// Pause or resume the sole discovery stream around an adapter owner that
/// lives outside this process. Local-BLE and Hue pairing consume the broker
/// directly and must not reserve the adapter through this hook.
pub fn set_external_adapter_reserved(reserved: bool) -> Result<()> {
    shared_runtime()?.set_external_reservation(reserved)
}

/// Final global shutdown boundary. Call only after each integration has
/// completed its own device-specific reset handoff.
pub fn quiesce_shared_runtime() -> Result<()> {
    shared_runtime()?.quiesce_and_join()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(sequence: u64, service: Uuid, manufacturer: Option<u16>) -> ScanObservation {
        ScanObservation {
            address: format!("00:00:00:00:00:{sequence:02X}").parse().unwrap(),
            rssi: Some(-40),
            service_uuids: HashSet::from([service]),
            service_data: HashMap::new(),
            manufacturer_data: manufacturer
                .map(|id| HashMap::from([(id, vec![sequence as u8])]))
                .unwrap_or_default(),
            sequence,
            observed_at: Instant::now(),
        }
    }

    fn subscription(sender: &broadcast::Sender<ScanObservation>) -> ScanSubscription {
        ScanSubscription {
            receiver: sender.subscribe(),
            lagged_observations: Arc::new(AtomicU64::new(0)),
        }
    }

    #[tokio::test]
    async fn observation_broker_fans_out_button_and_bulb_frames_without_cross_talk() {
        let (sender, _) = broadcast::channel(8);
        let mut button = subscription(&sender);
        let mut hue = subscription(&sender);
        let orein_service: Uuid = "00001511-0000-1000-8000-00805f9b34fb".parse().unwrap();
        let hue_service: Uuid = "0000fe0f-0000-1000-8000-00805f9b34fb".parse().unwrap();

        sender
            .send(observation(1, orein_service, Some(0x1511)))
            .unwrap();
        sender.send(observation(2, hue_service, None)).unwrap();
        sender
            .send(observation(3, orein_service, Some(0x1511)))
            .unwrap();

        let button_seen = [
            button.recv().await.unwrap(),
            button.recv().await.unwrap(),
            button.recv().await.unwrap(),
        ]
        .into_iter()
        .filter(|item| item.manufacturer_data.contains_key(&0x1511))
        .map(|item| item.sequence)
        .collect::<Vec<_>>();
        let hue_seen = [
            hue.recv().await.unwrap(),
            hue.recv().await.unwrap(),
            hue.recv().await.unwrap(),
        ]
        .into_iter()
        .filter(|item| item.service_uuids.contains(&hue_service))
        .map(|item| item.sequence)
        .collect::<Vec<_>>();

        assert_eq!(button_seen, vec![1, 3]);
        assert_eq!(hue_seen, vec![2]);
        drop(button);
        sender.send(observation(4, hue_service, None)).unwrap();
        assert_eq!(hue.recv().await.unwrap().sequence, 4);
    }

    #[tokio::test]
    async fn stuck_bulb_subscriber_cannot_backpressure_button_observations() {
        let (sender, _) = broadcast::channel(4);
        let _stuck_bulb = sender.subscribe();
        let mut button = subscription(&sender);
        let orein_service: Uuid = "00001511-0000-1000-8000-00805f9b34fb".parse().unwrap();

        for sequence in 1..=24 {
            sender
                .send(observation(sequence, orein_service, Some(0x1511)))
                .unwrap();
            assert_eq!(button.recv().await.unwrap().sequence, sequence);
        }
    }

    #[tokio::test]
    async fn scan_subscriptions_never_replay_pre_subscription_observations() {
        let (sender, _) = broadcast::channel(4);
        let _keep_sender_live = sender.subscribe();
        let service: Uuid = "0000fe0f-0000-1000-8000-00805f9b34fb".parse().unwrap();
        sender.send(observation(1, service, None)).unwrap();
        let mut fresh = subscription(&sender);

        sender.send(observation(2, service, None)).unwrap();

        assert_eq!(fresh.recv().await.unwrap().sequence, 2);
    }

    #[tokio::test]
    async fn lagged_input_subscriptions_fail_visibly_and_count_loss() {
        let (sender, _) = broadcast::channel(2);
        let lagged = Arc::new(AtomicU64::new(0));
        let mut subscription = ScanSubscription {
            receiver: sender.subscribe(),
            lagged_observations: lagged.clone(),
        };
        let service: Uuid = "00001511-0000-1000-8000-00805f9b34fb".parse().unwrap();

        for sequence in 1..=3 {
            sender
                .send(observation(sequence, service, Some(0x1511)))
                .unwrap();
        }

        let error = subscription.recv().await.unwrap_err();
        assert!(error.to_string().contains("lagged by 1 observations"));
        assert_eq!(lagged.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn discovery_stop_barrier_retries_only_the_transient_active_state() {
        let mut attempts = 0;
        let acknowledged = wait_for_discovery_stop(Duration::from_secs(1), Duration::ZERO, || {
            attempts += 1;
            let result = if attempts < 3 {
                DiscoveryStopAttempt::StillActive
            } else {
                DiscoveryStopAttempt::Complete
            };
            async move { result }
        })
        .await;

        assert!(acknowledged);
        assert_eq!(attempts, 3);
        assert!(
            !wait_for_discovery_stop(Duration::from_secs(1), Duration::ZERO, || async {
                DiscoveryStopAttempt::Failed
            },)
            .await
        );
    }

    #[tokio::test]
    async fn external_lease_drains_inflight_bulb_work_and_rejects_new_clients() {
        const PERMITS: u32 = 4;
        let gate = AdapterOperationGate::new(PERMITS);
        let inflight_bulb = gate.acquire().await.unwrap();
        let reserve_gate = gate.clone();
        let reservation =
            tokio::spawn(async move { reserve_gate.acquire_exclusive().await.unwrap() });
        tokio::task::yield_now().await;
        assert!(!reservation.is_finished());

        drop(inflight_bulb);
        let external_lease = reservation.await.unwrap();
        assert!(gate.try_acquire().is_err());
        drop(external_lease);
        assert!(gate.try_acquire().is_ok());
    }

    #[test]
    fn opportunistic_probe_reports_external_reservation_as_busy_but_quiesce_as_error() {
        let core = RuntimeCore::build().unwrap();
        let operations = core.client_operations(BluezDriverId::Hue).unwrap();
        let client = BluezClient {
            driver: BluezDriverId::Hue,
            core: core.clone(),
            operations,
            quiescing: AtomicBool::new(false),
        };
        let probe_called = Arc::new(AtomicBool::new(false));
        core.external_adapter_reserved
            .store(true, Ordering::Release);

        let called = probe_called.clone();
        assert_eq!(
            client
                .try_run_adapter_operation(move |_session, _adapter| async move {
                    called.store(true, Ordering::Release);
                    Ok(true)
                })
                .unwrap(),
            None
        );
        assert!(!probe_called.load(Ordering::Acquire));

        client.quiescing.store(true, Ordering::Release);
        let error = client
            .try_run_adapter_operation(|_session, _adapter| async move { Ok(true) })
            .expect_err("quiesce must remain a hard probe failure");
        assert!(error.to_string().contains("quiesced"));
    }

    #[test]
    fn scan_reservation_state_requires_stop_acknowledgement() {
        let mut state = ScanControlState::default();
        assert!(state.begin(false));
        state.mark_active();
        assert!(state.busy());
        state.finish(true);
        assert!(!state.busy());
        assert!(!state.stop_failed);

        assert!(state.begin(false));
        state.mark_active();
        state.finish(false);
        assert!(state.stop_failed);
        assert!(!state.begin(true));
    }

    #[test]
    fn initial_known_devices_require_a_subsequent_live_update() {
        let known: Address = "02:00:00:00:00:01".parse().unwrap();
        let newly_added: Address = "02:00:00:00:00:02".parse().unwrap();
        let mut filter = InitialKnownDeviceFilter::new([known]);

        assert!(!filter.accepts(known));
        assert!(filter.accepts(known));
        assert!(filter.accepts(newly_added));

        let removed_before_replay: Address = "02:00:00:00:00:03".parse().unwrap();
        let mut filter = InitialKnownDeviceFilter::new([removed_before_replay]);
        filter.removed(removed_before_replay);
        assert!(filter.accepts(removed_before_replay));
    }

    #[test]
    fn only_advertisement_properties_refresh_scan_observations() {
        assert!(!is_fresh_advertisement_property(
            &DeviceProperty::Connected(true)
        ));
        assert!(!is_fresh_advertisement_property(
            &DeviceProperty::ServicesResolved(true)
        ));
        assert!(!is_fresh_advertisement_property(&DeviceProperty::Paired(
            true
        )));

        assert!(is_fresh_advertisement_property(&DeviceProperty::Rssi(-42)));
        assert!(is_fresh_advertisement_property(
            &DeviceProperty::ManufacturerData(HashMap::from([(0x1511, vec![1])]))
        ));
        assert!(is_fresh_advertisement_property(
            &DeviceProperty::ServiceData(HashMap::from([(
                "00001511-0000-1000-8000-00805f9b34fb".parse().unwrap(),
                vec![1],
            )]))
        ));
    }

    #[test]
    fn removed_and_readded_address_rejects_queued_old_subscription_events() {
        let address: Address = "02:00:00:00:00:01".parse().unwrap();
        let mut generations = DeviceSubscriptionGenerations::default();

        let DeviceSubscriptionAdmission::Started(removed_generation) = generations.begin(address)
        else {
            panic!("first subscription must be admitted");
        };
        assert!(generations.is_current(address, &removed_generation));
        assert!(matches!(
            generations.begin(address),
            DeviceSubscriptionAdmission::AlreadySubscribed
        ));

        generations.remove(address);
        assert!(!generations.is_current(address, &removed_generation));

        let DeviceSubscriptionAdmission::Started(readded_generation) = generations.begin(address)
        else {
            panic!("re-added address must receive a new generation");
        };
        assert!(!generations.is_current(address, &removed_generation));
        assert!(generations.is_current(address, &readded_generation));
    }

    #[test]
    fn rotating_privacy_addresses_stay_bounded_and_all_receive_a_window() {
        let known = (1..=5)
            .map(|suffix| {
                format!("02:00:00:00:00:{suffix:02X}")
                    .parse::<Address>()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let mut rotation = DeviceSubscriptionRotation::default();
        let mut selected = HashSet::new();

        for _ in 0..3 {
            let window = rotation.next_window(&known, 2);
            assert!(window.overflowed);
            assert!(window.addresses.len() <= 2);
            selected.extend(window.addresses);
        }

        assert_eq!(selected, known.iter().copied().collect());
        assert_eq!(rotation.next_window(&known, 2).addresses, known[1..=2]);
    }

    #[test]
    fn subscription_prune_fences_queued_events_before_reusing_capacity() {
        let first: Address = "02:00:00:00:00:01".parse().unwrap();
        let second: Address = "02:00:00:00:00:02".parse().unwrap();
        let overflow: Address = "02:00:00:00:00:03".parse().unwrap();
        let mut generations = DeviceSubscriptionGenerations::new(2);

        let DeviceSubscriptionAdmission::Started(old_generation) = generations.begin(first) else {
            panic!("first subscription must be admitted");
        };
        assert!(matches!(
            generations.begin(second),
            DeviceSubscriptionAdmission::Started(_)
        ));
        assert!(matches!(
            generations.begin(overflow),
            DeviceSubscriptionAdmission::AtCapacity
        ));

        generations.invalidate_all();
        assert!(!generations.is_current(first, &old_generation));
        let DeviceSubscriptionAdmission::Started(new_generation) = generations.begin(first) else {
            panic!("pruned capacity must be reusable");
        };
        assert!(!generations.is_current(first, &old_generation));
        assert!(generations.is_current(first, &new_generation));
    }

    #[test]
    fn equal_driver_ids_share_one_coordinator_scope() {
        let core = RuntimeCore::build().unwrap();
        let first = core.client_operations(BluezDriverId::Hue).unwrap();
        let second = core.client_operations(BluezDriverId::Hue).unwrap();
        let local_profiles = core
            .client_operations(BluezDriverId::LocalProfiles)
            .unwrap();

        assert!(Arc::ptr_eq(&first, &second));
        assert!(!Arc::ptr_eq(&first, &local_profiles));
        assert_eq!(BluezDriverId::Hue.as_str(), "hue_ble");
    }

    #[test]
    fn reservation_arriving_during_scan_start_waits_for_lossless_ack() {
        let shared = Arc::new((Mutex::new(ScanControlState::default()), Condvar::new()));
        {
            let mut state = shared.0.lock().unwrap();
            assert!(state.begin(false));
            assert!(state.starting);
        }
        let worker = shared.clone();
        let join = std::thread::spawn(move || {
            let mut state = worker.0.lock().unwrap();
            state.finish(true);
            worker.1.notify_all();
        });

        let state = shared.0.lock().unwrap();
        let (state, timeout) = shared
            .1
            .wait_timeout_while(state, Duration::from_secs(1), |state| state.busy())
            .unwrap();
        assert!(!timeout.timed_out());
        assert!(!state.busy());
        assert!(!state.stop_failed);
        join.join().unwrap();
    }

    #[test]
    fn observation_cache_prunes_expired_privacy_addresses_and_caps_growth() {
        let service: Uuid = "00001511-0000-1000-8000-00805f9b34fb".parse().unwrap();
        let now = Instant::now();
        let mut observations = HashMap::new();
        let mut expired = observation(1, service, None);
        expired.observed_at = now - OBSERVATION_CACHE_TTL - Duration::from_secs(1);
        observations.insert(expired.address, expired);
        let mut current = observation(2, service, None);
        current.observed_at = now;
        observations.insert(current.address, current.clone());

        prune_observations(&mut observations, now);

        assert_eq!(observations.len(), 1);
        assert!(observations.contains_key(&current.address));
        const { assert!(OBSERVATION_CACHE_LIMIT < SCAN_CHANNEL_CAPACITY) };
    }

    #[test]
    fn diagnostic_lookup_does_not_initialize_an_unused_runtime() {
        let runtime = OnceLock::<Arc<RuntimeCore>>::new();

        assert!(diagnostics_from_initialized(&runtime).is_none());
        assert!(runtime.get().is_none());
    }

    #[test]
    fn scanner_health_diagnostics_are_stable_and_sanitized() {
        assert_eq!(ScannerHealth::Starting.as_str(), "starting");
        assert_eq!(ScannerHealth::Active.as_str(), "active");
        assert_eq!(
            ScannerHealth::PausedForExternalOwner.as_str(),
            "paused_for_external_owner"
        );
        assert_eq!(ScannerHealth::Recovering.as_str(), "recovering");
        assert_eq!(ScannerHealth::Stopped.as_str(), "stopped");
    }
}
