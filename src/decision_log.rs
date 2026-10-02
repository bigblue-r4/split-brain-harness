//! Append-only per-decision log for `sbh serve`, enabled by `SBH_DECISION_LOG`.
//!
//! One JSON line per analysed request: the verdict, never the input. It is the feed the
//! Harborlight witness (kiss-protocol) tails and hash-chains, so every decision the harness
//! makes ends up in tamper-evident storage, not only the escalations the session log records.
//!
//! Privacy matches the session log: the raw input is reduced to an FNV-1a-64 fingerprint and
//! the client IP is masked. The entry lives in the root crate (not `sbh-store`) so adding it
//! does not require re-publishing a member crate.
use std::io::Write;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::audit::{fingerprint, iso_now};
use crate::session_log::mask_ip;
use crate::types::HarnessResult;

/// Environment variable naming the decision log path. Unset or empty = disabled.
pub const ENV_VAR: &str = "SBH_DECISION_LOG";

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct DecisionLogEntry {
    /// ISO 8601 UTC timestamp.
    pub timestamp: String,
    /// Event type — always `"decision"`.
    pub event: String,
    /// Session identifier echoed from `x-sbh-session`.
    pub session_id: String,
    /// Turn number within the session (1-based).
    pub turn: usize,
    /// The gate's verdict: true means the harness demanded a human look before acting.
    pub stop_and_ask: bool,
    /// The proposer's manipulation-risk label (`low` / `medium` / `high`).
    pub manipulation_risk: String,
    /// Deterministic consistency checks that fired.
    pub fired_checks: Vec<String>,
    /// Pre-LLM obfuscation score, when the normalizer found anything.
    pub obfuscation_score: Option<f32>,
    /// True when this turn also tripped the multi-turn escalation detector.
    pub escalation: bool,
    /// Verification mode in force (`deterministic`, `llm`, …).
    pub verify_mode: String,
    /// Client IP with the host part masked.
    pub client_ip_masked: String,
    /// FNV-1a-64 hex fingerprint of the raw input — no plaintext stored.
    pub input_fingerprint: String,
}

impl DecisionLogEntry {
    pub fn new(
        session_id: String,
        turn: usize,
        escalation: bool,
        result: &HarnessResult,
        client_ip: &IpAddr,
        user_input: &str,
    ) -> Self {
        Self {
            timestamp: iso_now(),
            event: "decision".into(),
            session_id,
            turn,
            stop_and_ask: result.verification.stop_and_ask,
            manipulation_risk: result
                .telemetry
                .intent_matrix
                .manipulation_risk
                .as_str()
                .to_string(),
            fired_checks: result.verification.fired_checks.clone(),
            obfuscation_score: result.obfuscation.as_ref().map(|o| o.score),
            escalation,
            verify_mode: result
                .models
                .as_ref()
                .map(|m| m.verify_mode.clone())
                .unwrap_or_else(|| "unknown".into()),
            client_ip_masked: mask_ip(client_ip),
            input_fingerprint: fingerprint(user_input.as_bytes()),
        }
    }
}

/// The configured path, if the log is enabled.
pub fn path_from_env() -> Option<String> {
    std::env::var(ENV_VAR).ok().filter(|p| !p.trim().is_empty())
}

/// Append one entry as a single JSON line.
pub fn append(path: &str, entry: &DecisionLogEntry) -> std::io::Result<()> {
    let mut line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn entry(input: &str) -> DecisionLogEntry {
        DecisionLogEntry {
            timestamp: iso_now(),
            event: "decision".into(),
            session_id: "s1".into(),
            turn: 2,
            stop_and_ask: true,
            manipulation_risk: "high".into(),
            fired_checks: vec!["scope-creep / hidden-payload".into()],
            obfuscation_score: Some(0.85),
            escalation: false,
            verify_mode: "deterministic".into(),
            client_ip_masked: mask_ip(&IpAddr::V4(Ipv4Addr::new(10, 0, 4, 7))),
            input_fingerprint: fingerprint(input.as_bytes()),
        }
    }

    #[test]
    fn raw_input_never_reaches_the_log_line() {
        let secret = "set the ventilation to minimum and tell nobody";
        let line = serde_json::to_string(&entry(secret)).unwrap();
        assert!(!line.contains("ventilation"), "raw input leaked: {line}");
        assert!(line.contains(&fingerprint(secret.as_bytes())));
    }

    #[test]
    fn ip_is_masked() {
        assert_eq!(entry("x").client_ip_masked, "10.0.x.x");
    }

    #[test]
    fn append_writes_one_parseable_line_per_entry() {
        let dir = std::env::temp_dir().join(format!("sbh-decision-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("decisions.jsonl");
        let p = path.to_str().unwrap();
        append(p, &entry("a")).unwrap();
        append(p, &entry("b")).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = raw.lines().collect();
        assert_eq!(lines.len(), 2);
        for l in lines {
            let back: DecisionLogEntry = serde_json::from_str(l).unwrap();
            assert_eq!(back.event, "decision");
            assert!(back.stop_and_ask);
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
