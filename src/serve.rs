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
/// Multi-turn session tracking:
///   Pass `x-sbh-session: <id>` on requests to link turns into a session.
///   The response echoes the session ID. If the manipulation_risk signal shows
///   an upward trend across turns (slow-boil escalation), the response sets
///   `x-sbh-session-alert: escalation_detected`. Sessions expire after 30
///   minutes of inactivity (lazy eviction on each request).
///
/// Start with: `sbh serve [--listen <addr>]`   default: 127.0.0.1:8088
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::{analyze, session_log, types::Config};
use anyhow::Context as _;

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
// Session tracking — multi-turn manipulation detection
// ---------------------------------------------------------------------------

const SESSION_MAX_TURNS: usize = 10;
const SESSION_TTL: Duration = Duration::from_secs(30 * 60);
/// Maximum number of concurrent sessions held in memory. New sessions beyond
/// this cap are refused rather than allowing unbounded HashMap growth.
const SESSION_MAX_COUNT: usize = 10_000;
/// Background sweep interval for evicting expired sessions.
/// The per-request path no longer calls retain() — O(1) instead of O(N).
const SESSION_SWEEP_INTERVAL: Duration = Duration::from_secs(5 * 60);

// ---------------------------------------------------------------------------
// Rate limiter — 16-shard sliding window, no extra deps
// ---------------------------------------------------------------------------

const RATE_LIMITER_SHARDS: usize = 16;
/// Hard cap on total tracked IPs across all shards. Beyond this, new IPs
/// are passed through untracked rather than allocating unbounded memory.
const MAX_TRACKED_IPS: usize = 50_000;
const MAX_IPS_PER_SHARD: usize = MAX_TRACKED_IPS / RATE_LIMITER_SHARDS;

/// One shard: request timestamps per IP, guarded independently.
type RateShard = Mutex<HashMap<IpAddr, VecDeque<Instant>>>;

struct ShardedRateLimiter {
    shards: Box<[RateShard; RATE_LIMITER_SHARDS]>,
}

impl ShardedRateLimiter {
    fn new() -> Self {
        Self {
            shards: Box::new(std::array::from_fn(|_| Mutex::new(HashMap::new()))),
        }
    }

    fn shard_idx(ip: IpAddr) -> usize {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        ip.hash(&mut h);
        (h.finish() as usize) % RATE_LIMITER_SHARDS
    }

    fn check(&self, ip: IpAddr, max_per_minute: u32) -> bool {
        let idx = Self::shard_idx(ip);
        let now = Instant::now();
        let window = Duration::from_secs(60);
        let mut shard = self.shards[idx].lock().unwrap_or_else(|e| e.into_inner());
        let is_new = !shard.contains_key(&ip);
        if is_new && shard.len() >= MAX_IPS_PER_SHARD {
            // Shard full — try to evict one expired entry first.
            // If none are expired, pass request through untracked: a sustained
            // attack filling all shards still hits per-session caps.
            let expired = shard
                .iter()
                .find(|(_, q)| q.back().is_none_or(|&t| now.duration_since(t) > window))
                .map(|(k, _)| *k);
            match expired {
                Some(evict) => {
                    shard.remove(&evict);
                }
                None => return true,
            }
        }
        let queue = shard.entry(ip).or_default();
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
}

/// One analyzed turn in a session, recording the risk signals.
#[derive(Debug, Clone)]
struct SessionTurn {
    manipulation_risk: String,
}

/// Ring buffer of the most recent turns for one session.
#[derive(Debug)]
struct SessionHistory {
    turns: VecDeque<SessionTurn>,
    last_seen: Instant,
}

impl SessionHistory {
    fn new() -> Self {
        Self {
            turns: VecDeque::new(),
            last_seen: Instant::now(),
        }
    }

    fn push(&mut self, risk: &str) {
        let now = Instant::now();
        self.last_seen = now;
        if self.turns.len() >= SESSION_MAX_TURNS {
            self.turns.pop_front();
        }
        self.turns.push_back(SessionTurn {
            manipulation_risk: risk.to_string(),
        });
    }

