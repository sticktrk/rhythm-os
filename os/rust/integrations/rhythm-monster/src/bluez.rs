//! Linux fallback. BlueZ handles are created and destroyed inside the shared
//! rhythm-ble driver's admitted operation; never create a private radio runtime.
use crate::{
    ble::{self, LightBleTransport, LightGatt, LightWifiConfig},
    ble_cleanup::with_disconnect,
    LightError, LightResult, LightSecret,
};
use async_trait::async_trait;
use bluer::{
    gatt::{
        remote::{Characteristic, CharacteristicWriteRequest},
        WriteOp,
    },
    Address, Device,
};
use rhythm_ble::bluez::{BluezClient, BluezDriverId, DetachedBluezOutput};
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone)]
pub struct LightBluezTransport {
    address: Address,
}
impl LightBluezTransport {
    /// Address must come from the shared runtime's discovery subscription,
    /// filtered by Ayla ID_SERVICE. DSN readback is mandatory before writes.
    pub fn new(discovered_address: &str) -> LightResult<Self> {
        Ok(Self {
            address: discovered_address
                .parse()
                .map_err(|_| LightError::InvalidInput)?,
        })
    }
    /// Synchronous identify for callers already on a blocking thread (the
    /// appliance pairing handler). Runs inside the shared admitted operation.
    pub fn identify_blocking(&self) -> LightResult<String> {
        let address = self.address;
        let client =
            BluezClient::new(BluezDriverId::Monster).map_err(|_| LightError::Unavailable)?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(13);
        let output = client
            .run_adapter_operation_for(
                &address.to_string(),
                Duration::from_secs(5),
                Duration::from_secs(20),
                move |_, adapter| async move {
                    let device = adapter.device(address)?;
                    let result = with_disconnect(
                        deadline,
                        LightError::Unavailable,
                        async {
                            device
                                .connect()
                                .await
                                .map_err(|_| LightError::Unavailable)?;
                            ble::identify_gatt(&Gatt(device.clone())).await
                        },
                        async {
                            device
                                .disconnect()
                                .await
                                .map_err(|_| LightError::Unavailable)
                        },
                    )
                    .await;
                    DetachedBluezOutput::from_value(result)
                },
            )
            .map_err(|_| LightError::Unavailable)?;
        output.into_value().map_err(|_| LightError::Unavailable)?
    }

    /// Synchronous provisioning counterpart of `identify_blocking`.
    pub fn provision_blocking(
        &self,
        dsn: &str,
        token: &LightSecret,
        wifi: &LightWifiConfig,
    ) -> LightResult<()> {
        let address = self.address;
        let dsn = dsn.to_owned();
        let token = token.clone();
        let wifi = wifi.clone();
        let client =
            BluezClient::new(BluezDriverId::Monster).map_err(|_| LightError::Unavailable)?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(105);
        let output = client
            .run_adapter_operation_for(
                &address.to_string(),
                Duration::from_secs(5),
                Duration::from_secs(115),
                move |_, adapter| async move {
                    let device = adapter.device(address)?;
                    let result = with_disconnect(
                        deadline,
                        LightError::Uncertain,
                        async {
                            device
                                .connect()
                                .await
                                .map_err(|_| LightError::Unavailable)?;
                            ble::provision_gatt(&Gatt(device.clone()), &dsn, &token, &wifi).await
                        },
                        async { device.disconnect().await.map_err(|_| LightError::Uncertain) },
                    )
                    .await;
                    DetachedBluezOutput::from_value(result)
                },
            )
            .map_err(|_| LightError::Uncertain)?;
        output.into_value().map_err(|_| LightError::Uncertain)?
    }
}
struct Gatt(Device);
impl Gatt {
    async fn characteristic(
        &self,
        service: &str,
        characteristic: &str,
    ) -> LightResult<Characteristic> {
        let service = Uuid::parse_str(service).map_err(|_| LightError::InvalidInput)?;
        let characteristic =
            Uuid::parse_str(characteristic).map_err(|_| LightError::InvalidInput)?;
        for s in self
            .0
            .services()
            .await
            .map_err(|_| LightError::Unavailable)?
        {
            if s.uuid().await.map_err(|_| LightError::Unavailable)? != service {
                continue;
            }
            for c in s
                .characteristics()
                .await
                .map_err(|_| LightError::Unavailable)?
            {
                if c.uuid().await.map_err(|_| LightError::Unavailable)? == characteristic {
                    return Ok(c);
                }
            }
        }
        Err(LightError::Unsupported)
    }
}
#[async_trait]
impl LightGatt for Gatt {
    async fn read(&self, service: &str, characteristic: &str) -> LightResult<Vec<u8>> {
        self.characteristic(service, characteristic)
            .await?
            .read()
            .await
            .map_err(|_| LightError::Unavailable)
    }
    async fn write(&self, service: &str, characteristic: &str, value: &[u8]) -> LightResult<()> {
        self.characteristic(service, characteristic)
            .await?
            .write_ext(
                value,
                &CharacteristicWriteRequest {
                    op_type: WriteOp::Request,
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| LightError::Uncertain)
    }
}
#[async_trait]
impl LightBleTransport for LightBluezTransport {
    async fn identify(&self) -> LightResult<String> {
        let transport = self.clone();
        tokio::task::spawn_blocking(move || transport.identify_blocking())
            .await
            .map_err(|_| LightError::Unavailable)?
    }
    async fn provision(
        &self,
        dsn: &str,
        token: &LightSecret,
        wifi: &LightWifiConfig,
    ) -> LightResult<()> {
        let transport = self.clone();
        let dsn = dsn.to_owned();
        let token = token.clone();
        let wifi = wifi.clone();
        tokio::task::spawn_blocking(move || transport.provision_blocking(&dsn, &token, &wifi))
            .await
            .map_err(|_| LightError::Uncertain)?
    }
}
