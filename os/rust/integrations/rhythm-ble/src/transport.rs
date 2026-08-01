//! Portable transport boundary for simple local-BLE profiles.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use rhythm_os::hub::HubEvent;
use rhythm_os::pairing::{PairingFailure, PairingFailureStage};
use rhythm_os::registry::HubDeviceRegistry;
use serde::Serialize;

use crate::profile::{
    decode_profile_events, profile_by_id, BleAdvertisementView, BleNormalizedEvent,
    ValidatedBleSetup,
};
use crate::store::{LocalBleDevice, LocalBleDeviceStore, ReplayDisposition};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalBlePairingCandidate {
    pub transport_hint: String,
    pub initial_replay: BTreeMap<String, Vec<u8>>,
}

/// Privacy-bounded evidence from the most recent local-BLE association.
///
/// Counts intentionally describe pipeline categories only. They never retain
/// a Bluetooth address, raw advertisement, setup field, stable identity,
/// serial, or profile metadata. `u16` plus saturating updates makes every
/// field bounded even if a scanner produces an unexpectedly high frame rate.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LocalBleAssociationDiagnostics {
    pub succeeded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_stage: Option<PairingFailureStage>,
    pub observations_received: u16,
    pub observations_with_signal: u16,
    pub advertisements_decoded: u16,
    pub identity_matches: u16,
    pub gatt_proof_attempts: u16,
    pub candidate_open_failures: u16,
    pub connect_failures: u16,
    pub service_discovery_failures: u16,
    pub service_mismatches: u16,
    pub cleanup_failures: u16,
}

#[cfg(any(test, all(target_os = "linux", feature = "bluez")))]
impl LocalBleAssociationDiagnostics {
    pub(crate) fn increment(counter: &mut u16) {
        *counter = counter.saturating_add(1);
    }

    pub(crate) fn failed(mut self, stage: PairingFailureStage) -> Self {
        self.succeeded = false;
        self.failure_stage = Some(stage);
        self
    }

    pub(crate) fn succeeded(mut self) -> Self {
        self.succeeded = true;
        self.failure_stage = None;
        self
    }
}

/// Mutable, operation-local evidence tracker. It keeps only bounded counters
/// and the deepest stage reached; candidate-specific values never leave the
/// BlueZ operation.
#[derive(Debug, Default)]
#[cfg(any(test, all(target_os = "linux", feature = "bluez")))]
pub(crate) struct LocalBleAssociationEvidence {
    diagnostics: LocalBleAssociationDiagnostics,
    deepest_candidate_failure: Option<PairingFailureStage>,
}

#[cfg(any(test, all(target_os = "linux", feature = "bluez")))]
impl LocalBleAssociationEvidence {
    pub(crate) fn observation_received(&mut self) {
        LocalBleAssociationDiagnostics::increment(&mut self.diagnostics.observations_received);
    }

    pub(crate) fn observation_with_signal(&mut self) {
        LocalBleAssociationDiagnostics::increment(&mut self.diagnostics.observations_with_signal);
    }

    pub(crate) fn advertisement_decoded(&mut self) {
        LocalBleAssociationDiagnostics::increment(&mut self.diagnostics.advertisements_decoded);
    }

    pub(crate) fn identity_matched(&mut self) {
        LocalBleAssociationDiagnostics::increment(&mut self.diagnostics.identity_matches);
    }

    pub(crate) fn gatt_proof_attempted(&mut self) {
        LocalBleAssociationDiagnostics::increment(&mut self.diagnostics.gatt_proof_attempts);
    }

    pub(crate) fn candidate_failed(&mut self, stage: PairingFailureStage) {
        match stage {
            PairingFailureStage::CandidateOpen => LocalBleAssociationDiagnostics::increment(
                &mut self.diagnostics.candidate_open_failures,
            ),
            PairingFailureStage::CandidateConnect => {
                LocalBleAssociationDiagnostics::increment(&mut self.diagnostics.connect_failures)
            }
            PairingFailureStage::CandidateServiceDiscovery => {
                LocalBleAssociationDiagnostics::increment(
                    &mut self.diagnostics.service_discovery_failures,
                )
            }
            PairingFailureStage::CandidateServiceMismatch => {
                LocalBleAssociationDiagnostics::increment(&mut self.diagnostics.service_mismatches)
            }
            PairingFailureStage::CandidateCleanup => {
                LocalBleAssociationDiagnostics::increment(&mut self.diagnostics.cleanup_failures)
            }
            PairingFailureStage::TargetNotObserved
            | PairingFailureStage::Transport
            | PairingFailureStage::Unknown => {}
        }
        let stage_rank = candidate_stage_rank(stage);
        let is_deeper = match self.deepest_candidate_failure {
            Some(current) => stage_rank > candidate_stage_rank(current),
            None => stage_rank > 0,
        };
        if stage == PairingFailureStage::CandidateCleanup || is_deeper {
            self.deepest_candidate_failure = Some(stage);
        }
    }

