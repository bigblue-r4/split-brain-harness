//! Security utilities: secret redaction for telemetry/traces and
//! soul-path validation.
//!
//! No regex dependency by design — the project keeps third-party surface
//! area minimal, so both the secret detector and the path validator are
//! hand-rolled over plain string scanning.

use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Secret redaction
// ---------------------------------------------------------------------------

/// Key names whose values are redacted when they appear as `key=value`
/// or `key: value` pairs (case-insensitive).
const SENSITIVE_KEYS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "api-key",
    "access_token",
    "refresh_token",
    "private_key",
    "authorization",
    "auth",
];

/// Token prefixes that identify a credential regardless of context.
const TOKEN_PREFIXES: &[&str] = &[
    "sk-",         // OpenAI / Anthropic style API keys
    "ghp_",        // GitHub personal access token
    "gho_",        // GitHub OAuth token
    "github_pat_", // GitHub fine-grained PAT
    "xoxb-",       // Slack bot token
    "xoxp-",       // Slack user token
    "AKIA",        // AWS access key ID
    "eyJ",         // JWT header
];

/// Redact sensitive patterns from `text` before it is placed in trace
/// evidence or serialized telemetry. Covers `key=value` credentials,
/// bearer tokens, well-known token prefixes, email addresses, and SSNs.
///
/// The output preserves the original word layout so truncated evidence
/// stays readable.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_was_bearer = false;

    for segment in split_keep_whitespace(text) {
        if segment.chars().all(char::is_whitespace) {
            out.push_str(segment);
            continue;
        }

        let redacted = if prev_was_bearer {
            Some("[REDACTED]".to_string())
        } else {
            redact_word(segment)
        };

        prev_was_bearer = segment.eq_ignore_ascii_case("bearer");

        match redacted {
            Some(r) => out.push_str(&r),
            None => out.push_str(segment),
        }
    }
    out
}

/// Split into alternating word / whitespace segments, preserving both.
fn split_keep_whitespace(text: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut in_ws: Option<bool> = None;
    for (i, c) in text.char_indices() {
        let ws = c.is_whitespace();
        match in_ws {
            Some(prev) if prev != ws => {
                segments.push(&text[start..i]);
                start = i;
                in_ws = Some(ws);
            }
            None => in_ws = Some(ws),
            _ => {}
        }
    }
    if start < text.len() {
        segments.push(&text[start..]);
    }
    segments
}

fn redact_word(word: &str) -> Option<String> {
    // key=value / key:value credentials
    for sep in ['=', ':'] {
        if let Some(idx) = word.find(sep) {
            let key = word[..idx]
                .trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
                .to_lowercase();
            let value = &word[idx + 1..];
            if !value.is_empty() && SENSITIVE_KEYS.iter().any(|k| key.ends_with(k)) {
                return Some(format!("{}{}[REDACTED]", &word[..idx], sep));
            }
        }
    }

    // Well-known token prefixes (strip leading quotes/punctuation first)
    let bare = word.trim_matches(|c: char| matches!(c, '"' | '\'' | '(' | ')' | ',' | ';'));
    for prefix in TOKEN_PREFIXES {
        if bare.starts_with(prefix) && bare.len() >= prefix.len() + 8 {
            return Some("[REDACTED_TOKEN]".to_string());
        }
    }

    // Email addresses
    if is_email(bare) {
        return Some("[REDACTED_EMAIL]".to_string());
    }

    // US SSN: ddd-dd-dddd
    if is_ssn(bare) {
        return Some("[REDACTED_SSN]".to_string());
    }

    None
}

fn is_email(word: &str) -> bool {
    let Some(at) = word.find('@') else {
        return false;
    };
    let (local, domain) = (&word[..at], &word[at + 1..]);
    if local.is_empty() || domain.len() < 4 || domain.contains('@') {
        return false;
    }
    let Some(dot) = domain.rfind('.') else {
        return false;
    };
    let tld = &domain[dot + 1..];
    tld.len() >= 2
        && tld.chars().all(|c| c.is_ascii_alphabetic())
        && domain[..dot].chars().any(|c| c.is_alphanumeric())
        && local.chars().any(|c| c.is_alphanumeric())
}

fn is_ssn(word: &str) -> bool {
    let b = word.as_bytes();
    b.len() == 11
        && b[3] == b'-'
        && b[6] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, &c)| matches!(i, 3 | 6) || c.is_ascii_digit())
}

// ---------------------------------------------------------------------------
// Soul path validation
// ---------------------------------------------------------------------------

