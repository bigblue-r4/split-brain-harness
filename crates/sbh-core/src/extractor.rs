use anyhow::{anyhow, Result};
use serde::de::DeserializeOwned;

/// Pull the first valid JSON object of type T out of raw model output.
///
/// Handles (in order):
///   1. <think>...</think> blocks emitted by reasoning models
///   2. ```json ... ``` and ``` ... ``` markdown fences
///   3. Leading prose before the opening brace
///   4. Trailing text or a second JSON object after the first closes
pub fn extract<T: DeserializeOwned>(raw: &str) -> Result<T> {
    let step1 = strip_think_blocks(raw);
    let step2 = strip_fences(&step1);

    let from_brace = step2.find('{').map(|i| &step2[i..]).ok_or_else(|| {
        anyhow!(
            "no JSON object in model response. First 200 chars: {:?}",
            &raw[..raw.len().min(200)]
        )
    })?;

    // StreamDeserializer stops at the end of the first complete JSON value
    // and ignores anything that follows.
    let mut stream = serde_json::Deserializer::from_str(from_brace).into_iter::<T>();

    stream
        .next()
        .ok_or_else(|| anyhow!("model returned an empty response"))?
        .map_err(|e| {
            anyhow!(
                "JSON schema mismatch: {}. Raw snippet: {:?}",
                e,
                &from_brace[..from_brace.len().min(300)]
            )
        })
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Remove all <think>...</think> blocks. Unclosed tags drop the remainder.
fn strip_think_blocks(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find("<think>") {
        out.push_str(&rest[..open]);
        match rest[open..].find("</think>") {
            Some(close) => rest = &rest[open + close + "</think>".len()..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Strip opening ``` or ```json fence and its matching closing ```.
fn strip_fences(s: &str) -> String {
    let s = s.trim();
    if !s.starts_with("```") {
        return s.to_string();
    }
    let after_open = match s.find('\n') {
        Some(nl) => &s[nl + 1..],
        None => return s.to_string(),
    };
    match after_open.rfind("```") {
        Some(close) => after_open[..close].trim().to_string(),
        None => after_open.trim().to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TelemetryResult;

    fn good_json() -> &'static str {
        r#"{
  "affective_telemetry": {
    "primary_emotion": "neutral",
    "emotional_intensity": 0.1,
    "structural_tone": ["analytical"]
  },
  "intent_matrix": {
    "stated_objective": "user wants weather information today",
    "subtextual_motive": "routine informational query",
    "manipulation_risk": "low"
  },
  "cognitive_state": {
    "urgency_vector": 0.0,
    "coherence_rating": 0.95
  }
}"#
    }

    #[test]
    fn parses_clean_json() {
        extract::<TelemetryResult>(good_json()).expect("clean JSON should parse");
    }

    #[test]
    fn strips_markdown_fence() {
        let fenced = format!("```json\n{}\n```", good_json());
        extract::<TelemetryResult>(&fenced).expect("fenced JSON should parse");
    }

    #[test]
    fn strips_think_blocks() {
        let with_think = format!("<think>some reasoning here</think>\n{}", good_json());
        extract::<TelemetryResult>(&with_think).expect("JSON after think block should parse");
    }

    #[test]
    fn ignores_trailing_text() {
        let trailing = format!("{}\n\nHere is my analysis.", good_json());
        extract::<TelemetryResult>(&trailing).expect("trailing prose should be ignored");
    }

    #[test]
    fn ignores_leading_prose() {
        let leading = format!("Sure! Here is the JSON:\n{}", good_json());
        extract::<TelemetryResult>(&leading).expect("leading prose should be ignored");
    }

    #[test]
    fn errors_on_empty() {
        assert!(extract::<TelemetryResult>("").is_err());
        assert!(extract::<TelemetryResult>("no braces here").is_err());
    }

    #[test]
    fn errors_on_schema_mismatch() {
        assert!(extract::<TelemetryResult>(r#"{"foo": "bar"}"#).is_err());
    }
}
