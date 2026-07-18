//! sbh-forge — the Ephemeral Tool Forge.
//!
//! Safely turns an LLM's capability request into a sandboxed, reputation-gated
//! tool: generate → static analysis → compile → WASM/WASI sandbox → reputation.
//! Depends on sbh-core (contract), sbh-llm (generation), sbh-safety (policy),
//! and sbh-store (audit trail).

pub mod code_gen;
pub mod generative_forge;
pub mod regenerative_forge;
pub mod reputation;
pub mod static_analysis;
pub mod tool_forge;
pub mod tool_memory;
pub mod wasm_forge;
