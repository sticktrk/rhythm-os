//! One process-wide BlueZ central-role runtime and discovery broker.
//!
//! Integration drivers are clients of this module. They must not open their
//! own D-Bus session, Tokio runtime, or discovery stream.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock, RwLockReadGuard, TryLockError, Weak};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use bluer::{
    Adapter, AdapterEvent, Address, DiscoveryFilter, DiscoveryTransport, ErrorKind, Session,
};
use futures::StreamExt;
use serde::Serialize;
use tokio::runtime::Runtime;
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::coordination::{AdapterOperationGate, ClientOperationCoordinator};

const SCAN_CHANNEL_CAPACITY: usize = 512;
const SCAN_RESTART_MIN_BACKOFF: Duration = Duration::from_millis(250);
const SCAN_RESTART_MAX_BACKOFF: Duration = Duration::from_secs(5);
const OBSERVATION_CACHE_TTL: Duration = Duration::from_secs(120);
const OBSERVATION_CACHE_LIMIT: usize = 256;
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
    client_operations: Mutex<HashMap<&'static str, Weak<ClientOperationCoordinator>>>,
    external_operation_permit: Mutex<Option<OwnedSemaphorePermit>>,
    next_sequence: AtomicU64,
    supervisor_restarts: AtomicU64,
    subscriber_lagged_observations: Arc<AtomicU64>,
    last_failure_stage: Mutex<Option<&'static str>>,
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
        }))
    }

    fn client_operations(&self, name: &'static str) -> Result<Arc<ClientOperationCoordinator>> {
        let mut clients = self
            .client_operations
            .lock()
            .map_err(|_| anyhow::anyhow!("shared Bluetooth client coordinator map poisoned"))?;
        clients.retain(|_, coordinator| coordinator.strong_count() > 0);
        if let Some(coordinator) = clients.get(name).and_then(Weak::upgrade) {
            return Ok(coordinator);
        }
        let coordinator = Arc::new(ClientOperationCoordinator::new());
        clients.insert(name, Arc::downgrade(&coordinator));
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
            let result = self.scan_once(&mut reservation_rx, &mut shutdown_rx).await;
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
        let loop_result = {
            let events = match adapter
                .discover_devices_with_changes()
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

            if self.scan_interrupted() {
                Ok(())
            } else {
                tokio::pin!(events);
                loop {
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
                                    if !initial_known.accepts(address) {
                                        continue;
                                    }
                                    if let Some(observation) = self.snapshot(&adapter, address).await {
                                        self.cache_observation(observation.clone());
                                        let _ = self.observation_tx.send(observation);
                                    }
                                }
                                Some(AdapterEvent::DeviceRemoved(address)) => {
                                    initial_known.removed(address);
                                    if let Ok(mut observations) = self.observations.write() {
                                        observations.remove(&address);
                                    }
                                }
                                Some(AdapterEvent::PropertyChanged(_)) => {}
                                None => break Err(anyhow::anyhow!("shared BlueZ discovery stream ended")),
                            }
                        }
                    }
                }
            }
        };

        // `discover_devices_with_changes` forwards through an internal bluer
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
    name: &'static str,
    core: Arc<RuntimeCore>,
    operations: Arc<ClientOperationCoordinator>,
    quiescing: AtomicBool,
}

impl BluezClient {
    pub fn new(name: &'static str) -> Result<Self> {
        let core = shared_runtime()?;
        let operations = core.client_operations(name)?;
        Ok(Self {
            name,
            core,
            operations,
            quiescing: AtomicBool::new(false),
        })
    }

    /// Execute one complete adapter/GATT operation while holding this client's
    /// exclusive scope and the shared external-owner gate. Raw handles are
    /// created inside the admitted scope instead of being handed out by an
    /// independently callable accessor.
    pub fn run_adapter_operation<T, F, Fut>(&self, operation: F) -> Result<T>
    where
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

    fn run_adapter_operation_inner<T, F, Fut>(
        &self,
        lane_key: Option<&str>,
        admission_timeout: Duration,
        operation_timeout: Duration,
        operation: F,
    ) -> Result<T>
    where
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
        F: FnOnce(Session, Adapter) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.ensure_active()?;
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
        self.ensure_active()?;
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

    pub fn name(&self) -> &'static str {
        self.name
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
        loop {
            match self.receiver.recv().await {
                Ok(observation) => return Ok(observation),
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
        assert!(OBSERVATION_CACHE_LIMIT < SCAN_CHANNEL_CAPACITY);
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
