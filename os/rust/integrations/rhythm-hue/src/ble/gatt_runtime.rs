use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use futures::future::try_join_all;

use super::protocol;

pub(super) trait GattCatalogCharacteristic {
    fn catalog_id(&self) -> u16;
    fn catalog_uuid(&self) -> impl Future<Output = Result<String>>;
}

pub(super) trait GattCatalogService {
    type Characteristic: GattCatalogCharacteristic;

    fn catalog_id(&self) -> u16;
    fn catalog_uuid(&self) -> impl Future<Output = Result<String>>;
    fn catalog_characteristics(&self) -> impl Future<Output = Result<Vec<Self::Characteristic>>>;
}

pub(super) trait GattCatalogDevice {
    type Service: GattCatalogService;

    fn catalog_services(&self) -> impl Future<Output = Result<Vec<Self::Service>>>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HueBleGattCatalog {
    pub service_count: usize,
    pub service_id: u16,
    pub characteristic_ids: HashMap<String, u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatalogResolution {
    Required,
    CacheOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CatalogAccess {
    pub catalog: HueBleGattCatalog,
    pub cache_hit: bool,
}

#[derive(Default)]
pub(super) struct HueBleGattRuntime {
    catalogs: Mutex<HashMap<String, HueBleGattCatalog>>,
    foreground_operations: Mutex<HashMap<String, usize>>,
}

impl HueBleGattRuntime {
    pub async fn catalog_for_device<D>(
        &self,
        device_id: &str,
        device: &D,
        resolution: CatalogResolution,
    ) -> Result<Option<CatalogAccess>>
    where
        D: GattCatalogDevice,
    {
        if let Some(catalog) = self.catalog(device_id)? {
            return Ok(Some(CatalogAccess {
                catalog,
                cache_hit: true,
            }));
        }
        if resolution == CatalogResolution::CacheOnly {
            return Ok(None);
        }

        // Per-device admission serializes callers for the same bulb. Do not
        // hold the process-local cache lock while BlueZ performs D-Bus work.
        let catalog = discover_light_control_catalog(device).await?;
        self.catalogs
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE GATT catalog cache poisoned"))?
            .insert(device_id.to_string(), catalog.clone());
        Ok(Some(CatalogAccess {
            catalog,
            cache_hit: false,
        }))
    }

    pub fn invalidate_catalog(&self, device_id: &str) -> Result<()> {
        self.catalogs
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE GATT catalog cache poisoned"))?
            .remove(device_id);
        Ok(())
    }

    pub fn begin_foreground<I, S>(self: &Arc<Self>, device_ids: I) -> Result<HueBleForegroundGuard>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut device_ids = device_ids
            .into_iter()
            .map(Into::into)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        device_ids.sort();

        let mut operations = self
            .foreground_operations
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE foreground-operation registry poisoned"))?;
        for device_id in &device_ids {
            let count = operations.entry(device_id.clone()).or_default();
            *count = count
                .checked_add(1)
                .context("Hue BLE foreground-operation count overflowed")?;
        }
        drop(operations);

        Ok(HueBleForegroundGuard {
            runtime: Arc::clone(self),
            device_ids,
        })
    }

    pub fn foreground_pending(&self, device_id: &str) -> Result<bool> {
        Ok(self
            .foreground_operations
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE foreground-operation registry poisoned"))?
            .get(device_id)
            .copied()
            .unwrap_or_default()
            > 0)
    }

    fn catalog(&self, device_id: &str) -> Result<Option<HueBleGattCatalog>> {
        Ok(self
            .catalogs
            .lock()
            .map_err(|_| anyhow::anyhow!("Hue BLE GATT catalog cache poisoned"))?
            .get(device_id)
            .cloned())
    }
}

pub(super) struct HueBleForegroundGuard {
    runtime: Arc<HueBleGattRuntime>,
    device_ids: Vec<String>,
}

impl Drop for HueBleForegroundGuard {
    fn drop(&mut self) {
        let Ok(mut operations) = self.runtime.foreground_operations.lock() else {
            return;
        };
        for device_id in &self.device_ids {
            let Some(count) = operations.get_mut(device_id) else {
                continue;
            };
            *count = count.saturating_sub(1);
            if *count == 0 {
                operations.remove(device_id);
            }
        }
    }
}

async fn discover_light_control_catalog<D>(device: &D) -> Result<HueBleGattCatalog>
where
    D: GattCatalogDevice,
{
    let services = device.catalog_services().await?;
    let service_uuids = try_join_all(services.iter().map(|service| service.catalog_uuid())).await?;
    let mut selected = None;
    for (index, service_uuid) in service_uuids.iter().enumerate() {
        if service_uuid == protocol::LIGHT_CONTROL_SERVICE_UUID && selected.replace(index).is_some()
        {
            anyhow::bail!("Hue bulb exposes duplicate light-control GATT services");
        }
    }
    let service_index = selected
        .ok_or_else(|| anyhow::anyhow!("Hue bulb is missing its light-control GATT service"))?;
    let service = &services[service_index];
    let characteristics = service.catalog_characteristics().await?;
    let characteristic_uuids = try_join_all(
        characteristics
            .iter()
            .map(|characteristic| characteristic.catalog_uuid()),
    )
    .await?;
    let mut characteristic_ids = HashMap::new();
    for (characteristic, characteristic_uuid) in
        characteristics.into_iter().zip(characteristic_uuids)
    {
        if characteristic_ids
            .insert(characteristic_uuid.clone(), characteristic.catalog_id())
            .is_some()
        {
            anyhow::bail!(
                "Hue bulb exposes duplicate light-control characteristic {characteristic_uuid}"
            );
        }
    }
    Ok(HueBleGattCatalog {
        service_count: services.len(),
        service_id: service.catalog_id(),
        characteristic_ids,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use tokio::sync::Barrier;

    #[derive(Clone)]
    struct FakeCharacteristic {
        id: u16,
        uuid: String,
    }

    impl GattCatalogCharacteristic for FakeCharacteristic {
        fn catalog_id(&self) -> u16 {
            self.id
        }

        async fn catalog_uuid(&self) -> Result<String> {
            Ok(self.uuid.clone())
        }
    }

    #[derive(Clone)]
    struct FakeService {
        id: u16,
        uuid: String,
        characteristics: Vec<FakeCharacteristic>,
        enumeration_count: Arc<AtomicUsize>,
    }

    impl GattCatalogService for FakeService {
        type Characteristic = FakeCharacteristic;

        fn catalog_id(&self) -> u16 {
            self.id
        }

        async fn catalog_uuid(&self) -> Result<String> {
            Ok(self.uuid.clone())
        }

        async fn catalog_characteristics(&self) -> Result<Vec<Self::Characteristic>> {
            self.enumeration_count.fetch_add(1, Ordering::SeqCst);
            Ok(self.characteristics.clone())
        }
    }

    struct FakeDevice {
        services: Vec<FakeService>,
        enumeration_count: Arc<AtomicUsize>,
    }

    impl GattCatalogDevice for FakeDevice {
        type Service = FakeService;

        async fn catalog_services(&self) -> Result<Vec<Self::Service>> {
            self.enumeration_count.fetch_add(1, Ordering::SeqCst);
            Ok(self.services.clone())
        }
    }

    #[derive(Clone)]
    struct BarrierCharacteristic {
        id: u16,
        uuid: String,
        barrier: Arc<Barrier>,
    }

    impl GattCatalogCharacteristic for BarrierCharacteristic {
        fn catalog_id(&self) -> u16 {
            self.id
        }

        async fn catalog_uuid(&self) -> Result<String> {
            self.barrier.wait().await;
            Ok(self.uuid.clone())
        }
    }

    #[derive(Clone)]
    struct BarrierService {
        id: u16,
        uuid: String,
        uuid_barrier: Arc<Barrier>,
        characteristics: Vec<BarrierCharacteristic>,
    }

    impl GattCatalogService for BarrierService {
        type Characteristic = BarrierCharacteristic;

        fn catalog_id(&self) -> u16 {
            self.id
        }

        async fn catalog_uuid(&self) -> Result<String> {
            self.uuid_barrier.wait().await;
            Ok(self.uuid.clone())
        }

        async fn catalog_characteristics(&self) -> Result<Vec<Self::Characteristic>> {
            Ok(self.characteristics.clone())
        }
    }

    struct BarrierDevice {
        services: Vec<BarrierService>,
    }

    impl GattCatalogDevice for BarrierDevice {
        type Service = BarrierService;

        async fn catalog_services(&self) -> Result<Vec<Self::Service>> {
            Ok(self.services.clone())
        }
    }

    fn fake_device() -> (FakeDevice, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let device_enumerations = Arc::new(AtomicUsize::new(0));
        let discovery_enumerations = Arc::new(AtomicUsize::new(0));
        let control_enumerations = Arc::new(AtomicUsize::new(0));
        let information_enumerations = Arc::new(AtomicUsize::new(0));
        let device = FakeDevice {
            services: vec![
                FakeService {
                    id: 1,
                    uuid: protocol::HUE_DISCOVERY_SERVICE_UUID.to_string(),
                    characteristics: Vec::new(),
                    enumeration_count: Arc::clone(&discovery_enumerations),
                },
                FakeService {
                    id: 2,
                    uuid: protocol::LIGHT_CONTROL_SERVICE_UUID.to_string(),
                    characteristics: vec![
                        FakeCharacteristic {
                            id: 20,
                            uuid: protocol::POWER_UUID.to_string(),
                        },
                        FakeCharacteristic {
                            id: 21,
                            uuid: protocol::COMBINED_CONTROL_UUID.to_string(),
                        },
                    ],
                    enumeration_count: Arc::clone(&control_enumerations),
                },
                FakeService {
                    id: 3,
                    uuid: protocol::DEVICE_INFORMATION_SERVICE_UUID.to_string(),
                    characteristics: Vec::new(),
                    enumeration_count: Arc::clone(&information_enumerations),
                },
            ],
            enumeration_count: Arc::clone(&device_enumerations),
        };
        (device, device_enumerations, control_enumerations)
    }

    #[tokio::test]
    async fn resolved_catalog_is_reused_until_explicitly_invalidated() {
        let runtime = HueBleGattRuntime::default();
        let (device, device_enumerations, control_enumerations) = fake_device();

        let first = runtime
            .catalog_for_device("bulb-one", &device, CatalogResolution::Required)
            .await
            .unwrap()
            .unwrap();
        let second = runtime
            .catalog_for_device("bulb-one", &device, CatalogResolution::Required)
            .await
            .unwrap()
            .unwrap();

        assert!(!first.cache_hit);
        assert!(second.cache_hit);
        assert_eq!(first.catalog, second.catalog);
        assert_eq!(first.catalog.service_id, 2);
        assert_eq!(first.catalog.characteristic_ids.len(), 2);
        assert_eq!(device_enumerations.load(Ordering::SeqCst), 1);
        assert_eq!(control_enumerations.load(Ordering::SeqCst), 1);

        runtime.invalidate_catalog("bulb-one").unwrap();
        let refreshed = runtime
            .catalog_for_device("bulb-one", &device, CatalogResolution::Required)
            .await
            .unwrap()
            .unwrap();
        assert!(!refreshed.cache_hit);
        assert_eq!(device_enumerations.load(Ordering::SeqCst), 2);
        assert_eq!(control_enumerations.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn cache_only_passive_lookup_never_discovers_a_catalog() {
        let runtime = HueBleGattRuntime::default();
        let (device, device_enumerations, control_enumerations) = fake_device();

        assert!(runtime
            .catalog_for_device("bulb-one", &device, CatalogResolution::CacheOnly)
            .await
            .unwrap()
            .is_none());
        assert_eq!(device_enumerations.load(Ordering::SeqCst), 0);
        assert_eq!(control_enumerations.load(Ordering::SeqCst), 0);

        runtime
            .catalog_for_device("bulb-one", &device, CatalogResolution::Required)
            .await
            .unwrap()
            .unwrap();
        let passive = runtime
            .catalog_for_device("bulb-one", &device, CatalogResolution::CacheOnly)
            .await
            .unwrap()
            .unwrap();
        assert!(passive.cache_hit);
        assert_eq!(device_enumerations.load(Ordering::SeqCst), 1);
        assert_eq!(control_enumerations.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn discovery_enumerates_characteristics_only_for_the_exact_control_service() {
        let runtime = HueBleGattRuntime::default();
        let (device, device_enumerations, control_enumerations) = fake_device();

        let access = runtime
            .catalog_for_device("bulb-one", &device, CatalogResolution::Required)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(access.catalog.service_count, 3);
        assert_eq!(device_enumerations.load(Ordering::SeqCst), 1);
        assert_eq!(control_enumerations.load(Ordering::SeqCst), 1);
        assert_eq!(
            device.services[0].enumeration_count.load(Ordering::SeqCst),
            0
        );
        assert_eq!(
            device.services[2].enumeration_count.load(Ordering::SeqCst),
            0
        );
    }

    #[tokio::test]
    async fn discovery_rejects_missing_or_duplicate_control_services() {
        let runtime = HueBleGattRuntime::default();
        let (mut missing, _, _) = fake_device();
        missing
            .services
            .retain(|service| service.uuid != protocol::LIGHT_CONTROL_SERVICE_UUID);
        assert!(runtime
            .catalog_for_device("missing", &missing, CatalogResolution::Required)
            .await
            .is_err());

        let (mut duplicate, _, _) = fake_device();
        let mut second_control = duplicate.services[1].clone();
        second_control.id = 4;
        duplicate.services.push(second_control);
        assert!(runtime
            .catalog_for_device("duplicate", &duplicate, CatalogResolution::Required)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn discovery_rejects_duplicate_control_characteristics() {
        let runtime = HueBleGattRuntime::default();
        let (mut device, _, _) = fake_device();
        device.services[1].characteristics.push(FakeCharacteristic {
            id: 22,
            uuid: protocol::POWER_UUID.to_string(),
        });

        assert!(runtime
            .catalog_for_device("bulb-one", &device, CatalogResolution::Required)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn cold_discovery_fetches_independent_uuid_properties_concurrently() {
        let service_barrier = Arc::new(Barrier::new(3));
        let characteristic_barrier = Arc::new(Barrier::new(2));
        let characteristic = |id, uuid: &str| BarrierCharacteristic {
            id,
            uuid: uuid.to_string(),
            barrier: Arc::clone(&characteristic_barrier),
        };
        let service = |id, uuid: &str, characteristics| BarrierService {
            id,
            uuid: uuid.to_string(),
            uuid_barrier: Arc::clone(&service_barrier),
            characteristics,
        };
        let device = BarrierDevice {
            services: vec![
                service(1, protocol::HUE_DISCOVERY_SERVICE_UUID, Vec::new()),
                service(
                    2,
                    protocol::LIGHT_CONTROL_SERVICE_UUID,
                    vec![
                        characteristic(20, protocol::POWER_UUID),
                        characteristic(21, protocol::COMBINED_CONTROL_UUID),
                    ],
                ),
                service(3, protocol::DEVICE_INFORMATION_SERVICE_UUID, Vec::new()),
            ],
        };
        let runtime = HueBleGattRuntime::default();

        let access = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            runtime.catalog_for_device("bulb-one", &device, CatalogResolution::Required),
        )
        .await
        .expect("serial UUID property reads would deadlock on the phase barriers")
        .unwrap()
        .unwrap();

        assert_eq!(access.catalog.service_id, 2);
        assert_eq!(access.catalog.characteristic_ids.len(), 2);
    }

    #[test]
    fn foreground_registry_is_keyed_nested_and_released_by_guard_lifetime() {
        let runtime = Arc::new(HueBleGattRuntime::default());
        assert!(!runtime.foreground_pending("bulb-one").unwrap());
        assert!(!runtime.foreground_pending("bulb-two").unwrap());

        let outer = runtime.begin_foreground(["bulb-one", "bulb-one"]).unwrap();
        assert!(runtime.foreground_pending("bulb-one").unwrap());
        assert!(!runtime.foreground_pending("bulb-two").unwrap());
        {
            let _inner = runtime.begin_foreground(["bulb-one", "bulb-two"]).unwrap();
            assert!(runtime.foreground_pending("bulb-one").unwrap());
            assert!(runtime.foreground_pending("bulb-two").unwrap());
        }
        assert!(runtime.foreground_pending("bulb-one").unwrap());
        assert!(!runtime.foreground_pending("bulb-two").unwrap());

        drop(outer);
        assert!(!runtime.foreground_pending("bulb-one").unwrap());
    }
}
