/// Code generation layer for Phase 3 of the Ephemeral Tool Forge.
///
/// Sends a CapabilityRequest to the inference engine and parses the response
/// into a GeneratedTool with static analysis results. No compilation or
/// execution happens here.
use crate::backends::InferenceEngine;
use crate::capability::CapabilityRequest;
use crate::static_analysis::{self, StaticAnalysisReport};
use crate::types::Soul;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Output type
// ---------------------------------------------------------------------------

/// The result of one code generation pass.
///
/// Contains the raw source, extracted metadata, and the static analysis
/// report. Does not contain compiled artefacts — those live in Phase 4.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GeneratedTool {
    /// Rust source code as returned by the model (extracted from the code block).
    pub source: String,
    /// The first public/private `fn` name found in the source.
    pub function_name: String,
    /// True if the source contains at least two `#[test]` functions.
    pub tests_included: bool,
    /// Number of `#[test]` annotations found.
    pub test_count: usize,
    /// Static analysis results.
    pub static_analysis: StaticAnalysisReport,
}

// ---------------------------------------------------------------------------
// Generator
// ---------------------------------------------------------------------------

/// Uses an inference engine to generate Rust source for a CapabilityRequest.
pub struct CodeGenerator<'e> {
    engine: &'e dyn InferenceEngine,
    soul: &'e Soul,
}

impl<'e> CodeGenerator<'e> {
    pub fn new(engine: &'e dyn InferenceEngine, soul: &'e Soul) -> Self {
        Self { engine, soul }
    }

    /// Generate Rust source code for `req`.
    ///
    /// Returns `Err` if the model call fails or no code block is present in
    /// the response. Static analysis violations do NOT cause an Err — they
    /// are reported inside `GeneratedTool`.
    pub async fn generate(&self, req: &CapabilityRequest) -> Result<GeneratedTool, String> {
        let prompt = build_prompt(req);

        let raw = self
            .engine
            .generate(&self.soul.code_gen_system_prompt, &prompt)
            .await?;

        let source = extract_code_block(&raw).ok_or_else(|| {
            format!(
                "model did not return a Rust code block \
                 (expected ```rust ... ```) — raw response length: {} chars",
                raw.len()
            )
        })?;

        let function_name = extract_function_name(&source).unwrap_or_else(|| "unknown".to_string());
        let test_count = static_analysis::test_count(&source);
        let tests_included = test_count >= 2;
        let sa = static_analysis::check(&source);

        Ok(GeneratedTool {
            source,
            function_name,
            tests_included,
            test_count,
            static_analysis: sa,
        })
    }
}

// ---------------------------------------------------------------------------
// Prompt builder
// ---------------------------------------------------------------------------

