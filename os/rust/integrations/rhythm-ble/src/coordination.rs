//! Platform-neutral adapter admission shared by the BlueZ owner and the
//! deterministic mixed-device conformance harness.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use anyhow::{Context, Result as AnyResult};
use futures::future::join_all;
#[cfg(all(target_os = "linux", feature = "bluez"))]
use tokio::sync::TryAcquireError;
use tokio::sync::{AcquireError, OwnedSemaphorePermit, Semaphore};

pub(crate) enum ClientOperationGuard {
    Shared {
        _scope: tokio::sync::OwnedRwLockReadGuard<()>,
    },
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
    uncertain_devices: Mutex<HashSet<String>>,
}

impl ClientOperationCoordinator {
    pub(crate) fn new() -> Self {
        Self {
            scope: Arc::new(tokio::sync::RwLock::new(())),
            lanes: DeviceOperationLanes::new(),
            uncertain_devices: Mutex::new(HashSet::new()),
        }
    }

    pub(crate) async fn admit_shared(
        &self,
        deadline: tokio::time::Instant,
    ) -> AnyResult<ClientOperationGuard> {
        let scope = tokio::time::timeout_at(deadline, self.scope.clone().read_owned())
            .await
            .context("timed out waiting for shared Bluetooth client admission")?;
        Ok(ClientOperationGuard::Shared { _scope: scope })
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
        // Queue the stable-device lane before the driver scope so calls for
        // one identity retain FIFO order across global work. Keyed batches do
        // not retain a batch-wide scope while waiting for lanes, so a queued
        // global writer cannot form a scope/lane cycle with this order.
        let scope = tokio::time::timeout_at(deadline, self.scope.clone().read_owned())
            .await
            .context("timed out waiting for shared Bluetooth client admission")?;
        Ok(ClientOperationGuard::Device {
            _scope: scope,
            _lane: lane,
        })
    }

    pub(crate) fn mark_device_uncertain(&self, key: &str) -> AnyResult<()> {
        let _ = self.lanes.lane(key)?;
        self.uncertain_devices
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.to_string());
        Ok(())
    }

    pub(crate) fn clear_device_uncertain(&self, key: &str) -> AnyResult<()> {
        let _ = self.lanes.lane(key)?;
        self.uncertain_devices
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(key);
        Ok(())
    }

    pub(crate) fn device_is_uncertain(&self, key: &str) -> AnyResult<bool> {
        let _ = self.lanes.lane(key)?;
        Ok(self
            .uncertain_devices
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(key))
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

#[derive(Debug)]
pub struct BluezOperationDeadlineExceeded;

impl std::fmt::Display for BluezOperationDeadlineExceeded {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Bluetooth device operation deadline expired")
    }
}

impl std::error::Error for BluezOperationDeadlineExceeded {}

impl Default for ClientOperationCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

/// Validate one shared adapter context under ordinary admission, release the
/// validation guards, and only then fan out independently admitted keyed
/// operations. Keeping this composition portable lets the conformance suite
/// prove that neither the client scope nor one adapter permit leaks across the
/// fan-out boundary.
pub(crate) async fn run_validated_device_operation_batch<I, C, T, V, VFut, F, Fut>(
    coordinator: &ClientOperationCoordinator,
    adapter_gate: &AdapterOperationGate,
    deadline: tokio::time::Instant,
    validate: V,
    operations: Vec<(String, I)>,
    operation: F,
) -> AnyResult<Vec<(String, AnyResult<T>)>>
where
    C: Clone,
    V: FnOnce() -> VFut,
    VFut: Future<Output = AnyResult<C>>,
    F: Fn(I, C) -> Fut,
    Fut: Future<Output = AnyResult<T>>,
{
    let context = {
        let _validation_scope = coordinator.admit_shared(deadline).await?;
        let _validation_permit = tokio::time::timeout_at(deadline, adapter_gate.acquire())
            .await
            .context("Bluetooth device batch deadline expired during adapter admission")?
            .map_err(|_| anyhow::anyhow!("shared Bluetooth operation gate closed"))?;
        tokio::time::timeout_at(deadline, validate())
            .await
            .context("Bluetooth device batch deadline expired during adapter validation")??
    };
    Ok(run_device_operation_batch(
        coordinator,
        adapter_gate,
        deadline,
        operations,
        move |input| operation(input, context.clone()),
    )
    .await)
}

