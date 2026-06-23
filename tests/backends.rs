//! Integration tests for backend initialization and early-exit error paths.
//! These tests do not make real network calls — they verify that each backend
//! produces the expected error immediately for trivially-invalid config, and
//! that `init_engine` builds the right backend for each config variant.

use split_brain_harness::{
    backends,
    types::{BackendType, Config, VerifyMode},
};

fn make_config(backend: BackendType, api_key: Option<&str>) -> Config {
    Config {
        backend,
        endpoint: "http://localhost:1".into(), // unreachable — no real calls in these tests
        model_name: "test-model".into(),
        soul_path: String::new(),
        api_key: api_key.map(String::from),
        verify_mode: VerifyMode::Deterministic,
        timeout_secs: 5,
        dump_prompt: false,
        dump_raw: false,
        memory_path: None,
        audit_path: None,
        serve_key: None,
        serve_rate_limit: 60,
        serve_max_body_bytes: 1_048_576,
        session_log_path: None,
    }
}

// ---------------------------------------------------------------------------
// local-embedded — errors synchronously, no network
// ---------------------------------------------------------------------------

#[tokio::test]
async fn local_embedded_returns_not_implemented_error() {
    let cfg = make_config(BackendType::LocalEmbedded, None);
    let engine = backends::init_engine(&cfg);
    let err = engine.generate("sys", "payload").await.unwrap_err();
    assert!(
        err.contains("not yet implemented"),
        "expected 'not yet implemented' in error, got: {err}"
    );
}

#[tokio::test]
async fn local_embedded_error_mentions_model() {
    let cfg = make_config(BackendType::LocalEmbedded, None);
    let engine = backends::init_engine(&cfg);
    let err = engine.generate("sys", "payload").await.unwrap_err();
    assert!(
        err.contains("test-model"),
        "expected model name in error, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// anthropic — errors synchronously when api_key is empty, no network
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anthropic_empty_key_errors_before_network() {
    let cfg = make_config(BackendType::Anthropic, Some(""));
    let engine = backends::init_engine(&cfg);
    let err = engine.generate("sys", "payload").await.unwrap_err();
    assert!(
        err.contains("SBH_API_KEY"),
        "expected 'SBH_API_KEY' in error, got: {err}"
    );
}

#[tokio::test]
async fn anthropic_no_key_errors_before_network() {
    let cfg = make_config(BackendType::Anthropic, None);
    let engine = backends::init_engine(&cfg);
    let err = engine.generate("sys", "payload").await.unwrap_err();
    assert!(
        err.contains("SBH_API_KEY"),
        "expected 'SBH_API_KEY' in error, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// init_engine — correct backend selected for each config variant
// ---------------------------------------------------------------------------
// We can't downcast Box<dyn InferenceEngine> directly, so we probe behavior:
// local-embedded and anthropic-no-key error immediately and deterministically;
// ollama and openai-compat will fail with a network error (not a logic error).

#[tokio::test]
async fn ollama_engine_errors_with_network_not_logic_error() {
    let cfg = make_config(BackendType::OllamaNative, None);
    let engine = backends::init_engine(&cfg);
    // Should attempt a network call and fail — error must NOT say "not yet implemented"
    let err = engine.generate("sys", "payload").await.unwrap_err();
    assert!(
        !err.contains("not yet implemented"),
        "ollama engine should not return stub error: {err}"
    );
}

#[tokio::test]
async fn openai_engine_errors_with_network_not_logic_error() {
    let cfg = make_config(BackendType::OpenAiCompat, None);
    let engine = backends::init_engine(&cfg);
    let err = engine.generate("sys", "payload").await.unwrap_err();
    assert!(
        !err.contains("not yet implemented"),
        "openai engine should not return stub error: {err}"
    );
}

// ---------------------------------------------------------------------------
// validate_config — covered more thoroughly in config.rs unit tests;
// these ensure it integrates correctly with real Config objects.
// ---------------------------------------------------------------------------

#[test]
fn validate_rejects_local_embedded() {
    let cfg = make_config(BackendType::LocalEmbedded, None);
    let errs = split_brain_harness::validate_config(&cfg).unwrap_err();
    assert!(errs.iter().any(|e| e.contains("local-embedded")));
}

#[test]
fn validate_rejects_anthropic_without_key() {
    let cfg = make_config(BackendType::Anthropic, None);
    let errs = split_brain_harness::validate_config(&cfg).unwrap_err();
    assert!(errs.iter().any(|e| e.contains("SBH_API_KEY")));
}

#[test]
fn validate_accepts_ollama_no_key() {
    let cfg = make_config(BackendType::OllamaNative, None);
    assert!(split_brain_harness::validate_config(&cfg).is_ok());
}
