/// OpenAI-compatible HTTP proxy server.
///
/// Exposes `POST /v1/chat/completions` so any OpenAI-speaking client
/// (LangChain, Continue.dev, Cursor, custom agents) can route through the
/// soul-injected telemetry pipeline with zero code changes.
///
/// Telemetry is returned two ways:
///   1. The `content` field carries both the model's answer AND a
///      `<!-- sbh-telemetry: {...} -->` HTML comment at the end.
///   2. The `x-sbh-telemetry` response header carries the same JSON, URL-encoded.
///
/// Start with: `sbh serve [--listen <addr>]`   default: 127.0.0.1:8088
use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::{analyze, types::Config};

// ---------------------------------------------------------------------------
// Request / response types (OpenAI wire format subset)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub model: Option<String>,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: bool,
    // All other fields are accepted and ignored
    #[serde(flatten)]
    pub _extra: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    pub usage: Usage,
}

#[derive(Debug, Serialize)]
pub struct ChatChoice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: String,
}

#[derive(Debug, Serialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Debug, Serialize)]
struct ErrorDetail {
    message: String,
    #[serde(rename = "type")]
    kind: String,
}

// ---------------------------------------------------------------------------
// Server state
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct ServeState {
    config: Arc<Config>,
}

// ---------------------------------------------------------------------------
// Route handler
// ---------------------------------------------------------------------------

