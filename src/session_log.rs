/// Append-only session escalation log for `sbh serve`.
///
/// Every time the multi-turn slow-boil escalation algorithm fires, one JSON
/// line is written here.  The log is append-only and never modified in place,
/// making it suitable as a witness-layer feed.
///
/// Raw input is never stored.  Each entry carries an FNV-1a-64 fingerprint of
/// the user input and a masked client IP (last two IPv4 octets zeroed) so
/// entries can be correlated without preserving PII.
use std::io::Write;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::audit::{fingerprint, iso_now};

// ---------------------------------------------------------------------------
// Entry
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SessionLogEntry {
    /// ISO 8601 UTC timestamp of the escalation event.
    pub timestamp: String,
    /// Event type — always `"escalation_detected"` in this release.
    pub event: String,
    /// Session identifier echoed from `x-sbh-session`.
    pub session_id: String,
    /// Number of turns in this session at the time of the event.
    pub turn_count: usize,
    /// Full risk trajectory for the session window, e.g. `["low","low","high"]`.
    pub risk_trajectory: Vec<String>,
    /// Risk label of the turn that triggered the alert.
    pub current_risk: String,
    /// Mean risk score of all turns except the triggering turn (0=low,1=medium,2=high).
    pub historical_mean: f64,
    /// Client IP with last two IPv4 octets (or last four IPv6 groups) masked.
    pub client_ip_masked: String,
    /// FNV-1a-64 hex fingerprint of the raw user input — no plaintext stored.
    pub input_fingerprint: String,
}

impl SessionLogEntry {
    pub fn new(
        session_id: String,
        turn_count: usize,
        risk_trajectory: Vec<String>,
        historical_mean: f64,
        client_ip: &IpAddr,
        user_input: &str,
    ) -> Self {
        let current_risk = risk_trajectory
            .last()
            .cloned()
            .unwrap_or_else(|| "unknown".into());
        Self {
            timestamp: iso_now(),
            event: "escalation_detected".into(),
            session_id,
            turn_count,
            risk_trajectory,
            current_risk,
            historical_mean,
            client_ip_masked: mask_ip(client_ip),
            input_fingerprint: fingerprint(user_input.as_bytes()),
        }
    }
}

// ---------------------------------------------------------------------------
// IP masking — last two IPv4 octets zeroed; last 64 bits of IPv6 zeroed
// ---------------------------------------------------------------------------

pub fn mask_ip(ip: &IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            format!("{}.{}.x.x", o[0], o[1])
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}:x:x:x:x", s[0], s[1], s[2], s[3])
        }
    }
}

// ---------------------------------------------------------------------------
// Append
// ---------------------------------------------------------------------------

pub fn append(path: &str, entry: &SessionLogEntry) -> std::io::Result<()> {
    let mut line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())
}

// ---------------------------------------------------------------------------
// Read
// ---------------------------------------------------------------------------

pub fn read_all(path: &str) -> std::io::Result<Vec<SessionLogEntry>> {
    let raw = std::fs::read_to_string(path)?;
    let entries = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<SessionLogEntry>(l).ok())
        .collect();
    Ok(entries)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn mask_ipv4_zeros_last_two_octets() {
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50));
        assert_eq!(mask_ip(&ip), "192.168.x.x");
    }

    #[test]
    fn mask_ipv4_loopback() {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        assert_eq!(mask_ip(&ip), "127.0.x.x");
    }

    #[test]
    fn mask_ipv6_zeros_last_four_groups() {
        let ip = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 1, 2, 3, 4));
        let masked = mask_ip(&ip);
        assert!(masked.starts_with("2001:db8:0:0:"), "got: {masked}");
        assert!(masked.ends_with(":x:x:x:x"), "got: {masked}");
    }

    #[test]
    fn append_and_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let path_str = path.to_str().unwrap();

        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 1, 2));
        let entry = SessionLogEntry::new(
            "sbh-s-42".into(),
            3,
            vec!["low".into(), "low".into(), "high".into()],
            0.0,
            &ip,
            "test input that triggered escalation",
        );

        append(path_str, &entry).unwrap();
        append(path_str, &entry).unwrap();

        let entries = read_all(path_str).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].event, "escalation_detected");
        assert_eq!(entries[0].session_id, "sbh-s-42");
        assert_eq!(entries[0].turn_count, 3);
        assert_eq!(entries[0].current_risk, "high");
        assert_eq!(entries[0].risk_trajectory, vec!["low", "low", "high"]);
        assert_eq!(entries[0].client_ip_masked, "10.0.x.x");
        assert!(!entries[0].input_fingerprint.is_empty());
    }

    #[test]
    fn fingerprint_is_stable_across_entries() {
        let ip = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));
        let input = "same input every time";
        let e1 = SessionLogEntry::new(
            "s1".into(), 3, vec!["low".into(), "low".into(), "high".into()],
            0.0, &ip, input,
        );
        let e2 = SessionLogEntry::new(
            "s2".into(), 4, vec!["low".into(), "low".into(), "low".into(), "high".into()],
            0.0, &ip, input,
        );
        assert_eq!(e1.input_fingerprint, e2.input_fingerprint);
    }

    #[test]
    fn new_sets_current_risk_from_trajectory() {
        let ip = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));
        let entry = SessionLogEntry::new(
            "s".into(), 3,
            vec!["low".into(), "medium".into(), "high".into()],
            0.5, &ip, "x",
        );
        assert_eq!(entry.current_risk, "high");
    }

    #[test]
    fn new_empty_trajectory_current_risk_unknown() {
        let ip = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));
        let entry = SessionLogEntry::new("s".into(), 0, vec![], 0.0, &ip, "x");
        assert_eq!(entry.current_risk, "unknown");
    }
}
