/// Static analysis of generated Rust source code.
///
/// Scans source text for forbidden patterns without compilation.
/// Fast, deterministic, and free of subprocess calls.
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Forbidden patterns
// ---------------------------------------------------------------------------

/// (kind, pattern) pairs — kind is a category label, pattern is the literal
/// string to scan for. Checked line-by-line.
const FORBIDDEN: &[(&str, &str)] = &[
    // Process spawning
    ("process_spawn", "process::Command"),
    ("process_spawn", "Command::new("),
    // Filesystem writes
    ("filesystem_write", "fs::write("),
    ("filesystem_write", "File::create("),
    ("filesystem_write", "OpenOptions"),
    ("filesystem_write", ".write_all("),
    // Network access
    ("network_access", "std::net::"),
    ("network_access", "TcpStream"),
    ("network_access", "UdpSocket"),
    ("network_access", "reqwest"),
    ("network_access", "ureq::"),
    ("network_access", "hyper::"),
    ("network_access", "tokio::net"),
    // Unsafe code
    ("unsafe_code", "unsafe {"),
    ("unsafe_code", "unsafe fn "),
    ("unsafe_code", "unsafe impl "),
    // Environment access
    ("env_access", "std::env::"),
    ("env_access", "env::var("),
    ("env_access", "env::args("),
    // External crate usage — stdlib only is permitted
    ("external_crate", "serde_json"),
    ("external_crate", "serde::"),
    ("external_crate", "tokio::"),
    ("external_crate", "anyhow::"),
    ("external_crate", "thiserror::"),
    ("external_crate", "regex::"),
    ("external_crate", "chrono::"),
    ("external_crate", "rand::"),
    ("external_crate", "uuid::"),
    ("external_crate", "base64::"),
];

