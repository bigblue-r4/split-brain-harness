use crate::types::{ArbitratorMode, BackendType, Config, VerifyMode};
use serde::Deserialize;

#[derive(Deserialize, Default)]
struct FileConfig {
    backend: Option<String>,
    endpoint: Option<String>,
    model_name: Option<String>,
    soul_path: Option<String>,
    api_key: Option<String>,
    verify_mode: Option<String>,
    timeout_secs: Option<u64>,
    temperature: Option<f32>,
    memory_path: Option<String>,
    audit_path: Option<String>,
    serve_key: Option<String>,
    serve_rate_limit: Option<u32>,
    serve_max_body_bytes: Option<usize>,
    session_log_path: Option<String>,
    context_path: Option<String>,
    arbitrator: Option<String>,
    refine_max_iters: Option<usize>,
    refine_confidence_target: Option<f32>,
    stop_and_ask_threshold: Option<f32>,
    calibration_path: Option<String>,
    request_rationale: Option<bool>,
}

fn load_file_config() -> FileConfig {
    let path = std::env::var("SBH_CONFIG").unwrap_or_else(|_| "config.toml".to_string());
    match std::fs::read_to_string(&path) {
        Ok(c) => toml::from_str(&c).unwrap_or_default(),
        Err(_) => FileConfig::default(),
    }
}

/// Maps a backend name string to a BackendType and its default endpoint.
///
/// Unrecognized strings produce a warning on stderr and fall back to
/// `ollama-native`.  Valid values: `ollama-native`, `openai-compat`,
/// `anthropic`, `local-embedded`.
pub fn parse_backend(s: &str) -> (BackendType, &'static str) {
    match s {
        "openai-compat" => (BackendType::OpenAiCompat, "http://localhost:8080"),
        "anthropic" => (BackendType::Anthropic, "https://api.anthropic.com"),
        "local-embedded" => (BackendType::LocalEmbedded, ""),
        "ollama-native" => (BackendType::OllamaNative, "http://localhost:11434"),
        other => {
            eprintln!(
                "warning: unrecognized SBH_BACKEND={other:?} — \
                 valid values: ollama-native, openai-compat, anthropic, local-embedded. \
                 Falling back to ollama-native."
            );
            (BackendType::OllamaNative, "http://localhost:11434")
        }
    }
}

pub fn parse_verify_mode(s: &str) -> VerifyMode {
    match s {
        "llm" => VerifyMode::Llm,
        "reconcile" => VerifyMode::Reconcile,
        "none" => VerifyMode::None,
        _ => VerifyMode::Deterministic,
    }
}

/// Maps an arbitrator-mode name to an `ArbitratorMode`. Unknown → default (`Rules`).
pub fn parse_arbitrator_mode(s: &str) -> ArbitratorMode {
    match s {
        "off" => ArbitratorMode::Off,
        "rules" => ArbitratorMode::Rules,
        other => {
            eprintln!(
                "warning: unrecognized SBH_ARBITRATOR={other:?} — \
                 valid values: rules, off. Falling back to rules."
            );
            ArbitratorMode::Rules
        }
    }
}