/// Build the generation prompt from a CapabilityRequest.
pub fn build_prompt(req: &CapabilityRequest) -> String {
    format!(
        "<capability_request>\n\
         capability: {cap}\n\
         input_contract: {inp}\n\
         output_contract: {out}\n\
         reason: {reason}\n\
         constraints:\n\
           no_network: {no_net}\n\
           read_only_input: {ro}\n\
           max_runtime_ms: {rt}\n\
           max_memory_mb: {mem}\n\
         </capability_request>",
        cap = req.capability,
        inp = req.input_contract,
        out = req.output_contract,
        reason = req.reason,
        no_net = req.constraints.no_network,
        ro = req.constraints.read_only_input,
        rt = req.constraints.max_runtime_ms,
        mem = req.constraints.max_memory_mb,
    )
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Extract the content of the first ` ```rust ... ``` ` block in `response`.
/// Returns `None` if no such block exists or the block is empty after trimming.
pub fn extract_code_block(response: &str) -> Option<String> {
    // Try ```rust\n first, then ```rust\r\n
    let (marker, skip) = if response.contains("```rust\n") {
        ("```rust\n", "```rust\n".len())
    } else if response.contains("```rust\r\n") {
        ("```rust\r\n", "```rust\r\n".len())
    } else {
        return None;
    };

    let start = response.find(marker)? + skip;
    let rest = &response[start..];
    let end = rest.find("```")?;
    let code = rest[..end].trim().to_string();
    if code.is_empty() {
        return None;
    }
    Some(code)
}

/// Find the first `pub fn` or `fn` name in the source.
pub fn extract_function_name(source: &str) -> Option<String> {
    for line in source.lines() {
        let t = line.trim();
        let rest = t.strip_prefix("pub fn ").or_else(|| t.strip_prefix("fn "));
        if let Some(rest) = rest {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityConstraints;

    fn clean_req() -> CapabilityRequest {
        CapabilityRequest {
            kind: "capability_request".into(),
            capability: "word_count".into(),
            input_contract: "utf8 text".into(),
            output_contract: "json object with word_count".into(),
            constraints: CapabilityConstraints::default(),
            reason: "text reasoning insufficient".into(),
        }
    }

    const CLEAN_RESPONSE: &str = r#"Here is the Rust implementation:

```rust
pub fn run(input: &str) -> Result<String, String> {
    let count = input.split_whitespace().count();
    Ok(format!("{\"word_count\":{}}", count))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn two_words() {
        assert!(run("hello world").unwrap().contains("2"));
    }
    #[test]
    fn empty() {
        assert!(run("").unwrap().contains("0"));
    }
}
```

That should fulfil the contract.
"#;

    // --- extract_code_block ---

    #[test]
    fn extracts_rust_code_block() {
        let source = extract_code_block(CLEAN_RESPONSE).unwrap();
        assert!(
            source.contains("pub fn run"),
            "extracted code must contain the function"
        );
        assert!(
            !source.contains("```"),
            "backticks must not appear in extracted code"
        );
    }

    #[test]
    fn returns_none_when_no_code_block() {
        let response = "Here is some analysis but no code.";
        assert!(extract_code_block(response).is_none());
    }

    #[test]
    fn returns_none_for_empty_code_block() {
        let response = "```rust\n```";
        assert!(extract_code_block(response).is_none());
    }

    #[test]
    fn extract_code_block_ignores_leading_prose() {
        let r = "Some explanation.\n\n```rust\nfn run(i: &str) -> Result<String, String> { Ok(i.into()) }\n```\n";
        let code = extract_code_block(r).unwrap();
        assert!(code.starts_with("fn run"));
    }

    // --- extract_function_name ---

    #[test]
    fn extracts_pub_fn_name() {
        let src = "pub fn run(input: &str) -> Result<String, String> {\n    Ok(\"ok\".into())\n}";
        assert_eq!(extract_function_name(src).unwrap(), "run");
    }

    #[test]
    fn extracts_private_fn_name() {
        let src = "fn process(input: &str) -> Result<String, String> {\n    Ok(\"ok\".into())\n}";
        assert_eq!(extract_function_name(src).unwrap(), "process");
    }

    #[test]
    fn returns_none_for_no_function() {
        let src = "// just a comment\nconst X: u32 = 0;";
        assert!(extract_function_name(src).is_none());
    }

    // --- build_prompt ---

    #[test]
    fn build_prompt_includes_all_fields() {
        let req = clean_req();
        let prompt = build_prompt(&req);
        assert!(prompt.contains("word_count"));
        assert!(prompt.contains("utf8 text"));
        assert!(prompt.contains("no_network: true"));
        assert!(prompt.contains("read_only_input: true"));
    }

    // --- GeneratedTool construction ---

    #[test]
    fn generated_tool_from_clean_response() {
        let source = extract_code_block(CLEAN_RESPONSE).unwrap();
        let tc = static_analysis::test_count(&source);
        let sa = static_analysis::check(&source);
        let tool = GeneratedTool {
            function_name: extract_function_name(&source).unwrap_or_default(),
            tests_included: tc >= 2,
            test_count: tc,
            static_analysis: sa,
            source,
        };
        assert_eq!(tool.function_name, "run");
        assert!(tool.tests_included, "response includes 2 tests");
        assert!(tool.static_analysis.passed, "clean code passes analysis");
    }

    #[test]
    fn generated_tool_flags_unsafe_source() {
        let bad_response = "```rust\npub fn run(i: &str) -> Result<String, String> {\n    unsafe { }\n    Ok(\"ok\".into())\n}\n#[test]\nfn t1() {}\n#[test]\nfn t2() {}\n```";
        let source = extract_code_block(bad_response).unwrap();
        let sa = static_analysis::check(&source);
        assert!(!sa.passed, "unsafe code should fail static analysis");
    }
}
