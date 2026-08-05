//! Linux BlueZ transport for direct Hue BLE bulbs.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use bluer::agent::{Agent, ReqError};
use bluer::gatt::remote::{Characteristic, CharacteristicWriteRequest, Service};
use bluer::gatt::WriteOp;
use bluer::{Adapter, Address, Device, ErrorKind};
use rhythm_ble::bluez::{
    BluezClient, BluezDriverId, BluezOperationDeadlineExceeded, DetachedBluezOutput,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use uuid::Uuid;

use super::connection_pool::{BleConnectionPool, ConnectionLease, ConnectionPoolAdmission};
use super::gatt_runtime::{
    CatalogResolution, GattCatalogCharacteristic, GattCatalogDevice, GattCatalogService,
    HueBleGattRuntime,
};
use super::protocol;
use super::transport::{
    HueBleAdapterAvailability, HueBleCommandNotDispatched, HueBleCommandTimeout, HueBleTransport,
};
use super::types::{
    HueBleCapabilities, HueBleColor, HueBleCommand, HueBleDevice, HueBlePairingOutcome,
    HueBlePairingRequest, HueBleState,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const PAIR_TIMEOUT: Duration = Duration::from_secs(35);
const PASSIVE_READ_TIMEOUT: Duration = Duration::from_secs(2);
// Field measurements put a healthy cold Connect + GATT discovery at 3-7s.
// Bound speculative work below the foreground command budget and retain time
// for an explicit Disconnect if BlueZ leaves an operation ambiguous.
const PREWARM_WORK_TIMEOUT: Duration = Duration::from_secs(10);
const BOND_REMOVAL_TIMEOUT: Duration = Duration::from_secs(3);
const ADAPTER_ADMISSION_TIMEOUT: Duration = Duration::from_secs(5);
// Reserve part of the controller's nine-second physical-command budget for a
// bounded Disconnect. BlueZ documents Disconnect as the cancellation path for
// an outstanding Connect; dropping the Connect future alone can leave the
// daemon reporting OperationAlreadyInProgress on later commands.
const COMMAND_CLEANUP_RESERVE: Duration = Duration::from_secs(2);
const DEVICE_OPERATION_TIMEOUT: Duration = Duration::from_secs(120);
const PAIRING_OPERATION_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Copy)]
struct PairCandidate {
    address: Address,
    paired: bool,
    rssi: i16,
}

struct PooledCommandContext<'a> {
    pool: &'a Arc<BleConnectionPool>,
    client: &'a BluezClient,
    adapter: &'a Adapter,
    gatt: &'a HueBleGattRuntime,
}

/// Hue protocol client of rhythm-ble's process-wide BlueZ owner.
pub struct BluezHueBleTransport {
    client: Arc<BluezClient>,
    gatt: Arc<HueBleGattRuntime>,
    connections: Arc<BleConnectionPool>,
}

static SHARED_CONNECTION_POOL: OnceLock<Arc<BleConnectionPool>> = OnceLock::new();

fn shared_connection_pool() -> Arc<BleConnectionPool> {
    Arc::clone(SHARED_CONNECTION_POOL.get_or_init(BleConnectionPool::new))
}

impl GattCatalogCharacteristic for Characteristic {
    fn catalog_id(&self) -> u16 {
        self.id()
    }

    async fn catalog_uuid(&self) -> Result<String> {
        Ok(self.uuid().await?.to_string())
    }
}

impl GattCatalogService for Service {
    type Characteristic = Characteristic;

    fn catalog_id(&self) -> u16 {
        self.id()
    }

    async fn catalog_uuid(&self) -> Result<String> {
        Ok(self.uuid().await?.to_string())
    }

    async fn catalog_characteristics(&self) -> Result<Vec<Self::Characteristic>> {
        Ok(self.characteristics().await?)
    }
}

impl GattCatalogDevice for Device {
    type Service = Service;

    async fn catalog_services(&self) -> Result<Vec<Self::Service>> {
        Ok(self.services().await?)
    }
}

struct MaterializedLightControlCatalog {
    service_count: usize,
    cache_hit: bool,
    characteristics: HashMap<Uuid, Characteristic>,
}

