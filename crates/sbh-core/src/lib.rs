//! sbh-core — the Split-Brain Harness typed data model.
//!
//! Foundational, I/O-free types shared across the workspace: telemetry and
//! verification structs, runtime config, the capability contract, input
//! validation limits, and the model-output JSON extractor. These four modules
//! are mutually coupled (e.g. `types::HarnessResult` carries a
//! `capability::CapabilityRequest`) so they live together as the core cluster.

pub mod capability;
pub mod extractor;
pub mod input_validation;
pub mod types;
