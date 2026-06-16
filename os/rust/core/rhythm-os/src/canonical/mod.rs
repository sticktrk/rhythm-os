//! Canonical device registry and identity system.
//!
//! Assigns every physical device a stable Rhythm UUID, independent of any hub.
//! Handles cross-hub deduplication via hardware identifiers and queues
//! ambiguous matches for human resolution.

pub mod identity;
pub mod registry;
pub mod triage;
