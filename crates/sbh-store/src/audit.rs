/// Append-only audit log for the Ephemeral Tool Forge.
///
/// Every forge run (success or failure) writes one JSON line to the configured
/// log file. The log is append-only and never modified in place.
///
/// Source code is never stored. Each entry carries a 64-bit FNV-1a fingerprint
/// of the generated source so entries can be correlated to code that was
/// compiled and executed without storing the source itself.
use std::io::Write;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Entry
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AuditEntry {
    /// ISO 8601 UTC timestamp of the run.
    pub timestamp: String,
    /// Human-readable capability description.
    pub capability: String,
    /// Stable problem_signature derived from the capability request.
    pub signature: String,
    /// Number of generation attempts made.
    pub attempt_count: usize,
    /// Reputation tier before this run.
    pub tier_before: String,
    /// Reputation tier after this run.
    pub tier_after: String,
    /// Whether the forge produced a working tool.
    pub succeeded: bool,
    /// FNV-1a-64 hex fingerprint of the last generated source (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_fingerprint: Option<String>,
    /// First 200 chars of the last failure reason (if failed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_summary: Option<String>,
}

// ---------------------------------------------------------------------------
// Append
// ---------------------------------------------------------------------------

pub fn append(path: &str, entry: &AuditEntry) -> std::io::Result<()> {
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

pub fn read_all(path: &str) -> std::io::Result<Vec<AuditEntry>> {
    let raw = std::fs::read_to_string(path)?;
    let entries = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<AuditEntry>(l).ok())
        .collect();
    Ok(entries)
}

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

pub fn print_summary(path: &str, entries: &[AuditEntry]) {
    let total = entries.len();
    let succeeded = entries.iter().filter(|e| e.succeeded).count();
    let failed = total - succeeded;
    let last_ts = entries.last().map(|e| e.timestamp.as_str()).unwrap_or("—");

    // Unique capabilities by signature
    let mut caps: std::collections::HashMap<&str, (usize, usize, usize, &str)> =
        std::collections::HashMap::new();
    for e in entries {
        let entry = caps.entry(e.capability.as_str()).or_insert((0, 0, 0, ""));
        entry.0 += 1;
        if e.succeeded {
            entry.1 += 1;
        } else {
            entry.2 += 1;
        }
        entry.3 = e.tier_after.as_str();
    }

    println!("forge audit: {path}  ({total} entries)");
    println!();
    println!("summary");
    println!("  total runs:    {total}  ({succeeded} succeeded, {failed} failed)");
    println!("  unique caps:   {}", caps.len());
    println!("  last run:      {last_ts}");
    println!();

    if caps.is_empty() {
        return;
    }

    println!(
        "{:<38} {:>5}  {:>4}  {:>4}  tier",
        "capability", "runs", "pass", "fail"
    );
    println!("{}", "─".repeat(62));

    let mut rows: Vec<(&str, usize, usize, usize, &str)> = caps
        .iter()
        .map(|(&cap, &(runs, pass, fail, tier))| (cap, runs, pass, fail, tier))
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));

    for (cap, runs, pass, fail, tier) in rows {
        let display = if cap.len() > 37 {
            format!("{}…", &cap[..36])
        } else {
            cap.to_string()
        };
        println!(
            "{:<38} {:>5}  {:>4}  {:>4}  {tier}",
            display, runs, pass, fail
        );
    }
}

pub fn print_tail(entries: &[AuditEntry], n: usize) {
    let slice = if entries.len() > n {
        &entries[entries.len() - n..]
    } else {
        entries
    };

    if slice.is_empty() {
        println!("(no entries)");
        return;
    }

    println!(
        "{:<22}  {:<32}  {:>5}  {:>6}  tier_after",
        "timestamp", "capability", "atts", "result"
    );
    println!("{}", "─".repeat(78));

    for e in slice {
        let cap = if e.capability.len() > 31 {
            format!("{}…", &e.capability[..30])
        } else {
            e.capability.clone()
        };
        let result = if e.succeeded { "ok" } else { "FAIL" };
        println!(
            "{:<22}  {:<32}  {:>5}  {:>6}  {}",
            e.timestamp, cap, e.attempt_count, result, e.tier_after
        );
    }
}

// ---------------------------------------------------------------------------
// Fingerprint — FNV-1a-64, no external deps
// ---------------------------------------------------------------------------

pub fn fingerprint(data: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x00000100000001b3);
    }
    format!("{hash:016x}")
}

// ---------------------------------------------------------------------------
// ISO 8601 UTC timestamp — no external deps
// ---------------------------------------------------------------------------

pub fn iso_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    unix_secs_to_iso(secs)
}

fn is_leap(year: u64) -> bool {
    year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100))
}

fn unix_secs_to_iso(mut secs: u64) -> String {
    let s = secs % 60;
    secs /= 60;
    let m = secs % 60;
    secs /= 60;
    let h = secs % 24;
    let mut days = secs / 24;

    let mut year = 1970u64;
    loop {
        let dy = if is_leap(year) { 366 } else { 365 };
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }

    let month_days: [u64; 12] = [
        31,
        if is_leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1u64;
    for &dm in &month_days {
        if days < dm {
            break;
        }
        days -= dm;
        month += 1;
    }
    let day = days + 1;

    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_deterministic() {
        let a = fingerprint(b"hello world");
        let b = fingerprint(b"hello world");
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn fingerprint_differs_on_different_input() {
        assert_ne!(fingerprint(b"foo"), fingerprint(b"bar"));
    }

    #[test]
    fn iso_now_looks_right() {
        let ts = iso_now();
        assert!(ts.ends_with('Z'), "expected Z suffix: {ts}");
        assert!(ts.contains('T'), "expected T separator: {ts}");
        assert!(ts.starts_with("20"), "expected 20xx year: {ts}");
    }

    #[test]
    fn unix_secs_to_iso_epoch() {
        assert_eq!(unix_secs_to_iso(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn unix_secs_to_iso_known_date() {
        // 2026-01-01T00:00:00Z = 1767225600
        assert_eq!(unix_secs_to_iso(1767225600), "2026-01-01T00:00:00Z");
    }

    #[test]
    fn append_and_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let path_str = path.to_str().unwrap();

        let entry = AuditEntry {
            timestamp: "2026-01-01T00:00:00Z".into(),
            capability: "word count".into(),
            signature: "abc123".into(),
            attempt_count: 1,
            tier_before: "Untrusted".into(),
            tier_after: "Emerging".into(),
            succeeded: true,
            source_fingerprint: Some("deadbeef12345678".into()),
            error_summary: None,
        };

        append(path_str, &entry).unwrap();
        append(path_str, &entry).unwrap();

        let entries = read_all(path_str).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].capability, "word count");
        assert!(entries[1].succeeded);
    }
}
