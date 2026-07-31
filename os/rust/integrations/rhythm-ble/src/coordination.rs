//! Platform-neutral adapter admission shared by the BlueZ owner and the
//! deterministic mixed-device conformance harness.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use anyhow::{Context, Result as AnyResult};
#[cfg(all(target_os = "linux", feature = "bluez"))]
use tokio::sync::TryAcquireError;
use tokio::sync::{AcquireError, OwnedSemaphorePermit, Semaphore};

pub(crate) enum ClientOperationGuard {
    Global {
        _scope: tokio::sync::OwnedRwLockWriteGuard<()>,
    },
    Device {
        _scope: tokio::sync::OwnedRwLockReadGuard<()>,
        _lane: tokio::sync::OwnedMutexGuard<()>,
    },
}

/// One protocol driver's operation coordinator. Driver-wide work such as
/// pairing and enumeration is exclusive, while ordinary operations serialize
/// only by stable device key.
///
/// The coordinator is shared by every same-named client of a runtime. This
/// prevents accidentally constructing a second transport handle from
/// bypassing ordering for the same physical device.
pub(crate) struct ClientOperationCoordinator {
    scope: Arc<tokio::sync::RwLock<()>>,
    lanes: DeviceOperationLanes,
}

impl ClientOperationCoordinator {
    pub(crate) fn new() -> Self {
        Self {
            scope: Arc::new(tokio::sync::RwLock::new(())),
            lanes: DeviceOperationLanes::new(),
        }
    }

    pub(crate) async fn admit_global(
        &self,
        deadline: tokio::time::Instant,
    ) -> AnyResult<ClientOperationGuard> {
        let guard = tokio::time::timeout_at(deadline, self.scope.clone().write_owned())
            .await
            .context("timed out waiting for exclusive Bluetooth client admission")?;
        Ok(ClientOperationGuard::Global { _scope: guard })
    }

    pub(crate) async fn admit_device(
        &self,
        key: &str,
        deadline: tokio::time::Instant,
    ) -> AnyResult<ClientOperationGuard> {
        let lane = self.lanes.lane(key)?;
        let lane = tokio::time::timeout_at(deadline, lane.lock_owned())
            .await
            .context("timed out waiting for the Bluetooth device operation lane")?;
        let scope = tokio::time::timeout_at(deadline, self.scope.clone().read_owned())
            .await
            .context("timed out waiting for shared Bluetooth client admission")?;
        Ok(ClientOperationGuard::Device {
            _scope: scope,
            _lane: lane,
        })
    }

    #[cfg(all(target_os = "linux", feature = "bluez"))]
    pub(crate) fn try_admit_global(&self) -> AnyResult<Option<ClientOperationGuard>> {
        let Ok(guard) = self.scope.clone().try_write_owned() else {
            return Ok(None);
        };
        Ok(Some(ClientOperationGuard::Global { _scope: guard }))
    }

    #[cfg(all(target_os = "linux", feature = "bluez"))]
    pub(crate) fn try_admit_device(&self, key: &str) -> AnyResult<Option<ClientOperationGuard>> {
        let lane = self.lanes.lane(key)?;
        let Ok(lane) = lane.try_lock_owned() else {
            return Ok(None);
        };
        let Ok(scope) = self.scope.clone().try_read_owned() else {
            return Ok(None);
        };
        Ok(Some(ClientOperationGuard::Device {
            _scope: scope,
            _lane: lane,
        }))
    }

    pub(crate) async fn drain(&self, timeout: Duration) -> AnyResult<()> {
        let _exclusive = tokio::time::timeout(timeout, self.scope.clone().write_owned())
            .await
            .context("timed out draining Bluetooth client device operations")?;
        Ok(())
    }
}

impl Default for ClientOperationCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

/// Bounded admission for independent device operations plus an exclusive
/// drain used when an out-of-process commissioner must own the adapter.
///
/// Keeping this primitive independent from BlueZ lets the fake-runtime suite
/// exercise the same permit semantics on every development platform.
#[derive(Clone)]
pub(crate) struct AdapterOperationGate {
    semaphore: Arc<Semaphore>,
    #[cfg(all(target_os = "linux", feature = "bluez"))]
    permits: u32,
}

impl AdapterOperationGate {
    pub(crate) fn new(permits: u32) -> Self {
        assert!(permits > 0, "adapter operation gate requires a permit");
        Self {
            semaphore: Arc::new(Semaphore::new(permits as usize)),
            #[cfg(all(target_os = "linux", feature = "bluez"))]
            permits,
        }
    }

    pub(crate) async fn acquire(&self) -> Result<OwnedSemaphorePermit, AcquireError> {
        self.semaphore.clone().acquire_owned().await
    }

