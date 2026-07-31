use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Result;
use rhythm_core::ButtonAction;
use rhythm_os::hub::HubEvent;
use rhythm_os::registry::HubDeviceRegistry;

use crate::protocol::{press_counter, AidotSetupCode};
use crate::store::{AidotButtonDevice, AidotButtonStore, CounterDisposition};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AidotPairingCandidate {
    pub observed_address: String,
    pub initial_counter: Option<u8>,
}

pub trait AidotBleTransport: Send + Sync {
    fn is_available(&self) -> Result<bool>;

    fn associate(&self, setup: &AidotSetupCode, timeout: Duration)
        -> Result<AidotPairingCandidate>;

    fn start_monitor(
        &self,
        store: Arc<AidotButtonStore>,
        registry: Arc<Mutex<HubDeviceRegistry>>,
        event_tx: Sender<HubEvent>,
        shutdown: Arc<AtomicBool>,
    ) -> Result<JoinHandle<()>>;
}

/// Validate, durably deduplicate, and emit one canonical press event.
///
/// Persistence happens before emission, so repeated BlueZ observations and
/// process restarts cannot replay the same rolling counter value.
pub fn accept_press_advertisement(
    payload: &[u8],
    device: &AidotButtonDevice,
    store: &AidotButtonStore,
    registry: &Arc<Mutex<HubDeviceRegistry>>,
    event_tx: &Sender<HubEvent>,
) -> Result<bool> {
    let Some(counter) = press_counter(payload) else {
        return Ok(false);
    };
    if store.accept_counter(&device.ble_identity, counter)? != CounterDisposition::New {
        return Ok(false);
    }
    let button_id = device.button_id();
    let room_id = registry
        .lock()
        .ok()
        .and_then(|registry| registry.get_room_for_button(&button_id));
    let event = match room_id {
        Some(room_id) => HubEvent::Button {
            hub_key: None,
            room_id,
            action: ButtonAction::OnPress,
            device_id: Some(device.id.clone()),
        },
        None => HubEvent::UnroutableButton {
            hub_key: None,
            device_id: Some(device.id.clone()),
            button_id,
        },
    };
    Ok(event_tx.send(event).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhythm_core::runtime::hub_registry::DeviceType;
    use std::path::PathBuf;

    fn temporary_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "rhythm-aidot-press-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn repeated_frame_emits_once_while_gaps_and_wrap_emit() {
        let root = temporary_dir();
        let store = AidotButtonStore::load(&root).unwrap();
        let device = AidotButtonDevice {
            id: AidotButtonDevice::stable_id("1CD6BD2273F9"),
            ble_identity: "1CD6BD2273F9".to_string(),
            serial_metadata: "opaque-s".to_string(),
            model_metadata: "opaque-m".to_string(),
            observed_address: "C0:FF:EE:00:00:01".to_string(),
            last_counter: Some(0x85),
            associated_at_epoch_secs: 1,
        };
        store.upsert(device.clone()).unwrap();
        let registry = Arc::new(Mutex::new(HubDeviceRegistry::new()));
        registry.lock().unwrap().upsert_device(
            &device.id,
            Some("bedroom"),
            &[(device.button_id(), 1)],
            DeviceType::Button,
        );
        let (tx, rx) = std::sync::mpsc::channel();
        let frame = |counter| {
            let mut payload = [0_u8; 19];
            payload[..3].copy_from_slice(&[counter, 0x02, 0x01]);
            payload
        };

        assert!(
            !accept_press_advertisement(&frame(0x85), &device, &store, &registry, &tx).unwrap()
        );
        assert!(accept_press_advertisement(&frame(0x90), &device, &store, &registry, &tx).unwrap());
        assert!(
            !accept_press_advertisement(&frame(0x90), &device, &store, &registry, &tx).unwrap()
        );
        assert!(accept_press_advertisement(&frame(0x00), &device, &store, &registry, &tx).unwrap());
        let events = rx.try_iter().collect::<Vec<_>>();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| matches!(
            event,
            HubEvent::Button {
                room_id,
                action: ButtonAction::OnPress,
                ..
            } if room_id == "bedroom"
        )));
        std::fs::remove_dir_all(root).unwrap();
    }
}
