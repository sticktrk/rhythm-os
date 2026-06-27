//! Default light runtime module bundle for Rhythm OS hosts.
//!
//! `rhythm-os` owns the generic host registry. This crate owns the concrete
//! runtime modules linked by the default server/add-on/appliance builds.

use std::sync::Arc;

use anyhow::Result;
use rhythm_core::RuntimeHandle;
use rhythm_os::light_runtime::{
    register_light_runtime_modules, LightRuntimeModule, RHYTHM_ADAPTIVE_RUNTIME_ID,
};
use rhythm_os::state::AppState;
use rhythm_runtime_api::LightRuntime;

pub const DEFAULT_LIGHT_RUNTIME_MODULES: &[LightRuntimeModule] = &[LightRuntimeModule::ephemeral(
    RHYTHM_ADAPTIVE_RUNTIME_ID,
    &["rhythm", "rhythm_adaptive"],
    rhythm_adaptive::runtime_manifest,
    create_rhythm_adaptive_runtime,
)];

pub fn install_default_light_runtime_modules(state: &mut AppState) -> Result<()> {
    register_light_runtime_modules(state, DEFAULT_LIGHT_RUNTIME_MODULES.iter().copied())
}

fn create_rhythm_adaptive_runtime(runtime: Arc<dyn RuntimeHandle>) -> Box<dyn LightRuntime> {
    Box::new(rhythm_adaptive::RuntimeHandleAdaptiveRuntime::new(runtime))
}
