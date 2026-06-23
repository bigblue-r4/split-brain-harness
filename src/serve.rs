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
/// Hardening:
///   - `SBH_SERVE_KEY`      — require Bearer token on all requests
///   - `SBH_SERVE_RATE`     — max requests/min per IP (default 60)
///   - `SBH_SERVE_MAX_BODY` — max body bytes (default 1 MiB)
///
/// Start with: `sbh serve [--listen <addr>]`   default: 127.0.0.1:8088
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, State},
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
    /// Per-IP sliding window: timestamps of requests in the last minute.
    rate_limiter: Arc<Mutex<HashMap<IpAddr, VecDeque<Instant>>>>,
}

// ---------------------------------------------------------------------------
// Rate limiter — sliding window, no extra deps
// ---------------------------------------------------------------------------

fn check_rate_limit(
    limiter: &Arc<Mutex<HashMap<IpAddr, VecDeque<Instant>>>>,
    ip: IpAddr,
    max_per_minute: u32,
) -> bool {
    let now = Instant::now();
    let window = Duration::from_secs(60);
    let mut map = limiter.lock().unwrap();
    let queue = map.entry(ip).or_default();
    while let Some(&front) = queue.front() {
        if now.duration_since(front) > window {
            queue.pop_front();
        } else {
            break;
        }
    }
    if queue.len() >= max_per_minute as usize {
        return false;
    }
    queue.push_back(now);
    true
}

// ---------------------------------------------------------------------------
// Route handler
// ---------------------------------------------------------------------------

async fn chat_completions(
    State(state): State<ServeState>,
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<ChatRequest>,
) -> impl IntoResponse {
    let config = &*state.config;

    // --- serve-level auth (checked before anything else) ---
    if let Some(sk) = &config.serve_key {
        let provided = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim_start_matches("Bearer ").trim().to_string())
            .unwrap_or_default();
        if &provided != sk {
            let body = ErrorBody {
                error: ErrorDetail {
                    message: "Unauthorized: invalid or missing SBH serve key.".into(),
                    kind: "authentication_error".into(),
                },
            };
            return (
                StatusCode::UNAUTHORIZED,
                HeaderMap::new(),
                Json(serde_json::to_value(body).unwrap()),
            )
                .into_response();
        }
    }

    // --- per-IP rate limit ---
    let ip = remote_addr.ip();
    if !check_rate_limit(&state.rate_limiter, ip, config.serve_rate_limit) {
        let body = ErrorBody {
            error: ErrorDetail {
                message: format!(
                    "Rate limit exceeded: max {} requests/min per IP.",
                    config.serve_rate_limit
                ),
                kind: "rate_limit_error".into(),
            },
        };
        return (
            StatusCode::TOO_MANY_REQUESTS,
            HeaderMap::new(),
            Json(serde_json::to_value(body).unwrap()),
        )
            .into_response();
    }

    // --- streaming not supported ---
    if req.stream {
        let body = ErrorBody {
            error: ErrorDetail {
                message: "sbh serve does not support streaming. Set stream=false.".into(),
                kind: "unsupported_parameter".into(),
            },
        };
        return (
            StatusCode::BAD_REQUEST,
            HeaderMap::new(),
            Json(serde_json::to_value(body).unwrap()),
        )
            .into_response();
    }

    // --- extract last user message ---
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
        return (
            StatusCode::BAD_REQUEST,
            HeaderMap::new(),
            Json(serde_json::to_value(body).unwrap()),
        )
            .into_response();
    }

    // --- optionally forward Authorization as upstream API key
    //     (only when serve_key is NOT set — when serve_key is set, auth is
    //      used for access control and must not leak to the upstream) ---
    let mut cfg = (*state.config).clone();
    if config.serve_key.is_none() {
        if let Some(auth) = headers.get("authorization") {
            if let Ok(val) = auth.to_str() {
                let key = val.trim_start_matches("Bearer ").trim().to_string();
                if !key.is_empty() {
                    cfg.api_key = Some(key);
                }
            }
        }
    }

    // --- run the full harness pipeline ---
    let result = match analyze(user_input, &cfg).await {
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
            return (
                status,
                HeaderMap::new(),
                Json(serde_json::to_value(body).unwrap()),
            )
                .into_response();
        }
    };

    // --- build response ---
    let telemetry_json = serde_json::to_string(&result).unwrap_or_else(|_| "{}".into());
    let content = format!(
        "{}\n\n<!-- sbh-telemetry: {} -->",
        summarize_result(&result),
        telemetry_json,
    );

    let model_name = req.model.as_deref().unwrap_or(&cfg.model_name).to_string();

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

    let mut resp_headers = HeaderMap::new();
    if let Ok(encoded) = url_encode(&telemetry_json) {
        if let Ok(val) = HeaderValue::from_str(&encoded) {
            resp_headers.insert("x-sbh-telemetry", val);
        }
    }
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
    let rate_limit = config.serve_rate_limit;
    let max_body = config.serve_max_body_bytes;
    let auth_enabled = config.serve_key.is_some();

    let state = ServeState {
        config: Arc::new(config),
        rate_limiter: Arc::new(Mutex::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/health", axum::routing::get(health))
        .layer(DefaultBodyLimit::max(max_body))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(listen).await?;
    let addr = listener.local_addr()?;
    eprintln!("sbh serve: listening on http://{addr}");
    eprintln!("  POST /v1/chat/completions  — OpenAI-compatible harness proxy");
    eprintln!("  GET  /health               — liveness check");
    eprintln!(
        "  auth: {}  rate: {}/min/IP  max-body: {} bytes",
        if auth_enabled { "enabled" } else { "disabled" },
        rate_limit,
        max_body,
    );

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
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
        if v.stop_and_ask {
            "\n⚠ stop_and_ask=true — low confidence, review before acting"
        } else {
            ""
        },
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
    fn rate_limit_allows_up_to_max() {
        let limiter = Arc::new(Mutex::new(HashMap::new()));
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        for _ in 0..5 {
            assert!(check_rate_limit(&limiter, ip, 5));
        }
        assert!(!check_rate_limit(&limiter, ip, 5));
    }

    #[test]
    fn rate_limit_different_ips_are_independent() {
        let limiter = Arc::new(Mutex::new(HashMap::new()));
        let ip1: IpAddr = "10.0.0.1".parse().unwrap();
        let ip2: IpAddr = "10.0.0.2".parse().unwrap();
        for _ in 0..3 {
            assert!(check_rate_limit(&limiter, ip1, 3));
        }
        assert!(!check_rate_limit(&limiter, ip1, 3));
        assert!(check_rate_limit(&limiter, ip2, 3));
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
