//! Portable boundary around the vendor BLE protocol.

use anyhow::{Context, Result};
use std::time::Instant;

use super::types::{
    HueBleCommand, HueBleDevice, HueBlePairingOutcome, HueBlePairingRequest, HueBleState,
};

/// Operations needed by pairing, discovery, and the standard light controller.
///
/// A concrete transport owns adapter-specific connection and retry behavior.
pub trait HueBleTransport: Send + Sync {
    fn is_available(&self) -> Result<bool>;

    /// Permanently reject new adapter work and wait for any operation already
    /// in flight to finish.
    ///
    /// Appliance factory reset calls this after removing hub controllers but
    /// before stopping BlueZ and rotating its key database. Portable/mock
    /// transports without background adapter resources may keep the no-op
    /// default.
    fn quiesce(&self) -> Result<()> {
        Ok(())
    }

    /// Discover every newly advertising eligible Hue light, bond each one,
    /// and read its stable identity/capabilities.
    ///
    /// Implementations may recover partial success: return every successfully
    /// bonded bulb and fail only when none of the discovered bulbs can pair.
    fn pair_lights(
        &self,
        request: &HueBlePairingRequest,
        record_bond_intent: &(dyn Fn(&str) -> Result<()> + Send + Sync),
    ) -> Result<HueBlePairingOutcome>;

    fn apply_command(&self, device: &HueBleDevice, command: &HueBleCommand) -> Result<()>;

    fn read_state(&self, device: &HueBleDevice) -> Result<HueBleState>;

    /// Opportunistic observer read. Implementations should avoid initiating a
    /// long connection attempt or waiting behind foreground light commands.
    fn read_state_passive(&self, device: &HueBleDevice) -> Result<Option<HueBleState>> {
        self.read_state(device).map(Some)
    }

    /// Whether BlueZ still owns an authenticated local bond for this device.
    fn has_local_bond(&self, device: &HueBleDevice) -> Result<bool>;

    /// Every paired BlueZ address advertising Hue's vendor service, including
    /// crash-orphan bonds that may not have Rhythm metadata yet.
    fn local_hue_bond_addresses(&self) -> Result<Vec<String>>;

    /// Reconstruct stable Hue metadata from an already bonded BlueZ address.
    /// Factory reset uses this only for a local Hue key that is missing from
    /// Rhythm's store, so it can execute the vendor handoff instead of either
    /// silently scrubbing or permanently blocking reset.
    fn inspect_local_bond(&self, address: &str) -> Result<HueBleDevice>;

    /// Non-mutating preflight that proves this exact bonded bulb is reachable
    /// and exposes the authenticated vendor handoff characteristic.
    fn validate_pairing_handoff(&self, device: &HueBleDevice) -> Result<()>;

    /// Ask the authenticated Hue bulb to open its short-lived replacement
    /// pairing window. This must happen before deleting the Pi-side key.
    ///
    /// Returns the instant captured immediately after the authenticated write
    /// so callers can refuse a delayed key deletion.
    fn prepare_pairing_handoff(&self, device: &HueBleDevice) -> Result<Instant>;

    /// Delete only the Pi-side BlueZ device and link key.
    ///
    /// A deadline is supplied when this deletion follows Hue's short-lived
    /// authenticated replacement-pairing handoff. Implementations must check
    /// it immediately before crossing the destructive adapter boundary.
    /// Stale-bond cleanup that does not depend on a fresh handoff passes
    /// `None`.
    fn remove_local_bond(
        &self,
        device: &HueBleDevice,
        handoff_valid_until: Option<Instant>,
    ) -> Result<()>;

