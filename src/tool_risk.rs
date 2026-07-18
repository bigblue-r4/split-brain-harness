//! Tool-aware telemetry (phase C) — a **deterministic** classifier for whether an
//! input's intent would touch a tool surface (code execution / web / filesystem /
//! network / shell). Deliberately does NOT ask the model to self-report tool risk;
//! it pattern-matches the input and cross-checks the model's actual
//! `capability_request`. LLM output is untrusted here — this is the read-only,
//! auditable signal the verifier/operator can rely on.

use crate::capability::CapabilityRequest;
use crate::types::ToolRisk;

/// (category setter, human label, marker phrases). Lowercase, substring-matched.
type Rule = (fn(&mut ToolRisk), &'static str, &'static [&'static str]);

const RULES: &[Rule] = &[
    (
        |t| t.code_execution = true,
        "code_execution",
        &["execute", "run this code", "run the script", "eval(", "exec(", "compile and run", "```python", "```bash", "interpreter"],
    ),
    (
        |t| t.web_access = true,
        "web_access",
        &["http://", "https://", "www.", "fetch the url", "download from", "curl ", "wget ", "scrape", "browse to"],
    ),
    (
        |t| t.file_write = true,
        "file_write",
        &["write to file", "save to /", "save it to", "create a file", "overwrite", "delete the file", "append to the file"],
    ),
    (
        |t| t.network = true,
        "network",
        &["open a socket", "connect to the server", "send a packet", "post to the endpoint", "tcp connection", "listen on port"],
    ),
    (
        |t| t.shell = true,
        "shell",
        &["/bin/", "rm -rf", "sudo ", "chmod ", "os.system", "subprocess", "shell command"],
    ),
];

/// Classify an input's tool-use risk. Combines deterministic input patterns with
/// the model's declared `capability_request` (which, by definition, requests a
/// tool the supervisor must run).
pub fn classify(input: &str, capability_request: Option<&CapabilityRequest>) -> ToolRisk {
    let lo = input.to_lowercase();
    let mut tr = ToolRisk::default();

    for (set, label, pats) in RULES {
        if let Some(p) = pats.iter().find(|p| lo.contains(**p)) {
            set(&mut tr);
            tr.markers.push(format!("{label}: {:?}", p));
        }
    }
    if tr.any() {
        tr.sources.push("deterministic".into());
    }

    // A capability_request is the model explicitly asking to run a tool: trust the
    // declared constraints, not a guess.
    if let Some(cr) = capability_request {
        tr.code_execution = true;
        if !cr.constraints.no_network {
            tr.network = true;
            tr.web_access = true;
        }
        if !cr.constraints.read_only_input {
            tr.file_write = true;
        }
        tr.sources.push("capability_request".into());
        tr.markers.push(format!("capability_request: {}", cr.capability));
    }

    tr
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benign_input_has_no_tool_risk() {
        let tr = classify("write me a short poem about the sea", None);
        assert!(!tr.any());
        assert!(tr.sources.is_empty());
    }

    #[test]
    fn code_and_web_intents_are_flagged_deterministically() {
        let tr = classify("please execute this and fetch the url https://x.test", None);
        assert!(tr.code_execution && tr.web_access);
        assert!(tr.sources.contains(&"deterministic".to_string()));
        assert!(tr.markers.iter().any(|m| m.starts_with("code_execution")));
    }

    #[test]
    fn capability_request_marks_execution_and_respects_constraints() {
        use crate::capability::{CapabilityConstraints, CapabilityRequest};
        let cr = CapabilityRequest {
            kind: "capability_request".into(),
            capability: "stream_parse_logs".into(),
            input_contract: "x".into(),
            output_contract: "y".into(),
            constraints: CapabilityConstraints {
                no_network: false,
                read_only_input: false,
                ..Default::default()
            },
            reason: "large file".into(),
        };
        let tr = classify("count lines", Some(&cr));
        assert!(tr.code_execution && tr.network && tr.web_access && tr.file_write);
        assert!(tr.sources.contains(&"capability_request".to_string()));
    }
}
