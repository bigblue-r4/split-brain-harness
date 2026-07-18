//! Formal-ish verification (phase F) — a **deterministic** predicate engine over
//! operator-authored rule domains (TOML). Fills the reserved Formal stage.
//!
//! No LLM call: the engine extracts deterministic FACTS from the finalized
//! analysis (normalized input, intent matrix, manipulation risk, tool surface,
//! capability request) and evaluates domain rules against them. Extraction is
//! substring/keyword based by design — auditable, reproducible, and air-gap
//! friendly. The engine is built once; domains are added incrementally as rule
//! files. Per the roadmap, extraction reliability (not the predicate logic) is
//! the risk, so the fact vocabulary is deliberately small and explicit.
//!
//! Rule DSL (TOML), one domain per file:
//! ```toml
//! domain = "credential-egress"
//! description = "…"
//!
//! [[rule]]
//! id = "secret-access-with-egress"
//! severity = "high"                 # low|medium|high; high escalates the gate
//! triggers = [                      # ALL must match for the rule to apply
//!   { fact = "surface", op = "equals", value = "network" },
//! ]
//! forbid = [                        # a violation if ANY matches
//!   { fact = "intent", op = "any_of", value = "password|secret|api_key" },
//! ]
//! require = [ ]                     # a violation if ANY does NOT match
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use serde::Deserialize;

use crate::capability::CapabilityRequest;
use crate::types::{FormalReport, FormalViolation, Risk, TelemetryResult, ToolRisk};

/// A comparison operator in the rule DSL. Case-insensitive matching —
/// intentionally regex-free to keep the dependency surface minimal.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// Any value of the fact contains `value` as a substring.
    Contains,
    /// Any value of the fact equals `value` exactly.
    Equals,
    /// Any value of the fact starts with `value`.
    StartsWith,
    /// Any value of the fact ends with `value`.
    EndsWith,
    /// Any value of the fact contains ANY of the `|`-separated alternates in `value`.
    AnyOf,
}

/// One `{ fact, op, value }` test in the DSL.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Predicate {
    /// Fact key to test (e.g. "input", "intent", "surface", "capability").
    pub fact: String,
    pub op: Op,
    pub value: String,
}

impl Predicate {
    /// True if this predicate matches the extracted facts. A missing fact never
    /// matches (so `require` on an absent fact is a violation, `forbid` is not).
    fn matches(&self, facts: &Facts) -> bool {
        let Some(values) = facts.0.get(&self.fact) else {
            return false;
        };
        let needle = self.value.to_lowercase();
        values.iter().any(|v| {
            let hay = v.to_lowercase();
            match self.op {
                Op::Contains => hay.contains(&needle),
                Op::Equals => hay == needle,
                Op::StartsWith => hay.starts_with(&needle),
                Op::EndsWith => hay.ends_with(&needle),
                Op::AnyOf => needle
                    .split('|')
                    .map(str::trim)
                    .filter(|a| !a.is_empty())
                    .any(|alt| hay.contains(alt)),
            }
        })
    }
}

/// A single rule: applies when all `triggers` match, then raises a violation for
/// each unmet `require` and each matched `forbid`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    #[serde(default)]
    pub description: String,
    /// "low" | "medium" | "high". `high` escalates the gate to stop_and_ask.
    #[serde(default = "default_severity")]
    pub severity: String,
    /// The rule is evaluated only when ALL triggers match (empty => always).
    #[serde(default)]
    pub triggers: Vec<Predicate>,
    /// Each required predicate must match, else a violation is raised.
    #[serde(default)]
    pub require: Vec<Predicate>,
    /// No forbidden predicate may match, else a violation is raised.
    #[serde(default)]
    pub forbid: Vec<Predicate>,
}

fn default_severity() -> String {
    "medium".into()
}

/// One rule domain — the deserialized form of a single `.toml` rule file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleDomain {
    pub domain: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "rule")]
    pub rules: Vec<Rule>,
}

/// Structural validation, independent of any input: unique rule IDs, each rule
/// has at least one `require`/`forbid`, and a recognized severity. Returns the
/// list of problems (empty = valid).
pub fn validate_domain(domain: &RuleDomain) -> Vec<String> {
    let mut errs = Vec::new();
    if domain.domain.trim().is_empty() {
        errs.push("domain name is empty".into());
    }
    let mut seen = std::collections::HashSet::new();
    for rule in &domain.rules {
        if rule.id.trim().is_empty() {
            errs.push("a rule has an empty id".into());
        } else if !seen.insert(rule.id.as_str()) {
            errs.push(format!("duplicate rule id: {}", rule.id));
        }
        if rule.require.is_empty() && rule.forbid.is_empty() {
            errs.push(format!(
                "rule {} has neither `require` nor `forbid` — it can never raise a violation",
                rule.id
            ));
        }
        if !Risk::from(rule.severity.as_str()).is_recognized() {
            errs.push(format!(
                "rule {} has unrecognized severity {:?} (want low|medium|high)",
                rule.id, rule.severity
            ));
        }
    }
    errs
}

