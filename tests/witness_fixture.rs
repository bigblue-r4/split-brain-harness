//! Fixtures for the Harborlight witness (kiss-protocol), produced by this crate's own serializers.
//!
//! kiss-protocol tails three of this crate's logs: the forge audit log (`SBH_AUDIT_PATH`), the
//! session escalation log (`SBH_SESSION_LOG`) and the per-decision log (`SBH_DECISION_LOG`). Its
//! tests drive these lines through its real tailer. Hand-authored fixtures would only prove the
//! fixture matches the test; these come from the real types, so a schema change here shows up
//! as a failing test there once the fixtures are regenerated:
//!
//!     SBH_WITNESS_FIXTURE_DIR=<kiss-protocol>/internal/sbh/testdata \
//!       cargo test --test witness_fixture
use std::path::Path;

use split_brain_harness::audit::AuditEntry;
use split_brain_harness::decision_log::DecisionLogEntry;
use split_brain_harness::session_log::SessionLogEntry;

fn forge_rows() -> Vec<AuditEntry> {
    vec![
        AuditEntry {
            timestamp: "2026-10-02T21:00:00Z".into(),
            capability: "parse ammonia sensor csv".into(),
            signature: "sig-parse-csv".into(),
            attempt_count: 1,
            tier_before: "untrusted".into(),
            tier_after: "probation".into(),
            succeeded: true,
            source_fingerprint: Some("a1b2c3d4e5f60718".into()),
            error_summary: None,
        },
        AuditEntry {
            timestamp: "2026-10-02T21:05:00Z".into(),
            capability: "write controller setpoint".into(),
            signature: "sig-setpoint".into(),
            attempt_count: 3,
            tier_before: "untrusted".into(),
            tier_after: "untrusted".into(),
            succeeded: false,
            source_fingerprint: None,
            error_summary: Some("sandbox denied network access".into()),
        },
    ]
}

fn session_rows() -> Vec<SessionLogEntry> {
    vec![SessionLogEntry {
        timestamp: "2026-10-02T21:10:00Z".into(),
        event: "escalation_detected".into(),
        session_id: "farm-ops-7".into(),
        turn_count: 4,
        risk_trajectory: vec!["low".into(), "low".into(), "medium".into(), "high".into()],
        current_risk: "high".into(),
        historical_mean: 0.333,
        client_ip_masked: "10.0.x.x".into(),
        input_fingerprint: "0f1e2d3c4b5a6978".into(),
    }]
}

fn decision_rows() -> Vec<DecisionLogEntry> {
    let base = DecisionLogEntry {
        timestamp: "2026-10-02T21:00:00Z".into(),
        event: "decision".into(),
        session_id: "farm-ops-7".into(),
        turn: 1,
        stop_and_ask: false,
        manipulation_risk: "low".into(),
        fired_checks: vec![],
        obfuscation_score: None,
        escalation: false,
        verify_mode: "deterministic".into(),
        client_ip_masked: "10.0.x.x".into(),
        input_fingerprint: "1111111111111111".into(),
    };
    vec![
        base.clone(),
        DecisionLogEntry {
            timestamp: "2026-10-02T21:02:00Z".into(),
            turn: 2,
            stop_and_ask: true,
            manipulation_risk: "high".into(),
            fired_checks: vec!["scope-creep / hidden-payload".into()],
            obfuscation_score: Some(0.85),
            input_fingerprint: "2222222222222222".into(),
            ..base.clone()
        },
        DecisionLogEntry {
            timestamp: "2026-10-02T21:10:00Z".into(),
            turn: 4,
            manipulation_risk: "medium".into(),
            escalation: true,
            input_fingerprint: "4444444444444444".into(),
            ..base
        },
    ]
}

fn lines<T: serde::Serialize + serde::de::DeserializeOwned>(rows: &[T]) -> String {
    let mut out = String::new();
    for r in rows {
        let line = serde_json::to_string(r).unwrap();
        // Every line must round-trip through the same type the producer reads back with.
        let _: T = serde_json::from_str(&line).unwrap();
        out.push_str(&line);
        out.push('\n');
    }
    out
}

#[test]
fn every_line_serializes_with_the_real_types() {
    let files = [
        ("forge_audit.jsonl", lines(&forge_rows())),
        ("session_escalations.jsonl", lines(&session_rows())),
        ("decisions.jsonl", lines(&decision_rows())),
    ];
    for (_, body) in &files {
        assert!(!body.is_empty());
        for l in body.lines() {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            assert!(
                v.is_object(),
                "witness tailer needs one JSON object per line"
            );
        }
    }
    if let Ok(dir) = std::env::var("SBH_WITNESS_FIXTURE_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        for (name, body) in &files {
            std::fs::write(Path::new(&dir).join(name), body).unwrap();
        }
    }
}
