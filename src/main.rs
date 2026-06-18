use anyhow::{anyhow, Result};
use serde::Deserialize;
use split_brain_harness::{
    analyze,
    types::{BackendType, Config, HarnessResult, VerifyMode},
};
use std::io::Read;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (raw_output, show_trace, input) = parse_args(&args)?;

    let config = build_config();
    let result = analyze(&input, &config).await?;

    if result.verification.stop_and_ask {
        eprintln!(
            "WARNING: stop_and_ask=true (confidence={:.2}) — result may be unreliable. \
             Provide more context or review manually.",
            result.verification.confidence
        );
    }

    print_result(&result, raw_output, show_trace)?;
    Ok(())
}

fn print_result(result: &HarnessResult, raw: bool, show_trace: bool) -> Result<()> {
    let output = if show_trace {
        if raw {
            serde_json::to_string(result)?
        } else {
            serde_json::to_string_pretty(result)?
        }
    } else {
        // Default: telemetry + verification only, no trace
        let slim = serde_json::json!({
            "telemetry":    result.telemetry,
            "verification": result.verification,
        });
        if raw {
            serde_json::to_string(&slim)?
        } else {
            serde_json::to_string_pretty(&slim)?
        }
    };
    println!("{output}");
    Ok(())
}

fn parse_args(args: &[String]) -> Result<(bool, bool, String)> {
    let raw = args.contains(&"--raw".to_string());
    let show_trace = args.contains(&"--trace".to_string());

    if args.contains(&"--stdin".to_string()) {
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input)?;
        return Ok((raw, show_trace, input.trim().to_string()));
    }

    let positional: Vec<&str> = args[1..]
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(|s| s.as_str())
        .collect();

    if positional.is_empty() {
        return Err(anyhow!(
            "Usage: split-brain-harness [--raw] [--trace] \"input text\"\n\
             Usage: echo \"input\" | split-brain-harness --stdin [--raw] [--trace]"
        ));
    }

    Ok((raw, show_trace, positional.join(" ")))
}

// ---------------------------------------------------------------------------
// Config loading — priority: env vars > config.toml > hardcoded defaults
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct FileConfig {
    backend: Option<String>,
    endpoint: Option<String>,
    model_name: Option<String>,
    soul_path: Option<String>,
    api_key: Option<String>,
    verify_mode: Option<String>,
}

fn load_file_config() -> FileConfig {
    let path = std::env::var("SBH_CONFIG").unwrap_or_else(|_| "config.toml".to_string());
    match std::fs::read_to_string(&path) {
        Ok(contents) => toml::from_str(&contents).unwrap_or_default(),
        Err(_) => FileConfig::default(),
    }
}

fn parse_backend(s: &str) -> (BackendType, &'static str) {
    match s {
        "openai-compat" => (BackendType::OpenAiCompat, "http://localhost:8080"),
        "anthropic" => (BackendType::Anthropic, "https://api.anthropic.com"),
        "local-embedded" => (BackendType::LocalEmbedded, ""),
        _ => (BackendType::OllamaNative, "http://localhost:11434"),
    }
}

fn parse_verify_mode(s: &str) -> VerifyMode {
    match s {
        "llm" => VerifyMode::Llm,
        "none" => VerifyMode::None,
        _ => VerifyMode::Deterministic,
    }
}

fn build_config() -> Config {
    let file = load_file_config();

    let backend_str = std::env::var("SBH_BACKEND")
        .ok()
        .or(file.backend)
        .unwrap_or_else(|| "ollama-native".to_string());

    let (backend, default_endpoint) = parse_backend(&backend_str);

    let default_model = match &backend {
        BackendType::Anthropic => "claude-sonnet-4-6",
        _ => "llama3.2:3b",
    };

    let verify_mode = std::env::var("SBH_VERIFY")
        .ok()
        .or(file.verify_mode)
        .map(|s| parse_verify_mode(&s))
        .unwrap_or_default();

    Config {
        backend,
        endpoint: std::env::var("SBH_ENDPOINT")
            .ok()
            .or(file.endpoint)
            .unwrap_or_else(|| default_endpoint.to_string()),
        model_name: std::env::var("SBH_MODEL")
            .ok()
            .or(file.model_name)
            .unwrap_or_else(|| default_model.to_string()),
        soul_path: std::env::var("SBH_SOUL_PATH")
            .ok()
            .or(file.soul_path)
            .unwrap_or_default(),
        api_key: std::env::var("SBH_API_KEY").ok().or(file.api_key),
        verify_mode,
    }
}
