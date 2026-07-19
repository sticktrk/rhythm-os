pub mod collector;
pub mod model;
pub mod ring;
pub mod synthesis;

pub use collector::{
    boot_capture, collect_boot_id, collect_detail, collect_escalation, collect_summary,
    write_heartbeat, CollectorPaths, TriggerState, HARDWARE_WATCHDOG_HEARTBEAT_FILE,
    SERVER_HEARTBEAT_FILE,
};
pub use model::{EarlyBootSnapshot, FlightRecorderSynthesis, SourceStatus};
pub use ring::{
    read_ring, recorder_root, RingConfig, RingWriter, CURRENT_DIR, EARLY_BOOT_FILE, PREVIOUS_DIR,
    ROOT_RELATIVE_PATH,
};
pub use synthesis::build_synthesis;