    fn identify(&self, device: &HueBleDevice) -> Result<()> {
        let before = self
            .read_state(device)
            .context("reading Hue bulb state before identify")?;
        let flash_result: Result<()> = (|| {
            for _ in 0..2 {
                self.apply_command(
                    device,
                    &HueBleCommand {
                        on: Some(true),
                        brightness: Some(254),
                        ..Default::default()
                    },
                )?;
                std::thread::sleep(std::time::Duration::from_millis(250));
                self.apply_command(
                    device,
                    &HueBleCommand {
                        on: Some(false),
                        ..Default::default()
                    },
                )?;
                std::thread::sleep(std::time::Duration::from_millis(180));
            }
            Ok(())
        })();
        let restore_result = self.apply_command(
            device,
            &HueBleCommand {
                on: before.on,
                brightness: before.brightness,
                color: before.color,
                effect: before.effect,
                effect_speed: None,
            },
        );

        match (flash_result, restore_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error).context("flashing Hue bulb for identify"),
            (Ok(()), Err(error)) => Err(error).context("restoring Hue bulb after identify"),
            (Err(flash_error), Err(restore_error)) => anyhow::bail!(
                "Flashing Hue bulb for identify failed: {flash_error:#}; restoring its prior state also failed: {restore_error:#}"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::ble::types::{HueBleCapabilities, HueBleColor};

    struct FailingIdentifyTransport {
        commands: Mutex<Vec<HueBleCommand>>,
        fail_on_call: usize,
    }

    impl HueBleTransport for FailingIdentifyTransport {
        fn is_available(&self) -> Result<bool> {
            Ok(true)
        }

        fn pair_lights(
            &self,
            _request: &HueBlePairingRequest,
            _record_bond_intent: &(dyn Fn(&str) -> Result<()> + Send + Sync),
        ) -> Result<HueBlePairingOutcome> {
            unreachable!()
        }

        fn apply_command(&self, _device: &HueBleDevice, command: &HueBleCommand) -> Result<()> {
            let mut commands = self.commands.lock().unwrap();
            commands.push(*command);
            if commands.len() == self.fail_on_call {
                anyhow::bail!("injected write failure");
            }
            Ok(())
        }

        fn read_state(&self, _device: &HueBleDevice) -> Result<HueBleState> {
            Ok(HueBleState {
                on: Some(true),
                brightness: Some(91),
                color: Some(HueBleColor::ColorTemperature { mired: 367 }),
                effect: None,
            })
        }

        fn has_local_bond(&self, _device: &HueBleDevice) -> Result<bool> {
            Ok(true)
        }

        fn local_hue_bond_addresses(&self) -> Result<Vec<String>> {
            Ok(Vec::new())
        }

        fn inspect_local_bond(&self, _address: &str) -> Result<HueBleDevice> {
            anyhow::bail!("not implemented by identify-only test transport")
        }

        fn validate_pairing_handoff(&self, _device: &HueBleDevice) -> Result<()> {
            Ok(())
        }

        fn prepare_pairing_handoff(&self, _device: &HueBleDevice) -> Result<Instant> {
            Ok(Instant::now())
        }

        fn remove_local_bond(
            &self,
            _device: &HueBleDevice,
            _handoff_valid_until: Option<Instant>,
        ) -> Result<()> {
            Ok(())
        }
    }

    fn device() -> HueBleDevice {
        HueBleDevice {
            id: "hue-ble-test".to_string(),
            address: "00:00:00:00:00:00".to_string(),
            address_type: "random".to_string(),
            eui64: "0017880100000000".to_string(),
            name: "Hue lamp".to_string(),
            manufacturer: "Signify Netherlands B.V.".to_string(),
            model: "LCA013".to_string(),
            firmware: "1".to_string(),
            capabilities: HueBleCapabilities::default(),
            paired_at_epoch_secs: 0,
            last_state: None,
        }
    }

    #[test]
    fn identify_attempts_to_restore_state_after_a_flash_write_fails() {
        let transport = FailingIdentifyTransport {
            commands: Mutex::new(Vec::new()),
            fail_on_call: 2,
        };

        assert!(transport.identify(&device()).is_err());
        let commands = transport.commands.lock().unwrap();
        assert_eq!(commands.len(), 3);
        assert_eq!(
            commands.last().copied(),
            Some(HueBleCommand {
                on: Some(true),
                brightness: Some(91),
                color: Some(HueBleColor::ColorTemperature { mired: 367 }),
                effect: None,
                effect_speed: None,
            })
        );
    }
}
