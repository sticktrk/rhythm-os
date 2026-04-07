//! WiFi connection management for ESP32-C6.
//!
//! Handles WiFi station mode connection with automatic reconnection.

use anyhow::{bail, Result};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::WifiModemPeripheral;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use log::info;

/// Connect to a WiFi network in station mode.
///
/// Accepts any modem type implementing `WifiModemPeripheral` (both `Modem`
/// and the split `WifiModem` work).
pub fn connect_wifi<'a, M: WifiModemPeripheral + 'a>(
    modem: M,
    sysloop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    ssid: &str,
    password: &str,
) -> Result<BlockingWifi<EspWifi<'a>>> {
    let mut wifi = BlockingWifi::wrap(EspWifi::new(modem, sysloop.clone(), Some(nvs))?, sysloop)?;

    let wifi_configuration = Configuration::Client(ClientConfiguration {
        ssid: ssid
            .try_into()
            .map_err(|_| anyhow::anyhow!("SSID too long"))?,
        password: password
            .try_into()
            .map_err(|_| anyhow::anyhow!("Password too long"))?,
        ..Default::default()
    });

    wifi.set_configuration(&wifi_configuration)?;

    wifi.start()?;
    info!("WiFi started, connecting...");

    wifi.connect()?;
    info!("WiFi connected, waiting for IP...");

    wifi.wait_netif_up()?;
    info!("WiFi netif up");

    let ip_info = wifi.wifi().sta_netif().get_ip_info()?;
    info!("WiFi IP: {}", ip_info.ip);

    if ip_info.ip.is_unspecified() {
        bail!("Failed to get IP address");
    }

    Ok(wifi)
}
