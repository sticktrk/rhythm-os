#![cfg(feature = "test-support")]
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rhythm_matter::hub_state::MatterHubData;
use rhythm_matter::test_support::SpyTransport;
use rhythm_matter::transport::MatterTransport;
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::ExternalLightHubIntegration;
use rhythm_os::hub::HubType;
use rhythm_os::provisioning::WifiCredentials;
use rhythm_os::state::{AppState, SharedState};
use rhythm_os::storage::FileStorage;

pub struct TestRig {
    pub state: SharedState,
    pub transport: Arc<SpyTransport>,
    pub hub_data: Arc<MatterHubData>,
    pub data_dir: PathBuf,
}

pub fn connect_rig(data_dir: Option<PathBuf>) -> TestRig {
    connect_rig_with_transport(data_dir, |_| {})
}

pub fn connect_rig_with_transport<F>(data_dir: Option<PathBuf>, init_transport: F) -> TestRig
where
    F: FnOnce(&Arc<SpyTransport>),
{
    connect_rig_internal(data_dir, false, init_transport)
}

pub fn reconnect_rig_with_transport<F>(data_dir: PathBuf, init_transport: F) -> TestRig
where
    F: FnOnce(&Arc<SpyTransport>),
{
    connect_rig_internal(Some(data_dir), true, init_transport)
}

fn connect_rig_internal<F>(
    data_dir: Option<PathBuf>,
    load_persisted_state: bool,
    init_transport: F,
) -> TestRig
where
    F: FnOnce(&Arc<SpyTransport>),
{
    let data_dir = data_dir.unwrap_or_else(unique_data_dir);
    std::fs::create_dir_all(&data_dir).unwrap();

    let state: SharedState = Arc::new(Mutex::new(AppState::default()));
    {
        let mut state_guard = state.lock().unwrap();
        state_guard.data_dir = data_dir.display().to_string();
        state_guard.storage = Some(Box::new(
            FileStorage::new(data_dir.to_str().unwrap()).unwrap(),
        ));
        state_guard.ensure_runtime_fn = Some(Arc::new(|state: &SharedState| {
            let integrations: [&'static dyn ExternalLightHubIntegration; 1] =
                [&rhythm_matter::desktop_lifecycle::INTEGRATION];
            rhythm_os::lifecycle::ensure_composite_runtime(state, &integrations)
        }));
        if load_persisted_state {
            rhythm_os::storage::load_persisted_state(&mut state_guard);
        }
    }

    let transport = Arc::new(SpyTransport::new());
    init_transport(&transport);
    let transport_obj: Arc<dyn MatterTransport> = transport.clone();
    let (mut hub, _event_rx) =
        rhythm_matter::lifecycle::connect_matter(&state, transport_obj.clone()).unwrap();
    let hub_data = hub.data::<Arc<MatterHubData>>().cloned().unwrap();
    hub.discovery = Some(Arc::new(rhythm_matter::discovery::MatterDiscovery::new(
        transport_obj,
        hub_data.clone(),
    )));

    {
        let mut state_guard = state.lock().unwrap();
        let hub_key = HubKey::new(HubType::new("matter"), "local");
        state_guard.hubs.insert(hub_key.clone(), hub);
        state_guard.set_hub_connected(&hub_key, true);
    }

    TestRig {
        state,
        transport,
        hub_data,
        data_dir,
    }
}

pub fn store_commissioning_wifi(state: &SharedState, ssid: &str, password: &str) {
    let state_guard = state.lock().unwrap();
    let storage = state_guard.storage.as_ref().unwrap();
    storage
        .save_commissioning_wifi_credentials(&WifiCredentials {
            ssid: ssid.to_string(),
            password: password.to_string(),
        })
        .unwrap();
}

fn unique_data_dir() -> PathBuf {
    let base = std::env::temp_dir().join(format!("rhythm-matter-test-{}", std::process::id()));
    base.join(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .to_string(),
    )
}