/// Build Config from env vars → config.toml → hardcoded defaults.
pub fn build_config() -> Config {
    let file = load_file_config();
    let backend_str = std::env::var("SBH_BACKEND")
        .ok()
        .or(file.backend)
        .unwrap_or_else(|| "ollama-native".to_string());
    let (backend, default_ep) = parse_backend(&backend_str);
    let default_model = match &backend {
        BackendType::Anthropic => "claude-sonnet-4-6",
        _ => "llama3.2:3b",
    };
    Config {
        backend,
        endpoint: std::env::var("SBH_ENDPOINT")
            .ok()
            .or(file.endpoint)
            .unwrap_or_else(|| default_ep.to_string()),
        model_name: std::env::var("SBH_MODEL")
            .ok()
            .or(file.model_name)
            .unwrap_or_else(|| default_model.to_string()),
        soul_path: std::env::var("SBH_SOUL_PATH")
            .ok()
            .or(file.soul_path)
            .unwrap_or_default(),
        api_key: std::env::var("SBH_API_KEY").ok().or(file.api_key),
        verify_mode: std::env::var("SBH_VERIFY")
            .ok()
            .or(file.verify_mode)
            .map(|s| parse_verify_mode(&s))
            .unwrap_or_default(),
        timeout_secs: std::env::var("SBH_TIMEOUT_SECONDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .or(file.timeout_secs)
            .unwrap_or(120),
        temperature: std::env::var("SBH_TEMPERATURE")
            .ok()
            .and_then(|s| s.parse().ok())
            .or(file.temperature)
            .unwrap_or(0.1),
        dump_prompt: false,
        dump_raw: false,
        memory_path: std::env::var("SBH_MEMORY_PATH").ok().or(file.memory_path),
        audit_path: std::env::var("SBH_AUDIT_PATH").ok().or(file.audit_path),
        serve_key: std::env::var("SBH_SERVE_KEY").ok().or(file.serve_key),
        serve_rate_limit: std::env::var("SBH_SERVE_RATE")
            .ok()
            .and_then(|s| s.parse().ok())
            .or(file.serve_rate_limit)
            .unwrap_or(60),
        serve_max_body_bytes: std::env::var("SBH_SERVE_MAX_BODY")
            .ok()
            .and_then(|s| s.parse().ok())
            .or(file.serve_max_body_bytes)
            .unwrap_or(1_048_576),
        session_log_path: std::env::var("SBH_SESSION_LOG")
            .ok()
            .or(file.session_log_path),
        context_path: std::env::var("SBH_CONTEXT_PATH").ok().or(file.context_path),
        arbitrator: std::env::var("SBH_ARBITRATOR")
            .ok()
            .or(file.arbitrator)
            .map(|s| parse_arbitrator_mode(&s))
            .unwrap_or_default(),
        refine_max_iters: std::env::var("SBH_REFINE_ITERS")
            .ok()
            .and_then(|s| s.parse().ok())
            .or(file.refine_max_iters)
            .unwrap_or(2),
        refine_confidence_target: std::env::var("SBH_REFINE_TARGET")
            .ok()
            .and_then(|s| s.parse().ok())
            .or(file.refine_confidence_target)
            .unwrap_or(0.4),
        stop_and_ask_threshold: std::env::var("SBH_STOP_THRESHOLD")
            .ok()
            .and_then(|s| s.parse().ok())
            .or(file.stop_and_ask_threshold)
            .unwrap_or(0.4),
        calibration_path: std::env::var("SBH_CALIBRATION_PATH")
            .ok()
            .or(file.calibration_path),
        request_rationale: std::env::var("SBH_RATIONALE")
            .ok()
            .map(|s| {
                matches!(
                    s.trim().to_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .or(file.request_rationale)
            .unwrap_or(false),
    }
}

/// Validate a Config and return a list of human-readable error messages.
///
/// Should be called before dispatching any command that reaches the backend
/// (analyze, serve, forge).  The `doctor` command bypasses this and does its
/// own reporting so users can inspect a broken config.
pub fn validate_config(config: &Config) -> Result<(), Vec<String>> {
    let mut errors: Vec<String> = Vec::new();

    if config.model_name.trim().is_empty() {
        errors.push("model_name is empty — set SBH_MODEL or model_name in config.toml".into());
    }

    if config.timeout_secs == 0 {
        errors.push(
            "timeout_secs must be > 0 — set SBH_TIMEOUT_SECONDS or timeout_secs in config.toml"
                .into(),
        );
    }

    if !(0.0..=2.0).contains(&config.temperature) {
        errors.push(
            "temperature must be between 0.0 and 2.0 — set SBH_TEMPERATURE or temperature in config.toml"
                .into(),
        );
    }

    if config.serve_rate_limit == 0 {
        errors.push(
            "serve_rate_limit must be > 0 — set SBH_SERVE_RATE or serve_rate_limit in config.toml"
                .into(),
        );
    }

    if config.serve_max_body_bytes == 0 {
        errors.push(
            "serve_max_body_bytes must be > 0 — set SBH_SERVE_MAX_BODY or serve_max_body_bytes in config.toml"
                .into(),
        );
    }

    if matches!(config.backend, BackendType::Anthropic)
        && config
            .api_key
            .as_deref()
            .map(|k| k.trim().is_empty())
            .unwrap_or(true)
    {
        errors.push(
            "SBH_API_KEY is required when using the anthropic backend — \
             set SBH_API_KEY or api_key in config.toml"
                .into(),
        );
    }

    if matches!(config.backend, BackendType::LocalEmbedded) {
        errors.push(
            "local-embedded backend is not yet implemented — \
             use ollama-native, openai-compat, or anthropic"
                .into(),
        );
    }

    if config.refine_max_iters == 0 {
        errors.push(
            "refine_max_iters must be >= 1 — set SBH_REFINE_ITERS or refine_max_iters in config.toml"
                .into(),
        );
    }

    if !(0.0..=1.0).contains(&config.refine_confidence_target) {
        errors.push(
            "refine_confidence_target must be between 0.0 and 1.0 — set SBH_REFINE_TARGET or refine_confidence_target in config.toml"
                .into(),
        );
    }

    if !(0.0..=1.0).contains(&config.stop_and_ask_threshold) {
        errors.push(
            "stop_and_ask_threshold must be between 0.0 and 1.0 — set SBH_STOP_THRESHOLD or stop_and_ask_threshold in config.toml"
                .into(),
        );
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::BackendType;

    fn base_config() -> Config {
        Config::default()
    }

    #[test]
    fn valid_ollama_config_passes() {
        assert!(validate_config(&base_config()).is_ok());
    }

    #[test]
    fn anthropic_without_api_key_is_invalid() {
        let mut c = base_config();
        c.backend = BackendType::Anthropic;
        c.api_key = None;
        let errs = validate_config(&c).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("SBH_API_KEY")));
    }

    #[test]
    fn anthropic_with_empty_api_key_is_invalid() {
        let mut c = base_config();
        c.backend = BackendType::Anthropic;
        c.api_key = Some("   ".into());
        let errs = validate_config(&c).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("SBH_API_KEY")));
    }

    #[test]
    fn anthropic_with_api_key_passes() {
        let mut c = base_config();
        c.backend = BackendType::Anthropic;
        c.api_key = Some("sk-ant-test".into());
        assert!(validate_config(&c).is_ok());
    }

    #[test]
    fn local_embedded_is_invalid() {
        let mut c = base_config();
        c.backend = BackendType::LocalEmbedded;
        let errs = validate_config(&c).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("local-embedded")));
    }

    #[test]
    fn empty_model_name_is_invalid() {
        let mut c = base_config();
        c.model_name = "   ".into();
        let errs = validate_config(&c).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("model_name")));
    }

    #[test]
    fn zero_timeout_is_invalid() {
        let mut c = base_config();
        c.timeout_secs = 0;
        let errs = validate_config(&c).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("timeout_secs")));
    }

    #[test]
    fn zero_rate_limit_is_invalid() {
        let mut c = base_config();
        c.serve_rate_limit = 0;
        let errs = validate_config(&c).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("serve_rate_limit")));
    }

    #[test]
    fn zero_max_body_is_invalid() {
        let mut c = base_config();
        c.serve_max_body_bytes = 0;
        let errs = validate_config(&c).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("serve_max_body_bytes")));
    }

    #[test]
    fn multiple_errors_all_reported() {
        let mut c = base_config();
        c.model_name = String::new();
        c.timeout_secs = 0;
        c.serve_rate_limit = 0;
        let errs = validate_config(&c).unwrap_err();
        assert!(errs.len() >= 3);
    }

    #[test]
    fn parse_backend_known_values() {
        assert!(matches!(
            parse_backend("ollama-native").0,
            BackendType::OllamaNative
        ));
        assert!(matches!(
            parse_backend("openai-compat").0,
            BackendType::OpenAiCompat
        ));
        assert!(matches!(
            parse_backend("anthropic").0,
            BackendType::Anthropic
        ));
        assert!(matches!(
            parse_backend("local-embedded").0,
            BackendType::LocalEmbedded
        ));
    }

    #[test]
    fn parse_verify_mode_maps_reconcile() {
        // Regression: "reconcile" previously fell through to Deterministic.
        assert!(matches!(
            parse_verify_mode("reconcile"),
            VerifyMode::Reconcile
        ));
        assert!(matches!(parse_verify_mode("llm"), VerifyMode::Llm));
        assert!(matches!(parse_verify_mode("none"), VerifyMode::None));
        assert!(matches!(
            parse_verify_mode("anything"),
            VerifyMode::Deterministic
        ));
    }

    #[test]
    fn parse_arbitrator_mode_values() {
        assert_eq!(parse_arbitrator_mode("off"), ArbitratorMode::Off);
        assert_eq!(parse_arbitrator_mode("rules"), ArbitratorMode::Rules);
        // Unknown falls back to Rules (warning to stderr).
        assert_eq!(parse_arbitrator_mode("typo"), ArbitratorMode::Rules);
    }

    #[test]
    fn zero_refine_iters_is_invalid() {
        let mut c = base_config();
        c.refine_max_iters = 0;
        let errs = validate_config(&c).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("refine_max_iters")));
    }

    #[test]
    fn out_of_range_threshold_is_invalid() {
        let mut c = base_config();
        c.stop_and_ask_threshold = 1.5;
        let errs = validate_config(&c).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("stop_and_ask_threshold")));
    }

    #[test]
    fn parse_backend_unknown_falls_back_to_ollama() {
        // Falls back to ollama-native with a warning (warning goes to stderr, not assertable here)
        assert!(matches!(
            parse_backend("typo-backend").0,
            BackendType::OllamaNative
        ));
    }
}
