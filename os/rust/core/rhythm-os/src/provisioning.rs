//! Shared Wi-Fi provisioning contract for hardware bootstrap flows.
//!
//! The BLE transport differs substantially across targets (ESP-IDF vs. BlueZ),
//! but the provisioning contract is the same:
//! - advertise the same GATT UUIDs
//! - accept `{"ssid","password"}` credentials
//! - report `waiting` / `connecting` / `connected` / `wifi_failed`
//! - keep retrying until the network comes up

use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// BLE service UUID for Rhythm Wi-Fi provisioning.
pub const PROVISIONING_SERVICE_UUID: u128 = 0x72797468_6d00_1000_8000_00805f9b34fb;
/// BLE write characteristic UUID for Wi-Fi credentials.
pub const PROVISIONING_WIFI_CMD_UUID: u128 = 0x72797468_6d01_1000_8000_00805f9b34fb;
/// BLE status characteristic UUID for provisioning progress.
pub const PROVISIONING_STATUS_UUID: u128 = 0x72797468_6d02_1000_8000_00805f9b34fb;
/// BLE read characteristic UUID for device metadata.
pub const PROVISIONING_DEVICE_INFO_UUID: u128 = 0x72797468_6d03_1000_8000_00805f9b34fb;

/// Wi-Fi credentials received from a provisioning frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WifiCredentials {
    pub ssid: String,
    pub password: String,
}

/// Metadata exposed to the provisioning client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvisioningDeviceInfo {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
}

impl ProvisioningDeviceInfo {
    pub fn json_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }
}

/// Build a stable provisioning device name for a hardware target.
///
/// Format: `rhythm-<target>-<id>`
pub fn provisioning_device_name(target: &str, id: &str) -> String {
    let normalized_target: String = target
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
        .to_ascii_lowercase();
    let normalized_id: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_uppercase();

    let target_part = if normalized_target.is_empty() {
        "device"
    } else {
        normalized_target.as_str()
    };
    let id_part = if normalized_id.is_empty() {
        "0000"
    } else {
        normalized_id.as_str()
    };

    format!("rhythm-{}-{}", target_part, id_part)
}

/// Status payload surfaced to the provisioning client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisioningStatus {
    Waiting,
    Connecting,
    Connected {
        ip: String,
        owner_token: Option<String>,
    },
    WifiFailed {
        error: String,
    },
    Failed {
        error: String,
    },
}

#[derive(Serialize)]
struct ProvisioningStatusPayload<'a> {
    status: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    ip: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_token: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

impl ProvisioningStatus {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Connecting => "connecting",
            Self::Connected { .. } => "connected",
            Self::WifiFailed { .. } => "wifi_failed",
            Self::Failed { .. } => "failed",
        }
    }

    pub fn json_bytes(&self) -> Result<Vec<u8>> {
        let payload = match self {
            Self::Waiting => ProvisioningStatusPayload {
                status: self.code(),
                ip: None,
                owner_token: None,
                error: None,
            },
            Self::Connecting => ProvisioningStatusPayload {
                status: self.code(),
                ip: None,
                owner_token: None,
                error: None,
            },
            Self::Connected { ip, owner_token } => ProvisioningStatusPayload {
                status: self.code(),
                ip: Some(ip),
                owner_token: owner_token.as_deref(),
                error: None,
            },
            Self::WifiFailed { error } | Self::Failed { error } => ProvisioningStatusPayload {
                status: self.code(),
                ip: None,
                owner_token: None,
                error: Some(error),
            },
        };

        Ok(serde_json::to_vec(&payload)?)
    }
}

/// A target-specific frontend event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisioningEvent {
    Credentials(WifiCredentials),
    Error(String),
}

/// Result of a hardware-specific network verification attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisioningConnectResult {
    Connected {
        ip: String,
        owner_token: Option<String>,
    },
    Failed {
        error: String,
    },
}

