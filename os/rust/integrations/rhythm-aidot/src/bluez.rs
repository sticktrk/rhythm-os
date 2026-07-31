//! Linux BlueZ transport for QR-bound Orein/AiDot buttons.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result};
use bluer::{Adapter, AdapterEvent, DiscoveryFilter, DiscoveryTransport, Session};
use futures::StreamExt;
use rhythm_os::hub::HubEvent;
use rhythm_os::registry::HubDeviceRegistry;
use tokio::runtime::Runtime;
use uuid::Uuid;

use crate::protocol::{
    press_counter, service_data_identity, ADVERTISEMENT_SERVICE_UUID, ALTERNATE_GATT_SERVICE_UUID,
    MANUFACTURER_ID, PRIMARY_GATT_SERVICE_UUID,
};
use crate::store::AidotButtonStore;
use crate::transport::{accept_press_advertisement, AidotBleTransport, AidotPairingCandidate};
use crate::AidotSetupCode;

pub struct BluezAidotTransport {
    runtime: Mutex<Runtime>,
}

impl BluezAidotTransport {
    pub fn new() -> Result<Self> {
        Ok(Self {
            runtime: Mutex::new(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .thread_name("aidot-ble")
                    .enable_all()
                    .build()
                    .context("building AiDot Bluetooth runtime")?,
            ),
        })
    }

    fn block_on<T>(&self, future: impl std::future::Future<Output = Result<T>>) -> Result<T> {
        self.runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("AiDot Bluetooth runtime lock poisoned"))?
            .block_on(future)
    }

    async fn session_adapter() -> Result<(Session, Adapter)> {
        let session = Session::new().await.context("opening BlueZ session")?;
        let adapter = session
            .default_adapter()
            .await
            .context("finding Bluetooth adapter")?;
        adapter
            .set_powered(true)
            .await
            .context("powering Bluetooth adapter")?;
        Ok((session, adapter))
    }

    async fn associate_inner(
        setup: &AidotSetupCode,
        timeout: Duration,
    ) -> Result<AidotPairingCandidate> {
        let (_session, adapter) = Self::session_adapter().await?;
        let service_uuid = uuid(ADVERTISEMENT_SERVICE_UUID);
        adapter
            .set_discovery_filter(DiscoveryFilter {
                uuids: HashSet::from([service_uuid]),
                transport: DiscoveryTransport::Le,
                duplicate_data: true,
                ..Default::default()
            })
            .await
            .context("setting AiDot discovery filter")?;
        let events = adapter
            .discover_devices_with_changes()
            .await
            .context("starting AiDot button discovery")?;
        tokio::pin!(events);
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                anyhow::bail!("The button was not found. Put it in pairing mode, keep it near the Rhythm Box, and try again");
            }
            let event = tokio::time::timeout(remaining, events.next())
                .await
                .context("timed out looking for the button")?;
            let Some(AdapterEvent::DeviceAdded(address)) = event else {
                continue;
            };
            let device = adapter.device(address)?;
            let Some(service_data) = device.service_data().await.ok().flatten() else {
                continue;
            };
            let Some(identity) = service_data
                .get(&service_uuid)
                .and_then(|payload| service_data_identity(payload))
            else {
                continue;
            };
            if !identity.eq_ignore_ascii_case(&setup.ble_identity) {
                continue;
            }
            if device.rssi().await.ok().flatten().is_none() {
                continue;
            }

            if !device.is_connected().await.unwrap_or(false) {
                tokio::time::timeout(Duration::from_secs(30), device.connect())
                    .await
                    .context("timed out connecting to the button")?
                    .context("connecting to the button")?;
            }
            let services = tokio::time::timeout(Duration::from_secs(30), device.services())
                .await
                .context("timed out resolving button services")?
                .context("resolving button services")?;
            let primary = uuid(PRIMARY_GATT_SERVICE_UUID);
            let alternate = uuid(ALTERNATE_GATT_SERVICE_UUID);
            let mut supported = false;
            for service in services {
                let id = service.uuid().await?;
                supported |= id == primary || id == alternate;
            }
            if !supported {
                let _ = device.disconnect().await;
                anyhow::bail!("The QR identity matched, but the nearby Bluetooth device did not expose a supported Orein/AiDot button service");
            }
            let initial_counter =
                device
                    .manufacturer_data()
                    .await
                    .ok()
                    .flatten()
                    .and_then(|data| {
                        data.get(&MANUFACTURER_ID)
                            .and_then(|payload| press_counter(payload))
                    });
            let _ = device.disconnect().await;
            return Ok(AidotPairingCandidate {
                observed_address: address.to_string(),
                initial_counter,
            });
        }
    }

    async fn monitor_inner(
        store: Arc<AidotButtonStore>,
        registry: Arc<Mutex<HubDeviceRegistry>>,
        event_tx: Sender<HubEvent>,
        shutdown: Arc<AtomicBool>,
    ) -> Result<()> {
        let (_session, adapter) = Self::session_adapter().await?;
        let service_uuid = uuid(ADVERTISEMENT_SERVICE_UUID);
        adapter
            .set_discovery_filter(DiscoveryFilter {
                uuids: HashSet::from([service_uuid]),
                transport: DiscoveryTransport::Le,
                duplicate_data: true,
                ..Default::default()
            })
            .await?;
        let events = adapter.discover_devices_with_changes().await?;
        tokio::pin!(events);
        let mut adapter_connected = true;

        while !shutdown.load(Ordering::Relaxed) {
            let _ = tokio::time::timeout(Duration::from_millis(125), events.next()).await;
            let addresses = match adapter.device_addresses().await {
                Ok(addresses) => addresses,
                Err(error) => {
                    if adapter_connected {
                        let _ = event_tx.send(HubEvent::Disconnected {
                            hub_key: None,
                            reason: "Bluetooth adapter is unavailable".to_string(),
                        });
                        adapter_connected = false;
                    }
                    log::debug!(target: "evt", "AiDot address poll failed: {error}");
                    continue;
                }
            };
            if !adapter_connected {
                let _ = event_tx.send(HubEvent::Connected { hub_key: None });
                adapter_connected = true;
            }
            for address in addresses {
                let Ok(device) = adapter.device(address) else {
                    continue;
                };
                if device.rssi().await.ok().flatten().is_none() {
                    continue;
                }
                let service_identity =
                    device.service_data().await.ok().flatten().and_then(|data| {
                        data.get(&service_uuid)
                            .and_then(|payload| service_data_identity(payload))
                    });
                let Some(known) = service_identity
                    .as_deref()
                    .and_then(|identity| store.get_by_identity(identity))
                    .or_else(|| store.get_by_observed_address(&address.to_string()))
                else {
                    continue;
                };
                let payload = device
                    .manufacturer_data()
                    .await
                    .ok()
                    .flatten()
                    .and_then(|data| data.get(&MANUFACTURER_ID).cloned());
                let Some(payload) = payload else {
                    continue;
                };
                accept_press_advertisement(&payload, &known, &store, &registry, &event_tx)?;
            }
        }
        Ok(())
    }
}