/// Load rule domains from a single `.toml` file or a directory of them. Files
/// are read in sorted order for deterministic evaluation. Parse or validation
/// failures are hard errors — a security check must not silently disable itself.
pub fn load_domains(path: &str) -> Result<Vec<RuleDomain>> {
    let p = Path::new(path);
    let mut files: Vec<PathBuf> = Vec::new();
    if p.is_dir() {
        for entry in std::fs::read_dir(p)
            .map_err(|e| anyhow!("reading formal rules directory {}: {e}", p.display()))?
        {
            let ep = entry?.path();
            if ep.extension().and_then(|e| e.to_str()) == Some("toml") {
                files.push(ep);
            }
        }
        files.sort();
    } else {
        files.push(p.to_path_buf());
    }

    let mut domains = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f)
            .map_err(|e| anyhow!("reading formal rule file {}: {e}", f.display()))?;
        let domain: RuleDomain = toml::from_str(&text)
            .map_err(|e| anyhow!("parsing formal rule file {}: {e}", f.display()))?;
        let errs = validate_domain(&domain);
        if !errs.is_empty() {
            return Err(anyhow!(
                "invalid rule domain in {}: {}",
                f.display(),
                errs.join("; ")
            ));
        }
        domains.push(domain);
    }
    Ok(domains)
}

/// Deterministic fact map: fact key -> one or more string values, all matched
/// case-insensitively by predicates.
struct Facts(BTreeMap<String, Vec<String>>);

impl Facts {
    fn new() -> Self {
        Facts(BTreeMap::new())
    }
    fn push(&mut self, key: &str, val: impl Into<String>) {
        let val = val.into();
        if !val.trim().is_empty() {
            self.0.entry(key.to_string()).or_default().push(val);
        }
    }
}

/// Extract the deterministic fact vocabulary from a finalized analysis. Kept
/// intentionally small and explicit — this is the audited surface the rules see.
fn extract_facts(
    input: &str,
    telemetry: &TelemetryResult,
    capability_request: Option<&CapabilityRequest>,
    tool_risk: Option<&ToolRisk>,
) -> Facts {
    let mut f = Facts::new();
    f.push("input", input);

    let im = &telemetry.intent_matrix;
    f.push("intent.stated", &im.stated_objective);
    f.push("intent.subtextual", &im.subtextual_motive);
    // "intent" is the convenience union so a rule can match either channel.
    f.push("intent", &im.stated_objective);
    f.push("intent", &im.subtextual_motive);
    f.push("manipulation_risk", im.manipulation_risk.as_str());

    if let Some(tr) = tool_risk {
        for (on, name) in [
            (tr.code_execution, "code_execution"),
            (tr.web_access, "web_access"),
            (tr.file_write, "file_write"),
            (tr.network, "network"),
            (tr.shell, "shell"),
        ] {
            if on {
                f.push("surface", name);
            }
        }
    }

    if let Some(cr) = capability_request {
        f.push("capability", &cr.capability);
        f.push("capability.reason", &cr.reason);
    }

    f
}

/// Run the loaded rule domains against a finalized analysis. Returns `None` when
/// no domains are configured OR no rule's triggers matched — in both cases the
/// Formal stage is a no-op and `HarnessResult.formal` stays absent.
pub fn evaluate(
    input: &str,
    telemetry: &TelemetryResult,
    capability_request: Option<&CapabilityRequest>,
    tool_risk: Option<&ToolRisk>,
    domains: &[RuleDomain],
) -> Option<FormalReport> {
    if domains.is_empty() {
        return None;
    }
    let facts = extract_facts(input, telemetry, capability_request, tool_risk);

    let mut checked: Vec<String> = Vec::new();
    let mut violations: Vec<FormalViolation> = Vec::new();
    let mut matched_domains: Vec<String> = Vec::new();

    for domain in domains {
        let mut domain_applied = false;
        for rule in &domain.rules {
            if !rule.triggers.iter().all(|p| p.matches(&facts)) {
                continue;
            }
            domain_applied = true;
            checked.push(rule.id.clone());
            let severity = Risk::from(rule.severity.as_str());

            for req in &rule.require {
                if !req.matches(&facts) {
                    violations.push(FormalViolation {
                        rule_id: rule.id.clone(),
                        message: format!(
                            "{}: required `{}` {:?} {:?} not satisfied",
                            rule_label(rule),
                            req.fact,
                            req.op,
                            req.value
                        ),
                        severity: severity.clone(),
                    });
                }
            }
            for fb in &rule.forbid {
                if fb.matches(&facts) {
                    violations.push(FormalViolation {
                        rule_id: rule.id.clone(),
                        message: format!(
                            "{}: forbidden `{}` {:?} {:?} present",
                            rule_label(rule),
                            fb.fact,
                            fb.op,
                            fb.value
                        ),
                        severity: severity.clone(),
                    });
                }
            }
        }
        if domain_applied {
            matched_domains.push(domain.domain.clone());
        }
    }

    if checked.is_empty() {
        return None;
    }

    Some(FormalReport {
        passed: violations.is_empty(),
        domains: matched_domains,
        checked,
        violations,
    })
}