    /// Record GATT validation before candidate cleanup starts.
    ///
    /// This preserves the validation counter if cleanup also fails; the caller
    /// can then record cleanup as the terminal/deepest stage.
    pub(crate) fn record_candidate_validation(
        &mut self,
        validation: std::result::Result<(), PairingFailureStage>,
    ) -> bool {
        match validation {
            Ok(()) => true,
            Err(stage) => {
                self.candidate_failed(stage);
                false
            }
        }
    }

    pub(crate) fn failed(self, fallback: PairingFailureStage) -> LocalBleAssociationDiagnostics {
        let stage = self.deepest_candidate_failure.unwrap_or(fallback);
        self.diagnostics.failed(stage)
    }

    pub(crate) fn succeeded(self) -> LocalBleAssociationDiagnostics {
        self.diagnostics.succeeded()
    }
}

#[cfg(any(test, all(target_os = "linux", feature = "bluez")))]
fn candidate_stage_rank(stage: PairingFailureStage) -> u8 {
    match stage {
        PairingFailureStage::CandidateOpen => 1,
        PairingFailureStage::CandidateConnect => 2,
        PairingFailureStage::CandidateServiceDiscovery => 3,
        PairingFailureStage::CandidateServiceMismatch => 4,
        PairingFailureStage::CandidateCleanup => 5,
        PairingFailureStage::TargetNotObserved
        | PairingFailureStage::Transport
        | PairingFailureStage::Unknown => 0,
    }
}

/// Typed association failure retained through the integration boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalBleAssociationError {
    diagnostics: LocalBleAssociationDiagnostics,
}

impl LocalBleAssociationError {
    #[cfg(all(target_os = "linux", feature = "bluez"))]
    pub(crate) fn new(diagnostics: LocalBleAssociationDiagnostics) -> Self {
        debug_assert!(diagnostics.failure_stage.is_some());
        Self { diagnostics }
    }

    pub fn stage(&self) -> PairingFailureStage {
        self.diagnostics
            .failure_stage
            .unwrap_or(PairingFailureStage::Transport)
    }

    #[cfg(all(target_os = "linux", feature = "bluez"))]
    pub fn diagnostics(&self) -> &LocalBleAssociationDiagnostics {
        &self.diagnostics
    }

    pub fn into_pairing_failure(self) -> PairingFailure {
        let stage = self.stage();
        let message = match stage {
            PairingFailureStage::TargetNotObserved => {
                "Device not found. Tap Find Device before putting it in pairing mode, keep it close to the Rhythm Box, and try again."
            }
            PairingFailureStage::CandidateOpen | PairingFailureStage::CandidateConnect => {
                "Device found, but the Rhythm Box could not connect. Reset it into pairing mode, keep it close, and try again."
            }
            PairingFailureStage::CandidateServiceDiscovery
            | PairingFailureStage::CandidateServiceMismatch => {
                "Device found, but its Bluetooth identity could not be verified. Reset it into pairing mode and try again."
            }
            PairingFailureStage::CandidateCleanup => {
                "Bluetooth pairing stopped while releasing the device. Restart the Rhythm Box before trying again."
            }
            PairingFailureStage::Transport | PairingFailureStage::Unknown => {
                "The Rhythm Box Bluetooth service was unavailable. Restart the Rhythm Box and try again."
            }
        };
        PairingFailure::new(stage, message)
    }
}

impl std::fmt::Display for LocalBleAssociationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "local Bluetooth association failed at stage={}",
            self.stage().as_str()
        )
    }
}

impl std::error::Error for LocalBleAssociationError {}

/// One absolute association budget split into proof and cleanup phases.
///
/// Candidate discovery and GATT identity proof must stop at `proof_end`, so a
/// connection attempt always leaves bounded time to release the candidate
/// before the complete association budget expires.
#[cfg(any(test, all(target_os = "linux", feature = "bluez")))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct LocalBleAssociationDeadline {
    proof_end: Instant,
    cleanup_end: Instant,
}

#[cfg(any(test, all(target_os = "linux", feature = "bluez")))]
impl LocalBleAssociationDeadline {
    #[cfg(all(target_os = "linux", feature = "bluez"))]
    pub(crate) fn new(cleanup_end: Instant, cleanup_reserve: Duration) -> Result<Self> {
        Self::new_at(cleanup_end, cleanup_reserve, Instant::now())
    }