async fn chat_completions(
    State(state): State<ServeState>,
    headers: HeaderMap,
    Json(req): Json<ChatRequest>,
) -> impl IntoResponse {
    // Streaming is not supported — return a clear error
    if req.stream {
        let body = ErrorBody {
            error: ErrorDetail {
                message: "sbh serve does not support streaming. Set stream=false.".into(),
                kind: "unsupported_parameter".into(),
            },
        };
        return (StatusCode::BAD_REQUEST, HeaderMap::new(), Json(serde_json::to_value(body).unwrap())).into_response();
    }

    // Extract the last user message as the input to analyze
    let user_input = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .unwrap_or("");

    if user_input.is_empty() {
        let body = ErrorBody {
            error: ErrorDetail {
                message: "No user message found in messages array.".into(),
                kind: "invalid_request_error".into(),
            },
        };
        return (StatusCode::BAD_REQUEST, HeaderMap::new(), Json(serde_json::to_value(body).unwrap())).into_response();
    }

    // Optionally override API key from the Authorization header
    let mut config = (*state.config).clone();
    if let Some(auth) = headers.get("authorization") {
        if let Ok(val) = auth.to_str() {
            let key = val.trim_start_matches("Bearer ").trim().to_string();
            if !key.is_empty() {
                config.api_key = Some(key);
            }
        }
    }

    // Run through the full harness pipeline (input validation + propose + verify)
    let result = match analyze(user_input, &config).await {
        Ok(r) => r,
        Err(e) => {
            let msg = e.to_string();
            let (status, kind) = if msg.contains("input")
                || msg.contains("null byte")
                || msg.contains("too long")
                || msg.contains("control char")
            {
                (StatusCode::BAD_REQUEST, "invalid_request_error")
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
            };
            let body = ErrorBody {
                error: ErrorDetail {
                    message: msg,
                    kind: kind.into(),
                },
            };
            return (status, HeaderMap::new(), Json(serde_json::to_value(body).unwrap()))
                .into_response();
        }
    };

    // Serialize telemetry for embedding in the response
    let telemetry_json = serde_json::to_string(&result).unwrap_or_else(|_| "{}".into());

    // Build content: telemetry embedded as an HTML comment so plain-text
    // callers don't see it, but structured callers can strip and parse it.
    let content = format!(
        "{}\n\n<!-- sbh-telemetry: {} -->",
        summarize_result(&result),
        telemetry_json,
    );

    let model_name = req
        .model
        .as_deref()
        .unwrap_or(&config.model_name)
        .to_string();

    let response_body = ChatResponse {
        id: format!("sbh-{}", monotonic_id()),
        object: "chat.completion".into(),
        created: unix_now(),
        model: model_name,
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage {
                role: "assistant".into(),
                content,
            },
            finish_reason: "stop".into(),
        }],
        usage: Usage {
            prompt_tokens: (user_input.len() / 4) as u32,
            completion_tokens: (telemetry_json.len() / 4) as u32,
            total_tokens: ((user_input.len() + telemetry_json.len()) / 4) as u32,
        },
    };

    // Attach raw telemetry as a response header (URL-encoded for safety)
    let mut resp_headers = HeaderMap::new();
    if let Ok(encoded) = url_encode(&telemetry_json) {
        if let Ok(val) = HeaderValue::from_str(&encoded) {
            resp_headers.insert("x-sbh-telemetry", val);
        }
    }
    // Signal that this is a harness-wrapped response
    resp_headers.insert(
        "x-sbh-version",
        HeaderValue::from_static(env!("CARGO_PKG_VERSION")),
    );

    (
        StatusCode::OK,
        resp_headers,
        Json(serde_json::to_value(response_body).unwrap()),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Health check
// ---------------------------------------------------------------------------

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "service": "split-brain-harness"
    }))
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub async fn run_server(listen: &str, config: Config) -> anyhow::Result<()> {
    let state = ServeState {
        config: Arc::new(config),
    };

    let app = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/health", axum::routing::get(health))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(listen).await?;
    let addr = listener.local_addr()?;
    eprintln!("sbh serve: listening on http://{addr}");
    eprintln!("  POST /v1/chat/completions  — OpenAI-compatible harness proxy");
    eprintln!("  GET  /health               — liveness check");

    axum::serve(listener, app).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn summarize_result(result: &crate::types::HarnessResult) -> String {
    let t = &result.telemetry;
    let v = &result.verification;
    format!(
        "[SBH Analysis]\nEmotion: {} (intensity {:.2})\nManipulation risk: {}\nCoherence: {:.2}\nVerification: {} (confidence {:.2}){}",
        t.affective_telemetry.primary_emotion,
        t.affective_telemetry.emotional_intensity,
        t.intent_matrix.manipulation_risk,
        t.cognitive_state.coherence_rating,
        if v.passed { "passed" } else { "flagged" },
        v.confidence,
        if v.stop_and_ask { "\n⚠ stop_and_ask=true — low confidence, review before acting" } else { "" },
    )
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn monotonic_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(1);
    CTR.fetch_add(1, Ordering::Relaxed)
}

fn url_encode(s: &str) -> Result<String, ()> {
    // Minimal percent-encoding: replace chars that are invalid in header values
    Ok(s.chars()
        .map(|c| match c {
            ' ' => "%20".to_string(),
            '"' => "%22".to_string(),
            '\n' => "%0A".to_string(),
            '\r' => "%0D".to_string(),
            c if (c as u32) > 127 => format!("%{:02X}", c as u32),
            c => c.to_string(),
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encode_spaces_and_quotes() {
        let s = r#"{"key": "val ue"}"#;
        let encoded = url_encode(s).unwrap();
        assert!(!encoded.contains(' '));
        assert!(!encoded.contains('"'));
        assert!(encoded.contains("%20"));
        assert!(encoded.contains("%22"));
    }

    #[test]
    fn url_encode_clean_string_unchanged() {
        let s = "hello-world_123";
        assert_eq!(url_encode(s).unwrap(), s);
    }

    #[test]
    fn unix_now_is_nonzero() {
        assert!(unix_now() > 0);
    }

    #[test]
    fn monotonic_id_increases() {
        let a = monotonic_id();
        let b = monotonic_id();
        assert!(b > a);
    }

    #[test]
    fn summarize_result_contains_key_fields() {
        use crate::types::*;
        let result = HarnessResult {
            telemetry: TelemetryResult {
                affective_telemetry: AfferentTelemetry {
                    primary_emotion: "neutral".into(),
                    emotional_intensity: 0.1,
                    structural_tone: vec!["analytical".into()],
                },
                intent_matrix: IntentMatrix {
                    stated_objective: "test query".into(),
                    subtextual_motive: "none".into(),
                    manipulation_risk: "low".into(),
                },
                cognitive_state: CognitiveState {
                    urgency_vector: 0.0,
                    coherence_rating: 0.9,
                },
            },
            verification: VerificationReport {
                passed: true,
                consistency_flags: vec![],
                unsupported_claims: vec![],
                assumptions: vec![],
                unresolved: vec![],
                confidence: 0.9,
                stop_and_ask: false,
            },
            trace: vec![],
            capability_request: None,
        };
        let s = summarize_result(&result);
        assert!(s.contains("neutral"));
        assert!(s.contains("low"));
        assert!(s.contains("passed"));
    }
}
