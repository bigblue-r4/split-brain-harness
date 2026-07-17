/// Centralized input validation layer.
///
/// All external inputs — user text to the harness, tool forge inputs,
/// endpoint URLs, soul paths — pass through this module before any
/// processing begins. Fail fast, fail loudly.
use crate::capability::CapabilityRequest;

// ---------------------------------------------------------------------------
// Hard limits (tunable via constants, not config — these are security floors)
// ---------------------------------------------------------------------------

/// Maximum bytes accepted for harness `analyze()` input.
pub const MAX_HARNESS_INPUT_BYTES: usize = 32_768; // 32 KB

/// Maximum bytes accepted by mock tool implementations.
pub const MAX_FORGE_INPUT_BYTES: usize = 65_536; // 64 KB

/// Maximum length for a capability name.
pub const MAX_CAPABILITY_NAME_BYTES: usize = 64;

/// Maximum length for a capability reason field.
pub const MAX_REASON_BYTES: usize = 1_024;

/// Maximum length for an input/output contract description.
pub const MAX_CONTRACT_BYTES: usize = 256;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError(pub String);

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ---------------------------------------------------------------------------
// Harness input
// ---------------------------------------------------------------------------

/// Validate text entering the harness pipeline.
///
/// Rejects: oversized inputs, null bytes, non-printable control characters
/// (ASCII 0x01–0x1F, excluding \t, \n, \r which appear in normal text).
pub fn validate_harness_input(input: &str) -> Result<(), ValidationError> {
    if input.len() > MAX_HARNESS_INPUT_BYTES {
        return Err(ValidationError(format!(
            "input too long: {} bytes (max {})",
            input.len(),
            MAX_HARNESS_INPUT_BYTES
        )));
    }
    check_string_chars(input, "harness input")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Forge input
// ---------------------------------------------------------------------------

/// Validate text passed to a mock tool implementation.
///
/// Rejects: oversized inputs, null bytes, non-printable control characters.
pub fn validate_forge_input(input: &str) -> Result<(), ValidationError> {
    if input.len() > MAX_FORGE_INPUT_BYTES {
        return Err(ValidationError(format!(
            "forge input too long: {} bytes (max {})",
            input.len(),
            MAX_FORGE_INPUT_BYTES
        )));
    }
    check_string_chars(input, "forge input")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Capability request fields
// ---------------------------------------------------------------------------

/// Validate per-field length and content rules on a CapabilityRequest.
///
/// Called in addition to `CapabilityRequest::validate()` which handles
/// structural/schema correctness. This covers length-based abuse vectors.
pub fn validate_capability_fields(req: &CapabilityRequest) -> Result<(), ValidationError> {
    check_field_len("capability", &req.capability, MAX_CAPABILITY_NAME_BYTES)?;
    check_field_len("reason", &req.reason, MAX_REASON_BYTES)?;
    check_field_len("input_contract", &req.input_contract, MAX_CONTRACT_BYTES)?;
    check_field_len("output_contract", &req.output_contract, MAX_CONTRACT_BYTES)?;

    check_string_chars(&req.capability, "capability")?;
    check_string_chars(&req.reason, "reason")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Endpoint URL allowlist
// ---------------------------------------------------------------------------

/// Validate a backend endpoint URL.
///
/// Only `http://` and `https://` are accepted. `file://`, `javascript:`,
/// data URIs, and other schemes are rejected.
pub fn validate_endpoint(url: &str) -> Result<(), ValidationError> {
    let lower = url.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Ok(())
    } else {
        Err(ValidationError(format!(
            "endpoint must use http:// or https:// — got: {url}"
        )))
    }
}

// ---------------------------------------------------------------------------
// Soul path
// ---------------------------------------------------------------------------

/// Validate a user-supplied soul path.
///
/// Empty path is allowed (means: use embedded soul).
/// Non-empty paths must:
/// - not contain `../` traversal sequences
/// - end with `.md`
pub fn validate_soul_path(path: &str) -> Result<(), ValidationError> {
    if path.is_empty() {
        return Ok(());
    }
    if path.contains("../") || path.contains("..\\") || path.starts_with("..") {
        return Err(ValidationError(format!(
            "soul_path contains path traversal: {path}"
        )));
    }
    if !path.ends_with(".md") {
        return Err(ValidationError(format!(
            "soul_path must be a .md file: {path}"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn check_field_len(name: &str, value: &str, max: usize) -> Result<(), ValidationError> {
    if value.len() > max {
        Err(ValidationError(format!(
            "{name} too long: {} bytes (max {max})",
            value.len()
        )))
    } else {
        Ok(())
    }
}

/// Reject null bytes and non-printable ASCII controls (except \t, \n, \r).
fn check_string_chars(s: &str, label: &str) -> Result<(), ValidationError> {
    for (i, ch) in s.char_indices() {
        if ch == '\0' {
            return Err(ValidationError(format!(
                "{label}: null byte at byte offset {i}"
            )));
        }
        // Reject C0 control characters other than the normal whitespace trio
        if ch.is_control() && ch != '\n' && ch != '\r' && ch != '\t' {
            return Err(ValidationError(format!(
                "{label}: disallowed control character {:?} at byte offset {i}",
                ch
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{CapabilityConstraints, CapabilityRequest};

    fn clean_req() -> CapabilityRequest {
        CapabilityRequest {
            kind: "capability_request".into(),
            capability: "word_count".into(),
            input_contract: "utf8 text".into(),
            output_contract: "json".into(),
            constraints: CapabilityConstraints::default(),
            reason: "text reasoning insufficient".into(),
        }
    }

    // --- validate_harness_input ---

    #[test]
    fn harness_input_valid_text() {
        assert!(validate_harness_input("hello world").is_ok());
    }

    #[test]
    fn harness_input_with_newlines_allowed() {
        assert!(validate_harness_input("line one\nline two\r\n").is_ok());
    }

    #[test]
    fn harness_input_too_long() {
        let big = "a".repeat(MAX_HARNESS_INPUT_BYTES + 1);
        let err = validate_harness_input(&big).unwrap_err();
        assert!(err.0.contains("too long"));
    }

    #[test]
    fn harness_input_null_byte_rejected() {
        let err = validate_harness_input("hello\x00world").unwrap_err();
        assert!(err.0.contains("null byte"));
    }

    #[test]
    fn harness_input_control_char_rejected() {
        // ASCII 0x01 (SOH) should be rejected
        let err = validate_harness_input("hello\x01world").unwrap_err();
        assert!(err.0.contains("control character"));
    }

    #[test]
    fn harness_input_tab_allowed() {
        assert!(validate_harness_input("col1\tcol2").is_ok());
    }

    // --- validate_forge_input ---

    #[test]
    fn forge_input_valid() {
        assert!(validate_forge_input("log line 200 OK").is_ok());
    }

    #[test]
    fn forge_input_too_long() {
        let big = "x".repeat(MAX_FORGE_INPUT_BYTES + 1);
        let err = validate_forge_input(&big).unwrap_err();
        assert!(err.0.contains("too long"));
    }

    #[test]
    fn forge_input_null_byte_rejected() {
        assert!(validate_forge_input("a\x00b").is_err());
    }

    // --- validate_capability_fields ---

    #[test]
    fn capability_fields_valid() {
        assert!(validate_capability_fields(&clean_req()).is_ok());
    }

    #[test]
    fn capability_name_too_long() {
        let mut req = clean_req();
        req.capability = "x".repeat(MAX_CAPABILITY_NAME_BYTES + 1);
        let err = validate_capability_fields(&req).unwrap_err();
        assert!(err.0.contains("capability"));
    }

    #[test]
    fn reason_too_long() {
        let mut req = clean_req();
        req.reason = "r".repeat(MAX_REASON_BYTES + 1);
        let err = validate_capability_fields(&req).unwrap_err();
        assert!(err.0.contains("reason"));
    }

    #[test]
    fn input_contract_too_long() {
        let mut req = clean_req();
        req.input_contract = "c".repeat(MAX_CONTRACT_BYTES + 1);
        let err = validate_capability_fields(&req).unwrap_err();
        assert!(err.0.contains("input_contract"));
    }

    #[test]
    fn output_contract_too_long() {
        let mut req = clean_req();
        req.output_contract = "c".repeat(MAX_CONTRACT_BYTES + 1);
        let err = validate_capability_fields(&req).unwrap_err();
        assert!(err.0.contains("output_contract"));
    }

    #[test]
    fn capability_null_byte_rejected() {
        let mut req = clean_req();
        req.capability = "foo\x00bar".into();
        let err = validate_capability_fields(&req).unwrap_err();
        assert!(err.0.contains("null byte"));
    }

    // --- validate_endpoint ---

    #[test]
    fn https_endpoint_accepted() {
        assert!(validate_endpoint("https://api.example.com/v1").is_ok());
    }

    #[test]
    fn http_endpoint_accepted() {
        assert!(validate_endpoint("http://localhost:11434").is_ok());
    }

    #[test]
    fn file_url_rejected() {
        let err = validate_endpoint("file:///etc/passwd").unwrap_err();
        assert!(err.0.contains("http://"));
    }

    #[test]
    fn javascript_url_rejected() {
        assert!(validate_endpoint("javascript:alert(1)").is_err());
    }

    #[test]
    fn bare_hostname_rejected() {
        assert!(validate_endpoint("localhost:8080").is_err());
    }

    // --- validate_soul_path ---

    #[test]
    fn empty_soul_path_accepted() {
        assert!(validate_soul_path("").is_ok());
    }

    #[test]
    fn valid_soul_path_accepted() {
        assert!(validate_soul_path("/home/user/soul.md").is_ok());
    }

    #[test]
    fn path_traversal_rejected() {
        assert!(validate_soul_path("../../etc/passwd.md").is_err());
    }

    #[test]
    fn relative_traversal_rejected() {
        assert!(validate_soul_path("../config/soul.md").is_err());
    }

    #[test]
    fn non_md_extension_rejected() {
        let err = validate_soul_path("/home/user/soul.txt").unwrap_err();
        assert!(err.0.contains(".md"));
    }
}