    /// Returns true when the current session shows an upward escalation in
    /// manipulation_risk compared to the historical mean. Requires ≥3 turns.
    ///
    /// Algorithm: map risk to 0/1/2, compute mean of all-but-last turns.
    /// Escalation fires when the latest turn scores above the historical mean
    /// by more than 0.5 AND is not "low".
    fn is_escalating(&self) -> bool {
        if self.turns.len() < 3 {
            return false;
        }
        let scores: Vec<f64> = self
            .turns
            .iter()
            .map(|t| risk_score(&t.manipulation_risk))
            .collect();
        let n = scores.len();
        let historical_mean: f64 = scores[..n - 1].iter().sum::<f64>() / (n - 1) as f64;
        let current = scores[n - 1];
        current > (historical_mean + 0.5) && current >= 1.0
    }

    fn turn_count(&self) -> usize {
        self.turns.len()
    }

    /// Returns (trajectory, historical_mean) — the same values used by
    /// `is_escalating`, exposed so the caller can write a session log entry.
    fn risk_summary(&self) -> (Vec<String>, f64) {
        let trajectory: Vec<String> = self
            .turns
            .iter()
            .map(|t| t.manipulation_risk.clone())
            .collect();
        let n = trajectory.len();
        if n < 2 {
            return (trajectory, 0.0);
        }
        let scores: Vec<f64> = self
            .turns
            .iter()
            .map(|t| risk_score(&t.manipulation_risk))
            .collect();
        let historical_mean = scores[..n - 1].iter().sum::<f64>() / (n - 1) as f64;
        (trajectory, historical_mean)
    }
}

fn risk_score(risk: &str) -> f64 {
    match risk {
        "high" => 2.0,
        "medium" => 1.0,
        _ => 0.0,
    }
}

// ---------------------------------------------------------------------------
// Witness status cache — polled every 30 s by a background task
// ---------------------------------------------------------------------------

const WITNESS_ACTIVE: u8 = 0;
const WITNESS_INACTIVE: u8 = 1;
const WITNESS_UNCONFIGURED: u8 = 2;

fn witness_status_str(v: u8) -> &'static str {
    match v {
        WITNESS_ACTIVE => "active",
        WITNESS_INACTIVE => "inactive",
        _ => "not-configured",
    }
}

/// Spawn a background task that polls `witness status` once at startup and
/// every 30 seconds thereafter. The result is stored in `cache` (an AtomicU8)
/// so that the hot request path never blocks on a subprocess.
///
/// Only spawned when `audit_path` is Some — otherwise status is fixed to
/// WITNESS_UNCONFIGURED.
fn spawn_witness_poller(cache: Arc<std::sync::atomic::AtomicU8>) {
    tokio::spawn(async move {
        loop {
            let result = tokio::process::Command::new("witness")
                .arg("status")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await;
            let val = match result {
                Ok(s) if s.success() => WITNESS_ACTIVE,
                _ => WITNESS_INACTIVE,
            };
            cache.store(val, std::sync::atomic::Ordering::Relaxed);
            tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
        }
    });
}

// ---------------------------------------------------------------------------
// Metrics — lock-free counters, Prometheus text exposition
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct Metrics {
    pub requests_total: AtomicU64,
    pub requests_ok_total: AtomicU64,
    pub requests_error_total: AtomicU64,
    pub auth_failures_total: AtomicU64,
    pub rate_limit_total: AtomicU64,
    pub escalations_total: AtomicU64,
    /// Analyses that fired at least one consistency flag.
    pub flagged_total: AtomicU64,
    /// Analyses whose gate demanded stop_and_ask.
    pub stop_and_ask_total: AtomicU64,
    /// Sum of refinement iterations across analyses (avg = / requests_ok).
    pub refinement_iterations_total: AtomicU64,
    /// Formal-stage violations raised across analyses (phase F).
    pub formal_violations_total: AtomicU64,
    /// Advocate dissents that escalated the gate (phase E).
    pub advocate_dissent_total: AtomicU64,
}