    fn new_at(cleanup_end: Instant, cleanup_reserve: Duration, now: Instant) -> Result<Self> {
        let proof_end = cleanup_end
            .checked_sub(cleanup_reserve)
            .filter(|proof_end| *proof_end > now)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "local Bluetooth association deadline left no candidate cleanup budget"
                )
            })?;
        Ok(Self {
            proof_end,
            cleanup_end,
        })
    }

    pub(crate) fn proof_end(self) -> Instant {
        self.proof_end
    }

    pub(crate) fn cleanup_end(self) -> Instant {
        self.cleanup_end
    }

    /// Deadline for the cleanup I/O itself, leaving a short tail inside the
    /// complete operation budget to classify and return a cleanup timeout.
    #[cfg(all(target_os = "linux", feature = "bluez"))]
    pub(crate) fn cleanup_attempt_end(self, result_margin: Duration) -> Result<Instant> {
        self.cleanup_attempt_end_at(Instant::now(), result_margin)
    }

    fn cleanup_attempt_end_at(self, now: Instant, result_margin: Duration) -> Result<Instant> {
        if result_margin.is_zero() {
            anyhow::bail!(
                "local Bluetooth candidate cleanup requires a failure-classification margin"
            );
        }
        let attempt_end = self
            .cleanup_end
            .checked_sub(result_margin)
            .filter(|attempt_end| *attempt_end > self.proof_end && *attempt_end > now)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "local Bluetooth candidate cleanup left no failure-classification margin"
                )
            })?;
        Ok(attempt_end)
    }

    #[cfg(all(target_os = "linux", feature = "bluez"))]
    pub(crate) fn stage_end(self, maximum: Duration) -> Result<Instant> {
        self.stage_end_at(Instant::now(), maximum)
    }

    fn stage_end_at(self, now: Instant, maximum: Duration) -> Result<Instant> {
        if maximum.is_zero() || now >= self.proof_end {
            anyhow::bail!("local Bluetooth candidate proof deadline expired");
        }
        Ok(now
            .checked_add(maximum)
            .unwrap_or(self.proof_end)
            .min(self.proof_end))
    }
}

pub trait LocalBleTransport: Send + Sync {
    fn is_available(&self) -> Result<bool>;

    /// Probe adapter availability under the caller's absolute budget. The
    /// default preserves portable test/desktop transports; a transport with
    /// blocking admission must override this so setup cannot consume a fresh
    /// timeout before association sees the original pairing deadline.
    fn is_available_until(&self, deadline: Instant) -> Result<bool> {
        if deadline <= Instant::now() {
            anyhow::bail!("local Bluetooth availability deadline expired");
        }
        self.is_available()
    }

    fn associate(
        &self,
        profile_id: &str,
        setup: &ValidatedBleSetup,
        timeout: Duration,
    ) -> Result<LocalBlePairingCandidate>;

    /// Associate under an absolute deadline. Existing transports retain the
    /// duration-based method; deadline-aware transports override this so
    /// admission and every protocol stage consume the same budget.
    fn associate_until(
        &self,
        profile_id: &str,
        setup: &ValidatedBleSetup,
        deadline: Instant,
    ) -> Result<LocalBlePairingCandidate> {
        let timeout = deadline
            .checked_duration_since(Instant::now())
            .filter(|timeout| !timeout.is_zero())
            .ok_or_else(|| anyhow::anyhow!("local Bluetooth association deadline expired"))?;
        self.associate(profile_id, setup, timeout)
    }

    /// Associate after notifying the caller that the transport's fresh
    /// observation listener is installed.
    ///
    /// Portable and fake transports retain compatibility through the default
    /// handoff notification. BlueZ overrides this to notify only after its
    /// fresh-only broadcast subscription exists and before the first receive.
    fn associate_until_with_readiness(
        &self,
        profile_id: &str,
        setup: &ValidatedBleSetup,
        deadline: Instant,
        on_ready: &mut dyn FnMut(),
    ) -> Result<LocalBlePairingCandidate> {
        on_ready();
        self.associate_until(profile_id, setup, deadline)
    }

    fn start_monitor(
        &self,
        store: Arc<LocalBleDeviceStore>,
        registry: Arc<Mutex<HubDeviceRegistry>>,
        event_tx: Sender<HubEvent>,
        shutdown: Arc<AtomicBool>,
    ) -> Result<JoinHandle<()>>;

    /// Stop profile-owned work and wait for it to leave the shared adapter.
    /// The shared runtime and rich clients such as Hue remain available.
    fn quiesce(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(any(test, all(target_os = "linux", feature = "bluez")))]
pub(crate) fn install_association_listener<T>(
    install: impl FnOnce() -> Result<T>,
    on_ready: &mut dyn FnMut(),
) -> Result<T> {
    let listener = install()?;
    on_ready();
    Ok(listener)
}

#[cfg(test)]
mod deadline_tests {
    use super::*;

    #[test]
    fn absolute_association_budget_caps_every_proof_stage_and_reserves_cleanup() {
        let start = Instant::now();
        let end = start + Duration::from_secs(60);
        let budget =
            LocalBleAssociationDeadline::new_at(end, Duration::from_secs(2), start).unwrap();

        assert_eq!(budget.proof_end(), start + Duration::from_secs(58));
        assert_eq!(budget.cleanup_end(), end);
        assert_eq!(
            budget.stage_end_at(start, Duration::from_secs(30)).unwrap(),
            start + Duration::from_secs(30)
        );
        assert_eq!(
            budget
                .stage_end_at(start + Duration::from_secs(40), Duration::from_secs(30),)
                .unwrap(),
            start + Duration::from_secs(58)
        );
    }