impl AidotBleTransport for BluezAidotTransport {
    fn is_available(&self) -> Result<bool> {
        self.block_on(async {
            let (_session, _adapter) = Self::session_adapter().await?;
            Ok(true)
        })
    }

    fn associate(
        &self,
        setup: &AidotSetupCode,
        timeout: Duration,
    ) -> Result<AidotPairingCandidate> {
        self.block_on(Self::associate_inner(setup, timeout))
    }

    fn start_monitor(
        &self,
        store: Arc<AidotButtonStore>,
        registry: Arc<Mutex<HubDeviceRegistry>>,
        event_tx: Sender<HubEvent>,
        shutdown: Arc<AtomicBool>,
    ) -> Result<JoinHandle<()>> {
        std::thread::Builder::new()
            .name("aidot-ble-monitor".to_string())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                let result =
                    runtime
                        .context("building AiDot monitor runtime")
                        .and_then(|runtime| {
                            runtime.block_on(Self::monitor_inner(
                                store,
                                registry,
                                event_tx.clone(),
                                shutdown,
                            ))
                        });
                if let Err(error) = result {
                    let _ = event_tx.send(HubEvent::Disconnected {
                        hub_key: None,
                        reason: format!("AiDot button monitor stopped: {error:#}"),
                    });
                }
            })
            .context("spawning AiDot button monitor")
    }
}

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).expect("AiDot UUID constant")
}
