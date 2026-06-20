//! Default light runtime module bundle for Rhythm OS hosts.
//!
//! `rhythm-os` owns the generic host registry. This crate owns the concrete
//! runtime modules linked by the default server/add-on/appliance builds.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use rhythm_core::RuntimeHandle;
use rhythm_os::light_runtime::{
    build_runtime_snapshot, register_light_runtime_modules, LightRuntimeModule, SharedLightRuntime,
    removed_circadian_RUNTIME_ID, RHYTHM_ADAPTIVE_RUNTIME_ID,
};
use rhythm_os::state::{AppState, SharedState};
use rhythm_runtime_api::LightRuntime;

pub const DEFAULT_LIGHT_RUNTIME_MODULES: &[LightRuntimeModule] = &[
    LightRuntimeModule::ephemeral(
        RHYTHM_ADAPTIVE_RUNTIME_ID,
        &["rhythm", "rhythm_adaptive"],
        rhythm_adaptive::runtime_manifest,
        create_rhythm_adaptive_runtime,
    ),
    LightRuntimeModule::cached(
        removed_circadian_RUNTIME_ID,
        &["removed-project-circadian", "removed-project", "removed_circadian"],
        removed_circadian::runtime_manifest,
        ensure_removed-project_runtime,
    ),
];

pub fn install_default_light_runtime_modules(state: &mut AppState) -> Result<()> {
    register_light_runtime_modules(state, DEFAULT_LIGHT_RUNTIME_MODULES.iter().copied())
}

fn create_rhythm_adaptive_runtime(runtime: Arc<dyn RuntimeHandle>) -> Box<dyn LightRuntime> {
    Box::new(rhythm_adaptive::RuntimeHandleAdaptiveRuntime::new(runtime))
}

fn ensure_removed-project_runtime(
    state: &SharedState,
    module: &LightRuntimeModule,
) -> Result<SharedLightRuntime> {
    let (runtime, runtime_state, hour, sun_times, current_fingerprint, existing_runtime) = {
        let s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
        let runtime = s
            .hub_runtime()
            .ok_or_else(|| anyhow::anyhow!("no runtime is available"))?;
        (
            runtime,
            s.light_runtime_state.get(module.id).cloned(),
            s.hub_runtime()
                .map(|runtime| runtime.current_hour() as f64)
                .unwrap_or(12.0),
            removed-project_sun_times_from_state(&s),
            s.light_runtime_config_fingerprint.clone(),
            s.light_runtime.clone(),
        )
    };
    let snapshot = build_runtime_snapshot(runtime.as_ref(), runtime_state.as_ref());
    let fingerprint = removed_circadian::runtime_config_fingerprint_from_snapshot(
        &snapshot,
        runtime_state.as_ref(),
    );

    if current_fingerprint.as_deref() == Some(fingerprint.as_str()) {
        if let Some(existing_runtime) = existing_runtime {
            return Ok(existing_runtime);
        }
    }

    let mut light_runtime = removed_circadian::removed-projectRuntime::new();
    light_runtime.load_from_runtime_snapshot(
        &snapshot,
        runtime_state.as_ref(),
        hour,
        sun_times,
        0.0,
    );
    let light_runtime: SharedLightRuntime = Arc::new(Mutex::new(Box::new(light_runtime)));

    let mut s = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
    if s.light_runtime_kind.as_str() == module.id
        && (s.light_runtime.is_none()
            || s.light_runtime_config_fingerprint.as_deref() != Some(fingerprint.as_str()))
    {
        s.light_runtime = Some(light_runtime.clone());
        s.light_runtime_config_fingerprint = Some(fingerprint);
    }
    Ok(light_runtime)
}

fn removed-project_sun_times_from_state(s: &AppState) -> removed_circadian::SunTimes {
    let mut sun_times = removed_circadian::SunTimes::default();
    let Some(lat) = s.latitude else {
        return sun_times;
    };
    let Some(lon) = s.longitude else {
        return sun_times;
    };

    let Some(tz_name) = s.timezone_name.as_deref() else {
        sun_times.solar_noon = s.solar_noon_hour() as f64;
        sun_times.solar_mid = (sun_times.solar_noon + 12.0) % 24.0;
        return sun_times;
    };
    let tz = rhythm_core::Timezone::new(tz_name);
    let (year, month, day, _) = tz.local_date_hour_from_utc(chrono::Utc::now().naive_utc());
    let core = rhythm_core::calculate_sun_times(lat, lon, year, month, day, &tz);
    sun_times.sunrise = core.sunrise as f64;
    sun_times.sunset = core.sunset as f64;
    sun_times.solar_noon = s.solar_noon_hour() as f64;
    sun_times.solar_mid = (sun_times.solar_noon + 12.0) % 24.0;
    sun_times.outdoor_source = "host".to_string();
    sun_times
}
