//! Reusable deterministic seam for mixed local-BLE conformance tests.
//!
//! The fake runtime owns one session/scanner identity, the same bounded
//! adapter admission primitive as the Linux BlueZ runtime, and one shared
//! advertisement broker. Protocol drivers supply their own fake device state
//! on top of `SharedBleTestClient::run_bounded`.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;

use tokio::sync::broadcast;
#[cfg(test)]
use tokio::sync::Mutex;

use crate::coordination::{AdapterOperationGate, ClientOperationCoordinator};

const TEST_BROKER_CAPACITY: usize = 64;
static NEXT_RUNTIME_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SharedBleOwnerIds {
    pub runtime: u64,
    pub session: u64,
    pub scanner: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestAdvertisementKind {
    Button { logical_sequence: u64 },
    StatefulBulb { state_revision: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TestAdvertisement {
    pub broker_sequence: u64,
    pub kind: TestAdvertisementKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestRuntimeError {
    AdmissionTimeout,
    OperationTimeout,
    Operation(String),
    ObservationLagged(u64),
    ObservationClosed,
}

impl fmt::Display for TestRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AdmissionTimeout => formatter.write_str("adapter admission timed out"),
            Self::OperationTimeout => formatter.write_str("device operation timed out"),
            Self::Operation(error) => formatter.write_str(error),
            Self::ObservationLagged(count) => {
                write!(formatter, "observation subscriber lagged by {count}")
            }
            Self::ObservationClosed => formatter.write_str("observation broker closed"),
        }
    }
}

impl std::error::Error for TestRuntimeError {}

struct SharedBleTestRuntimeInner {
    owner_ids: SharedBleOwnerIds,
    gate: AdapterOperationGate,
    client_operations: StdMutex<HashMap<&'static str, Weak<ClientOperationCoordinator>>>,
    observation_tx: broadcast::Sender<TestAdvertisement>,
    next_broker_sequence: AtomicU64,
}

/// One fake session/scanner owner shared by event profiles and stateful
/// device drivers. Clones and clients retain the same owner identities.
#[derive(Clone)]
pub struct SharedBleTestRuntime {
    inner: Arc<SharedBleTestRuntimeInner>,
}

impl SharedBleTestRuntime {
    pub fn new(adapter_operation_permits: u32) -> Self {
        let runtime = NEXT_RUNTIME_ID.fetch_add(1, Ordering::Relaxed);
        let (observation_tx, _) = broadcast::channel(TEST_BROKER_CAPACITY);
        Self {
            inner: Arc::new(SharedBleTestRuntimeInner {
                owner_ids: SharedBleOwnerIds {
                    runtime,
                    session: runtime,
                    scanner: runtime,
                },
                gate: AdapterOperationGate::new(adapter_operation_permits),
                client_operations: StdMutex::new(HashMap::new()),
                observation_tx,
                next_broker_sequence: AtomicU64::new(1),
            }),
        }
    }

    pub fn owner_ids(&self) -> SharedBleOwnerIds {
        self.inner.owner_ids
    }

    pub fn client(&self, name: &'static str) -> SharedBleTestClient {
        let coordinator = {
            let mut clients = self
                .inner
                .client_operations
                .lock()
                .expect("fake Bluetooth client coordinator map is not poisoned");
            clients.retain(|_, coordinator| coordinator.strong_count() > 0);
            if let Some(coordinator) = clients.get(name).and_then(Weak::upgrade) {
                coordinator
            } else {
                let coordinator = Arc::new(ClientOperationCoordinator::new());
                clients.insert(name, Arc::downgrade(&coordinator));
                coordinator
            }
        };
        SharedBleTestClient {
            name,
            runtime: self.inner.clone(),
            coordinator,
            quiescing: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn subscribe(&self) -> TestScanSubscription {
        TestScanSubscription {
            receiver: self.inner.observation_tx.subscribe(),
        }
    }

    pub fn advertise(&self, kind: TestAdvertisementKind) -> Result<u64, TestRuntimeError> {
        let broker_sequence = self
            .inner
            .next_broker_sequence
            .fetch_add(1, Ordering::Relaxed);
        self.inner
            .observation_tx
            .send(TestAdvertisement {
                broker_sequence,
                kind,
            })
            .map_err(|_| TestRuntimeError::ObservationClosed)?;
        Ok(broker_sequence)
    }
}

#[derive(Clone)]
pub struct SharedBleTestClient {
    name: &'static str,
    runtime: Arc<SharedBleTestRuntimeInner>,
    coordinator: Arc<ClientOperationCoordinator>,
    quiescing: Arc<AtomicBool>,
}

impl SharedBleTestClient {
    fn ensure_active(&self) -> Result<(), TestRuntimeError> {
        if self.quiescing.load(Ordering::Acquire) {
            return Err(TestRuntimeError::Operation(
                "Bluetooth client is quiesced for appliance shutdown".to_string(),
            ));
        }
        Ok(())
    }

    fn admission_error(error: anyhow::Error) -> TestRuntimeError {
        let message = error.to_string();
        if message.contains("timed out") {
            TestRuntimeError::AdmissionTimeout
        } else {
            TestRuntimeError::Operation(message)
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn owner_ids(&self) -> SharedBleOwnerIds {
        self.runtime.owner_ids
    }

    /// Run one driver-wide fake transaction, exclusive with every device lane
    /// owned by same-named clients of this runtime.
    pub async fn run_global_bounded<T, F, Fut>(
        &self,
        admission_timeout: Duration,
        operation_timeout: Duration,
        operation: F,
    ) -> Result<T, TestRuntimeError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, String>>,
    {
        self.ensure_active()?;
        let admission_deadline = tokio::time::Instant::now() + admission_timeout;
        let _scope = self
            .coordinator
            .admit_global(admission_deadline)
            .await
            .map_err(Self::admission_error)?;
        self.ensure_active()?;
        self.run_admitted(admission_deadline, operation_timeout, operation)
            .await
    }

    /// Run one fake device transaction through the same keyed-lane and adapter
    /// permit primitives as production. Equal device keys are ordered across
    /// same-named client handles; unrelated devices remain independent.
    pub async fn run_bounded_for<T, F, Fut>(
        &self,
        device_key: &str,
        admission_timeout: Duration,
        operation_timeout: Duration,
        operation: F,
    ) -> Result<T, TestRuntimeError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, String>>,
    {
        self.ensure_active()?;
        let admission_deadline = tokio::time::Instant::now() + admission_timeout;
        let _scope = self
            .coordinator
            .admit_device(device_key, admission_deadline)
            .await
            .map_err(Self::admission_error)?;
        self.ensure_active()?;
        self.run_admitted(admission_deadline, operation_timeout, operation)
            .await
    }

    async fn run_admitted<T, F, Fut>(
        &self,
        admission_deadline: tokio::time::Instant,
        operation_timeout: Duration,
        operation: F,
    ) -> Result<T, TestRuntimeError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, String>>,
    {
        let _permit = tokio::time::timeout_at(admission_deadline, self.runtime.gate.acquire())
            .await
            .map_err(|_| TestRuntimeError::AdmissionTimeout)?
            .map_err(|_| TestRuntimeError::Operation("adapter gate closed".to_string()))?;
        self.ensure_active()?;
        tokio::time::timeout(operation_timeout, operation())
            .await
            .map_err(|_| TestRuntimeError::OperationTimeout)?
            .map_err(TestRuntimeError::Operation)
    }

    /// Reject new same-driver work and wait for every admitted device lane to
    /// drain under one deadline.
    pub async fn quiesce(&self, timeout: Duration) -> Result<(), TestRuntimeError> {
        self.quiescing.store(true, Ordering::SeqCst);
        self.coordinator
            .drain(timeout)
            .await
            .map_err(|_| TestRuntimeError::OperationTimeout)
    }
}

pub struct TestScanSubscription {
    receiver: broadcast::Receiver<TestAdvertisement>,
}

impl TestScanSubscription {
    pub async fn recv(&mut self) -> Result<TestAdvertisement, TestRuntimeError> {
        match self.receiver.recv().await {
            Ok(observation) => Ok(observation),
            Err(broadcast::error::RecvError::Lagged(count)) => {
                Err(TestRuntimeError::ObservationLagged(count))
            }
            Err(broadcast::error::RecvError::Closed) => Err(TestRuntimeError::ObservationClosed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use tokio::sync::{broadcast, oneshot};

    const ADMISSION_BUDGET: Duration = Duration::from_millis(40);
    const OPERATION_BUDGET: Duration = Duration::from_millis(200);

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum BulbCommand {
        Power(bool),
        Brightness(u8),
        ColorTemperature(u16),
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct BulbState {
        paired: bool,
        connected: bool,
        notifications_enabled: bool,
        power: bool,
        brightness: u8,
        color_temperature: u16,
        revision: u64,
        applied_commands: Vec<BulbCommand>,
    }

    impl Default for BulbState {
        fn default() -> Self {
            Self {
                paired: false,
                connected: false,
                notifications_enabled: false,
                power: false,
                brightness: 0,
                color_temperature: 2700,
                revision: 0,
                applied_commands: Vec::new(),
            }
        }
    }

    #[derive(Clone)]
    struct FakeStatefulBulb {
        client: SharedBleTestClient,
        device_key: &'static str,
        state: Arc<Mutex<BulbState>>,
        notification_tx: broadcast::Sender<BulbState>,
    }

    impl FakeStatefulBulb {
        fn new(runtime: &SharedBleTestRuntime, device_key: &'static str) -> Self {
            let (notification_tx, _) = broadcast::channel(8);
            Self {
                client: runtime.client("stateful_bulb"),
                device_key,
                state: Arc::new(Mutex::new(BulbState::default())),
                notification_tx,
            }
        }

        async fn pair(&self) -> Result<(), TestRuntimeError> {
            let state = self.state.clone();
            self.client
                .run_global_bounded(ADMISSION_BUDGET, OPERATION_BUDGET, move || async move {
                    let mut state = state.lock().await;
                    state.paired = true;
                    state.connected = true;
                    Ok(())
                })
                .await
        }

        async fn subscribe_notifications(
            &self,
        ) -> Result<broadcast::Receiver<BulbState>, TestRuntimeError> {
            let state = self.state.clone();
            let receiver = self.notification_tx.subscribe();
            self.client
                .run_bounded_for(
                    self.device_key,
                    ADMISSION_BUDGET,
                    OPERATION_BUDGET,
                    move || async move {
                        let mut state = state.lock().await;
                        if !state.connected {
                            return Err("bulb is disconnected".to_string());
                        }
                        state.notifications_enabled = true;
                        Ok(receiver)
                    },
                )
                .await
        }

        async fn read(&self) -> Result<BulbState, TestRuntimeError> {
            let state = self.state.clone();
            self.client
                .run_bounded_for(
                    self.device_key,
                    ADMISSION_BUDGET,
                    OPERATION_BUDGET,
                    move || async move {
                        let state = state.lock().await;
                        if !state.connected {
                            return Err("bulb is disconnected".to_string());
                        }
                        Ok(state.clone())
                    },
                )
                .await
        }

        async fn write_ordered_after(
            &self,
            commands: Vec<BulbCommand>,
            started: oneshot::Sender<()>,
            release: oneshot::Receiver<()>,
        ) -> Result<BulbState, TestRuntimeError> {
            let state = self.state.clone();
            let notification_tx = self.notification_tx.clone();
            self.client
                .run_bounded_for(
                    self.device_key,
                    ADMISSION_BUDGET,
                    OPERATION_BUDGET,
                    move || async move {
                        let _ = started.send(());
                        release
                            .await
                            .map_err(|_| "write release dropped".to_string())?;
                        let mut state = state.lock().await;
                        if !state.connected {
                            return Err("bulb is disconnected".to_string());
                        }
                        for command in commands {
                            match command {
                                BulbCommand::Power(power) => state.power = power,
                                BulbCommand::Brightness(brightness) => {
                                    state.brightness = brightness
                                }
                                BulbCommand::ColorTemperature(color_temperature) => {
                                    state.color_temperature = color_temperature
                                }
                            }
                            state.applied_commands.push(command);
                        }
                        state.revision += 1;
                        let observed = state.clone();
                        if state.notifications_enabled {
                            let _ = notification_tx.send(observed.clone());
                        }
                        Ok(observed)
                    },
                )
                .await
        }

        async fn set_connected(&self, connected: bool) -> Result<(), TestRuntimeError> {
            let state = self.state.clone();
            self.client
                .run_bounded_for(
                    self.device_key,
                    ADMISSION_BUDGET,
                    OPERATION_BUDGET,
                    move || async move {
                        let mut state = state.lock().await;
                        if !state.paired {
                            return Err("bulb is not paired".to_string());
                        }
                        state.connected = connected;
                        Ok(())
                    },
                )
                .await
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct MixedWorkloadOutcome {
        buttons: Vec<u64>,
        raw_buttons: Vec<u64>,
        bulb_revisions: Vec<u64>,
        final_bulb_state: BulbState,
    }

    async fn run_mixed_workload() -> MixedWorkloadOutcome {
        let runtime = SharedBleTestRuntime::new(2);
        let bulb = FakeStatefulBulb::new(&runtime, "0A:0B:0C:0D:0E:01");
        // A separately constructed handle for the same rich driver must share
        // its keyed coordinator, while this second bulb keeps its own lane.
        let isolated = runtime.client("stateful_bulb");
        let isolated_device_key = "0A:0B:0C:0D:0E:02";
        let owners = runtime.owner_ids();
        assert_eq!(bulb.client.owner_ids(), owners);
        assert_eq!(isolated.owner_ids(), owners);
        assert_eq!(owners.runtime, owners.session);
        assert_eq!(owners.runtime, owners.scanner);

        let mut observations = runtime.subscribe();
        let observation_task = tokio::spawn(async move {
            let mut buttons = Vec::new();
            let mut raw_buttons = Vec::new();
            let mut seen_button_sequences = HashSet::new();
            let mut bulb_revisions = Vec::new();
            for expected_broker_sequence in 1..=12 {
                let observation = observations.recv().await.unwrap();
                assert_eq!(observation.broker_sequence, expected_broker_sequence);
                match observation.kind {
                    TestAdvertisementKind::Button { logical_sequence } => {
                        raw_buttons.push(logical_sequence);
                        if seen_button_sequences.insert(logical_sequence) {
                            buttons.push(logical_sequence);
                        }
                    }
                    TestAdvertisementKind::StatefulBulb { state_revision } => {
                        bulb_revisions.push(state_revision)
                    }
                }
            }
            (buttons, raw_buttons, bulb_revisions)
        });

        runtime
            .advertise(TestAdvertisementKind::Button {
                logical_sequence: 1,
            })
            .unwrap();
        tokio::time::timeout(OPERATION_BUDGET, bulb.pair())
            .await
            .unwrap()
            .unwrap();
        runtime
            .advertise(TestAdvertisementKind::StatefulBulb { state_revision: 0 })
            .unwrap();
        runtime
            .advertise(TestAdvertisementKind::Button {
                logical_sequence: 2,
            })
            .unwrap();

        let mut notifications = bulb.subscribe_notifications().await.unwrap();
        assert!(bulb.read().await.unwrap().connected);

        let commands = vec![
            BulbCommand::Power(true),
            BulbCommand::Brightness(73),
            BulbCommand::ColorTemperature(3200),
        ];
        let (write_started_tx, write_started_rx) = oneshot::channel();
        let (write_release_tx, write_release_rx) = oneshot::channel();
        let writing_bulb = bulb.clone();
        let expected_commands = commands.clone();
        let write_task = tokio::spawn(async move {
            writing_bulb
                .write_ordered_after(commands, write_started_tx, write_release_rx)
                .await
        });
        write_started_rx.await.unwrap();

        runtime
            .advertise(TestAdvertisementKind::Button {
                logical_sequence: 3,
            })
            .unwrap();
        // The fake profile side sees the same replay token twice while the
        // bulb transaction is in flight and must normalize it exactly once.
        runtime
            .advertise(TestAdvertisementKind::Button {
                logical_sequence: 3,
            })
            .unwrap();
        runtime
            .advertise(TestAdvertisementKind::Button {
                logical_sequence: 4,
            })
            .unwrap();
        tokio::task::yield_now().await;
        assert!(!write_task.is_finished());
        write_release_tx.send(()).unwrap();
        let written = write_task.await.unwrap().unwrap();
        assert_eq!(written.applied_commands, expected_commands);
        assert_eq!((written.power, written.brightness), (true, 73));
        assert_eq!(written.color_temperature, 3200);
        assert_eq!(notifications.recv().await.unwrap(), written);
        runtime
            .advertise(TestAdvertisementKind::StatefulBulb {
                state_revision: written.revision,
            })
            .unwrap();
        runtime
            .advertise(TestAdvertisementKind::Button {
                logical_sequence: 5,
            })
            .unwrap();

        bulb.set_connected(false).await.unwrap();
        assert!(matches!(
            bulb.read().await,
            Err(TestRuntimeError::Operation(ref error)) if error == "bulb is disconnected"
        ));
        runtime
            .advertise(TestAdvertisementKind::Button {
                logical_sequence: 6,
            })
            .unwrap();
        bulb.set_connected(true).await.unwrap();
        runtime
            .advertise(TestAdvertisementKind::StatefulBulb {
                state_revision: written.revision,
            })
            .unwrap();
        assert_eq!(bulb.read().await.unwrap(), written);

        let (stuck_started_tx, stuck_started_rx) = oneshot::channel();
        let (stuck_release_tx, stuck_release_rx) = oneshot::channel();
        let stuck_client = bulb.client.clone();
        let stuck_task = tokio::spawn(async move {
            stuck_client
                .run_bounded_for(
                    "0A:0B:0C:0D:0E:01",
                    ADMISSION_BUDGET,
                    Duration::from_secs(1),
                    move || async move {
                        let _ = stuck_started_tx.send(());
                        stuck_release_rx
                            .await
                            .map_err(|_| "stuck release dropped".to_string())?;
                        Ok(())
                    },
                )
                .await
        });
        stuck_started_rx.await.unwrap();
        runtime
            .advertise(TestAdvertisementKind::Button {
                logical_sequence: 7,
            })
            .unwrap();

        assert_eq!(
            bulb.read().await.unwrap_err(),
            TestRuntimeError::AdmissionTimeout
        );
        let isolated_result = isolated
            .run_bounded_for(
                isolated_device_key,
                ADMISSION_BUDGET,
                OPERATION_BUDGET,
                || async { Ok::<_, String>("isolated-ok") },
            )
            .await
            .unwrap();
        assert_eq!(isolated_result, "isolated-ok");
        runtime
            .advertise(TestAdvertisementKind::Button {
                logical_sequence: 8,
            })
            .unwrap();

        stuck_release_tx.send(()).unwrap();
        stuck_task.await.unwrap().unwrap();
        assert_eq!(bulb.read().await.unwrap(), written);

        let (buttons, raw_buttons, bulb_revisions) =
            tokio::time::timeout(OPERATION_BUDGET, observation_task)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(buttons, (1..=8).collect::<Vec<_>>());
        assert_eq!(raw_buttons, vec![1, 2, 3, 3, 4, 5, 6, 7, 8]);
        assert_eq!(bulb_revisions, vec![0, 1, 1]);

        MixedWorkloadOutcome {
            buttons,
            raw_buttons,
            bulb_revisions,
            final_bulb_state: written,
        }
    }

    #[tokio::test]
    async fn operation_timeout_releases_same_device_lane_and_adapter_permit() {
        let runtime = SharedBleTestRuntime::new(1);
        let first = runtime.client("stateful_bulb");
        let second = runtime.client("stateful_bulb");
        let device_key = "0A:0B:0C:0D:0E:03";

        let error = first
            .run_bounded_for(
                device_key,
                ADMISSION_BUDGET,
                Duration::from_millis(5),
                || async {
                    std::future::pending::<()>().await;
                    Ok::<_, String>(())
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error, TestRuntimeError::OperationTimeout);

        assert_eq!(
            second
                .run_bounded_for(device_key, ADMISSION_BUDGET, OPERATION_BUDGET, || async {
                    Ok::<_, String>("recovered")
                },)
                .await
                .unwrap(),
            "recovered"
        );
    }

    #[tokio::test]
    async fn driver_global_work_is_exclusive_with_its_device_lanes() {
        let runtime = SharedBleTestRuntime::new(2);
        let device_client = runtime.client("stateful_bulb");
        let global_client = runtime.client("stateful_bulb");
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();

        let held = tokio::spawn(async move {
            device_client
                .run_bounded_for(
                    "0A:0B:0C:0D:0E:04",
                    ADMISSION_BUDGET,
                    OPERATION_BUDGET,
                    move || async move {
                        let _ = started_tx.send(());
                        release_rx
                            .await
                            .map_err(|_| "device release dropped".to_string())?;
                        Ok(())
                    },
                )
                .await
        });
        started_rx.await.unwrap();

        assert_eq!(
            global_client
                .run_global_bounded(Duration::from_millis(5), OPERATION_BUDGET, || async {
                    Ok::<_, String>(())
                },)
                .await
                .unwrap_err(),
            TestRuntimeError::AdmissionTimeout
        );

        release_tx.send(()).unwrap();
        held.await.unwrap().unwrap();
        global_client
            .run_global_bounded(ADMISSION_BUDGET, OPERATION_BUDGET, || async {
                Ok::<_, String>(())
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn quiesce_drains_all_device_lanes_and_rejects_new_work() {
        let runtime = SharedBleTestRuntime::new(2);
        let first = runtime.client("stateful_bulb");
        let second = runtime.client("stateful_bulb");
        let quiescing = runtime.client("stateful_bulb");
        let (first_started_tx, first_started_rx) = oneshot::channel();
        let (first_release_tx, first_release_rx) = oneshot::channel();
        let (second_started_tx, second_started_rx) = oneshot::channel();
        let (second_release_tx, second_release_rx) = oneshot::channel();

        let first_task = tokio::spawn(async move {
            first
                .run_bounded_for(
                    "0A:0B:0C:0D:0E:05",
                    ADMISSION_BUDGET,
                    OPERATION_BUDGET,
                    move || async move {
                        let _ = first_started_tx.send(());
                        first_release_rx
                            .await
                            .map_err(|_| "first release dropped".to_string())?;
                        Ok(())
                    },
                )
                .await
        });
        let second_task = tokio::spawn(async move {
            second
                .run_bounded_for(
                    "0A:0B:0C:0D:0E:06",
                    ADMISSION_BUDGET,
                    OPERATION_BUDGET,
                    move || async move {
                        let _ = second_started_tx.send(());
                        second_release_rx
                            .await
                            .map_err(|_| "second release dropped".to_string())?;
                        Ok(())
                    },
                )
                .await
        });
        first_started_rx.await.unwrap();
        second_started_rx.await.unwrap();

        assert_eq!(
            quiescing
                .quiesce(Duration::from_millis(5))
                .await
                .unwrap_err(),
            TestRuntimeError::OperationTimeout
        );
        assert!(matches!(
            quiescing
                .run_bounded_for(
                    "0A:0B:0C:0D:0E:07",
                    ADMISSION_BUDGET,
                    OPERATION_BUDGET,
                    || async { Ok::<_, String>(()) },
                )
                .await,
            Err(TestRuntimeError::Operation(ref error)) if error.contains("quiesced")
        ));

        first_release_tx.send(()).unwrap();
        second_release_tx.send(()).unwrap();
        first_task.await.unwrap().unwrap();
        second_task.await.unwrap().unwrap();
        quiescing.quiesce(ADMISSION_BUDGET).await.unwrap();

        // Factory reset intentionally keeps the old quiesced Hue transport
        // alive while a fresh transport performs the final bond handoff. The
        // new handle shares ordering lanes, but not the old handle's terminal
        // lifecycle state.
        let final_handoff = runtime.client("stateful_bulb");
        final_handoff
            .run_global_bounded(ADMISSION_BUDGET, OPERATION_BUDGET, || async {
                Ok::<_, String>(())
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn mixed_button_and_stateful_bulb_share_one_runtime_with_bounded_isolation() {
        let first = run_mixed_workload().await;
        let second = run_mixed_workload().await;

        assert_eq!(first, second);
    }
}
