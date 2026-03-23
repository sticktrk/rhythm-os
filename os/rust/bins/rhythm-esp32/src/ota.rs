//! OTA firmware update module.
//!
//! Receives firmware bytes pushed by a client over HTTP POST and
//! streams them to the OTA partition. No outbound TLS connections needed —
//! the app downloads the binary and pushes it over local HTTP.

use anyhow::Result;
use esp_idf_svc::ota::EspOta;
use log::info;

/// Chunk size for reading the firmware binary (4KB).
const OTA_CHUNK_SIZE: usize = 4096;

/// Write firmware to the next OTA partition from a reader.
///
/// Reads `total_size` bytes from `reader` in 4KB chunks, writing each to flash.
/// On success, sets the new partition as boot target. The caller should reboot
/// after this returns `Ok(())`.
///
/// On error or drop, `esp_ota_abort()` is called automatically by the
/// `EspOtaUpdate` destructor — no partial writes persist.
pub fn write_ota<R: embedded_svc::io::Read>(reader: &mut R, total_size: usize) -> Result<()>
where
    <R as embedded_svc::io::ErrorType>::Error: std::fmt::Debug,
{
    info!(target: "ota", "Starting OTA write: {} bytes", total_size);

    let mut ota = EspOta::new()?;
    let mut update = ota.initiate_update_with_known_size(total_size)?;

    let mut buf = [0u8; OTA_CHUNK_SIZE];
    let mut written: usize = 0;

    loop {
        let n = embedded_svc::io::Read::read(reader, &mut buf)
            .map_err(|e| anyhow::anyhow!("Read error: {:?}", e))?;
        if n == 0 {
            break;
        }
        embedded_svc::io::Write::write_all(&mut update, &buf[..n])?;
        written += n;
    }

    if written != total_size {
        return Err(anyhow::anyhow!(
            "Size mismatch: expected {}, got {}",
            total_size,
            written
        ));
    }

    update.complete()?;
    info!(target: "ota", "OTA write complete: {} bytes, ready for reboot", written);
    Ok(())
}
