//! Matter device discovery for room sync.

use std::sync::Arc;

use anyhow::Result;
use rhythm_os::discovery::{DiscoveredDevice, DiscoveredRoom, HubDiscovery};

use crate::transport::MatterTransport;

/// Matter hub discovery — intentionally empty.
///
/// Matter devices are tracked through explicit pairing and the canonical
/// registry, not through hub sync discovery.
pub struct MatterDiscovery {
    _transport: Arc<dyn MatterTransport>,
}

impl MatterDiscovery {
    pub fn new(transport: Arc<dyn MatterTransport>) -> Self {
        Self {
            _transport: transport,
        }
    }
}

impl HubDiscovery for MatterDiscovery {
    fn discover_rooms(&self) -> Result<Vec<DiscoveredRoom>> {
        Ok(Vec::new())
    }

    fn discover_devices(&self) -> Result<Vec<DiscoveredDevice>> {
        Ok(Vec::new())
    }
}
