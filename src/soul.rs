use anyhow::{anyhow, Result};
use crate::types::Soul;

// Embedded default — binary works standalone with no soul.md on disk.
// Custom path in config overrides this at runtime.
const DEFAULT_SOUL: &str = include_str!("../soul.md");

const OPEN_LOGIC:    &str = "[LOGIC_SYSTEM_PROMPT]";
const CLOSE_LOGIC:   &str = "[/LOGIC_SYSTEM_PROMPT]";
const OPEN_CREATIVE: &str = "[CREATIVE_SYSTEM_PROMPT]";
const CLOSE_CREATIVE:&str = "[/CREATIVE_SYSTEM_PROMPT]";

/// Load a Soul from disk (if path given) or fall back to the embedded default.
pub fn load(path: Option<&str>) -> Result<Soul> {
    let raw = match path {
        Some(p) if !p.is_empty() => std::fs::read_to_string(p)
            .map_err(|e| anyhow!("failed to read soul file '{}': {}", p, e))?,
        _ => DEFAULT_SOUL.to_string(),
    };
    parse(&raw)
}

/// Wrap a raw user input string in payload tags for injection into the prompt.
pub fn wrap_payload(input: &str) -> String {
    format!("<payload>\n{}\n</payload>", input)
}

// ---------------------------------------------------------------------------
// Internal
// ---------------------------------------------------------------------------

fn parse(raw: &str) -> Result<Soul> {
    Ok(Soul {
        logic_system_prompt:    extract(raw, OPEN_LOGIC,    CLOSE_LOGIC)?,
        creative_system_prompt: extract(raw, OPEN_CREATIVE, CLOSE_CREATIVE)
            .unwrap_or_default(),  // creative section is optional
    })
}

fn extract(raw: &str, open: &str, close: &str) -> Result<String> {
    let start = raw.find(open)
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
        assert!(!soul.logic_system_prompt.is_empty(), "logic prompt must not be empty");
        assert!(
            soul.logic_system_prompt.contains("telemetry engine"),
            "logic prompt should contain identity marker"
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