/// Transport frontend for provisioning sessions.
pub trait ProvisioningFrontend {
    fn start(&mut self, info: &ProvisioningDeviceInfo) -> Result<()>;
    fn poll_event(&mut self, timeout: Duration) -> Result<Option<ProvisioningEvent>>;
    fn publish_status(&mut self, status: &ProvisioningStatus) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
}

/// Network verification backend for provisioning sessions.
pub trait ProvisioningBackend {
    fn begin_connect(&mut self, creds: WifiCredentials) -> Result<()>;
    fn poll_result(&mut self, timeout: Duration) -> Result<Option<ProvisioningConnectResult>>;
}

/// Runtime tuning for the provisioning session loop.
#[derive(Debug, Clone)]
pub struct ProvisioningSessionConfig {
    pub poll_interval: Duration,
    pub connect_timeout: Duration,
    pub success_grace_period: Duration,
    /// Upper bound on the full provisioning session. If `Some(d)`, the loop
    /// aborts after `d` of wall-clock time and returns an error. A malformed
    /// BLE stream or stuck Wi-Fi driver can otherwise keep the appliance
    /// stuck in provisioning mode indefinitely. `None` disables the cap
    /// (kept only for tests that want to assert the no-cap behavior
    /// explicitly).
    pub session_timeout: Option<Duration>,
}

impl Default for ProvisioningSessionConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(100),
            connect_timeout: Duration::from_secs(30),
            success_grace_period: Duration::from_secs(2),
            // 30 minutes is generous — a real user typically completes
            // onboarding in under 60s — but short enough that a unit stuck
            // in provisioning mode will eventually exit and retry from a
            // clean state instead of draining battery or blocking HTTP.
            session_timeout: Some(Duration::from_secs(30 * 60)),
        }
    }
}

struct PendingConnect {
    creds: WifiCredentials,
    deadline: Instant,
}