impl Metrics {
    fn inc(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn render(&self, active_sessions: usize, uptime_secs: u64) -> String {
        let mut out = String::with_capacity(512);
        let pairs: &[(&str, &str, &str, u64)] = &[
            (
                "sbh_requests_total",
                "counter",
                "Total POST /v1/chat/completions requests",
                self.requests_total.load(Ordering::Relaxed),
            ),
            (
                "sbh_requests_ok_total",
                "counter",
                "Requests that returned 200 OK",
                self.requests_ok_total.load(Ordering::Relaxed),
            ),
            (
                "sbh_requests_error_total",
                "counter",
                "Requests that returned 4xx or 5xx",
                self.requests_error_total.load(Ordering::Relaxed),
            ),
            (
                "sbh_auth_failures_total",
                "counter",
                "Requests rejected for missing/invalid auth key",
                self.auth_failures_total.load(Ordering::Relaxed),
            ),
            (
                "sbh_rate_limit_total",
                "counter",
                "Requests rejected by per-IP rate limiter",
                self.rate_limit_total.load(Ordering::Relaxed),
            ),
            (
                "sbh_escalations_total",
                "counter",
                "Slow-boil session escalation events detected",
                self.escalations_total.load(Ordering::Relaxed),
            ),
            (
                "sbh_flagged_total",
                "counter",
                "Analyses that fired at least one consistency flag",
                self.flagged_total.load(Ordering::Relaxed),
            ),
            (
                "sbh_stop_and_ask_total",
                "counter",
                "Analyses whose gate demanded stop_and_ask",
                self.stop_and_ask_total.load(Ordering::Relaxed),
            ),
            (
                "sbh_refinement_iterations_total",
                "counter",
                "Sum of refinement iterations (avg = / requests_ok_total)",
                self.refinement_iterations_total.load(Ordering::Relaxed),
            ),
            (
                "sbh_formal_violations_total",
                "counter",
                "Formal-stage predicate violations raised (phase F)",
                self.formal_violations_total.load(Ordering::Relaxed),
            ),
            (
                "sbh_advocate_dissent_total",
                "counter",
                "Devil's-Advocate dissents that escalated the gate (phase E)",
                self.advocate_dissent_total.load(Ordering::Relaxed),
            ),
            (
                "sbh_active_sessions",
                "gauge",
                "Sessions currently held in memory",
                active_sessions as u64,
            ),
            (
                "sbh_uptime_seconds",
                "gauge",
                "Seconds since sbh serve started",
                uptime_secs,
            ),
        ];
        for (name, kind, help, value) in pairs {
            out.push_str(&format!("# HELP {name} {help}\n"));
            out.push_str(&format!("# TYPE {name} {kind}\n"));
            out.push_str(&format!("{name} {value}\n"));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Server state
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct ServeState {
    config: Arc<Config>,
    /// Per-IP sliding window — sharded to avoid global lock contention.
    rate_limiter: Arc<ShardedRateLimiter>,
    /// Per-session turn history for multi-turn escalation detection.
    sessions: Arc<Mutex<HashMap<String, SessionHistory>>>,
    /// Path to append-only session escalation log. Written on every escalation event.
    session_log_path: Option<String>,
    /// Prometheus-style counters, shared across handler clones.
    metrics: Arc<Metrics>,
    /// Timestamp of server start, used to compute uptime.
    start_time: Arc<Instant>,
    /// Cached witness status, refreshed every 30s by a background task.
    /// "active" | "inactive" | "not-configured"
    witness_status: Arc<std::sync::atomic::AtomicU8>,
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
    Metrics::inc(&state.metrics.requests_total);

    // --- serve-level auth (checked before anything else) ---
    if let Some(sk) = &config.serve_key {
        let provided = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim_start_matches("Bearer ").trim().to_string())
            .unwrap_or_default();
        if &provided != sk {
            Metrics::inc(&state.metrics.auth_failures_total);
            Metrics::inc(&state.metrics.requests_error_total);
            let body = ErrorBody {
                error: ErrorDetail {
                    message: "Unauthorized: invalid or missing SBH serve key.".into(),
                    kind: "authentication_error".into(),
                },
            };
            return (
                StatusCode::UNAUTHORIZED,
                HeaderMap::new(),
                Json(serde_json::to_value(body).unwrap_or_else(|_| serde_json::json!({"error":{"message":"serialization error","type":"internal_error"}}))),
            )
                .into_response();
        }
    }

    // --- per-IP rate limit ---
    let ip = remote_addr.ip();
    if !state.rate_limiter.check(ip, config.serve_rate_limit) {
        Metrics::inc(&state.metrics.rate_limit_total);
        Metrics::inc(&state.metrics.requests_error_total);
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
            Json(serde_json::to_value(body).unwrap_or_else(|_| serde_json::json!({"error":{"message":"serialization error","type":"internal_error"}}))),
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
            Json(serde_json::to_value(body).unwrap_or_else(|_| serde_json::json!({"error":{"message":"serialization error","type":"internal_error"}}))),
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
            Json(serde_json::to_value(body).unwrap_or_else(|_| serde_json::json!({"error":{"message":"serialization error","type":"internal_error"}}))),
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

    // --- session ID: validate client-supplied or mint a cryptographically random one ---
    let session_id = headers
        .get("x-sbh-session")
        .and_then(|v| v.to_str().ok())
        // Only accept IDs that are safe for HTTP headers and won't enable enumeration
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 64
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
        .map(|s| s.to_string())
        .unwrap_or_else(mint_session_id);

    // --- run the full harness pipeline ---
    let result = match analyze(user_input, &cfg).await {
        Ok(r) => r,
        Err(e) => {
            Metrics::inc(&state.metrics.requests_error_total);
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
                Json(serde_json::to_value(body).unwrap_or_else(|_| serde_json::json!({"error":{"message":"serialization error","type":"internal_error"}}))),
            )
                .into_response();
        }
    };

    // --- observability: detection-behavior counters (phase B) ---
    if !result.verification.consistency_flags.is_empty() {
        Metrics::inc(&state.metrics.flagged_total);
    }
    if result.verification.stop_and_ask {
        Metrics::inc(&state.metrics.stop_and_ask_total);
    }
    if let Some(ref rf) = result.refinement {
        state
            .metrics
            .refinement_iterations_total
            .fetch_add(rf.iterations.len() as u64, Ordering::Relaxed);
    }
    if let Some(ref f) = result.formal {
        state
            .metrics
            .formal_violations_total
            .fetch_add(f.violations.len() as u64, Ordering::Relaxed);
    }
    if let Some(ref a) = result.advocate {
        if a.dissented {
            Metrics::inc(&state.metrics.advocate_dissent_total);
        }
    }

    // --- session tracking: push turn, check for escalation, evict stale ---
    let (session_turn_count, session_escalating, session_log_info) = {
        let mut sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        // Lazy TTL: evict only the accessed session if it has expired.
        // Full map cleanup runs in the background sweeper — no O(N) walk per request.
        if let Some(h) = sessions.get(&session_id) {
            if now.duration_since(h.last_seen) >= SESSION_TTL {
                sessions.remove(&session_id);
            }
        }
        // Refuse new sessions beyond the cap to prevent memory DoS.
        let is_new = !sessions.contains_key(&session_id);
        if is_new && sessions.len() >= SESSION_MAX_COUNT {
            drop(sessions);
            Metrics::inc(&state.metrics.requests_error_total);
            let body = ErrorBody {
                error: ErrorDetail {
                    message: "session capacity reached — retry later".into(),
                    kind: "capacity_error".into(),
                },
            };
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                HeaderMap::new(),
                Json(serde_json::to_value(body).unwrap_or_else(|_| serde_json::json!({}))),
            )
                .into_response();
        }
        let hist = sessions
            .entry(session_id.clone())
            .or_insert_with(SessionHistory::new);
        hist.push(result.telemetry.intent_matrix.manipulation_risk.as_str());
        let escalating = hist.is_escalating();
        let summary = if escalating {
            Some(hist.risk_summary())
        } else {
            None
        };
        (hist.turn_count(), escalating, summary)
    };

    // --- write session log entry on escalation ---
    if session_escalating {
        Metrics::inc(&state.metrics.escalations_total);
        if let (Some(ref log_path), Some((trajectory, historical_mean))) =
            (&state.session_log_path, session_log_info)
        {
            let entry = session_log::SessionLogEntry::new(
                session_id.clone(),
                session_turn_count,
                trajectory,
                historical_mean,
                &ip,
                user_input,
            );
            if let Err(e) = session_log::append(log_path, &entry) {
                eprintln!("sbh serve: session log write error: {e}");
            }
        }
    }

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
    // Witness status is refreshed every 30s by a background task — zero blocking here.
    let witness_status = witness_status_str(
        state
            .witness_status
            .load(std::sync::atomic::Ordering::Relaxed),
    );
    if let Ok(val) = HeaderValue::from_str(witness_status) {
        resp_headers.insert("x-sbh-witness", val);
    }
    // Session headers
    if let Ok(val) = HeaderValue::from_str(&session_id) {
        resp_headers.insert("x-sbh-session", val);
    }
    if let Ok(val) = HeaderValue::from_str(&session_turn_count.to_string()) {
        resp_headers.insert("x-sbh-session-turns", val);
    }
    if session_escalating {
        resp_headers.insert(
            "x-sbh-session-alert",
            HeaderValue::from_static("escalation_detected"),
        );
    }

    Metrics::inc(&state.metrics.requests_ok_total);
    (
        StatusCode::OK,
        resp_headers,
        Json(serde_json::to_value(response_body).unwrap_or_else(|_| serde_json::json!({"error":{"message":"serialization error","type":"internal_error"}}))),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Metrics endpoint — Prometheus text exposition format
// ---------------------------------------------------------------------------

async fn metrics_handler(State(state): State<ServeState>, headers: HeaderMap) -> impl IntoResponse {
    // /metrics is protected by the same bearer key as the main endpoint.
    // Without this, an unauthenticated observer can read request rates,
    // escalation counts, and active session count.
    if let Some(sk) = &state.config.serve_key {
        let provided = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim_start_matches("Bearer ").trim().to_string())
            .unwrap_or_default();
        if &provided != sk {
            return (
                StatusCode::UNAUTHORIZED,
                [("content-type", "text/plain; charset=utf-8")],
                "Unauthorized".to_string(),
            );
        }
    }

    let active_sessions = state
        .sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .len();
    let uptime_secs = state.start_time.elapsed().as_secs();
    let body = state.metrics.render(active_sessions, uptime_secs);
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
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

pub async fn run_server(
    listen: &str,
    config: Config,
    tls_cert: Option<&str>,
    tls_key: Option<&str>,
) -> anyhow::Result<()> {
    let rate_limit = config.serve_rate_limit;
    let max_body = config.serve_max_body_bytes;
    let auth_enabled = config.serve_key.is_some();
    let session_log_path = config.session_log_path.clone();
    let context_path = config.context_path.clone();

    let witness_cache = Arc::new(std::sync::atomic::AtomicU8::new(WITNESS_UNCONFIGURED));
    if config.audit_path.is_some() {
        spawn_witness_poller(Arc::clone(&witness_cache));
    }

    let sessions: Arc<Mutex<HashMap<String, SessionHistory>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Background task: sweep expired sessions every SESSION_SWEEP_INTERVAL.
    // The hot path no longer calls retain() — this is the only full-map walk.
    {
        let sessions_sweep = Arc::clone(&sessions);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(SESSION_SWEEP_INTERVAL).await;
                let mut map = sessions_sweep.lock().unwrap_or_else(|e| e.into_inner());
                let now = Instant::now();
                map.retain(|_, h| now.duration_since(h.last_seen) < SESSION_TTL);
            }
        });
    }

    let state = ServeState {
        config: Arc::new(config),
        rate_limiter: Arc::new(ShardedRateLimiter::new()),
        sessions,
        session_log_path: session_log_path.clone(),
        metrics: Arc::new(Metrics::default()),
        start_time: Arc::new(Instant::now()),
        witness_status: witness_cache,
    };

    let app = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/health", get(health))
        .route("/metrics", get(metrics_handler))
        .layer(DefaultBodyLimit::max(max_body))
        .with_state(state);

    let print_banner = |scheme: &str, addr: SocketAddr| {
        eprintln!("sbh serve: listening on {scheme}://{addr}");
        eprintln!("  POST /v1/chat/completions  — OpenAI-compatible harness proxy");
        eprintln!("  GET  /health               — liveness check");
        eprintln!("  GET  /metrics              — Prometheus counters");
        eprintln!(
            "  auth: {}  rate: {}/min/IP  max-body: {} bytes",
            if auth_enabled { "enabled" } else { "disabled" },
            rate_limit,
            max_body,
        );
        match &session_log_path {
            Some(p) => eprintln!("  session log: {p}"),
            None => eprintln!("  session log: disabled (set SBH_SESSION_LOG or --session-log)"),
        };
        {
            use crate::rag::ContextCorpus;
            let embedded_count = ContextCorpus::embedded().len();
            match context_path.as_deref() {
                None => eprintln!("  context: {embedded_count} embedded docs (set SBH_CONTEXT_PATH to add operator docs)"),
                Some(p) => match ContextCorpus::load(p) {
                    Ok(extra) => eprintln!("  context: {} embedded + {} operator docs from {p}", embedded_count, extra.len()),
                    Err(e) => eprintln!("  context: {p} load error — {e}"),
                },
            }
        }
    };

    match (tls_cert, tls_key) {
        (Some(cert), Some(key)) => {
            use axum_server::tls_rustls::RustlsConfig;
            let tls_config = RustlsConfig::from_pem_file(cert, key)
                .await
                .with_context(|| format!("TLS: failed to load cert={cert} key={key}"))?;
            let addr: SocketAddr = listen
                .parse()
                .with_context(|| format!("invalid listen address: {listen}"))?;
            print_banner("https", addr);
            axum_server::bind_rustls(addr, tls_config)
                .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                .await?;
        }
        (Some(_), None) => anyhow::bail!("--tls-cert requires --tls-key"),
        (None, Some(_)) => anyhow::bail!("--tls-key requires --tls-cert"),
        (None, None) => {
            let listener = tokio::net::TcpListener::bind(listen).await?;
            let addr = listener.local_addr()?;
            print_banner("http", addr);
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await?;
        }
    }
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

/// Generate a cryptographically random session ID using OS entropy.
/// Falls back to monotonic counter + timestamp mix if /dev/urandom is unavailable.
fn mint_session_id() -> String {
    // Read 16 bytes from /dev/urandom — available on all Linux targets.
    let mut buf = [0u8; 16];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| {
            use std::io::Read;
            f.read_exact(&mut buf)
        })
        .is_ok();
    if ok {
        format!(
            "sbh-{:08x}{:08x}{:08x}{:08x}",
            u32::from_le_bytes(buf[0..4].try_into().unwrap()),
            u32::from_le_bytes(buf[4..8].try_into().unwrap()),
            u32::from_le_bytes(buf[8..12].try_into().unwrap()),
            u32::from_le_bytes(buf[12..16].try_into().unwrap()),
        )
    } else {
        format!("sbh-s-{}-{}", monotonic_id(), unix_now())
    }
}

/// Percent-encode a string for use in HTTP header values.
///
/// Encodes each UTF-8 byte that is not an unreserved ASCII character.
/// This is correct: we encode bytes, not Unicode codepoints, so multibyte
/// chars like `é` (UTF-8: 0xC3 0xA9) become `%C3%A9`, not `%E9`.
fn url_encode(s: &str) -> Result<String, ()> {
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        match byte {
            // Unreserved ASCII — pass through as-is
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b':'
            | b'/'
            | b','
            | b'['
            | b']'
            | b'{'
            | b'}' => out.push(*byte as char),
            // Everything else (including %, space, quotes, newlines, high bytes)
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- metrics ---

    #[test]
    fn metrics_render_contains_all_metric_names() {
        let m = Metrics::default();
        let out = m.render(0, 0);
        for name in &[
            "sbh_requests_total",
            "sbh_requests_ok_total",
            "sbh_requests_error_total",
            "sbh_auth_failures_total",
            "sbh_rate_limit_total",
            "sbh_escalations_total",
            "sbh_active_sessions",
            "sbh_uptime_seconds",
        ] {
            assert!(out.contains(name), "missing metric: {name}");
        }
    }

    #[test]
    fn metrics_render_prometheus_format() {
        let m = Metrics::default();
        let out = m.render(3, 42);
        assert!(out.contains("# HELP sbh_requests_total"));
        assert!(out.contains("# TYPE sbh_requests_total counter"));
        assert!(out.contains("sbh_requests_total 0\n"));
        assert!(out.contains("sbh_active_sessions 3\n"));
        assert!(out.contains("sbh_uptime_seconds 42\n"));
    }

    #[test]
    fn metrics_counters_increment_correctly() {
        let m = Metrics::default();
        Metrics::inc(&m.requests_total);
        Metrics::inc(&m.requests_total);
        Metrics::inc(&m.escalations_total);
        let out = m.render(0, 0);
        assert!(out.contains("sbh_requests_total 2\n"));
        assert!(out.contains("sbh_escalations_total 1\n"));
        assert!(out.contains("sbh_requests_ok_total 0\n"));
    }

    #[test]
    fn metrics_render_has_help_and_type_for_every_metric() {
        let m = Metrics::default();
        let out = m.render(0, 0);
        let help_count = out.lines().filter(|l| l.starts_with("# HELP")).count();
        let type_count = out.lines().filter(|l| l.starts_with("# TYPE")).count();
        assert_eq!(help_count, 13, "expected 13 # HELP lines");
        assert_eq!(type_count, 13, "expected 13 # TYPE lines");
    }

    // --- url_encode ---

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
    fn session_no_escalation_below_three_turns() {
        let mut h = SessionHistory::new();
        h.push("high");
        h.push("high");
        assert!(!h.is_escalating(), "need ≥3 turns before firing");
    }

    #[test]
    fn session_escalation_detected_on_slow_boil() {
        let mut h = SessionHistory::new();
        h.push("low");
        h.push("low");
        h.push("high");
        assert!(h.is_escalating(), "low→low→high is slow-boil escalation");
    }

    #[test]
    fn session_no_escalation_when_already_high() {
        let mut h = SessionHistory::new();
        h.push("high");
        h.push("high");
        h.push("high");
        // All turns already high — no upward delta
        assert!(!h.is_escalating());
    }

    #[test]
    fn session_no_escalation_medium_to_medium() {
        let mut h = SessionHistory::new();
        h.push("low");
        h.push("medium");
        h.push("medium");
        // medium is 1.0; historical mean 0.5 → delta 0.5, but not > 0.5
        assert!(!h.is_escalating());
    }

    #[test]
    fn session_escalation_low_to_high_five_turns() {
        let mut h = SessionHistory::new();
        for _ in 0..4 {
            h.push("low");
        }
        h.push("high");
        assert!(h.is_escalating());
    }

    #[test]
    fn session_ring_caps_at_max_turns() {
        let mut h = SessionHistory::new();
        for _ in 0..SESSION_MAX_TURNS + 5 {
            h.push("low");
        }
        assert_eq!(h.turn_count(), SESSION_MAX_TURNS);
    }

    #[test]
    fn risk_score_mapping() {
        assert_eq!(risk_score("low"), 0.0);
        assert_eq!(risk_score("medium"), 1.0);
        assert_eq!(risk_score("high"), 2.0);
        assert_eq!(risk_score("unknown"), 0.0);
    }

    #[test]
    fn rate_limit_allows_up_to_max() {
        let limiter = ShardedRateLimiter::new();
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        for _ in 0..5 {
            assert!(limiter.check(ip, 5));
        }
        assert!(!limiter.check(ip, 5));
    }

    #[test]
    fn rate_limit_different_ips_are_independent() {
        let limiter = ShardedRateLimiter::new();
        let ip1: IpAddr = "10.0.0.1".parse().unwrap();
        let ip2: IpAddr = "10.0.0.2".parse().unwrap();
        for _ in 0..3 {
            assert!(limiter.check(ip1, 3));
        }
        assert!(!limiter.check(ip1, 3));
        assert!(limiter.check(ip2, 3));
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
                disagreement: Default::default(),
                stop_and_ask: false,
                fired_checks: vec![],
            },
            trace: vec![],
            capability_request: None,
            obfuscation: None,
            refinement: None,
            tool_risk: None,
            formal: None,
            advocate: None,
        };
        let s = summarize_result(&result);
        assert!(s.contains("neutral"));
        assert!(s.contains("low"));
        assert!(s.contains("passed"));
    }
}
