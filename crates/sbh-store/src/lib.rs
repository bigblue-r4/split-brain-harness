//! sbh-store — JSONL telemetry and explicit SQLite research memory persistence.
//!
//! Three stores sharing one pattern (serde struct → one JSON line → append):
//! - [`audit`] — the forge audit log (also home to the shared `fingerprint`
//!   FNV-1a and `iso_now` timestamp helpers, no external deps).
//! - [`session_log`] — serve slow-boil escalation events (privacy-by-fingerprint).
//! - [`calibration`] — per-run confidence features for offline Platt fitting.
//! - [`dream_memory`] — transactional candidate/revision history, separated
//!   from the existing JSONL stores and from production runtime wiring.

pub mod audit;
pub mod calibration;
pub mod dream_memory;
pub mod introspect;
pub mod memory_authority;
pub mod session_log;