async fn disconnect_and_confirm<D, DFut, C, CFut, T>(
    deadline: tokio::time::Instant,
    disconnect: D,
    is_connected: C,
    timeout_error: T,
) -> Result<()>
where
    D: FnOnce() -> DFut,
    DFut: Future<Output = Result<()>>,
    C: FnOnce() -> CFut,
    CFut: Future<Output = Result<bool>>,
    T: Fn(&'static str) -> anyhow::Error,
{
    let disconnect_result = match tokio::time::timeout_at(deadline, disconnect()).await {
        Ok(result) => result,
        Err(_) => return Err(timeout_error("during disconnect")),
    };
    let still_connected = match tokio::time::timeout_at(deadline, is_connected()).await {
        Ok(result) => result.context("verifying Hue BLE disconnect state")?,
        Err(_) => return Err(timeout_error("while verifying disconnect")),
    };
    if !still_connected {
        return Ok(());
    }
    match disconnect_result {
        Ok(()) => anyhow::bail!("Hue bulb remains connected after an acknowledged disconnect"),
        Err(disconnect_error) => Err(disconnect_error)
            .context("disconnecting Hue bulb was not acknowledged and it remains connected"),
    }
}

impl BluezHueBleTransport {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: Arc::new(BluezClient::new(BluezDriverId::Hue)?),
            gatt: Arc::new(HueBleGattRuntime::default()),
            connections: shared_connection_pool(),
        })
    }

    fn run_adapter_operation<T, F, Fut>(&self, operation: F) -> Result<T>
    where
        T: Serialize + DeserializeOwned + Send + 'static,
        F: FnOnce(bluer::Session, Adapter) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.client
            .run_adapter_operation(move |session, adapter| async move {
                DetachedBluezOutput::from_value(operation(session, adapter).await?)
            })?
            .into_value()
    }

    fn run_pairing_adapter_operation<T, F, Fut>(&self, operation: F) -> Result<T>
    where
        T: Serialize + DeserializeOwned + Send + 'static,
        F: FnOnce(bluer::Session, Adapter) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.client
            .run_adapter_operation_bounded(
                ADAPTER_ADMISSION_TIMEOUT,
                PAIRING_OPERATION_TIMEOUT,
                move |session, adapter| async move {
                    DetachedBluezOutput::from_value(operation(session, adapter).await?)
                },
            )?
            .into_value()
    }

    fn run_device_adapter_operations_until<I, T, F, Fut>(
        &self,
        deadline: Instant,
        operations: Vec<(String, I)>,
        operation: F,
    ) -> Result<Vec<(String, Result<T>)>>
    where
        T: Serialize + DeserializeOwned + Send + 'static,
        F: Fn(I, bluer::Session, Adapter) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let operation = Arc::new(operation);
        Ok(self
            .client
            .run_adapter_operations_for_until(
                deadline,
                operations,
                move |input, session, adapter| {
                    let operation = Arc::clone(&operation);
                    async move {
                        DetachedBluezOutput::from_value(operation(input, session, adapter).await?)
                    }
                },
            )?
            .into_iter()
            .map(|(device_id, result)| {
                (device_id, result.and_then(DetachedBluezOutput::into_value))
            })
            .collect())
    }

    fn run_device_adapter_operation<T, F, Fut>(&self, device_key: &str, operation: F) -> Result<T>
    where
        T: Serialize + DeserializeOwned + Send + 'static,
        F: FnOnce(bluer::Session, Adapter) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.client
            .run_adapter_operation_for(
                device_key,
                ADAPTER_ADMISSION_TIMEOUT,
                DEVICE_OPERATION_TIMEOUT,
                move |session, adapter| async move {
                    DetachedBluezOutput::from_value(operation(session, adapter).await?)
                },
            )?
            .into_value()
    }

    fn try_run_device_adapter_operation<T, F, Fut>(
        &self,
        device_key: &str,
        operation: F,
    ) -> Result<Option<T>>
    where
        T: Serialize + DeserializeOwned + Send + 'static,
        F: FnOnce(bluer::Session, Adapter) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.client
            .try_run_adapter_operation_for(
                device_key,
                DEVICE_OPERATION_TIMEOUT,
                move |session, adapter| async move {
                    DetachedBluezOutput::from_value(operation(session, adapter).await?)
                },
            )?
            .map(DetachedBluezOutput::into_value)
            .transpose()
    }

    fn run_device_instant_adapter_operation<F, Fut>(
        &self,
        device_key: &str,
        operation: F,
    ) -> Result<Instant>
    where
        F: FnOnce(bluer::Session, Adapter) -> Fut,
        Fut: Future<Output = Result<Instant>>,
    {
        // `Instant` is a core-owned sealed scalar and cannot retain BlueZ
        // handles, so this one monotonic factory-reset timestamp needs no DTO
        // serialization round trip.
        self.client.run_adapter_operation_for(
            device_key,
            ADAPTER_ADMISSION_TIMEOUT,
            DEVICE_OPERATION_TIMEOUT,
            operation,
        )
    }

    fn ensure_persistent_bond_storage() -> Result<()> {
        if std::env::var("RHYTHM_PLATFORM_TYPE").as_deref() != Ok("appliance") {
            return Ok(());
        }
        let mounts = std::fs::read_to_string("/proc/mounts")
            .context("checking persistent Bluetooth bond storage")?;
        let mounted = mounts
            .lines()
            .any(|line| line.split_whitespace().nth(1) == Some("/var/lib/bluetooth"));
        if !mounted || !std::path::Path::new("/data/bluetooth").is_dir() {
            anyhow::bail!(
                "Persistent Bluetooth bond storage is not mounted; restart the appliance before pairing Hue bulbs"
            );
        }
        Ok(())
    }

    async fn discover_candidates(
        client: &BluezClient,
        adapter: &Adapter,
        request: &HueBlePairingRequest,
    ) -> Result<Vec<PairCandidate>> {
        let hue_service = uuid(protocol::HUE_DISCOVERY_SERVICE_UUID);
        let mut observations = client.subscribe()?;

        let overall_deadline =
            tokio::time::Instant::now() + Duration::from_secs(request.scan_timeout_secs);
        let mut candidates: HashMap<Address, (bool, i16)> = HashMap::new();

        loop {
            let remaining = overall_deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let observation = match tokio::time::timeout(remaining, observations.recv()).await {
                Ok(Ok(observation)) => observation,
                Ok(Err(_)) | Err(_) => break,
            };
            let address = observation.address;
            if request
                .candidate_address
                .as_deref()
                .is_some_and(|wanted| !address.to_string().eq_ignore_ascii_case(wanted))
            {
                continue;
            }
            let device = adapter.device(address)?;
            if !observation.service_uuids.contains(&hue_service) {
                continue;
            }
            let Some(rssi) = observation.rssi else {
                // BlueZ also yields cached, out-of-range objects. Only a live
                // advertisement is eligible for association with the label
                // the user just scanned.
                continue;
            };
            let paired = match device.is_paired().await {
                Ok(paired) => paired,
                Err(error) => {
                    tracing::warn!(
                        target: "pair",
                        "Skipping Hue candidate {address}: BlueZ pairing state is unknown: {error}"
                    );
                    continue;
                }
            };
            if !Self::pair_candidate_is_eligible(request, &address.to_string(), paired) {
                continue;
            }
            candidates.insert(address, (paired, rssi));

            if request.candidate_address.is_some() {
                break;
            }
        }
        if candidates.is_empty() {
            anyhow::bail!(
                "No eligible Hue Bluetooth bulb found. Factory-reset the bulb, keep it powered on nearby, and try again"
            );
        }

        let mut candidates = candidates
            .into_iter()
            .map(|(address, (paired, rssi))| PairCandidate {
                address,
                paired,
                rssi,
            })
            .collect::<Vec<_>>();
        // Pair the closest bulbs first. The ordering is otherwise stable so a
        // partial adapter failure has predictable recovery on the next scan.
        candidates.sort_by(|left, right| {
            right
                .rssi
                .cmp(&left.rssi)
                .then_with(|| left.address.to_string().cmp(&right.address.to_string()))
        });
        Ok(candidates)
    }

    fn pair_candidate_is_eligible(
        request: &HueBlePairingRequest,
        address: &str,
        paired: bool,
    ) -> bool {
        let matches = |candidate: &String| address.eq_ignore_ascii_case(candidate);

        // An actively tracked bulb is never a new pairing candidate, even if
        // an inconsistent caller also places it on the re-association list.
        if request.known_addresses.iter().any(matches) {
            return false;
        }
        if !paired {
            return true;
        }
        if request.explicit_reassociation_addresses.iter().any(matches) {
            return true;
        }
        request.allow_paired_orphan_adoption
            && !request.blocked_paired_addresses.iter().any(matches)
    }

    fn stale_bond_replacement_is_allowed(
        request: &HueBlePairingRequest,
        stale_address: &str,
    ) -> bool {
        request
            .explicit_reassociation_addresses
            .iter()
            .any(|address| address.eq_ignore_ascii_case(stale_address))
            && !request
                .known_addresses
                .iter()
                .any(|address| address.eq_ignore_ascii_case(stale_address))
    }

    async fn evict_connection(
        adapter: &Adapter,
        key: &str,
        deadline: tokio::time::Instant,
    ) -> Result<()> {
        let address: Address = key
            .parse()
            .with_context(|| format!("invalid pooled Hue BLE address {key}"))?;
        if !adapter.device_addresses().await?.contains(&address) {
            return Ok(());
        }
        let device = adapter
            .device(address)
            .with_context(|| format!("opening pooled Hue bulb {address} for eviction"))?;
        if !device
            .is_connected()
            .await
            .with_context(|| format!("checking pooled Hue bulb {address} before eviction"))?
        {
            return Ok(());
        }
        disconnect_and_confirm(
            deadline,
            || async {
                device
                    .disconnect()
                    .await
                    .with_context(|| format!("disconnecting pooled Hue bulb {address}"))
            },
            || async {
                device
                    .is_connected()
                    .await
                    .with_context(|| format!("verifying pooled Hue bulb {address} eviction"))
            },
            |stage| anyhow::anyhow!("Hue BLE connection-pool eviction timed out {stage}"),
        )
        .await
    }

    async fn connect_pooled(
        pool: &Arc<BleConnectionPool>,
        client: &BluezClient,
        adapter: &Adapter,
        device: &Device,
        deadline: tokio::time::Instant,
    ) -> Result<(bool, ConnectionLease)> {
        let key = device.address().to_string();
        loop {
            match pool.admit(&key, deadline).await? {
                ConnectionPoolAdmission::Evict(eviction) => {
                    let victim = eviction.key().to_string();
                    client.mark_device_operation_uncertain(&victim)?;
                    Self::evict_connection(adapter, &victim, deadline).await?;
                    client.clear_device_operation_uncertain(&victim)?;
                    eviction.complete();
                }
                ConnectionPoolAdmission::Lease(lease) => {
                    let connected_at_start = device.is_connected().await.unwrap_or(false);
                    if connected_at_start {
                        return Ok((true, lease));
                    }

                    let connection_result: Result<bool> = async {
                        let _connect_permit = pool.admit_connect(deadline).await?;
                        // Another BlueZ owner may have completed a connection
                        // while this request waited for the narrower connect gate.
                        if device.is_connected().await.unwrap_or(false) {
                            return Ok(true);
                        }
                        let connect_deadline =
                            deadline.min(tokio::time::Instant::now() + CONNECT_TIMEOUT);
                        tokio::time::timeout_at(connect_deadline, device.connect())
                            .await
                            .context("timed out connecting to pooled Hue bulb")?
                            .context("connecting to pooled Hue bulb")?;
                        Ok(false)
                    }
                    .await;
                    match connection_result {
                        Ok(connected_at_start) => return Ok((connected_at_start, lease)),
                        Err(error) => {
                            // A failed connection must not occupy one of the warm
                            // slots indefinitely. The device uncertainty fence
                            // separately protects a cancel-unsafe BlueZ Connect.
                            let _ = pool.forget(&key);
                            return Err(error);
                        }
                    }
                }
            }
        }
    }

    async fn cancel_failed_command(device: &Device, deadline: Instant) -> Result<()> {
        if deadline <= Instant::now() {
            anyhow::bail!("Hue BLE command cleanup deadline expired before disconnect");
        }
        let deadline = tokio::time::Instant::from_std(deadline);
        disconnect_and_confirm(
            deadline,
            || async {
                device
                    .disconnect()
                    .await
                    .context("disconnecting Hue bulb after a failed operation")
            },
            || async {
                device
                    .is_connected()
                    .await
                    .context("checking Hue bulb connection after failed-operation cleanup")
            },
            |stage| anyhow::anyhow!("Hue BLE command cleanup deadline expired {stage}"),
        )
        .await
    }

    async fn reconcile_uncertain_command(
        device: &Device,
        deadline: tokio::time::Instant,
    ) -> Result<()> {
        disconnect_and_confirm(
            deadline,
            || async {
                device
                    .disconnect()
                    .await
                    .context("reconciling a previously ambiguous Hue BLE command")
            },
            || async {
                device
                    .is_connected()
                    .await
                    .context("verifying Hue BLE command reconciliation")
            },
            |stage| anyhow::Error::new(HueBleCommandTimeout).context(stage),
        )
        .await
    }

    fn record_fence_address(record: &HueBleDevice) -> Result<String> {
        Ok(record
            .address
            .parse::<Address>()
            .with_context(|| format!("invalid stored Hue BLE address {}", record.address))?
            .to_string())
    }

    fn record_operation_is_uncertain(client: &BluezClient, record: &HueBleDevice) -> Result<bool> {
        let address = Self::record_fence_address(record)?;
        Ok(client.device_operation_is_uncertain(&record.id)?
            || client.device_operation_is_uncertain(&address)?)
    }

    fn mark_record_operation_uncertain(client: &BluezClient, record: &HueBleDevice) -> Result<()> {
        let address = Self::record_fence_address(record)?;
        client
            .mark_device_operation_uncertain(&record.id)
            .with_context(|| format!("fencing stable Hue device {}", record.id))?;
        // BlueZ Connect is addressed by the D-Bus device locator. Retaining an
        // address fence as well as the stable EUI-64 fence prevents pairing or
        // bond-recovery paths (which do not know the EUI-64 yet) from reopening
        // an operation the daemon may still own.
        client
            .mark_device_operation_uncertain(&address)
            .with_context(|| format!("fencing Hue BlueZ device {address}"))
    }

    fn clear_record_operation_uncertain(client: &BluezClient, record: &HueBleDevice) -> Result<()> {
        let address = Self::record_fence_address(record)?;
        client
            .clear_device_operation_uncertain(&record.id)
            .with_context(|| format!("clearing stable Hue device fence {}", record.id))?;
        client
            .clear_device_operation_uncertain(&address)
            .with_context(|| format!("clearing Hue BlueZ device fence {address}"))
    }

    async fn characteristics(device: &Device) -> Result<HashMap<Uuid, Characteristic>> {
        let mut result = HashMap::new();
        for service in device.services().await? {
            for characteristic in service.characteristics().await? {
                result.insert(characteristic.uuid().await?, characteristic);
            }
        }
        Ok(result)
    }

    async fn light_control_characteristics(
        gatt: &HueBleGattRuntime,
        device: &Device,
        device_id: &str,
        resolution: CatalogResolution,
    ) -> Result<Option<MaterializedLightControlCatalog>> {
        let Some(access) = gatt
            .catalog_for_device(device_id, device, resolution)
            .await?
        else {
            return Ok(None);
        };
        let service = device.service(access.catalog.service_id).await?;
        let mut characteristics = HashMap::new();
        for (characteristic_uuid, characteristic_id) in &access.catalog.characteristic_ids {
            characteristics.insert(
                uuid(characteristic_uuid),
                service.characteristic(*characteristic_id).await?,
            );
        }
        Ok(Some(MaterializedLightControlCatalog {
            service_count: access.catalog.service_count,
            cache_hit: access.cache_hit,
            characteristics,
        }))
    }

    async fn read_optional(
        characteristics: &HashMap<Uuid, Characteristic>,
        id: &str,
    ) -> Result<Option<Vec<u8>>> {
        let Some(characteristic) = characteristics.get(&uuid(id)) else {
            return Ok(None);
        };
        Ok(Some(characteristic.read().await.with_context(|| {
            format!("reading Hue characteristic {id}")
        })?))
    }

    async fn write(
        characteristics: &HashMap<Uuid, Characteristic>,
        id: &str,
        payload: &[u8],
    ) -> Result<()> {
        let characteristic = characteristics
            .get(&uuid(id))
            .ok_or_else(|| anyhow::anyhow!("Hue bulb is missing characteristic {id}"))?;
        characteristic
            .write_ext(
                payload,
                &CharacteristicWriteRequest {
                    op_type: WriteOp::Request,
                    ..Default::default()
                },
            )
            .await
            .with_context(|| format!("writing Hue characteristic {id}"))
    }

    async fn apply_light_command(
        context: PooledCommandContext<'_>,
        device: &Device,
        device_id: &str,
        command: &HueBleCommand,
        connection_deadline: tokio::time::Instant,
    ) -> Result<()> {
        let PooledCommandContext {
            pool,
            client,
            adapter,
            gatt,
        } = context;
        let total_started = Instant::now();
        let connect_started = Instant::now();
        let (connected_at_start, _connection_lease) =
            match Self::connect_pooled(pool, client, adapter, device, connection_deadline).await {
                Ok(connection) => connection,
                Err(error) => {
                    tracing::warn!(
                        target: "cmd",
                        event = "hue_ble_device_command",
                        device_id,
                        failed_stage = "connect",
                        connect_ms = connect_started.elapsed().as_millis() as u64,
                        total_ms = total_started.elapsed().as_millis() as u64,
                        outcome = "error",
                        error = %error,
                        "Hue BLE GATT command failed before acknowledgement"
                    );
                    return Err(error.context(HueBleCommandNotDispatched));
                }
            };
        let connect_ms = connect_started.elapsed().as_millis() as u64;
        if !connected_at_start {
            // Bluer documents GATT object IDs as connection-scoped. Never
            // carry an ID catalog across a reconnect without rediscovery.
            gatt.invalidate_catalog(device_id)?;
        }

        let mut resolve_ms = 0u64;
        let mut write_ms = 0u64;
        let mut catalog_cache_hit = false;
        let mut catalog_refreshes = 0u8;
        let mut first_lookup = true;
        loop {
            let lookup_started = Instant::now();
            let catalog = match Self::light_control_characteristics(
                gatt,
                device,
                device_id,
                CatalogResolution::Required,
            )
            .await
            {
                Ok(Some(catalog)) => catalog,
                Ok(None) => unreachable!("required Hue BLE GATT lookup returned no catalog"),
                Err(error) => {
                    resolve_ms =
                        resolve_ms.saturating_add(lookup_started.elapsed().as_millis() as u64);
                    let failed_stage = if catalog_refreshes == 0 {
                        "resolve"
                    } else {
                        "refresh"
                    };
                    tracing::warn!(
                        target: "cmd",
                        event = "hue_ble_device_command",
                        device_id,
                        connected_at_start,
                        failed_stage,
                        catalog_cache_hit,
                        catalog_refreshes,
                        connect_ms,
                        resolve_ms,
                        write_ms,
                        total_ms = total_started.elapsed().as_millis() as u64,
                        outcome = "error",
                        error = %error,
                        "Hue BLE GATT command failed before acknowledgement"
                    );
                    return Err(error.context(HueBleCommandNotDispatched));
                }
            };
            resolve_ms = resolve_ms.saturating_add(lookup_started.elapsed().as_millis() as u64);
            if first_lookup {
                catalog_cache_hit = catalog.cache_hit;
                first_lookup = false;
            }

            let write_started = Instant::now();
            let result = Self::apply_to_characteristics(&catalog.characteristics, command).await;
            write_ms = write_ms.saturating_add(write_started.elapsed().as_millis() as u64);
            if result
                .as_ref()
                .is_err_and(|error| catalog.cache_hit && Self::is_stale_gatt_catalog_error(error))
                && catalog_refreshes == 0
            {
                // Hue commands set absolute values. If BlueZ proves that a
                // cached object path disappeared, refreshing and replaying
                // once is idempotent even when an earlier write in a compound
                // command was already acknowledged.
                gatt.invalidate_catalog(device_id)?;
                catalog_refreshes = 1;
                continue;
            }

            match &result {
                Ok(()) => tracing::info!(
                    target: "cmd",
                    event = "hue_ble_device_command",
                    device_id,
                    connected_at_start,
                    service_count = catalog.service_count,
                    characteristic_count = catalog.characteristics.len(),
                    catalog_cache_hit,
                    catalog_refreshes,
                    connect_ms,
                    resolve_ms,
                    write_ms,
                    total_ms = total_started.elapsed().as_millis() as u64,
                    outcome = "acknowledged",
                    "Hue BLE GATT command acknowledged"
                ),
                Err(error) => tracing::warn!(
                    target: "cmd",
                    event = "hue_ble_device_command",
                    device_id,
                    connected_at_start,
                    service_count = catalog.service_count,
                    characteristic_count = catalog.characteristics.len(),
                    failed_stage = "write",
                    catalog_cache_hit,
                    catalog_refreshes,
                    connect_ms,
                    resolve_ms,
                    write_ms,
                    total_ms = total_started.elapsed().as_millis() as u64,
                    outcome = "error",
                    error = %error,
                    "Hue BLE GATT command failed before acknowledgement"
                ),
            }
            return result;
        }
    }

    fn is_stale_gatt_catalog_error(error: &anyhow::Error) -> bool {
        error.chain().any(|cause| {
            cause.downcast_ref::<bluer::Error>().is_some_and(|error| {
                matches!(
                    error.kind,
                    ErrorKind::DoesNotExist | ErrorKind::NotFound | ErrorKind::ServicesUnresolved
                )
            })
        })
    }

    fn validate_light_characteristic_ids(characteristic_ids: &HashSet<Uuid>) -> Result<()> {
        if !characteristic_ids.contains(&uuid(protocol::POWER_UUID)) {
            anyhow::bail!(
                "Paired Hue device is not a controllable bulb (missing light power control)"
            );
        }
        Ok(())
    }

    async fn inspect_paired_device(
        client: &BluezClient,
        pool: &Arc<BleConnectionPool>,
        adapter: &Adapter,
        device: &Device,
    ) -> Result<HueBleDevice> {
        let address = device.address().to_string();
        if client.device_operation_is_uncertain(&address)? {
            Self::reconcile_uncertain_command(
                device,
                tokio::time::Instant::now() + CONNECT_TIMEOUT,
            )
            .await
            .with_context(|| format!("reconciling ambiguous Hue BlueZ operation for {address}"))?;
            client.clear_device_operation_uncertain(&address)?;
        }

        // Mark before Connect so cancellation of this future leaves a fence
        // visible to every later path, including those that only know the
        // BlueZ address and have not read the stable EUI-64 yet.
        client.mark_device_operation_uncertain(&address)?;
        let inspection = async {
            let (_connected_at_start, _connection_lease) = Self::connect_pooled(
                pool,
                client,
                adapter,
                device,
                tokio::time::Instant::now() + CONNECT_TIMEOUT,
            )
            .await?;
            Self::inspect_connected_device(device).await
        }
        .await;
        match inspection {
            Ok(record) => {
                // The caller may still trust and disconnect this same BlueZ
                // device. Extend the address fence to the stable identity and
                // let that caller clear both only after its final operation is
                // acknowledged.
                Self::mark_record_operation_uncertain(client, &record)?;
                Ok(record)
            }
            Err(inspection_error) => {
                match Self::cancel_failed_command(device, Instant::now() + CONNECT_TIMEOUT).await {
                    Ok(()) => {
                        let _ = pool.forget(&address);
                        client.clear_device_operation_uncertain(&address)?;
                        Err(inspection_error)
                    }
                    Err(cleanup_error) => Err(inspection_error.context(format!(
                        "cancelling failed Hue inspection also failed: {cleanup_error:#}; the BlueZ address remains fenced"
                    ))),
                }
            }
        }
    }

    async fn inspect_connected_device(device: &Device) -> Result<HueBleDevice> {
        let characteristics = Self::characteristics(device).await?;
        Self::validate_light_characteristic_ids(&characteristics.keys().copied().collect())?;
        let eui64_raw = Self::read_optional(&characteristics, protocol::EUI64_UUID)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Paired device does not expose a Hue EUI-64"))?;
        let eui64 = protocol::decode_eui64(&eui64_raw)?;

        let read_text = |id: &'static str| {
            let characteristics = characteristics.clone();
            async move {
                Self::read_optional(&characteristics, id)
                    .await?
                    .map(|bytes| protocol::decode_utf8(&bytes))
                    .transpose()
            }
        };
        let manufacturer = read_text(protocol::MANUFACTURER_NAME_UUID)
            .await?
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Signify Netherlands B.V.".to_string());
        let model = read_text(protocol::MODEL_NUMBER_UUID)
            .await?
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Hue bulb did not report a model number"))?;
        let firmware = read_text(protocol::FIRMWARE_REVISION_UUID)
            .await?
            .unwrap_or_default();
        let name = read_text(protocol::DEVICE_NAME_UUID)
            .await?
            .filter(|value| !value.is_empty())
            .or(device.name().await?)
            .unwrap_or_else(|| "Hue lamp".to_string());

        let color_temperature =
            characteristics.contains_key(&uuid(protocol::COLOR_TEMPERATURE_UUID));
        let xy_color = characteristics.contains_key(&uuid(protocol::XY_COLOR_UUID));
        let combined_control = characteristics.contains_key(&uuid(protocol::COMBINED_CONTROL_UUID));
        let mut capabilities = HueBleCapabilities::from_gatt(
            &manufacturer,
            &model,
            characteristics.contains_key(&uuid(protocol::BRIGHTNESS_UUID)),
            color_temperature,
            xy_color,
            combined_control,
        );
        if color_temperature {
            match Self::read_optional(&characteristics, protocol::LIGHT_CAPABILITIES_UUID).await {
                Ok(Some(raw)) => match protocol::decode_color_temperature_range(&raw) {
                    Ok(Some((minimum, maximum))) => {
                        capabilities.min_mired = Some(minimum);
                        capabilities.max_mired = Some(maximum);
                    }
                    Ok(None) => {}
                    Err(error) => tracing::debug!(
                        target: "pair",
                        "Ignoring malformed Hue CT capability range for {model}: {error:#}"
                    ),
                },
                Ok(None) => {}
                Err(error) => tracing::debug!(
                    target: "pair",
                    "Could not read Hue CT capability range for {model}; using model fallback: {error:#}"
                ),
            }
        }
        let state = Self::read_state_from_characteristics(&characteristics)
            .await
            .ok();

        Ok(HueBleDevice {
            id: HueBleDevice::stable_id(&eui64),
            address: device.address().to_string(),
            address_type: device.address_type().await?.to_string(),
            eui64,
            name,
            manufacturer,
            model,
            firmware,
            capabilities,
            paired_at_epoch_secs: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            last_state: state,
        })
    }

    fn pairing_agent(selected: HashSet<Address>) -> Agent {
        let selected = Arc::new(selected);
        let confirmation_addresses = selected.clone();
        let authorization_addresses = selected.clone();
        let service_addresses = selected;
        Agent {
            request_confirmation: Some(Box::new(move |request| {
                let confirmation_addresses = confirmation_addresses.clone();
                Box::pin(async move {
                    if confirmation_addresses.contains(&request.device) {
                        Ok(())
                    } else {
                        Err(ReqError::Rejected)
                    }
                })
            })),
            request_authorization: Some(Box::new(move |request| {
                let authorization_addresses = authorization_addresses.clone();
                Box::pin(async move {
                    if authorization_addresses.contains(&request.device) {
                        Ok(())
                    } else {
                        Err(ReqError::Rejected)
                    }
                })
            })),
            authorize_service: Some(Box::new(move |request| {
                let service_addresses = service_addresses.clone();
                Box::pin(async move {
                    if service_addresses.contains(&request.device) {
                        Ok(())
                    } else {
                        Err(ReqError::Rejected)
                    }
                })
            })),
            ..Default::default()
        }
    }

    async fn read_state_from_characteristics(
        characteristics: &HashMap<Uuid, Characteristic>,
    ) -> Result<HueBleState> {
        let power = Self::read_optional(characteristics, protocol::POWER_UUID).await?;
        let brightness = Self::read_optional(characteristics, protocol::BRIGHTNESS_UUID).await?;
        let color_temperature =
            Self::read_optional(characteristics, protocol::COLOR_TEMPERATURE_UUID).await?;
        let xy = Self::read_optional(characteristics, protocol::XY_COLOR_UUID).await?;
        protocol::state_from_values(
            power.as_deref(),
            brightness.as_deref(),
            color_temperature.as_deref(),
            xy.as_deref(),
        )
    }

    async fn read_power_state_from_characteristics(
        characteristics: &HashMap<Uuid, Characteristic>,
    ) -> Result<HueBleState> {
        let power = Self::read_optional(characteristics, protocol::POWER_UUID).await?;
        protocol::state_from_values(power.as_deref(), None, None, None)
    }

    async fn read_state_with_catalog(
        gatt: &HueBleGattRuntime,
        device: &Device,
        device_id: &str,
    ) -> Result<HueBleState> {
        let mut refreshed = false;
        loop {
            let catalog = Self::light_control_characteristics(
                gatt,
                device,
                device_id,
                CatalogResolution::Required,
            )
            .await?
            .expect("required Hue BLE GATT lookup returns a catalog");
            let result = Self::read_state_from_characteristics(&catalog.characteristics).await;
            if !refreshed
                && catalog.cache_hit
                && result
                    .as_ref()
                    .is_err_and(Self::is_stale_gatt_catalog_error)
            {
                gatt.invalidate_catalog(device_id)?;
                refreshed = true;
                continue;
            }
            return result;
        }
    }

    async fn apply_to_characteristics(
        characteristics: &HashMap<Uuid, Characteristic>,
        command: &HueBleCommand,
    ) -> Result<()> {
        if characteristics.contains_key(&uuid(protocol::COMBINED_CONTROL_UUID)) {
            let payload = protocol::encode_combined(command)?;
            return Self::write(characteristics, protocol::COMBINED_CONTROL_UUID, &payload).await;
        }

        if command.on == Some(false) {
            return Self::write(
                characteristics,
                protocol::POWER_UUID,
                &protocol::encode_power(false),
            )
            .await;
        }
        if let Some(color) = command.color {
            match color {
                HueBleColor::ColorTemperature { mired } => {
                    Self::write(
                        characteristics,
                        protocol::COLOR_TEMPERATURE_UUID,
                        &protocol::encode_color_temperature(mired),
                    )
                    .await?;
                }
                HueBleColor::Xy { x, y } => {
                    Self::write(
                        characteristics,
                        protocol::XY_COLOR_UUID,
                        &protocol::encode_xy(x, y),
                    )
                    .await?;
                }
            }
        }
        if let Some(brightness) = command.brightness {
            Self::write(
                characteristics,
                protocol::BRIGHTNESS_UUID,
                &protocol::encode_brightness(brightness),
            )
            .await?;
        }
        if let Some(on) = command.on {
            Self::write(
                characteristics,
                protocol::POWER_UUID,
                &protocol::encode_power(on),
            )
            .await?;
        }
        if command.effect.is_some() || command.effect_speed.is_some() {
            anyhow::bail!("This Hue bulb does not expose combined effect control");
        }
        Ok(())
    }

    fn ensure_handoff_is_fresh(handoff_valid_until: Option<Instant>) -> Result<()> {
        if handoff_valid_until.is_some_and(|deadline| Instant::now() >= deadline) {
            anyhow::bail!(
                "Hue Bluetooth replacement-pairing window expired before the local key deletion boundary; the bond was retained so removal can be retried safely"
            );
        }
        Ok(())
    }

    async fn remove_exact_local_bond(
        adapter: &Adapter,
        address: Address,
        handoff_valid_until: Option<Instant>,
    ) -> Result<()> {
        if !adapter.device_addresses().await?.contains(&address) {
            return Ok(());
        }
        let device = adapter
            .device(address)
            .with_context(|| format!("opening Hue bulb {address} before local bond removal"))?;
        if device.is_connected().await.unwrap_or(false) {
            let _ = device.disconnect().await;
        }
        // Adapter/session lookup and disconnect can consume the rest of Hue's
        // short handoff window. Re-check at the last non-destructive line.
        Self::ensure_handoff_is_fresh(handoff_valid_until)?;
        match adapter.remove_device(address).await {
            Ok(()) => {}
            Err(error) if matches!(error.kind, ErrorKind::NotFound | ErrorKind::DoesNotExist) => {
                // Verify below: this is harmless only when the exact local
                // object and key are actually gone.
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("removing Hue bulb {address} bond from BlueZ"));
            }
        }
        let deadline = tokio::time::Instant::now() + BOND_REMOVAL_TIMEOUT;
        loop {
            let address_present = adapter
                .device_addresses()
                .await
                .with_context(|| format!("verifying Hue bulb {address} removal from BlueZ"))?
                .contains(&address);
            let paired =
                if address_present {
                    let device = adapter.device(address).with_context(|| {
                        format!("opening Hue bulb {address} after local removal")
                    })?;
                    Some(device.is_paired().await.with_context(|| {
                        format!("checking Hue bulb {address} bond after removal")
                    })?)
                } else {
                    None
                };
            if local_bond_removal_complete(paired) {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!(
                    "BlueZ still reports Hue bulb {address} as paired after local removal"
                );
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn existing_device(adapter: &Adapter, record: &HueBleDevice) -> Result<Device> {
        let address: Address = record
            .address
            .parse()
            .with_context(|| format!("invalid stored Hue BLE address {}", record.address))?;
        adapter
            .device(address)
            .with_context(|| format!("opening bonded Hue BLE device {address}"))
    }
}

impl HueBleTransport for BluezHueBleTransport {
    fn is_available(&self) -> Result<bool> {
        self.run_adapter_operation(
            |_session, adapter| async move { Ok(adapter.is_powered().await?) },
        )
    }

    fn probe_availability(&self) -> Result<HueBleAdapterAvailability> {
        let observed = self
            .client
            .try_run_adapter_operation(|_session, adapter| async move {
                Ok(adapter.is_powered().await?)
            })?;
        Ok(classify_adapter_probe(observed))
    }

    fn quiesce(&self) -> Result<()> {
        self.client.quiesce()
    }

    fn pair_lights(
        &self,
        request: &HueBlePairingRequest,
        record_bond_intent: &(dyn Fn(&str) -> Result<()> + Send + Sync),
    ) -> Result<HueBlePairingOutcome> {
        Self::ensure_persistent_bond_storage()?;
        let request = request.clone();
        let client = self.client.clone();
        let connections = Arc::clone(&self.connections);
        self.run_pairing_adapter_operation(move |session, adapter| async move {
            for stale_address in &request.replace_stale_bond_addresses {
                if !Self::stale_bond_replacement_is_allowed(&request, stale_address) {
                    anyhow::bail!(
                        "Refusing to replace non-quarantined Hue Bluetooth bond {stale_address}"
                    );
                }
                // Persist the exact address before crossing the deletion
                // boundary. A crash now leaves an explicit retry target, not
                // an adoptable unknown orphan.
                record_bond_intent(stale_address).with_context(|| {
                    format!("journaling stale Hue bond {stale_address} before replacement")
                })?;
                let address: Address = stale_address.parse().with_context(|| {
                    format!("invalid quarantined Hue BLE address {stale_address}")
                })?;
                let address_key = address.to_string();
                client.mark_device_operation_uncertain(&address_key)?;
                Self::remove_exact_local_bond(&adapter, address, None)
                    .await
                    .with_context(|| {
                        format!(
                            "removing stale BlueZ bond for physically reset Hue bulb {stale_address}"
                        )
                    })?;
                client.clear_device_operation_uncertain(&address_key)?;
            }
            let candidates = Self::discover_candidates(&client, &adapter, &request).await?;
            let _agent = session
                .register_agent(Self::pairing_agent(
                    candidates
                        .iter()
                        .map(|candidate| candidate.address)
                        .collect(),
                ))
                .await
                .context("registering Hue pairing agent")?;

            let mut paired = Vec::new();
            let mut failures = Vec::new();
            let mut retained_bond_addresses = Vec::new();
            for candidate in candidates {
                let address = candidate.address;
                let device = match adapter.device(address) {
                    Ok(device) => device,
                    Err(error) => {
                        failures.push(format!("{address}: opening device failed: {error}"));
                        continue;
                    }
                };
                let already_paired = if candidate.paired {
                    true
                } else {
                    match device.is_paired().await {
                        Ok(paired) => paired,
                        Err(error) => {
                            failures.push(format!(
                                "{address}: could not verify BlueZ pairing state: {error}"
                            ));
                            continue;
                        }
                    }
                };
                if !already_paired {
                    if let Err(error) = record_bond_intent(&address.to_string()) {
                        failures.push(format!(
                            "{address}: could not journal Hue bond intent before pairing: {error:#}"
                        ));
                        continue;
                    }
                    let pairing = match tokio::time::timeout(PAIR_TIMEOUT, device.pair()).await {
                        Ok(pairing) => pairing,
                        Err(error) => {
                            // Pair may have completed just before the timeout.
                            // Never one-sidedly delete a possible Hue bond;
                            // persist this exact address for an explicit retry.
                            retained_bond_addresses.push(address.to_string());
                            failures.push(format!(
                                "{address}: timed out bonding with Hue bulb: {error}"
                            ));
                            continue;
                        }
                    };
                    match pairing {
                        Ok(()) => {}
                        Err(error) => {
                            match device.is_paired().await {
                                Ok(true) => {
                                    // Recover a bond that completed while
                                    // BlueZ reported the final Pair error.
                                    tracing::debug!(
                                        target: "pair",
                                        "Hue bulb became paired despite Pair error: {error}"
                                    );
                                }
                                Ok(false) => {
                                    retained_bond_addresses.push(address.to_string());
                                    failures
                                        .push(format!("{address}: bonding failed: {error}"));
                                    continue;
                                }
                                Err(state_error) => {
                                    retained_bond_addresses.push(address.to_string());
                                    failures.push(format!(
                                        "{address}: bonding failed ({error}) and BlueZ pairing state could not be verified: {state_error}"
                                    ));
                                    continue;
                                }
                            }
                        }
                    }
                }
                let record = match Self::inspect_paired_device(
                    &client,
                    &connections,
                    &adapter,
                    &device,
                )
                .await
                {
                    Ok(record) => record,
                    Err(error) => {
                        retained_bond_addresses.push(address.to_string());
                        failures.push(format!(
                            "{address}: validating paired Hue bulb failed: {error:#}"
                        ));
                        continue;
                    }
                };
                if let Err(error) = device.set_trusted(true).await {
                    let cleanup =
                        Self::cancel_failed_command(&device, Instant::now() + CONNECT_TIMEOUT).await;
                    if cleanup.is_ok() {
                        let _ = connections.forget(&address.to_string());
                        Self::clear_record_operation_uncertain(&client, &record)?;
                    }
                    retained_bond_addresses.push(address.to_string());
                    let cleanup = cleanup
                        .err()
                        .map(|cleanup| format!("; disconnect also failed: {cleanup:#}"))
                        .unwrap_or_default();
                    failures.push(format!(
                        "{address}: trusting validated Hue bulb failed: {error}{cleanup}"
                    ));
                    continue;
                }
                // Free the controller connection slot before interviewing the
                // next bulb in the batch. Normal control reconnects on demand.
                match Self::cancel_failed_command(&device, Instant::now() + CONNECT_TIMEOUT).await {
                    Ok(()) => {
                        let _ = connections.forget(&address.to_string());
                        Self::clear_record_operation_uncertain(&client, &record)?
                    }
                    Err(error) => failures.push(format!(
                        "{address}: paired Hue bulb could not release its controller connection; later control will reconcile it: {error:#}"
                    )),
                }
                paired.push(record);
            }

            for failure in &failures {
                tracing::warn!(target: "pair", "Hue BLE batch pairing partial failure: {failure}");
            }
            retained_bond_addresses.sort();
            retained_bond_addresses.dedup();
            Ok(HueBlePairingOutcome {
                devices: paired,
                warnings: failures,
                retained_bond_addresses,
            })
        })
    }

    fn apply_command(&self, record: &HueBleDevice, command: &HueBleCommand) -> Result<()> {
        let device_key = record.id.clone();
        let _foreground = self.gatt.begin_foreground([device_key.clone()])?;
        let record = record.clone();
        let command = *command;
        let client = Arc::clone(&self.client);
        let gatt = Arc::clone(&self.gatt);
        let connections = Arc::clone(&self.connections);
        self.run_device_adapter_operation(&device_key, move |_session, adapter| async move {
            if Self::record_operation_is_uncertain(&client, &record)? {
                anyhow::bail!(
                    "{} is fenced after an ambiguous prior command",
                    record.display_name()
                );
            }
            let device = Self::existing_device(&adapter, &record).await?;
            Self::mark_record_operation_uncertain(&client, &record)?;
            let command_result: Result<()> = async {
                Self::apply_light_command(
                    PooledCommandContext {
                        pool: &connections,
                        client: &client,
                        adapter: &adapter,
                        gatt: &gatt,
                    },
                    &device,
                    &record.id,
                    &command,
                    tokio::time::Instant::now() + CONNECT_TIMEOUT,
                )
                .await
            }
            .await;
            match command_result {
                Ok(()) => {
                    Self::clear_record_operation_uncertain(&client, &record)?;
                    Ok(())
                }
                Err(command_error) => {
                    gatt.invalidate_catalog(&record.id)?;
                    match Self::cancel_failed_command(
                        &device,
                        Instant::now() + COMMAND_CLEANUP_RESERVE,
                    )
                    .await
                    {
                        Ok(()) => {
                            Self::clear_record_operation_uncertain(&client, &record)?;
                            Err(command_error)
                        }
                        Err(cleanup_error) => Err(command_error.context(format!(
                            "cancelling the failed Hue BLE operation also failed: {cleanup_error:#}; the stable device lane remains fenced"
                        ))),
                    }
                }
            }
        })
    }

    fn apply_commands_until(
        &self,
        commands: &[(HueBleDevice, HueBleCommand)],
        deadline: Instant,
    ) -> Result<Vec<(String, Result<()>)>> {
        let _foreground = self
            .gatt
            .begin_foreground(commands.iter().map(|(record, _)| record.id.clone()))?;
        let work_deadline = deadline
            .checked_sub(COMMAND_CLEANUP_RESERVE)
            .unwrap_or(deadline);
        let operations = commands
            .iter()
            .map(|(record, command)| (record.id.clone(), (record.clone(), *command)))
            .collect();
        let client = Arc::clone(&self.client);
        let gatt = Arc::clone(&self.gatt);
        let connections = Arc::clone(&self.connections);
        let result = self.run_device_adapter_operations_until(
            deadline,
            operations,
            move |(record, command), _session, adapter| {
                let client = Arc::clone(&client);
                let gatt = Arc::clone(&gatt);
                let connections = Arc::clone(&connections);
                async move {
                let work_deadline = tokio::time::Instant::from_std(work_deadline);
                let device = tokio::time::timeout_at(
                    work_deadline,
                    Self::existing_device(&adapter, &record),
                )
                .await
                .map_err(|_| anyhow::Error::new(HueBleCommandTimeout))??;

                if Self::record_operation_is_uncertain(&client, &record)? {
                    gatt.invalidate_catalog(&record.id)?;
                    Self::reconcile_uncertain_command(&device, work_deadline)
                        .await
                        .with_context(|| {
                            format!(
                                "{} remains fenced after an ambiguous prior command",
                                record.display_name()
                            )
                        })?;
                    Self::clear_record_operation_uncertain(&client, &record)?;
                }

                // Connect is cancel-unsafe at the D-Bus boundary. Fence both
                // identities before starting it so even outer-deadline
                // cancellation leaves a durable in-process exclusion marker.
                Self::mark_record_operation_uncertain(&client, &record)?;
                let command_started = Instant::now();
                let command_result = match tokio::time::timeout_at(work_deadline, async {
                    Self::apply_light_command(
                        PooledCommandContext {
                            pool: &connections,
                            client: &client,
                            adapter: &adapter,
                            gatt: &gatt,
                        },
                        &device,
                        &record.id,
                        &command,
                        work_deadline,
                    )
                    .await
                })
                .await
                {
                    Ok(result) => result,
                    Err(_) => {
                        tracing::warn!(
                            target: "cmd",
                            event = "hue_ble_device_command",
                            device_id = %record.id,
                            failed_stage = "deadline",
                            total_ms = command_started.elapsed().as_millis() as u64,
                            outcome = "timeout",
                            "Hue BLE GATT command did not acknowledge before its deadline"
                        );
                        Err(anyhow::Error::new(HueBleCommandTimeout))
                    }
                };

                match command_result {
                    Ok(()) => {
                        Self::clear_record_operation_uncertain(&client, &record)?;
                        Ok(())
                    }
                    Err(command_error) => {
                        gatt.invalidate_catalog(&record.id)?;
                        match Self::cancel_failed_command(&device, deadline).await {
                            Ok(()) => {
                                Self::clear_record_operation_uncertain(&client, &record)?;
                                Err(command_error)
                            }
                            Err(cleanup_error) => {
                                Err(command_error.context(format!(
                                    "cancelling the failed Hue BLE operation also failed: {cleanup_error:#}; the stable device lane remains fenced"
                                )))
                            }
                        }
                    }
                }
                }
            },
        );
        match result {
            Ok(outcomes) => Ok(outcomes
                .into_iter()
                .map(|(device_id, outcome)| {
                    let outcome = match outcome {
                        Err(error) if error.is::<BluezOperationDeadlineExceeded>() => {
                            Err(error.context(HueBleCommandTimeout))
                        }
                        outcome => outcome,
                    };
                    (device_id, outcome)
                })
                .collect()),
            Err(error) if Instant::now() >= deadline => Err(error.context(HueBleCommandTimeout)),
            result => result,
        }
    }

    fn initialize_connection_pool(&self, devices: &[HueBleDevice]) -> Result<()> {
        let devices = devices.to_vec();
        let client = Arc::clone(&self.client);
        let connections = Arc::clone(&self.connections);
        self.run_adapter_operation(move |_session, adapter| async move {
            let known_addresses = adapter
                .device_addresses()
                .await
                .context("listing Hue BLE devices before connection-pool initialization")?;
            for record in devices {
                let address: Address = record.address.parse().with_context(|| {
                    format!("invalid stored Hue BLE address {}", record.address)
                })?;
                if !known_addresses.contains(&address) {
                    let _ = connections.forget(&record.address);
                    continue;
                }
                let device = adapter.device(address).with_context(|| {
                    format!("opening Hue bulb {address} during pool initialization")
                })?;
                if !device.is_connected().await.with_context(|| {
                    format!("checking Hue bulb {address} during pool initialization")
                })? {
                    let _ = connections.forget(&record.address);
                    continue;
                }

                Self::mark_record_operation_uncertain(&client, &record)?;
                Self::cancel_failed_command(&device, Instant::now() + CONNECT_TIMEOUT)
                    .await
                    .with_context(|| format!("releasing inherited Hue BLE connection {address}"))?;
                connections.forget(&record.address)?;
                Self::clear_record_operation_uncertain(&client, &record)?;
            }
            Ok(())
        })
    }

    fn prewarm(&self, record: &HueBleDevice) -> Result<bool> {
        let device_key = record.id.clone();
        if self.gatt.foreground_pending(&device_key)? {
            return Ok(false);
        }

        let record = record.clone();
        let client = Arc::clone(&self.client);
        let gatt = Arc::clone(&self.gatt);
        let connections = Arc::clone(&self.connections);
        Ok(self
            .try_run_device_adapter_operation(&device_key, move |_session, adapter| async move {
                if gatt.foreground_pending(&record.id)?
                    || Self::record_operation_is_uncertain(&client, &record)?
                {
                    return Ok(false);
                }

                let total_started = Instant::now();
                let device = Self::existing_device(&adapter, &record).await?;
                // Connect is cancellation-unsafe at the D-Bus boundary. Fence
                // both device identities until success or explicit cleanup.
                Self::mark_record_operation_uncertain(&client, &record)?;
                let warm_result: Result<bool> = match tokio::time::timeout(
                    PREWARM_WORK_TIMEOUT,
                    async {
                        let connect_started = Instant::now();
                        let (connected_at_start, _connection_lease) = Self::connect_pooled(
                            &connections,
                            &client,
                            &adapter,
                            &device,
                            tokio::time::Instant::now() + PREWARM_WORK_TIMEOUT,
                        )
                        .await?;
                        let connect_ms = connect_started.elapsed().as_millis() as u64;
                        if !connected_at_start {
                            gatt.invalidate_catalog(&record.id)?;
                        }

                        // A real command registered while Connect was in
                        // flight. Leave the now-safe connection open, release
                        // the lane, and let that command own GATT discovery.
                        if gatt.foreground_pending(&record.id)? {
                            tracing::info!(
                                target: "cmd",
                                event = "hue_ble_prewarm",
                                device_id = %record.id,
                                connected_at_start,
                                connect_ms,
                                total_ms = total_started.elapsed().as_millis() as u64,
                                outcome = "yielded",
                                "Hue BLE startup prewarm yielded after connecting"
                            );
                            return Ok(false);
                        }

                        let resolve_started = Instant::now();
                        let catalog = Self::light_control_characteristics(
                            &gatt,
                            &device,
                            &record.id,
                            CatalogResolution::Required,
                        )
                        .await?
                        .ok_or_else(|| anyhow::anyhow!("required Hue BLE GATT lookup returned no catalog"))?;
                        let resolve_ms = resolve_started.elapsed().as_millis() as u64;
                        tracing::info!(
                            target: "cmd",
                            event = "hue_ble_prewarm",
                            device_id = %record.id,
                            connected_at_start,
                            catalog_cache_hit = catalog.cache_hit,
                            connect_ms,
                            resolve_ms,
                            total_ms = total_started.elapsed().as_millis() as u64,
                            outcome = "warmed",
                            "Hue BLE startup prewarm completed"
                        );
                        Ok(true)
                    },
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => anyhow::bail!("Hue BLE startup prewarm timed out"),
                };

                match warm_result {
                    Ok(warmed) => {
                        Self::clear_record_operation_uncertain(&client, &record)?;
                        Ok(warmed)
                    }
                    Err(warm_error) => {
                        gatt.invalidate_catalog(&record.id)?;
                        tracing::warn!(
                            target: "cmd",
                            event = "hue_ble_prewarm",
                            device_id = %record.id,
                            total_ms = total_started.elapsed().as_millis() as u64,
                            outcome = "error",
                            error = %warm_error,
                            "Hue BLE startup prewarm failed"
                        );
                        match Self::cancel_failed_command(
                            &device,
                            Instant::now() + COMMAND_CLEANUP_RESERVE,
                        )
                        .await
                        {
                            Ok(()) => {
                                Self::clear_record_operation_uncertain(&client, &record)?;
                                Err(warm_error)
                            }
                            Err(cleanup_error) => Err(warm_error.context(format!(
                                "cancelling the failed Hue BLE prewarm also failed: {cleanup_error:#}; the stable device lane remains fenced"
                            ))),
                        }
                    }
                }
            })?
            .unwrap_or(false))
    }

    fn read_state(&self, record: &HueBleDevice) -> Result<HueBleState> {
        let device_key = record.id.clone();
        let _foreground = self.gatt.begin_foreground([device_key.clone()])?;
        let record = record.clone();
        let client = Arc::clone(&self.client);
        let gatt = Arc::clone(&self.gatt);
        let connections = Arc::clone(&self.connections);
        self.run_device_adapter_operation(&device_key, move |_session, adapter| async move {
            if Self::record_operation_is_uncertain(&client, &record)? {
                anyhow::bail!(
                    "{} state is indeterminate after an ambiguous prior command",
                    record.display_name()
                );
            }
            let device = Self::existing_device(&adapter, &record).await?;
            Self::mark_record_operation_uncertain(&client, &record)?;
            let read_result: Result<HueBleState> = async {
                let (connected_at_start, _connection_lease) = Self::connect_pooled(
                    &connections,
                    &client,
                    &adapter,
                    &device,
                    tokio::time::Instant::now() + CONNECT_TIMEOUT,
                )
                .await?;
                if !connected_at_start {
                    gatt.invalidate_catalog(&record.id)?;
                }
                Self::read_state_with_catalog(&gatt, &device, &record.id).await
            }
            .await;
            match read_result {
                Ok(state) => {
                    Self::clear_record_operation_uncertain(&client, &record)?;
                    Ok(state)
                }
                Err(read_error) => {
                    gatt.invalidate_catalog(&record.id)?;
                    match Self::cancel_failed_command(
                        &device,
                        Instant::now() + COMMAND_CLEANUP_RESERVE,
                    )
                    .await
                    {
                        Ok(()) => {
                            Self::clear_record_operation_uncertain(&client, &record)?;
                            Err(read_error)
                        }
                        Err(cleanup_error) => Err(read_error.context(format!(
                            "cancelling the failed Hue BLE read also failed: {cleanup_error:#}; the stable device lane remains fenced"
                        ))),
                    }
                }
            }
        })
    }

    fn read_state_passive(&self, record: &HueBleDevice) -> Result<Option<HueBleState>> {
        let device_key = record.id.clone();
        if self.gatt.foreground_pending(&device_key)? {
            return Ok(None);
        }
        let record = record.clone();
        let client = Arc::clone(&self.client);
        let gatt = Arc::clone(&self.gatt);
        let connections = Arc::clone(&self.connections);
        Ok(self
            .try_run_device_adapter_operation(&device_key, move |_session, adapter| async move {
                if gatt.foreground_pending(&record.id)? {
                    return Ok(None);
                }
                if Self::record_operation_is_uncertain(&client, &record)? {
                    return Ok(None);
                }
                let device = Self::existing_device(&adapter, &record).await?;
                if !device.is_connected().await.unwrap_or(false) {
                    return Ok(None);
                }
                let address = device.address().to_string();
                let Some(_connection_lease) = connections.try_lease_existing(&address)? else {
                    // A connected link not tracked by this pool may be owned by
                    // another BlueZ client, or already selected for eviction.
                    return Ok(None);
                };
                let Some(catalog) = Self::light_control_characteristics(
                    &gatt,
                    &device,
                    &record.id,
                    CatalogResolution::CacheOnly,
                )
                .await?
                else {
                    return Ok(None);
                };
                // Close the race with a foreground request that arrived while
                // the connected-state property was being read. Once the one
                // power ReadValue starts it remains bounded and fenced.
                if gatt.foreground_pending(&record.id)? {
                    return Ok(None);
                }
                // Even a passive ReadValue is a daemon-owned D-Bus operation.
                // Mark before the bounded future so timeout/cancellation cannot
                // release this lane and let a foreground Connect overlap it.
                Self::mark_record_operation_uncertain(&client, &record)?;
                let read_result = tokio::time::timeout(PASSIVE_READ_TIMEOUT, async {
                    Self::read_power_state_from_characteristics(&catalog.characteristics).await
                })
                .await
                .context("passive Hue state read timed out")
                .and_then(|result| result);
                match read_result {
                    Ok(state) => {
                        Self::clear_record_operation_uncertain(&client, &record)?;
                        Ok(Some(state))
                    }
                    Err(read_error) => {
                        gatt.invalidate_catalog(&record.id)?;
                        match Self::cancel_failed_command(
                            &device,
                            Instant::now() + COMMAND_CLEANUP_RESERVE,
                        )
                        .await
                        {
                            Ok(()) => {
                                Self::clear_record_operation_uncertain(&client, &record)?;
                                Err(read_error)
                            }
                            Err(cleanup_error) => Err(read_error.context(format!(
                                "cancelling the failed passive Hue BLE read also failed: {cleanup_error:#}; the stable device lane remains fenced"
                            ))),
                        }
                    }
                }
            })?
            .flatten())
    }

    fn has_local_bond(&self, record: &HueBleDevice) -> Result<bool> {
        Self::ensure_persistent_bond_storage()?;
        let device_key = record.id.clone();
        let record = record.clone();
        self.run_device_adapter_operation(&device_key, move |_session, adapter| async move {
            let address: Address = record
                .address
                .parse()
                .with_context(|| format!("invalid stored Hue BLE address {}", record.address))?;
            if !adapter.device_addresses().await?.contains(&address) {
                return Ok(false);
            }
            let device = adapter
                .device(address)
                .context("opening stored Hue bulb to inspect its bond")?;
            device
                .is_paired()
                .await
                .context("checking the stored Hue bulb bond")
        })
    }

    fn local_hue_bond_addresses(&self) -> Result<Vec<String>> {
        Self::ensure_persistent_bond_storage()?;
        self.run_adapter_operation(|_session, adapter| async move {
            let hue_service = uuid(protocol::HUE_DISCOVERY_SERVICE_UUID);
            let mut bonded = Vec::new();
            for address in adapter
                .device_addresses()
                .await
                .context("listing BlueZ devices before Hue bond cleanup")?
            {
                let device = adapter
                    .device(address)
                    .with_context(|| format!("opening BlueZ device {address}"))?;
                if !device
                    .is_paired()
                    .await
                    .with_context(|| format!("checking whether {address} is paired"))?
                {
                    continue;
                }
                let services = device
                    .uuids()
                    .await
                    .with_context(|| format!("reading cached services for {address}"))?
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "paired BlueZ device {address} has no cached service UUIDs, so Rhythm cannot safely rule out an untracked Hue Bluetooth bond"
                        )
                    })?;
                if services.contains(&hue_service) {
                    bonded.push(address.to_string());
                }
            }
            bonded.sort();
            Ok(bonded)
        })
    }

    fn inspect_local_bond(&self, address: &str) -> Result<HueBleDevice> {
        Self::ensure_persistent_bond_storage()?;
        let address: Address = address
            .parse()
            .with_context(|| format!("invalid bonded Hue BLE address {address}"))?;
        let client = Arc::clone(&self.client);
        let connections = Arc::clone(&self.connections);
        // This recovery path starts from a locator and learns the stable EUI-64
        // only after opening the bond. Keep it driver-wide so it cannot race a
        // keyed operation for the same physical bulb under a changed address.
        self.run_adapter_operation(move |_session, adapter| async move {
            if !adapter.device_addresses().await?.contains(&address) {
                anyhow::bail!("BlueZ no longer has bonded device {address}");
            }
            let device = adapter
                .device(address)
                .with_context(|| format!("opening bonded Hue BLE device {address}"))?;
            if !device
                .is_paired()
                .await
                .with_context(|| format!("checking whether {address} is paired"))?
            {
                anyhow::bail!("BlueZ device {address} is no longer paired");
            }
            let record = Self::inspect_paired_device(&client, &connections, &adapter, &device)
                .await
                .with_context(|| {
                    format!("reconstructing metadata for bonded Hue bulb {address}")
                })?;
            Self::cancel_failed_command(&device, Instant::now() + CONNECT_TIMEOUT)
                .await
                .context("disconnecting Hue bulb after metadata reconstruction")?;
            let _ = connections.forget(&address.to_string());
            Self::clear_record_operation_uncertain(&client, &record)?;
            Ok(record)
        })
    }

    fn validate_pairing_handoff(&self, record: &HueBleDevice) -> Result<()> {
        Self::ensure_persistent_bond_storage()?;
        let device_key = record.id.clone();
        let record = record.clone();
        let client = Arc::clone(&self.client);
        let connections = Arc::clone(&self.connections);
        self.run_device_adapter_operation(&device_key, move |_session, adapter| async move {
            let address: Address = record
                .address
                .parse()
                .with_context(|| format!("invalid stored Hue BLE address {}", record.address))?;
            if !adapter.device_addresses().await?.contains(&address) {
                anyhow::bail!("The Hue bulb's local BlueZ bond is already absent");
            }
            let device = adapter
                .device(address)
                .context("opening bonded Hue bulb for pairing-handoff validation")?;
            if !device
                .is_paired()
                .await
                .context("checking the Hue bulb bond before pairing-handoff validation")?
            {
                anyhow::bail!("The Hue bulb is no longer bonded to this Rhythm Box");
            }
            if Self::record_operation_is_uncertain(&client, &record)? {
                Self::reconcile_uncertain_command(
                    &device,
                    tokio::time::Instant::now() + CONNECT_TIMEOUT,
                )
                .await
                .context("reconciling Hue bulb before pairing-handoff validation")?;
                Self::clear_record_operation_uncertain(&client, &record)?;
            }
            Self::mark_record_operation_uncertain(&client, &record)?;
            let validation_result: Result<()> = async {
                let (_connected_at_start, _connection_lease) = Self::connect_pooled(
                    &connections,
                    &client,
                    &adapter,
                    &device,
                    tokio::time::Instant::now() + CONNECT_TIMEOUT,
                )
                .await
                    .context("connecting to Hue bulb to validate pairing handoff")?;
                let characteristics = Self::characteristics(&device)
                    .await
                    .context("resolving Hue bulb pairing-handoff characteristic")?;
                let characteristic = characteristics
                    .get(&uuid(protocol::PAIRING_HANDOFF_UUID))
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Hue bulb does not expose its pairing-handoff characteristic"
                        )
                    })?;
                let flags = characteristic
                    .flags()
                    .await
                    .context("reading Hue bulb pairing-handoff permissions")?;
                if !flags.write && !flags.write_without_response {
                    anyhow::bail!("Hue bulb pairing-handoff characteristic is not writable");
                }
                Ok(())
            }
            .await;
            let disconnect_result =
                Self::cancel_failed_command(&device, Instant::now() + CONNECT_TIMEOUT).await;
            match disconnect_result {
                Ok(()) => {
                    let _ = connections.forget(&record.address);
                    Self::clear_record_operation_uncertain(&client, &record)?;
                    validation_result
                }
                Err(cleanup_error) => match validation_result {
                    Ok(()) => Err(cleanup_error)
                        .context("disconnecting Hue bulb after handoff validation"),
                    Err(validation_error) => Err(validation_error.context(format!(
                        "pairing-handoff validation cleanup also failed: {cleanup_error:#}; the bulb remains fenced"
                    ))),
                },
            }
        })
    }

    fn prepare_pairing_handoff(&self, record: &HueBleDevice) -> Result<std::time::Instant> {
        Self::ensure_persistent_bond_storage()?;
        let device_key = record.id.clone();
        let record = record.clone();
        let client = Arc::clone(&self.client);
        let connections = Arc::clone(&self.connections);
        self.run_device_instant_adapter_operation(
            &device_key,
            move |_session, adapter| async move {
                let address: Address = record.address.parse().with_context(|| {
                    format!("invalid stored Hue BLE address {}", record.address)
                })?;
                if !adapter.device_addresses().await?.contains(&address) {
                    anyhow::bail!("The Hue bulb's local BlueZ bond is already absent");
                }
                let device = adapter
                    .device(address)
                    .context("opening bonded Hue bulb for pairing handoff")?;
                if !device
                    .is_paired()
                    .await
                    .context("checking the Hue bulb bond before pairing handoff")?
                {
                    anyhow::bail!("The Hue bulb is no longer bonded to this Rhythm Box");
                }
                if Self::record_operation_is_uncertain(&client, &record)? {
                    Self::reconcile_uncertain_command(
                        &device,
                        tokio::time::Instant::now() + CONNECT_TIMEOUT,
                    )
                    .await
                    .context("reconciling Hue bulb before pairing handoff")?;
                    Self::clear_record_operation_uncertain(&client, &record)?;
                }
                Self::mark_record_operation_uncertain(&client, &record)?;
                let handoff_result: Result<std::time::Instant> = async {
                    let (_connected_at_start, _connection_lease) = Self::connect_pooled(
                        &connections,
                        &client,
                        &adapter,
                        &device,
                        tokio::time::Instant::now() + CONNECT_TIMEOUT,
                    )
                    .await
                        .context("connecting to Hue bulb before releasing its bond")?;
                    let characteristics = Self::characteristics(&device)
                        .await
                        .context("resolving Hue bulb pairing-handoff characteristic")?;
                    Self::write(&characteristics, protocol::PAIRING_HANDOFF_UUID, &[0x01])
                        .await
                        .context(
                            "opening the Hue bulb pairing window before removing its BlueZ bond",
                        )?;
                    Ok(std::time::Instant::now())
                }
                .await;
                // Factory reset refreshes every bulb before deleting any keys.
                // Release each ACL slot after the authenticated write so a large
                // installation cannot exhaust the controller before later bulbs.
                let disconnect_result =
                    Self::cancel_failed_command(&device, Instant::now() + CONNECT_TIMEOUT).await;
                match disconnect_result {
                    Ok(()) => {
                        let _ = connections.forget(&record.address);
                        Self::clear_record_operation_uncertain(&client, &record)?;
                        handoff_result
                    }
                    Err(cleanup_error) => match handoff_result {
                        Ok(_) => Err(cleanup_error)
                            .context("disconnecting Hue bulb after pairing handoff"),
                        Err(handoff_error) => Err(handoff_error.context(format!(
                            "pairing-handoff cleanup also failed: {cleanup_error:#}; the bulb remains fenced"
                        ))),
                    },
                }
            },
        )
    }

    fn remove_local_bond(
        &self,
        record: &HueBleDevice,
        handoff_valid_until: Option<Instant>,
    ) -> Result<()> {
        Self::ensure_persistent_bond_storage()?;
        let device_key = record.id.clone();
        let record = record.clone();
        let address_key = record.address.clone();
        let client = Arc::clone(&self.client);
        let result =
            self.run_device_adapter_operation(&device_key, move |_session, adapter| async move {
                let address: Address = record.address.parse().with_context(|| {
                    format!("invalid stored Hue BLE address {}", record.address)
                })?;
                Self::mark_record_operation_uncertain(&client, &record)?;
                Self::remove_exact_local_bond(&adapter, address, handoff_valid_until).await?;
                // Exact bond removal is a terminal acknowledgement from BlueZ;
                // the old stable-ID and locator fences cannot protect anything
                // after the device object/key is gone.
                Self::clear_record_operation_uncertain(&client, &record)
            });
        if result.is_ok() {
            let _ = self.connections.forget(&address_key);
            self.gatt.invalidate_catalog(&device_key)?;
        }
        result
    }
}