/// Validate a user-supplied soul path (e.g. from `SBH_SOUL_PATH`) before
/// reading it. Canonicalizes the path (resolving symlinks), requires a
/// regular `.md` file, and requires the resolved path to live under one of
/// the allowed roots: the current working directory, the user's home
/// directory, or `/usr/share/sbh`. Prevents symlink traversal to arbitrary
/// files like `/etc/passwd`.
pub fn validate_soul_path(path: &str) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|e| anyhow!("soul path {path:?} cannot be resolved: {e}"))?;

    if !canonical.is_file() {
        return Err(anyhow!("soul path {path:?} is not a regular file"));
    }
    if canonical.extension().and_then(|e| e.to_str()) != Some("md") {
        return Err(anyhow!(
            "soul path {path:?} must be a .md file (resolved to {canonical:?})"
        ));
    }

    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir().and_then(std::fs::canonicalize) {
        roots.push(cwd);
    }
    if let Some(home) = std::env::var_os("HOME") {
        if let Ok(h) = std::fs::canonicalize(&home) {
            roots.push(h);
        }
    }
    roots.push(Path::new("/usr/share/sbh").to_path_buf());

    if roots.iter().any(|r| canonical.starts_with(r)) {
        Ok(canonical)
    } else {
        Err(anyhow!(
            "soul path {path:?} resolves to {canonical:?}, outside the allowed \
             directories (cwd, home, /usr/share/sbh) — refusing to load"
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- redact ---

    #[test]
    fn redacts_password_assignment() {
        let out = redact("login with password=hunter2 please");
        assert_eq!(out, "login with password=[REDACTED] please");
    }

    #[test]
    fn redacts_api_key_colon_form() {
        let out = redact("api_key:sk_live_abcdef123456");
        assert_eq!(out, "api_key:[REDACTED]");
    }

    #[test]
    fn redacts_bearer_token() {
        let out = redact("Authorization: Bearer abc123def456");
        assert!(out.ends_with("Bearer [REDACTED]"), "got: {out}");
    }

    #[test]
    fn redacts_openai_style_key() {
        let out = redact("my key is sk-proj-abcdefghij1234");
        assert_eq!(out, "my key is [REDACTED_TOKEN]");
    }

    #[test]
    fn redacts_aws_access_key() {
        let out = redact("AKIAIOSFODNN7EXAMPLE was leaked");
        assert_eq!(out, "[REDACTED_TOKEN] was leaked");
    }

    #[test]
    fn redacts_email() {
        let out = redact("contact alice@example.com for access");
        assert_eq!(out, "contact [REDACTED_EMAIL] for access");
    }

    #[test]
    fn redacts_ssn() {
        let out = redact("SSN 123-45-6789 on file");
        assert_eq!(out, "SSN [REDACTED_SSN] on file");
    }

    #[test]
    fn benign_text_unchanged() {
        let input = "write me a haiku about the north shore at dawn";
        assert_eq!(redact(input), input);
    }

    #[test]
    fn short_sk_prefix_not_redacted() {
        // "sk-1" is too short to be a credential
        let input = "the sk-1 part number";
        assert_eq!(redact(input), input);
    }

    #[test]
    fn preserves_whitespace_layout() {
        let out = redact("a  b\tpassword=x\nc");
        assert_eq!(out, "a  b\tpassword=[REDACTED]\nc");
    }

    // --- validate_soul_path ---

    #[test]
    fn accepts_soul_in_cwd() {
        // soul.md ships at the crate root, which is the test cwd
        let p = validate_soul_path("soul.md").expect("repo soul.md should validate");
        assert!(p.ends_with("soul.md"));
    }

    #[test]
    fn rejects_etc_passwd() {
        let err = validate_soul_path("/etc/passwd").unwrap_err().to_string();
        assert!(err.contains(".md") || err.contains("outside"), "got: {err}");
    }

    #[test]
    fn rejects_nonexistent_path() {
        assert!(validate_soul_path("/nonexistent/soul.md").is_err());
    }

    #[test]
    fn rejects_symlink_escaping_allowed_roots() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("evil.md");
        std::os::unix::fs::symlink("/etc/passwd", &link).unwrap();
        let err = validate_soul_path(link.to_str().unwrap())
            .unwrap_err()
            .to_string();
        // canonicalize resolves the symlink to /etc/passwd → rejected
        assert!(err.contains(".md") || err.contains("outside"), "got: {err}");
    }

    #[test]
    fn rejects_non_md_file_in_cwd() {
        assert!(validate_soul_path("Cargo.toml").is_err());
    }
}