    #[test]
    fn expired_proof_never_consumes_the_reserved_cleanup_window() {
        let start = Instant::now();
        let end = start + Duration::from_secs(60);
        let budget =
            LocalBleAssociationDeadline::new_at(end, Duration::from_secs(2), start).unwrap();

        assert!(budget
            .stage_end_at(start + Duration::from_secs(58), Duration::from_secs(1),)
            .is_err());
        assert_eq!(budget.cleanup_end(), end);
        assert!(LocalBleAssociationDeadline::new_at(
            start + Duration::from_secs(2),
            Duration::from_secs(2),
            start,
        )
        .is_err());
    }

    #[test]
    fn timeout_without_a_matching_observation_reports_target_not_observed() {
        let mut evidence = LocalBleAssociationEvidence::default();
        evidence.observation_received();
        evidence.observation_with_signal();
        evidence.advertisement_decoded();

        let diagnostics = evidence.failed(PairingFailureStage::TargetNotObserved);

        assert_eq!(
            diagnostics.failure_stage,
            Some(PairingFailureStage::TargetNotObserved)
        );
        assert_eq!(diagnostics.observations_received, 1);
        assert_eq!(diagnostics.identity_matches, 0);
        assert!(!diagnostics.succeeded);
    }

    #[test]
    fn deepest_candidate_proof_failure_survives_later_failures_and_timeout() {
        let mut evidence = LocalBleAssociationEvidence::default();
        evidence.identity_matched();
        evidence.gatt_proof_attempted();
        evidence.candidate_failed(PairingFailureStage::CandidateServiceMismatch);
        evidence.identity_matched();
        evidence.gatt_proof_attempted();
        evidence.candidate_failed(PairingFailureStage::CandidateConnect);

        let diagnostics = evidence.failed(PairingFailureStage::TargetNotObserved);

        assert_eq!(
            diagnostics.failure_stage,
            Some(PairingFailureStage::CandidateServiceMismatch)
        );
        assert_eq!(diagnostics.identity_matches, 2);
        assert_eq!(diagnostics.gatt_proof_attempts, 2);
        assert_eq!(diagnostics.connect_failures, 1);
        assert_eq!(diagnostics.service_mismatches, 1);
    }

    #[test]
    fn successful_candidate_clears_failure_stage_but_retains_bounded_evidence() {
        let mut evidence = LocalBleAssociationEvidence::default();
        evidence.identity_matched();
        evidence.gatt_proof_attempted();
        evidence.candidate_failed(PairingFailureStage::CandidateConnect);
        evidence.identity_matched();

        let diagnostics = evidence.succeeded();

        assert!(diagnostics.succeeded);
        assert_eq!(diagnostics.failure_stage, None);
        assert_eq!(diagnostics.identity_matches, 2);
        assert_eq!(diagnostics.connect_failures, 1);
    }

    #[test]
    fn cleanup_failure_has_precedence_over_candidate_proof_evidence() {
        let mut evidence = LocalBleAssociationEvidence::default();
        evidence.candidate_failed(PairingFailureStage::CandidateServiceMismatch);
        evidence.candidate_failed(PairingFailureStage::CandidateCleanup);

        let diagnostics = evidence.failed(PairingFailureStage::Transport);

        assert_eq!(
            diagnostics.failure_stage,
            Some(PairingFailureStage::CandidateCleanup)
        );
        assert_eq!(diagnostics.service_mismatches, 1);
        assert_eq!(diagnostics.cleanup_failures, 1);
    }

    #[test]
    fn candidate_attempt_retains_validation_failure_when_cleanup_also_fails() {
        let mut evidence = LocalBleAssociationEvidence::default();

        let validation_succeeded =
            evidence.record_candidate_validation(Err(PairingFailureStage::CandidateConnect));
        evidence.candidate_failed(PairingFailureStage::CandidateCleanup);
        let diagnostics = evidence.failed(PairingFailureStage::Transport);

        assert!(!validation_succeeded);
        assert_eq!(
            diagnostics.failure_stage,
            Some(PairingFailureStage::CandidateCleanup)
        );
        assert_eq!(diagnostics.connect_failures, 1);
        assert_eq!(diagnostics.cleanup_failures, 1);
    }

    #[test]
    fn cleanup_attempt_deadline_leaves_a_tail_for_typed_timeout_delivery() {
        let start = Instant::now();
        let outer_end = start + Duration::from_secs(60);
        let budget =
            LocalBleAssociationDeadline::new_at(outer_end, Duration::from_secs(2), start).unwrap();
        let result_margin = Duration::from_millis(250);

        let attempt_end = budget
            .cleanup_attempt_end_at(budget.proof_end(), result_margin)
            .unwrap();

        assert_eq!(attempt_end, outer_end - result_margin);
        assert!(attempt_end < budget.cleanup_end());
        assert!(budget
            .cleanup_attempt_end_at(attempt_end, result_margin)
            .is_err());
    }

    #[test]
    fn diagnostic_counters_saturate_at_their_public_bound() {
        let mut diagnostics = LocalBleAssociationDiagnostics {
            observations_received: u16::MAX,
            ..LocalBleAssociationDiagnostics::default()
        };

        LocalBleAssociationDiagnostics::increment(&mut diagnostics.observations_received);

        assert_eq!(diagnostics.observations_received, u16::MAX);
    }