fn uuid(value: &str) -> Uuid {
    value.parse().expect("Hue BLE UUID constants are valid")
}

fn classify_adapter_probe(observed: Option<bool>) -> HueBleAdapterAvailability {
    match observed {
        Some(true) => HueBleAdapterAvailability::Available,
        Some(false) => HueBleAdapterAvailability::Unavailable,
        None => HueBleAdapterAvailability::Busy,
    }
}

/// `None` means the BlueZ device object is absent. Property-read failures are
/// intentionally propagated before reaching this helper.
fn local_bond_removal_complete(paired: Option<bool>) -> bool {
    matches!(paired, None | Some(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opportunistic_adapter_contention_is_busy_not_disconnected() {
        assert_eq!(
            classify_adapter_probe(None),
            HueBleAdapterAvailability::Busy
        );
        assert_eq!(
            classify_adapter_probe(Some(true)),
            HueBleAdapterAvailability::Available
        );
        assert_eq!(
            classify_adapter_probe(Some(false)),
            HueBleAdapterAvailability::Unavailable
        );
    }

    #[test]
    fn catalog_retry_is_limited_to_definitive_bluez_object_loss() {
        let stale = anyhow::Error::new(bluer::Error {
            kind: ErrorKind::NotFound,
            message: String::new(),
        })
        .context("writing cached Hue characteristic");
        assert!(BluezHueBleTransport::is_stale_gatt_catalog_error(&stale));

        let transient = anyhow::Error::new(bluer::Error {
            kind: ErrorKind::NotAvailable,
            message: String::new(),
        })
        .context("writing live Hue characteristic");
        assert!(!BluezHueBleTransport::is_stale_gatt_catalog_error(
            &transient
        ));
    }

    #[test]
    fn pairing_validation_rejects_hue_accessories_without_light_power_control() {
        let accessory_characteristics =
            HashSet::from([uuid(protocol::EUI64_UUID), uuid(protocol::DEVICE_NAME_UUID)]);
        assert!(BluezHueBleTransport::validate_light_characteristic_ids(
            &accessory_characteristics
        )
        .is_err());

        let bulb_characteristics = HashSet::from([
            uuid(protocol::EUI64_UUID),
            uuid(protocol::POWER_UUID),
            uuid(protocol::BRIGHTNESS_UUID),
        ]);
        BluezHueBleTransport::validate_light_characteristic_ids(&bulb_characteristics).unwrap();
    }

    #[tokio::test]
    async fn disconnect_confirmation_accepts_only_proven_terminal_state() {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        let timeout = |_| anyhow::anyhow!("test deadline expired");

        disconnect_and_confirm(
            deadline,
            || async { Ok(()) },
            || async { Ok(false) },
            timeout,
        )
        .await
        .unwrap();
        disconnect_and_confirm(
            deadline,
            || async { Err(anyhow::anyhow!("already disconnected")) },
            || async { Ok(false) },
            timeout,
        )
        .await
        .unwrap();
        assert!(disconnect_and_confirm(
            deadline,
            || async { Ok(()) },
            || async { Ok(true) },
            timeout,
        )
        .await
        .is_err());
        assert!(disconnect_and_confirm(
            deadline,
            || async { Err(anyhow::anyhow!("disconnect rejected")) },
            || async { Ok(true) },
            timeout,
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn disconnect_confirmation_rejects_unknown_or_expired_state() {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        assert!(disconnect_and_confirm(
            deadline,
            || async { Ok(()) },
            || async { Err(anyhow::anyhow!("connection state unavailable")) },
            |_| anyhow::anyhow!("test deadline expired"),
        )
        .await
        .is_err());

        assert!(disconnect_and_confirm(
            tokio::time::Instant::now(),
            || futures::future::pending::<Result<()>>(),
            || async { Ok(false) },
            |_| anyhow::anyhow!("test deadline expired"),
        )
        .await
        .is_err());
    }

    #[test]
    fn exact_explicit_reassociation_can_adopt_a_paired_tombstone() {
        let mut request = HueBlePairingRequest::from_value(&serde_json::json!({})).unwrap();
        request.allow_paired_orphan_adoption = false;
        request.blocked_paired_addresses = vec!["AA:BB:CC:DD:EE:FF".to_string()];
        request.explicit_reassociation_addresses = vec!["aa:bb:cc:dd:ee:ff".to_string()];

        assert!(BluezHueBleTransport::pair_candidate_is_eligible(
            &request,
            "AA:BB:CC:DD:EE:FF",
            true,
        ));
    }

    #[test]
    fn unknown_paired_candidate_stays_blocked_when_orphan_adoption_is_disabled() {
        let mut request = HueBlePairingRequest::from_value(&serde_json::json!({})).unwrap();
        request.allow_paired_orphan_adoption = false;
        request.explicit_reassociation_addresses = vec!["AA:BB:CC:DD:EE:FF".to_string()];

        assert!(!BluezHueBleTransport::pair_candidate_is_eligible(
            &request,
            "11:22:33:44:55:66",
            true,
        ));
    }

    #[test]
    fn active_known_address_is_never_reassociated() {
        let mut request = HueBlePairingRequest::from_value(&serde_json::json!({})).unwrap();
        request.known_addresses = vec!["AA:BB:CC:DD:EE:FF".to_string()];
        request.explicit_reassociation_addresses = vec!["aa:bb:cc:dd:ee:ff".to_string()];

        assert!(!BluezHueBleTransport::pair_candidate_is_eligible(
            &request,
            "AA:BB:CC:DD:EE:FF",
            true,
        ));
        assert!(!BluezHueBleTransport::pair_candidate_is_eligible(
            &request,
            "AA:BB:CC:DD:EE:FF",
            false,
        ));
    }

    #[test]
    fn stale_bond_replacement_targets_must_also_be_exact_reassociations() {
        let mut request = HueBlePairingRequest::from_value(&serde_json::json!({
            "replace_stale_bonds": true
        }))
        .unwrap();
        request.explicit_reassociation_addresses = vec!["AA:BB:CC:DD:EE:FF".to_string()];
        request.replace_stale_bond_addresses = request.explicit_reassociation_addresses.clone();

        assert!(BluezHueBleTransport::stale_bond_replacement_is_allowed(
            &request,
            "aa:bb:cc:dd:ee:ff"
        ));
        assert!(!BluezHueBleTransport::stale_bond_replacement_is_allowed(
            &request,
            "11:22:33:44:55:66"
        ));
        request.known_addresses = vec!["aa:bb:cc:dd:ee:ff".to_string()];
        assert!(!BluezHueBleTransport::stale_bond_replacement_is_allowed(
            &request,
            "AA:BB:CC:DD:EE:FF"
        ));
    }

    #[test]
    fn local_bond_removal_accepts_absent_or_unpaired_but_not_paired() {
        assert!(local_bond_removal_complete(None));
        assert!(local_bond_removal_complete(Some(false)));
        assert!(!local_bond_removal_complete(Some(true)));
    }

    #[test]
    fn exact_bond_removal_deadline_is_optional_but_fails_closed_when_expired() {
        BluezHueBleTransport::ensure_handoff_is_fresh(None).unwrap();
        BluezHueBleTransport::ensure_handoff_is_fresh(Some(
            Instant::now() + Duration::from_secs(1),
        ))
        .unwrap();

        let error = BluezHueBleTransport::ensure_handoff_is_fresh(Some(Instant::now()))
            .expect_err("an expired Hue handoff must retain the exact BlueZ bond");
        assert!(format!("{error:#}").contains("window expired"));
    }

    #[test]
    fn quiesce_permanently_rejects_future_bluez_runtime_work() {
        let transport = BluezHueBleTransport::new().unwrap();

        transport.quiesce().unwrap();

        let error = transport
            .run_adapter_operation(|_session, _adapter| async { Ok(()) })
            .expect_err("a quiesced live transport must never reopen BlueZ");
        assert!(format!("{error:#}").contains("quiesced"));
    }
}
