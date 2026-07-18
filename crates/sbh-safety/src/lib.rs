//! sbh-safety — the shared safety mid-layer for the Split-Brain Harness.
//!
//! Depends only on `sbh-core`, and is used by both the core pipeline
//! (harness/verifier/transformer/adaptor) and the forge:
//! - [`security`] — soul-path confinement + secret redaction (no internal deps).
//! - [`soul`] — load a `Soul` from disk or the embedded default (uses `security`).
//! - [`policy`] — capability policy / budget enforcement over the core contract.

pub mod policy;
pub mod security;
pub mod soul;