/// Run a full provisioning session using a target-specific frontend/backend pair.
pub fn run_provisioning_session<F, B>(
    frontend: &mut F,
    backend: &mut B,
    info: &ProvisioningDeviceInfo,
    config: &ProvisioningSessionConfig,
) -> Result<WifiCredentials>
where
    F: ProvisioningFrontend,
    B: ProvisioningBackend,
{
    frontend.start(info)?;

    let result = (|| -> Result<WifiCredentials> {
        frontend.publish_status(&ProvisioningStatus::Waiting)?;

        let mut pending: Option<PendingConnect> = None;
        let session_deadline = config.session_timeout.map(|d| Instant::now() + d);

        loop {
            if let Some(deadline) = session_deadline {
                if Instant::now() >= deadline {
                    let error = "Provisioning session timed out".to_string();
                    let _ = frontend.publish_status(&ProvisioningStatus::Failed {
                        error: error.clone(),
                    });
                    anyhow::bail!("{}", error);
                }
            }
            if let Some(event) = frontend.poll_event(config.poll_interval)? {
                match event {
                    ProvisioningEvent::Credentials(creds) => {
                        if pending.is_none() {
                            frontend.publish_status(&ProvisioningStatus::Connecting)?;
                            backend.begin_connect(creds.clone())?;
                            pending = Some(PendingConnect {
                                creds,
                                deadline: Instant::now() + config.connect_timeout,
                            });
                        }
                    }
                    ProvisioningEvent::Error(error) => {
                        frontend.publish_status(&ProvisioningStatus::Failed { error })?;
                    }
                }
            }

            let mut clear_pending = false;
            if let Some(active) = pending.as_ref() {
                if let Some(result) = backend.poll_result(Duration::from_millis(0))? {
                    match result {
                        ProvisioningConnectResult::Connected { ip, owner_token } => {
                            frontend.publish_status(&ProvisioningStatus::Connected {
                                ip,
                                owner_token,
                            })?;
                            std::thread::sleep(config.success_grace_period);
                            frontend.stop()?;
                            return Ok(active.creds.clone());
                        }
                        ProvisioningConnectResult::Failed { error } => {
                            frontend.publish_status(&ProvisioningStatus::WifiFailed { error })?;
                            clear_pending = true;
                        }
                    }
                } else if Instant::now() >= active.deadline {
                    frontend.publish_status(&ProvisioningStatus::WifiFailed {
                        error: "Connection timed out".to_string(),
                    })?;
                    clear_pending = true;
                }
            }

            if clear_pending {
                pending = None;
            }
        }
    })();

    if result.is_err() {
        let _ = frontend.stop();
    }

    result
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    struct FakeFrontend {
        events: VecDeque<ProvisioningEvent>,
        statuses: Vec<ProvisioningStatus>,
        started: bool,
        stopped: bool,
    }

    impl FakeFrontend {
        fn new(events: Vec<ProvisioningEvent>) -> Self {
            Self {
                events: VecDeque::from(events),
                statuses: Vec::new(),
                started: false,
                stopped: false,
            }
        }
    }

    impl ProvisioningFrontend for FakeFrontend {
        fn start(&mut self, _info: &ProvisioningDeviceInfo) -> Result<()> {
            self.started = true;
            Ok(())
        }

        fn poll_event(&mut self, _timeout: Duration) -> Result<Option<ProvisioningEvent>> {
            Ok(self.events.pop_front())
        }

        fn publish_status(&mut self, status: &ProvisioningStatus) -> Result<()> {
            self.statuses.push(status.clone());
            Ok(())
        }

        fn stop(&mut self) -> Result<()> {
            self.stopped = true;
            Ok(())
        }
    }

    struct FakeBackend {
        results: VecDeque<Option<ProvisioningConnectResult>>,
        requested: Vec<WifiCredentials>,
    }

    impl FakeBackend {
        fn new(results: Vec<Option<ProvisioningConnectResult>>) -> Self {
            Self {
                results: VecDeque::from(results),
                requested: Vec::new(),
            }
        }
    }

    impl ProvisioningBackend for FakeBackend {
        fn begin_connect(&mut self, creds: WifiCredentials) -> Result<()> {
            self.requested.push(creds);
            Ok(())
        }

        fn poll_result(&mut self, _timeout: Duration) -> Result<Option<ProvisioningConnectResult>> {
            Ok(self.results.pop_front().flatten())
        }
    }

    fn device_info() -> ProvisioningDeviceInfo {
        ProvisioningDeviceInfo {
            name: provisioning_device_name("rpiz", "ABCD"),
            version: "1.2.3".to_string(),
            mac: Some("AA:BB:CC:DD:EE:FF".to_string()),
        }
    }

    #[test]
    fn provisioning_device_name_uses_stable_format() {
        assert_eq!(provisioning_device_name("rpiz", "abcd"), "rhythm-rpiz-ABCD");
        assert_eq!(provisioning_device_name("", "12ef"), "rhythm-device-12EF");
    }

    #[test]
    fn provisioning_session_reports_success() {
        let creds = WifiCredentials {
            ssid: "wifi".to_string(),
            password: "secret".to_string(),
        };
        let mut frontend = FakeFrontend::new(vec![ProvisioningEvent::Credentials(creds.clone())]);
        let mut backend = FakeBackend::new(vec![Some(ProvisioningConnectResult::Connected {
            ip: "192.168.1.10".to_string(),
            owner_token: Some("owner-token".to_string()),
        })]);

        let config = ProvisioningSessionConfig {
            success_grace_period: Duration::from_millis(0),
            ..Default::default()
        };

        let result =
            run_provisioning_session(&mut frontend, &mut backend, &device_info(), &config).unwrap();

        assert_eq!(result, creds);
        assert!(frontend.started);
        assert!(frontend.stopped);
        assert_eq!(
            frontend.statuses,
            vec![
                ProvisioningStatus::Waiting,
                ProvisioningStatus::Connecting,
                ProvisioningStatus::Connected {
                    ip: "192.168.1.10".to_string(),
                    owner_token: Some("owner-token".to_string()),
                },
            ]
        );
    }

    #[test]
    fn provisioning_session_aborts_when_session_timeout_elapses() {
        // No events, no results — a malformed BLE stream that never completes.
        // With a tiny session_timeout the loop must exit with an error rather
        // than blocking forever.
        let mut frontend = FakeFrontend::new(Vec::new());
        let mut backend = FakeBackend::new(Vec::new());

        let config = ProvisioningSessionConfig {
            poll_interval: Duration::from_millis(10),
            connect_timeout: Duration::from_secs(1),
            success_grace_period: Duration::from_millis(0),
            session_timeout: Some(Duration::from_millis(50)),
        };

        let start = std::time::Instant::now();
        let result = run_provisioning_session(&mut frontend, &mut backend, &device_info(), &config);
        let elapsed = start.elapsed();

        assert!(result.is_err(), "timeout must surface as an error");
        assert!(
            elapsed < Duration::from_secs(1),
            "session must exit promptly near the timeout, got {:?}",
            elapsed
        );
        assert!(
            frontend.statuses.iter().any(|s| matches!(
                s,
                ProvisioningStatus::Failed { error } if error.contains("timed out")
            )),
            "session timeout must publish a Failed status, got {:?}",
            frontend.statuses
        );
        assert!(frontend.stopped, "frontend must be stopped on timeout");
    }

    #[test]
    fn provisioning_session_completes_before_timeout_does_not_abort() {
        // Healthy session that completes within the session_timeout must
        // not be aborted by the deadline.
        let creds = WifiCredentials {
            ssid: "w".into(),
            password: "p".into(),
        };
        let mut frontend = FakeFrontend::new(vec![ProvisioningEvent::Credentials(creds.clone())]);
        let mut backend = FakeBackend::new(vec![Some(ProvisioningConnectResult::Connected {
            ip: "10.0.0.1".into(),
            owner_token: None,
        })]);

        let config = ProvisioningSessionConfig {
            success_grace_period: Duration::from_millis(0),
            session_timeout: Some(Duration::from_secs(5)),
            ..Default::default()
        };

        let result = run_provisioning_session(&mut frontend, &mut backend, &device_info(), &config)
            .expect("healthy session must succeed");
        assert_eq!(result, creds);
    }

    #[test]
    fn default_config_has_finite_session_timeout() {
        // Guards against a regression that accidentally removes the cap.
        let config = ProvisioningSessionConfig::default();
        assert!(
            config.session_timeout.is_some(),
            "default session_timeout must not be None to prevent indefinite provisioning"
        );
    }

    #[test]
    fn provisioning_session_retries_after_wifi_failure() {
        let first = WifiCredentials {
            ssid: "bad".to_string(),
            password: "pw1".to_string(),
        };
        let second = WifiCredentials {
            ssid: "good".to_string(),
            password: "pw2".to_string(),
        };
        let mut frontend = FakeFrontend::new(vec![
            ProvisioningEvent::Credentials(first.clone()),
            ProvisioningEvent::Credentials(second.clone()),
        ]);
        let mut backend = FakeBackend::new(vec![
            Some(ProvisioningConnectResult::Failed {
                error: "bad credentials".to_string(),
            }),
            Some(ProvisioningConnectResult::Connected {
                ip: "10.0.0.5".to_string(),
                owner_token: None,
            }),
        ]);

        let config = ProvisioningSessionConfig {
            success_grace_period: Duration::from_millis(0),
            ..Default::default()
        };

        let result =
            run_provisioning_session(&mut frontend, &mut backend, &device_info(), &config).unwrap();

        assert_eq!(result, second);
        assert_eq!(backend.requested, vec![first, second]);
        assert_eq!(
            frontend.statuses,
            vec![
                ProvisioningStatus::Waiting,
                ProvisioningStatus::Connecting,
                ProvisioningStatus::WifiFailed {
                    error: "bad credentials".to_string(),
                },
                ProvisioningStatus::Connecting,
                ProvisioningStatus::Connected {
                    ip: "10.0.0.5".to_string(),
                    owner_token: None,
                },
            ]
        );
    }
}
