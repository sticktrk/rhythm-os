use serde::Deserialize;
use std::{fmt, net::Ipv4Addr};
use zeroize::Zeroize;

pub type LightResult<T> = Result<T, LightError>;
#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq, serde::Serialize, Deserialize)]
pub enum LightError {
    #[error("invalid Monster input")]
    InvalidInput,
    #[error("Monster authentication required")]
    Authentication,
    #[error("Monster cloud request failed")]
    Cloud,
    #[error("Monster device identity mismatch")]
    Identity,
    #[error("Monster protocol authentication failed")]
    Integrity,
    #[error("unsupported Monster model or feature")]
    Unsupported,
    #[error("Monster operation timed out; delivery may be uncertain")]
    Timeout,
    #[error("Monster transport unavailable")]
    Unavailable,
    #[error("Monster provisioning delivery uncertain; reconcile before retry")]
    Uncertain,
    #[error("Monster Wi-Fi provisioning failed")]
    Wifi,
    #[error("Monster LAN readback mismatch")]
    Readback,
}

#[derive(Clone, Deserialize)]
#[serde(transparent)]
pub struct LightSecret(String);
impl LightSecret {
    pub fn new(value: String) -> Self {
        Self(value)
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for LightSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl Drop for LightSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Clone, Deserialize)]
pub struct LightCredentials {
    pub dsn: String,
    pub ip: Ipv4Addr,
    pub local_key: LightSecret,
    pub local_key_id: u32,
}
impl fmt::Debug for LightCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LightCredentials([REDACTED])")
    }
}
pub fn validate_dsn(dsn: &str) -> LightResult<()> {
    if !(8..=32).contains(&dsn.len()) || !dsn.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(LightError::InvalidInput);
    }
    Ok(())
}
impl LightCredentials {
    pub fn validate(&self) -> LightResult<()> {
        validate_dsn(&self.dsn)?;
        if !self.ip.is_private() || !(8..=128).contains(&self.local_key.expose().len()) {
            return Err(LightError::InvalidInput);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightProperty {
    Power,
    Brightness,
    ColorBrightness,
    Color,
    Saturation,
    Mode,
}
impl LightProperty {
    pub fn name(self) -> &'static str {
        match self {
            Self::Power => "power",
            Self::Brightness => "brightness",
            Self::ColorBrightness => "color_bright",
            Self::Color => "color_select",
            Self::Saturation => "color_saturation",
            Self::Mode => "mode",
        }
    }
    pub fn base_type(self) -> &'static str {
        match self {
            Self::Power => "boolean",
            Self::Mode => "string",
            _ => "integer",
        }
    }
    pub fn validate(self, value: &serde_json::Value) -> LightResult<()> {
        let valid = match self {
            Self::Power => value.as_u64().is_some_and(|v| v <= 1),
            Self::Mode => value.as_str() == Some("color"),
            Self::Color => value.as_u64().is_some_and(|v| v <= 0xffffff),
            _ => value.as_u64().is_some_and(|v| v <= 100),
        };
        if valid {
            Ok(())
        } else {
            Err(LightError::InvalidInput)
        }
    }
}
