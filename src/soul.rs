use crate::types::Soul;
use anyhow::{anyhow, Result};

const DEFAULT_SOUL: &str = include_str!("../soul.md");

const OPEN_LOGIC: &str = "[LOGIC_SYSTEM_PROMPT]";
const CLOSE_LOGIC: &str = "[/LOGIC_SYSTEM_PROMPT]";
const OPEN_CREATIVE: &str = "[CREATIVE_SYSTEM_PROMPT]";
const CLOSE_CREATIVE: &str = "[/CREATIVE_SYSTEM_PROMPT]";
const OPEN_VERIFIER: &str = "[VERIFIER_SYSTEM_PROMPT]";
const CLOSE_VERIFIER: &str = "[/VERIFIER_SYSTEM_PROMPT]";
const OPEN_CODE_GEN: &str = "[CODE_GEN_SYSTEM_PROMPT]";
const CLOSE_CODE_GEN: &str = "[/CODE_GEN_SYSTEM_PROMPT]";

/// Load a Soul from disk (if path given) or fall back to the embedded default.
///
/// User-supplied paths (e.g. `SBH_SOUL_PATH`) are canonicalized and validated
/// before reading: the resolved path must be a regular `.md` file inside an
/// allowed directory. See [`crate::security::validate_soul_path`].
pub fn load(path: Option<&str>) -> Result<Soul> {
    let raw = match path {
        Some(p) if !p.is_empty() => {
            let validated = crate::security::validate_soul_path(p)?;
            std::fs::read_to_string(&validated)
                .map_err(|e| anyhow!("failed to read soul file '{}': {}", p, e))?
        }
        _ => DEFAULT_SOUL.to_string(),
    };
    parse(&raw)
}

/// Wrap a raw user input string in payload tags for injection into the prompt.
pub fn wrap_payload(input: &str) -> String {
    format!("<payload>\n{}\n</payload>", input)
}

/// Wrap an original input + proposed analysis for the verifier prompt.
pub fn wrap_verifier_payload(original_input: &str, proposed_analysis: &str) -> String {
    format!(
        "<original_input>\n{}\n</original_input>\n<proposed_analysis>\n{}\n</proposed_analysis>",
        original_input, proposed_analysis
    )
}

// ---------------------------------------------------------------------------
// Internal
// ---------------------------------------------------------------------------

fn parse(raw: &str) -> Result<Soul> {
    Ok(Soul {
        logic_system_prompt: extract(raw, OPEN_LOGIC, CLOSE_LOGIC)?,
        creative_system_prompt: extract(raw, OPEN_CREATIVE, CLOSE_CREATIVE).unwrap_or_default(),
        verifier_system_prompt: extract(raw, OPEN_VERIFIER, CLOSE_VERIFIER).unwrap_or_default(),
        code_gen_system_prompt: extract(raw, OPEN_CODE_GEN, CLOSE_CODE_GEN).unwrap_or_default(),
    })
}

fn extract(raw: &str, open: &str, close: &str) -> Result<String> {
    let start = raw
        .find(open)
        .ok_or_else(|| anyhow!("soul.md missing opening tag: {}", open))?
        + open.len();

    let end = raw[start..]
        .find(close)
        .ok_or_else(|| anyhow!("soul.md missing closing tag: {}", close))?
        + start;

    Ok(raw[start..end].trim().to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_soul_parses() {
        let soul = load(None).expect("default soul should parse");
        assert!(
            !soul.logic_system_prompt.is_empty(),
            "logic prompt must not be empty"
        );
        assert!(
            soul.logic_system_prompt.contains("telemetry engine"),
            "logic prompt should contain identity marker"
        );
        assert!(
            !soul.verifier_system_prompt.is_empty(),
            "verifier prompt must not be empty"
        );
        assert!(
            soul.verifier_system_prompt.contains("claim verifier"),
            "verifier prompt should contain identity marker"
        );
    }

    #[test]
    fn wrap_payload_contains_tags() {
        let wrapped = wrap_payload("hello world");
        assert!(wrapped.contains("<payload>"));
        assert!(wrapped.contains("</payload>"));
        assert!(wrapped.contains("hello world"));
    }

    #[test]
    fn wrap_verifier_payload_contains_both_sections() {
        let wrapped = wrap_verifier_payload("raw input", r#"{"foo":"bar"}"#);
        assert!(wrapped.contains("<original_input>"));
        assert!(wrapped.contains("raw input"));
        assert!(wrapped.contains("<proposed_analysis>"));
        assert!(wrapped.contains(r#"{"foo":"bar"}"#));
    }

    #[test]
    fn extract_section_trims_whitespace() {
        let raw = "[LOGIC_SYSTEM_PROMPT]\n  content  \n[/LOGIC_SYSTEM_PROMPT]";
        let result = extract(raw, "[LOGIC_SYSTEM_PROMPT]", "[/LOGIC_SYSTEM_PROMPT]").unwrap();
        assert_eq!(result, "content");
    }

    #[test]
    fn missing_close_tag_errors() {
        let raw = "[LOGIC_SYSTEM_PROMPT]\ncontent";
        assert!(extract(raw, "[LOGIC_SYSTEM_PROMPT]", "[/LOGIC_SYSTEM_PROMPT]").is_err());
    }
}