// ---------------------------------------------------------------------------
// Report types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StaticViolation {
    pub kind: String,
    pub pattern: String,
    pub line: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StaticAnalysisReport {
    pub passed: bool,
    pub violations: Vec<StaticViolation>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Scan `source` for forbidden patterns. Returns a report of all violations.
/// An empty violations list means the source passed all checks.
pub fn check(source: &str) -> StaticAnalysisReport {
    let mut violations: Vec<StaticViolation> = vec![];

    for (line_num, line) in source.lines().enumerate() {
        // Skip comment lines — // and /// are analysis noise
        let trimmed = line.trim();
        if trimmed.starts_with("//") {
            continue;
        }
        for (kind, pattern) in FORBIDDEN {
            if line.contains(pattern) {
                violations.push(StaticViolation {
                    kind: (*kind).to_string(),
                    pattern: (*pattern).to_string(),
                    line: line_num + 1,
                });
            }
        }
    }

    StaticAnalysisReport {
        passed: violations.is_empty(),
        violations,
    }
}

/// True if the source contains at least one `#[test]` function.
pub fn has_tests(source: &str) -> bool {
    source.contains("#[test]")
}

/// Count the number of `#[test]` occurrences in the source.
pub fn test_count(source: &str) -> usize {
    source.matches("#[test]").count()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const CLEAN_SOURCE: &str = r#"
pub fn run(input: &str) -> Result<String, String> {
    let words = input.split_whitespace().count();
    Ok(format!("{{\"word_count\":{}}}", words))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counts_words() {
        let r = run("hello world").unwrap();
        assert!(r.contains("2"));
    }
    #[test]
    fn empty_input() {
        let r = run("").unwrap();
        assert!(r.contains("0"));
    }
}
"#;

    #[test]
    fn clean_source_passes() {
        let report = check(CLEAN_SOURCE);
        assert!(
            report.passed,
            "clean source should pass: {:?}",
            report.violations
        );
    }

    #[test]
    fn process_command_detected() {
        let source = r#"fn run(i: &str) -> Result<String, String> {
    let _ = std::process::Command::new("ls").output();
    Ok("ok".into())
}"#;
        let report = check(source);
        assert!(!report.passed);
        assert!(report.violations.iter().any(|v| v.kind == "process_spawn"));
    }

    #[test]
    fn command_new_shorthand_detected() {
        let source = r#"fn run(i: &str) -> Result<String, String> {
    Command::new("ls");
    Ok("ok".into())
}"#;
        let report = check(source);
        assert!(report
            .violations
            .iter()
            .any(|v| v.pattern == "Command::new("));
    }

    #[test]
    fn fs_write_detected() {
        let source = r#"fn run(i: &str) -> Result<String, String> {
    std::fs::write("out.txt", i).unwrap();
    Ok("ok".into())
}"#;
        let report = check(source);
        assert!(!report.passed);
        assert!(report
            .violations
            .iter()
            .any(|v| v.kind == "filesystem_write"));
    }

    #[test]
    fn file_create_detected() {
        let source = r#"fn run(i: &str) -> Result<String, String> {
    let f = File::create("out.txt").unwrap();
    Ok("ok".into())
}"#;
        let report = check(source);
        assert!(report
            .violations
            .iter()
            .any(|v| v.pattern == "File::create("));
    }

    #[test]
    fn tcpstream_detected() {
        let source = r#"fn run(i: &str) -> Result<String, String> {
    let s = TcpStream::connect("127.0.0.1:80").unwrap();
    Ok("ok".into())
}"#;
        let report = check(source);
        assert!(report.violations.iter().any(|v| v.kind == "network_access"));
    }

    #[test]
    fn reqwest_detected() {
        let source = r#"fn run(i: &str) -> Result<String, String> {
    reqwest::get("https://example.com");
    Ok("ok".into())
}"#;
        let report = check(source);
        assert!(report.violations.iter().any(|v| v.pattern == "reqwest"));
    }

    #[test]
    fn unsafe_block_detected() {
        let source = r#"fn run(i: &str) -> Result<String, String> {
    unsafe { let _ = 0; }
    Ok("ok".into())
}"#;
        let report = check(source);
        assert!(report.violations.iter().any(|v| v.kind == "unsafe_code"));
    }

    #[test]
    fn unsafe_fn_detected() {
        let source = "unsafe fn run(i: &str) -> Result<String, String> { Ok(\"ok\".into()) }";
        let report = check(source);
        assert!(report.violations.iter().any(|v| v.pattern == "unsafe fn "));
    }

    #[test]
    fn env_var_detected() {
        let source = r#"fn run(i: &str) -> Result<String, String> {
    let k = std::env::var("SECRET").unwrap();
    Ok(k)
}"#;
        let report = check(source);
        assert!(report.violations.iter().any(|v| v.kind == "env_access"));
    }

    #[test]
    fn comment_lines_are_skipped() {
        // A comment mentioning a forbidden pattern should NOT fire
        let source = r#"fn run(i: &str) -> Result<String, String> {
    // do NOT use std::process::Command here
    Ok("ok".into())
}"#;
        let report = check(source);
        assert!(report.passed, "comments must not trigger violations");
    }

    #[test]
    fn violation_line_number_is_accurate() {
        let source =
            "fn run(i: &str) -> Result<String, String> {\n    unsafe { }\n    Ok(\"ok\".into())\n}";
        let report = check(source);
        let unsafe_v = report
            .violations
            .iter()
            .find(|v| v.kind == "unsafe_code")
            .unwrap();
        assert_eq!(unsafe_v.line, 2, "violation line should be 2");
    }

    #[test]
    fn has_tests_true_when_test_attribute_present() {
        assert!(has_tests(CLEAN_SOURCE));
    }

    #[test]
    fn has_tests_false_when_no_test_attribute() {
        let source = "fn run(i: &str) -> Result<String, String> { Ok(\"ok\".into()) }";
        assert!(!has_tests(source));
    }

    #[test]
    fn test_count_correct() {
        assert_eq!(test_count(CLEAN_SOURCE), 2);
    }

    #[test]
    fn multiple_violations_all_reported() {
        let source = r#"fn run(i: &str) -> Result<String, String> {
    unsafe { }
    let _ = TcpStream::connect("x").unwrap();
    Ok("ok".into())
}"#;
        let report = check(source);
        assert!(!report.passed);
        assert!(report.violations.len() >= 2);
    }
}