    #[cfg(all(target_os = "linux", feature = "bluez"))]
    pub(crate) fn try_acquire(&self) -> Result<OwnedSemaphorePermit, TryAcquireError> {
        self.semaphore.clone().try_acquire_owned()
    }

    #[cfg(all(target_os = "linux", feature = "bluez"))]
    pub(crate) async fn acquire_exclusive(&self) -> Result<OwnedSemaphorePermit, AcquireError> {
        self.semaphore
            .clone()
            .acquire_many_owned(self.permits)
            .await
    }
}

/// Per-device serialization lanes shared by every client of one protocol
/// driver. Equal keys are ordered; unrelated devices remain independently
/// admissible through the shared adapter gate.
///
/// Weak entries keep a long-lived driver from accumulating every address it
/// has ever seen. An operation or waiter owns the strong reference while its
/// lane is relevant.
pub(crate) struct DeviceOperationLanes {
    lanes: Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
}

impl DeviceOperationLanes {
    pub(crate) fn new() -> Self {
        Self {
            lanes: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn lane(&self, key: &str) -> AnyResult<Arc<tokio::sync::Mutex<()>>> {
        if key.is_empty()
            || key.len() > 128
            || !key.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b':' | b'-' | b'_')
            })
        {
            anyhow::bail!("invalid Bluetooth operation lane key");
        }
        let mut lanes = self
            .lanes
            .lock()
            .map_err(|_| anyhow::anyhow!("Bluetooth operation lane map poisoned"))?;
        lanes.retain(|_, lane| lane.strong_count() > 0);
        if let Some(lane) = lanes.get(key).and_then(Weak::upgrade) {
            return Ok(lane);
        }
        let lane = Arc::new(tokio::sync::Mutex::new(()));
        lanes.insert(key.to_string(), Arc::downgrade(&lane));
        Ok(lane)
    }
}

impl Default for DeviceOperationLanes {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn equal_device_keys_share_a_lane_while_other_devices_do_not() {
        let lanes = DeviceOperationLanes::new();
        let first = lanes.lane("device-1").unwrap();
        let same = lanes.lane("device-1").unwrap();
        let other = lanes.lane("device-2").unwrap();

        assert!(Arc::ptr_eq(&first, &same));
        assert!(!Arc::ptr_eq(&first, &other));
        let _guard = first.lock_owned().await;
        assert!(same.try_lock_owned().is_err());
        assert!(other.try_lock_owned().is_ok());
    }

    #[tokio::test]
    async fn queued_global_work_precedes_a_same_device_fifo_backlog() {
        let coordinator = Arc::new(ClientOperationCoordinator::new());
        let device_key = "device-1";
        let deadline = || tokio::time::Instant::now() + Duration::from_secs(1);
        let active = coordinator
            .admit_device(device_key, deadline())
            .await
            .unwrap();
        let lane_probe = coordinator.lanes.lane(device_key).unwrap();
        let order = Arc::new(Mutex::new(Vec::new()));
        let (global_acquired_tx, global_acquired_rx) = oneshot::channel();
        let (release_global_tx, release_global_rx) = oneshot::channel();

        let global_coordinator = coordinator.clone();
        let global_order = order.clone();
        let global = tokio::spawn(async move {
            let _guard = global_coordinator.admit_global(deadline()).await.unwrap();
            global_order.lock().unwrap().push("global");
            let _ = global_acquired_tx.send(());
            release_global_rx.await.unwrap();
        });

        // An awaiting writer makes Tokio's fair RwLock reject new readers.
        // Wait for that state before queueing the per-device backlog.
        loop {
            match coordinator.scope.clone().try_read_owned() {
                Ok(guard) => {
                    drop(guard);
                    tokio::task::yield_now().await;
                }
                Err(_) => break,
            }
        }

        let first_coordinator = coordinator.clone();
        let first_order = order.clone();
        let first = tokio::spawn(async move {
            let _guard = first_coordinator
                .admit_device(device_key, deadline())
                .await
                .unwrap();
            first_order.lock().unwrap().push("first");
        });
        while Arc::strong_count(&lane_probe) < 3 {
            tokio::task::yield_now().await;
        }

        let second_coordinator = coordinator.clone();
        let second_order = order.clone();
        let second = tokio::spawn(async move {
            let _guard = second_coordinator
                .admit_device(device_key, deadline())
                .await
                .unwrap();
            second_order.lock().unwrap().push("second");
        });
        while Arc::strong_count(&lane_probe) < 4 {
            tokio::task::yield_now().await;
        }

        drop(active);
        global_acquired_rx.await.unwrap();
        assert_eq!(*order.lock().unwrap(), ["global"]);
        release_global_tx.send(()).unwrap();
        global.await.unwrap();
        first.await.unwrap();
        second.await.unwrap();
        assert_eq!(*order.lock().unwrap(), ["global", "first", "second"]);
    }
}