/// Poll a set of stable-device operations together while retaining the normal
/// per-device lanes, driver scope, and shared adapter-permit bound. There is no
/// batch-wide client-scope guard: each key admits independently, which avoids
/// coupling ready bulbs to a busy sibling and preserves global-writer
/// progress. Distinct keys become independently runnable in the same executor
/// turn; duplicate keys remain serialized.
pub(crate) async fn run_device_operation_batch<I, T, F, Fut>(
    coordinator: &ClientOperationCoordinator,
    adapter_gate: &AdapterOperationGate,
    deadline: tokio::time::Instant,
    operations: Vec<(String, I)>,
    operation: F,
) -> Vec<(String, AnyResult<T>)>
where
    F: Fn(I) -> Fut,
    Fut: Future<Output = AnyResult<T>>,
{
    let operation = &operation;
    join_all(operations.into_iter().map(|(key, input)| async move {
        let result = async {
            let _lane = coordinator.admit_device(&key, deadline).await?;
            let _permit = tokio::time::timeout_at(deadline, adapter_gate.acquire())
                .await
                .context("Bluetooth device operation deadline expired during adapter admission")?
                .map_err(|_| anyhow::anyhow!("shared Bluetooth operation gate closed"))?;
            tokio::time::timeout_at(deadline, operation(input))
                .await
                .context("Bluetooth device operation deadline expired")?
        }
        .await;
        let result = match result {
            Err(error)
                if tokio::time::Instant::now() >= deadline
                    && !error.is::<BluezOperationDeadlineExceeded>() =>
            {
                Err(error.context(BluezOperationDeadlineExceeded))
            }
            result => result,
        };
        (key, result)
    }))
    .await
}

/// Bounded admission for independent device operations plus an exclusive
/// drain used when an out-of-process commissioner must own the adapter.
///
/// Keeping this primitive independent from BlueZ lets the fake-runtime suite
/// exercise the same permit semantics on every development platform.
#[derive(Clone)]
pub(crate) struct AdapterOperationGate {
    semaphore: Arc<Semaphore>,
    permits: u32,
}

impl AdapterOperationGate {
    pub(crate) fn new(permits: u32) -> Self {
        assert!(permits > 0, "adapter operation gate requires a permit");
        Self {
            semaphore: Arc::new(Semaphore::new(permits as usize)),
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
    use std::sync::atomic::{AtomicUsize, Ordering};
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
    async fn device_batch_starts_distinct_keys_before_any_finishes() {
        let coordinator = ClientOperationCoordinator::new();
        let adapter_gate = AdapterOperationGate::new(4);
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let started = Arc::new(AtomicUsize::new(0));
        let operations = vec![
            ("bulb-1".to_string(), 1usize),
            ("bulb-2".to_string(), 2usize),
            ("bulb-3".to_string(), 3usize),
        ];
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

        let outcomes = run_device_operation_batch(
            &coordinator,
            &adapter_gate,
            deadline,
            operations,
            |value| {
                let barrier = Arc::clone(&barrier);
                let started = Arc::clone(&started);
                async move {
                    started.fetch_add(1, Ordering::SeqCst);
                    barrier.wait().await;
                    Ok(value)
                }
            },
        )
        .await;

        assert_eq!(started.load(Ordering::SeqCst), 3);
        assert_eq!(
            outcomes
                .into_iter()
                .map(|(key, result)| (key, result.unwrap()))
                .collect::<Vec<_>>(),
            [
                ("bulb-1".to_string(), 1),
                ("bulb-2".to_string(), 2),
                ("bulb-3".to_string(), 3)
            ]
        );
    }

    #[tokio::test]
    async fn slow_first_device_does_not_delay_fast_sibling_completion() {
        let coordinator = Arc::new(ClientOperationCoordinator::new());
        let adapter_gate = AdapterOperationGate::new(4);
        let slow_started = Arc::new(tokio::sync::Notify::new());
        let slow_release = Arc::new(tokio::sync::Notify::new());
        let fast_finished = Arc::new(tokio::sync::Notify::new());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

        let batch_coordinator = Arc::clone(&coordinator);
        let batch_gate = adapter_gate.clone();
        let batch_slow_started = Arc::clone(&slow_started);
        let batch_slow_release = Arc::clone(&slow_release);
        let batch_fast_finished = Arc::clone(&fast_finished);
        let batch = tokio::spawn(async move {
            run_device_operation_batch(
                &batch_coordinator,
                &batch_gate,
                deadline,
                vec![
                    ("slow-bulb".to_string(), true),
                    ("fast-bulb".to_string(), false),
                ],
                move |slow| {
                    let slow_started = Arc::clone(&batch_slow_started);
                    let slow_release = Arc::clone(&batch_slow_release);
                    let fast_finished = Arc::clone(&batch_fast_finished);
                    async move {
                        if slow {
                            slow_started.notify_one();
                            slow_release.notified().await;
                            Ok("slow")
                        } else {
                            fast_finished.notify_one();
                            Ok("fast")
                        }
                    }
                },
            )
            .await
        });

        tokio::time::timeout(Duration::from_secs(1), slow_started.notified())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), fast_finished.notified())
            .await
            .unwrap();
        assert!(!batch.is_finished());

        slow_release.notify_one();
        let outcomes = batch.await.unwrap();
        assert_eq!(outcomes[0].1.as_ref().unwrap(), &"slow");
        assert_eq!(outcomes[1].1.as_ref().unwrap(), &"fast");
    }