    #[test]
    fn readiness_is_emitted_after_listener_install_and_before_observation() {
        let sequence = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let install_sequence = sequence.clone();
        let ready_sequence = sequence.clone();
        let mut on_ready = move || ready_sequence.lock().unwrap().push("ready");

        let listener = install_association_listener(
            move || {
                install_sequence.lock().unwrap().push("listener_installed");
                Ok(())
            },
            &mut on_ready,
        )
        .unwrap();
        sequence.lock().unwrap().push("observation_window");

        assert_eq!(listener, ());
        assert_eq!(
            *sequence.lock().unwrap(),
            ["listener_installed", "ready", "observation_window"]
        );
    }

    #[test]
    fn listener_install_failure_does_not_emit_false_readiness() {
        let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let callback_ready = ready.clone();
        let mut on_ready = move || callback_ready.store(true, std::sync::atomic::Ordering::SeqCst);

        let result = install_association_listener::<()>(
            || anyhow::bail!("listener unavailable"),
            &mut on_ready,
        );

        assert!(result.is_err());
        assert!(!ready.load(std::sync::atomic::Ordering::SeqCst));
    }
}

/// Validate, durably deduplicate, and emit every normalized profile event.
///
/// Persistence happens before emission, so repeated observations and process
/// restarts cannot replay the same profile-defined transition.
pub fn accept_profile_advertisement(
    advertisement: BleAdvertisementView<'_>,
    device: &LocalBleDevice,
    store: &LocalBleDeviceStore,
    registry: &Arc<Mutex<HubDeviceRegistry>>,
    event_tx: &Sender<HubEvent>,
) -> Result<usize> {
    let Some(profile) = profile_by_id(&device.profile_id) else {
        return Ok(0);
    };
    let mut emitted = 0_usize;
    for profile_event in decode_profile_events(profile, advertisement)? {
        let Some(event) = project_normalized_event(&profile_event.normalized, device, registry)
        else {
            continue;
        };
        if store.accept_replay(
            &device.profile_id,
            &device.stable_identity,
            profile_event.replay.as_ref(),
        )? != ReplayDisposition::New
        {
            continue;
        }
        emitted += usize::from(event_tx.send(event).is_ok());
    }
    Ok(emitted)
}

fn project_normalized_event(
    normalized: &BleNormalizedEvent,
    device: &LocalBleDevice,
    registry: &Arc<Mutex<HubDeviceRegistry>>,
) -> Option<HubEvent> {
    Some(match normalized {
        BleNormalizedEvent::Button {
            endpoint_suffix,
            action,
        } => {
            let button_id = device.button_id(endpoint_suffix)?;
            let room_id = registry
                .lock()
                .ok()
                .and_then(|registry| registry.get_room_for_button(&button_id));
            match room_id {
                Some(room_id) => HubEvent::Button {
                    hub_key: None,
                    room_id,
                    action: *action,
                    device_id: Some(device.id.clone()),
                },
                None => HubEvent::UnroutableButton {
                    hub_key: None,
                    device_id: Some(device.id.clone()),
                    button_id,
                },
            }
        }
        BleNormalizedEvent::Motion { detected } => {
            let room_id = registry
                .lock()
                .ok()
                .and_then(|registry| registry.get_room_for_motion_sensor(&device.id))?;
            HubEvent::Motion {
                hub_key: None,
                room_id,
                sensor_id: device.id.clone(),
                detected: *detected,
            }
        }
        BleNormalizedEvent::Contact { open } => {
            let room_id = registry
                .lock()
                .ok()
                .and_then(|registry| registry.get_room_for_contact_sensor(&device.id))?;
            HubEvent::Contact {
                hub_key: None,
                room_id,
                sensor_id: device.id.clone(),
                open: *open,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{
        profile_by_id, profiles, BleAdvertisement, BleProfileAdmission, ReplayOrder,
        ValidatedBleSetup, OREIN_MANUFACTURER_ID, OREIN_OC02001_PROFILE_ID,
    };
    use rhythm_core::runtime::hub_registry::DeviceType;
    use rhythm_core::ButtonAction;
    use serde::Deserialize;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn temporary_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "rhythm-local-ble-trace-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn frame(counter: u8) -> [u8; 19] {
        let mut payload = [0_u8; 19];
        payload[..3].copy_from_slice(&[counter, 0x02, 0x01]);
        payload
    }

    fn device(counter: Option<u8>) -> LocalBleDevice {
        LocalBleDevice::from_setup(
            OREIN_OC02001_PROFILE_ID,
            &ValidatedBleSetup {
                stable_identity: "A1B2C3D4E5F6".to_string(),
                metadata: BTreeMap::from([
                    ("serial".to_string(), "sanitized-s".to_string()),
                    ("model".to_string(), "sanitized-m".to_string()),
                ]),
            },
            "02:00:00:00:00:01".to_string(),
            counter
                .map(|counter| BTreeMap::from([("press".to_string(), vec![counter])]))
                .unwrap_or_default(),
            false,
            1,
        )
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ConformanceManifest {
        schema_version: u32,
        fixtures: Vec<String>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ConformanceFixture {
        schema_version: u32,
        profile_id: String,
        device_type: String,
        fixture_name: String,
        purpose: String,
        provenance: String,
        setup: serde_json::Value,
        identity_advertisement: FixtureAdvertisement,
        admission: FixtureAdmission,
        initial_replay: BTreeMap<String, String>,
        observations: Vec<FixtureObservation>,
        negative_observations: Vec<FixtureAdvertisement>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(tag = "strategy", rename_all = "snake_case", deny_unknown_fields)]
    enum FixtureAdmission {
        AdvertisementOnly,
        GattServices { matching_service_uuid: String },
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FixtureAdvertisement {
        service_uuids: Vec<String>,
        service_data: BTreeMap<String, String>,
        manufacturer_data: BTreeMap<String, String>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FixtureObservation {
        at_ms: u64,
        advertisement: FixtureAdvertisement,
        events: Vec<FixtureExpectedEvent>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FixtureExpectedEvent {
        event: FixtureEvent,
        replay: Option<FixtureReplay>,
        expected_emit: bool,
    }

    #[derive(Debug, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
    enum FixtureEvent {
        Button {
            endpoint_suffix: String,
            action: ButtonAction,
        },
        Motion {
            detected: bool,
        },
        Contact {
            open: bool,
        },
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FixtureReplay {
        stream: String,
        value_hex: String,
        expected_order: FixtureReplayOrder,
    }

    #[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    enum FixtureReplayOrder {
        New,
        Forward,
        Duplicate,
        Stale,
    }

    fn fixture_bytes(value: &str) -> Vec<u8> {
        assert_eq!(value.len() % 2, 0, "fixture hex must contain whole bytes");
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16)
                    .expect("fixture hex must be valid")
            })
            .collect()
    }

    fn fixture_advertisement(fixture: &FixtureAdvertisement) -> BleAdvertisement {
        BleAdvertisement::try_from_parts(
            fixture
                .service_uuids
                .iter()
                .map(|value| Uuid::parse_str(value).expect("fixture service UUID must be valid")),
            fixture.service_data.iter().map(|(service_uuid, payload)| {
                (
                    Uuid::parse_str(service_uuid).expect("fixture service-data UUID must be valid"),
                    fixture_bytes(payload),
                )
            }),
            fixture
                .manufacturer_data
                .iter()
                .map(|(manufacturer_id, payload)| {
                    (
                        manufacturer_id
                            .parse::<u16>()
                            .expect("fixture manufacturer ID must be a decimal u16"),
                        fixture_bytes(payload),
                    )
                }),
        )
        .expect("fixture advertisement must satisfy host bounds")
    }

    fn assert_normalized_event(actual: &BleNormalizedEvent, expected: &FixtureEvent) {
        match (actual, expected) {
            (
                BleNormalizedEvent::Button {
                    endpoint_suffix: actual_suffix,
                    action: actual_action,
                },
                FixtureEvent::Button {
                    endpoint_suffix: expected_suffix,
                    action: expected_action,
                },
            ) => {
                assert_eq!(actual_suffix, expected_suffix);
                assert_eq!(actual_action, expected_action);
            }
            (
                BleNormalizedEvent::Motion { detected: actual },
                FixtureEvent::Motion { detected: expected },
            ) => assert_eq!(actual, expected),
            (
                BleNormalizedEvent::Contact { open: actual },
                FixtureEvent::Contact { open: expected },
            ) => assert_eq!(actual, expected),
            (actual, expected) => {
                panic!("normalized fixture event mismatch: {actual:?} vs {expected:?}")
            }
        }
    }

    fn replay_conformance_fixture(fixture: &ConformanceFixture) -> (Vec<usize>, usize) {
        assert_eq!(fixture.schema_version, 3);
        assert!(!fixture.fixture_name.is_empty());
        assert!(!fixture.purpose.is_empty());
        assert_eq!(fixture.provenance, "synthetic");
        assert!(fixture
            .observations
            .windows(2)
            .all(|steps| steps[0].at_ms < steps[1].at_ms));

        let profile =
            profile_by_id(&fixture.profile_id).expect("fixture profile must be registered");
        let descriptor = profile.descriptor();
        assert_eq!(fixture.profile_id, descriptor.id);
        assert_eq!(fixture.device_type, descriptor.device_type);
        let setup = profile
            .parse_pairing_setup(&fixture.setup)
            .expect("fixture setup must pass profile validation");
        let identity_advertisement = fixture_advertisement(&fixture.identity_advertisement);
        assert_eq!(
            profile.stable_identity_from_advertisement(identity_advertisement.view()),
            Some(setup.stable_identity().to_string())
        );
        match (&fixture.admission, profile.admission()) {
            (FixtureAdmission::AdvertisementOnly, BleProfileAdmission::AdvertisementOnly) => {}
            (
                FixtureAdmission::GattServices {
                    matching_service_uuid,
                },
                admission @ BleProfileAdmission::GattServiceProof { .. },
            ) => assert!(admission.accepts_gatt_service(matching_service_uuid)),
            (fixture, profile) => {
                panic!("fixture/profile admission mismatch: {fixture:?} vs {profile:?}")
            }
        }

        let mut replay_state = fixture
            .initial_replay
            .iter()
            .map(|(stream, value)| (stream.clone(), fixture_bytes(value)))
            .collect::<BTreeMap<_, _>>();
        let root = temporary_dir();
        let store = LocalBleDeviceStore::load(&root).unwrap();
        let device = LocalBleDevice::from_setup(
            &fixture.profile_id,
            &setup,
            "02:00:00:00:00:01".to_string(),
            replay_state.clone(),
            false,
            1,
        );
        store.upsert(device.clone()).unwrap();
        let registry = Arc::new(Mutex::new(HubDeviceRegistry::new()));
        let projection = profile.projection();
        let buttons = projection
            .button_endpoints
            .iter()
            .map(|endpoint| {
                (
                    device
                        .button_id(endpoint.suffix)
                        .expect("declared endpoint suffix must be valid"),
                    endpoint.control_id,
                )
            })
            .collect::<Vec<_>>();
        registry.lock().unwrap().upsert_device(
            &device.id,
            Some("test-room"),
            &buttons,
            projection.device_type,
        );
        let (tx, rx) = std::sync::mpsc::channel();

        let emitted = fixture
            .observations
            .iter()
            .map(|step| {
                let advertisement = fixture_advertisement(&step.advertisement);
                assert_eq!(
                    profile.stable_identity_from_advertisement(advertisement.view()),
                    Some(setup.stable_identity().to_string())
                );
                let decoded = decode_profile_events(profile, advertisement.view())
                    .expect("fixture decoder output must satisfy host bounds");
                assert_eq!(decoded.len(), step.events.len());
                for (decoded, expected) in decoded.iter().zip(&step.events) {
                    assert_normalized_event(&decoded.normalized, &expected.event);
                    match (&decoded.replay, &expected.replay) {
                        (None, None) => {}
                        (Some(actual), Some(expected)) => {
                            assert_eq!(actual.stream, expected.stream);
                            assert_eq!(actual.value, fixture_bytes(&expected.value_hex));
                            let order = replay_state.get(&actual.stream).map_or(
                                FixtureReplayOrder::New,
                                |previous| match profile.replay_order(
                                    &actual.stream,
                                    previous,
                                    &actual.value,
                                ) {
                                    ReplayOrder::Forward => FixtureReplayOrder::Forward,
                                    ReplayOrder::Duplicate => FixtureReplayOrder::Duplicate,
                                    ReplayOrder::Stale => FixtureReplayOrder::Stale,
                                },
                            );
                            assert_eq!(order, expected.expected_order);
                            if matches!(
                                order,
                                FixtureReplayOrder::New | FixtureReplayOrder::Forward
                            ) {
                                replay_state.insert(actual.stream.clone(), actual.value.clone());
                            }
                        }
                        (actual, expected) => {
                            panic!("fixture replay mismatch: {actual:?} vs {expected:?}")
                        }
                    }
                }

                let emitted_count = accept_profile_advertisement(
                    advertisement.view(),
                    &device,
                    &store,
                    &registry,
                    &tx,
                )
                .unwrap();
                let expected_count = step
                    .events
                    .iter()
                    .filter(|event| event.expected_emit)
                    .count();
                assert_eq!(emitted_count, expected_count);
                emitted_count
            })
            .collect::<Vec<_>>();

        for step in &fixture.negative_observations {
            let advertisement = fixture_advertisement(step);
            assert!(decode_profile_events(profile, advertisement.view())
                .expect("negative fixture decoder output must satisfy host bounds")
                .is_empty());
            assert_eq!(
                accept_profile_advertisement(
                advertisement.view(),
                &device,
                &store,
                &registry,
                &tx,
            )
                .unwrap(),
                0
            );
        }
        let event_count = rx.try_iter().count();
        std::fs::remove_dir_all(root).unwrap();
        (emitted, event_count)
    }

    /// Every checked-in profile fixture is discovered through one manifest and
    /// replayed through the generic host. A motion, contact, multi-control, or
    /// counterless profile adds data to the corpus rather than a new harness.
    #[test]
    fn sanitized_profile_fixture_corpus_is_complete_and_deterministic() {
        let fixture_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let manifest: ConformanceManifest = serde_json::from_str(
            &std::fs::read_to_string(fixture_root.join("conformance_manifest.json"))
                .expect("conformance manifest must be readable"),
        )
        .expect("conformance manifest must match its versioned schema");
        assert_eq!(manifest.schema_version, 1);
        assert!(!manifest.fixtures.is_empty());

        let listed = manifest
            .fixtures
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            listed.len(),
            manifest.fixtures.len(),
            "fixture names must be unique"
        );
        assert!(listed.iter().all(|name| {
            let path = std::path::Path::new(name);
            path.components().count() == 1 && path.extension().is_some_and(|value| value == "json")
        }));
        let on_disk = std::fs::read_dir(&fixture_root)
            .expect("fixture directory must be readable")
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.ends_with(".json") && name != "conformance_manifest.json")
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            listed, on_disk,
            "every fixture must be listed in the corpus manifest"
        );

        let mut fixture_names = std::collections::BTreeSet::new();
        let mut fixture_profile_ids = std::collections::BTreeSet::new();
        for name in &manifest.fixtures {
            let fixture: ConformanceFixture = serde_json::from_str(
                &std::fs::read_to_string(fixture_root.join(name))
                    .expect("profile fixture must be readable"),
            )
            .expect("profile fixture must match its versioned schema");
            assert!(
                fixture_names.insert(fixture.fixture_name.clone()),
                "fixture_name values must be unique: {}",
                fixture.fixture_name
            );
            fixture_profile_ids.insert(fixture.profile_id.clone());
            let expected = fixture
                .observations
                .iter()
                .map(|step| {
                    step.events
                        .iter()
                        .filter(|event| event.expected_emit)
                        .count()
                })
                .collect::<Vec<_>>();

            let first = replay_conformance_fixture(&fixture);
            let second = replay_conformance_fixture(&fixture);

            assert_eq!(
                first, second,
                "fixture {name} must replay deterministically"
            );
            assert_eq!(first.0, expected, "fixture {name} emit counts changed");
            assert_eq!(
                first.1,
                expected.iter().sum::<usize>(),
                "fixture {name} projected event count changed"
            );
        }

        let registered_profile_ids = profiles()
            .iter()
            .map(|profile| profile.descriptor().id.to_string())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            fixture_profile_ids, registered_profile_ids,
            "every registered current profile needs at least one conformance fixture"
        );
    }

    #[test]
    fn unacknowledged_replay_commit_never_emits_button_event() {
        let root = temporary_dir();
        let store = LocalBleDeviceStore::load(&root).unwrap();
        let device = device(None);
        store.upsert(device.clone()).unwrap();
        let registry = Arc::new(Mutex::new(HubDeviceRegistry::new()));
        registry.lock().unwrap().upsert_device(
            &device.id,
            Some("test-room"),
            &[(device.button_id("button-1").unwrap(), 1)],
            DeviceType::Button,
        );
        let (tx, rx) = std::sync::mpsc::channel();
        let advertisement =
            BleAdvertisement::try_from_parts([], [], [(OREIN_MANUFACTURER_ID, frame(1).to_vec())])
                .unwrap();

        store.fail_next_directory_sync();
        assert_eq!(
            accept_profile_advertisement(advertisement.view(), &device, &store, &registry, &tx,)
                .unwrap(),
            0
        );
        assert!(store.durability_degraded());
        assert!(rx.try_recv().is_err());

        // The logical replay commit remains the at-most-once head. If the
        // rename survives a restart, observing the same press is a duplicate
        // and cannot turn the unacknowledged commit into a late event.
        let restarted = LocalBleDeviceStore::load(&root).unwrap();
        assert_eq!(
            accept_profile_advertisement(
                advertisement.view(),
                &device,
                &restarted,
                &registry,
                &tx,
            )
            .unwrap(),
            0
        );
        assert!(rx.try_recv().is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn event_dispatch_uses_the_stored_profile_id_instead_of_an_orein_default() {
        let root = temporary_dir();
        let store = LocalBleDeviceStore::load(&root).unwrap();
        let mut device = device(None);
        device.profile_id = "example.future.sensor.v1".to_string();
        let registry = Arc::new(Mutex::new(HubDeviceRegistry::new()));
        let (tx, _rx) = std::sync::mpsc::channel();

        let advertisement =
            BleAdvertisement::try_from_parts([], [], [(OREIN_MANUFACTURER_ID, frame(1).to_vec())])
                .unwrap();
        assert_eq!(
            accept_profile_advertisement(advertisement.view(), &device, &store, &registry, &tx,)
                .unwrap(),
            0
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_supplied_button_action_and_endpoint_are_preserved() {
        let device = device(None);
        let registry = Arc::new(Mutex::new(HubDeviceRegistry::new()));
        registry.lock().unwrap().upsert_device(
            &device.id,
            Some("test-room"),
            &[(device.button_id("button-1").unwrap(), 1)],
            DeviceType::Button,
        );

        let event = project_normalized_event(
            &BleNormalizedEvent::Button {
                endpoint_suffix: "button-1".to_string(),
                action: ButtonAction::DownHold,
            },
            &device,
            &registry,
        )
        .unwrap();
        assert!(matches!(
            event,
            HubEvent::Button {
                action: ButtonAction::DownHold,
                ..
            }
        ));
        assert!(project_normalized_event(
            &BleNormalizedEvent::Button {
                endpoint_suffix: "undeclared".to_string(),
                action: ButtonAction::OnPress,
            },
            &device,
            &registry,
        )
        .is_none());
    }
}
