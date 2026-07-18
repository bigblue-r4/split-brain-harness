//! sbh-store — append-only JSONL persistence for the Split-Brain Harness.
//!
//! Three stores sharing one pattern (serde struct → one JSON line → append):
//! - [`audit`] — the forge audit log (also home to the shared `fingerprint`
//!   FNV-1a and `iso_now` timestamp helpers, no external deps).
//! - [`session_log`] — serve slow-boil escalation events (privacy-by-fingerprint).
//! - [`calibration`] — per-run confidence features for offline Platt fitting.

pub mod audit;
pub mod calibration;
pub mod introspect;
pub mod session_log;