    #[tokio::test]
    async fn validated_batch_releases_scope_before_waiting_for_a_device_lane() {
        let coordinator = Arc::new(ClientOperationCoordinator::new());
        let adapter_gate = AdapterOperationGate::new(4);
        let device_key = "busy-bulb";
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let active = coordinator
            .admit_device(device_key, deadline)
            .await
            .unwrap();
        let lane_probe = coordinator.lanes.lane(device_key).unwrap();
        let validation_finished = Arc::new(tokio::sync::Notify::new());
        let operation_started = Arc::new(AtomicUsize::new(0));

        let batch_coordinator = Arc::clone(&coordinator);
        let batch_gate = adapter_gate.clone();
        let batch_validation_finished = Arc::clone(&validation_finished);
        let batch_operation_started = Arc::clone(&operation_started);
        let batch = tokio::spawn(async move {
            run_validated_device_operation_batch(
                &batch_coordinator,
                &batch_gate,
                deadline,
                move || {
                    let validation_finished = Arc::clone(&batch_validation_finished);
                    async move {
                        validation_finished.notify_one();
                        Ok(())
                    }
                },
                vec![(device_key.to_string(), ())],
                move |(), ()| {
                    let operation_started = Arc::clone(&batch_operation_started);
                    async move {
                        operation_started.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    }
                },
            )
            .await
        });

        tokio::time::timeout(Duration::from_secs(1), validation_finished.notified())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while Arc::strong_count(&lane_probe) < 3 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let (global_acquired_tx, global_acquired_rx) = oneshot::channel();
        let (release_global_tx, release_global_rx) = oneshot::channel();
        let global_coordinator = Arc::clone(&coordinator);
        let global = tokio::spawn(async move {
            let _guard = global_coordinator.admit_global(deadline).await.unwrap();
            let _ = global_acquired_tx.send(());
            release_global_rx.await.unwrap();
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match coordinator.scope.clone().try_read_owned() {
                    Ok(guard) => {
                        drop(guard);
                        tokio::task::yield_now().await;
                    }
                    Err(_) => break,
                }
            }
        })
        .await
        .unwrap();

        drop(active);
        tokio::time::timeout(Duration::from_secs(1), global_acquired_rx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(operation_started.load(Ordering::SeqCst), 0);
        release_global_tx.send(()).unwrap();
        global.await.unwrap();
        batch.await.unwrap().unwrap();
        assert_eq!(operation_started.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn validated_batch_releases_validation_permit_for_queued_external_owner() {
        let coordinator = Arc::new(ClientOperationCoordinator::new());
        let adapter_gate = AdapterOperationGate::new(2);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let (validation_started_tx, validation_started_rx) = oneshot::channel();
        let (release_validation_tx, release_validation_rx) = oneshot::channel();
        let operation_started = Arc::new(AtomicUsize::new(0));

        let batch_coordinator = Arc::clone(&coordinator);
        let batch_gate = adapter_gate.clone();
        let batch_operation_started = Arc::clone(&operation_started);
        let batch = tokio::spawn(async move {
            run_validated_device_operation_batch(
                &batch_coordinator,
                &batch_gate,
                deadline,
                move || async move {
                    let _ = validation_started_tx.send(());
                    release_validation_rx.await.unwrap();
                    Ok(())
                },
                vec![("bulb-1".to_string(), ())],
                move |(), ()| {
                    let operation_started = Arc::clone(&batch_operation_started);
                    async move {
                        operation_started.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    }
                },
            )
            .await
        });

        validation_started_rx.await.unwrap();
        let external_owner = adapter_gate.acquire_exclusive();
        tokio::pin!(external_owner);
        assert!(matches!(
            futures::poll!(external_owner.as_mut()),
            std::task::Poll::Pending
        ));

        release_validation_tx.send(()).unwrap();
        let external_lease = tokio::time::timeout(Duration::from_secs(1), external_owner.as_mut())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(operation_started.load(Ordering::SeqCst), 0);
        drop(external_lease);

        batch.await.unwrap().unwrap();
        assert_eq!(operation_started.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn device_batch_marks_only_the_operation_that_reaches_its_deadline() {
        let coordinator = ClientOperationCoordinator::new();
        let adapter_gate = AdapterOperationGate::new(4);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(30);

        let outcomes = run_device_operation_batch(
            &coordinator,
            &adapter_gate,
            deadline,
            vec![("failed".to_string(), false), ("slow".to_string(), true)],
            |slow| async move {
                if slow {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    Ok(())
                } else {
                    anyhow::bail!("injected protocol failure")
                }
            },
        )
        .await;

        let failed = outcomes[0].1.as_ref().unwrap_err();
        let slow = outcomes[1].1.as_ref().unwrap_err();
        assert!(!failed.is::<BluezOperationDeadlineExceeded>());
        assert!(slow.is::<BluezOperationDeadlineExceeded>());
    }

    #[test]
    fn uncertain_device_fence_is_shared_and_explicitly_cleared() {
        let coordinator = ClientOperationCoordinator::new();
        assert!(!coordinator.device_is_uncertain("bulb-1").unwrap());
        coordinator.mark_device_uncertain("bulb-1").unwrap();
        assert!(coordinator.device_is_uncertain("bulb-1").unwrap());
        coordinator.clear_device_uncertain("bulb-1").unwrap();
        assert!(!coordinator.device_is_uncertain("bulb-1").unwrap());
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