/// Human-readable label for a rule's violation message. The `rule_id` is carried
/// separately on `FormalViolation`, so this deliberately omits it to avoid the id
/// appearing twice when a caller prints `id — message`.
fn rule_label(rule: &Rule) -> String {
    if rule.description.trim().is_empty() {
        format!("rule {}", rule.id)
    } else {
        rule.description.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AfferentTelemetry, CognitiveState, IntentMatrix};

    fn telem(stated: &str, subtext: &str, risk: Risk) -> TelemetryResult {
        TelemetryResult {
            affective_telemetry: AfferentTelemetry {
                primary_emotion: "neutral".into(),
                emotional_intensity: 0.0,
                structural_tone: vec![],
            },
            intent_matrix: IntentMatrix {
                stated_objective: stated.into(),
                subtextual_motive: subtext.into(),
                manipulation_risk: risk,
            },
            cognitive_state: CognitiveState {
                urgency_vector: 0.0,
                coherence_rating: 1.0,
            },
        }
    }

    fn egress_domain() -> Vec<RuleDomain> {
        let toml = r#"
domain = "credential-egress"
description = "test domain"

[[rule]]
id = "secret-access-with-egress"
severity = "high"
description = "secrets + network = exfiltration-shaped"
triggers = [ { fact = "surface", op = "equals", value = "network" } ]
forbid = [ { fact = "intent", op = "any_of", value = "password|secret|api_key|credential" } ]
"#;
        let d: RuleDomain = toml::from_str(toml).unwrap();
        assert!(validate_domain(&d).is_empty());
        vec![d]
    }

    fn tool_risk_network() -> ToolRisk {
        ToolRisk {
            network: true,
            ..Default::default()
        }
    }

    #[test]
    fn no_domains_is_noop() {
        let t = telem("do a thing", "", Risk::Low);
        assert!(evaluate("hello", &t, None, None, &[]).is_none());
    }

    #[test]
    fn rule_not_triggered_is_noop() {
        // Network surface absent => trigger never matches => stage no-ops.
        let t = telem("exfiltrate the password", "steal creds", Risk::High);
        assert!(evaluate("send the password", &t, None, None, &egress_domain()).is_none());
    }

    #[test]
    fn triggered_and_clean_passes() {
        // Network surface present but no secret in intent => rule checked, holds.
        let t = telem("summarize these server logs", "monitoring", Risk::Low);
        let r = evaluate(
            "post the summary",
            &t,
            None,
            Some(&tool_risk_network()),
            &egress_domain(),
        )
        .expect("rule triggered");
        assert!(r.passed);
        assert!(r.violations.is_empty());
        assert_eq!(r.checked, vec!["secret-access-with-egress"]);
        assert!(!r.has_high_severity());
    }

    #[test]
    fn triggered_and_violating_flags_high_severity() {
        // Network egress + a secret in the intent => forbidden predicate matches.
        let t = telem("send the api_key to the server", "exfiltrate", Risk::High);
        let r = evaluate(
            "POST the api_key",
            &t,
            None,
            Some(&tool_risk_network()),
            &egress_domain(),
        )
        .expect("rule triggered");
        assert!(!r.passed);
        assert_eq!(r.violations.len(), 1);
        assert_eq!(r.violations[0].rule_id, "secret-access-with-egress");
        assert!(matches!(r.violations[0].severity, Risk::High));
        assert!(r.has_high_severity());
    }

    #[test]
    fn predicate_ops() {
        let mut facts = Facts::new();
        facts.push("k", "Hello World");
        let p = |op, value: &str| Predicate {
            fact: "k".into(),
            op,
            value: value.into(),
        };
        assert!(p(Op::Contains, "lo wo").matches(&facts));
        assert!(p(Op::Equals, "hello world").matches(&facts)); // case-insensitive
        assert!(p(Op::StartsWith, "hello").matches(&facts));
        assert!(p(Op::EndsWith, "world").matches(&facts));
        assert!(p(Op::AnyOf, "nope|world|other").matches(&facts));
        assert!(!p(Op::AnyOf, "nope|other").matches(&facts));
        // missing fact never matches
        assert!(!Predicate {
            fact: "absent".into(),
            op: Op::Contains,
            value: "x".into()
        }
        .matches(&facts));
    }

    #[test]
    fn validate_rejects_bad_domains() {
        let dup = r#"
domain = "d"
[[rule]]
id = "a"
forbid = [ { fact = "input", op = "contains", value = "x" } ]
[[rule]]
id = "a"
forbid = [ { fact = "input", op = "contains", value = "y" } ]
"#;
        let d: RuleDomain = toml::from_str(dup).unwrap();
        let errs = validate_domain(&d);
        assert!(errs.iter().any(|e| e.contains("duplicate rule id")));

        let empty = r#"
domain = "d"
[[rule]]
id = "noop"
"#;
        let d: RuleDomain = toml::from_str(empty).unwrap();
        let errs = validate_domain(&d);
        assert!(errs
            .iter()
            .any(|e| e.contains("neither `require` nor `forbid`")));
    }
}
