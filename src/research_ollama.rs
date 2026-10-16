//! Research-only localhost Ollama transport; not the production/benchmark engine.
pub mod diagnostic;
use crate::{
    audit::iso_now,
    backends::InferenceEngine,
    dream::{ContextId, TestOutcome},
    dream_engine::{
        AnswerScorer, EngineSession, ExecutionFailure, FreshContextProvider, InferenceSettings,
        ProviderProfile,
    },
};
use async_trait::async_trait;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

const RESPONSE_LIMIT: usize = 65_536;
const REGISTRY_LIMIT: usize = 224;

#[derive(Debug, Clone, Serialize)]
pub struct CallTelemetry {
    pub context_id: ContextId,
    pub seed: u64,
    pub request_sent: bool,
    pub temperature_millidegrees: Option<i64>,
    pub done_reason: Option<String>,
    pub eval_count: Option<u64>,
    pub prompt_eval_count: Option<u64>,
    pub total_duration_ns: Option<u64>,
    pub completed: bool,
}
#[derive(Clone, Default)]
pub struct TelemetryStore(Arc<Mutex<Vec<CallTelemetry>>>);
impl TelemetryStore {
    pub fn snapshot(&self) -> Result<Vec<CallTelemetry>, String> {
        self.0
            .lock()
            .map(|v| v.clone())
            .map_err(|_| "telemetry unavailable".into())
    }
    fn add(&self, record: CallTelemetry) -> Result<usize, String> {
        let mut records = self.0.lock().map_err(|_| "telemetry unavailable")?;
        if records.len() >= REGISTRY_LIMIT
            || records.iter().any(|r| r.context_id == record.context_id)
        {
            return Err("research call registry limit or reused context".into());
        }
        let index = records.len();
        records.push(record);
        Ok(index)
    }
    fn update(&self, index: usize, f: impl FnOnce(&mut CallTelemetry)) -> Result<(), String> {
        let mut records = self.0.lock().map_err(|_| "telemetry unavailable")?;
        let record = records.get_mut(index).ok_or("telemetry missing")?;
        f(record);
        Ok(())
    }
}

pub struct FinalIntegerScorer {
    telemetry: TelemetryStore,
}
impl FinalIntegerScorer {
    pub fn new(telemetry: TelemetryStore) -> Self {
        Self { telemetry }
    }
}
/// Strict preregistered last-line parser, no number extraction elsewhere.
pub fn parse_final_integer(answer: &str) -> Option<i64> {
    let line = answer.lines().rev().find(|line| !line.is_empty())?;
    let value = line.strip_prefix("FINAL: ")?;
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}
impl AnswerScorer for FinalIntegerScorer {
    fn id(&self) -> &'static str {
        "final_integer_v1"
    }
    fn output_contract(&self) -> &'static str {
        "You may show working. The LAST line must be exactly: FINAL: <integer>."
    }
    fn score(
        &self,
        answer: &str,
        oracle: Option<&str>,
        context: &ContextId,
    ) -> Result<TestOutcome, ExecutionFailure> {
        let records = self
            .telemetry
            .0
            .lock()
            .map_err(|_| ExecutionFailure::RejectedOutput)?;
        let entry = records
            .iter()
            .find(|r| &r.context_id == context && r.completed)
            .ok_or(ExecutionFailure::RejectedOutput)?;
        if entry.done_reason.as_deref() == Some("length") {
            return Ok(TestOutcome::Failed);
        }
        let Some(expected) = oracle else {
            return Ok(TestOutcome::Inconclusive);
        };
        let expected: i64 = expected
            .parse()
            .map_err(|_| ExecutionFailure::RejectedOutput)?;
        Ok(if parse_final_integer(answer) == Some(expected) {
            TestOutcome::Passed
        } else {
            TestOutcome::Failed
        })
    }
}

pub trait ResearchCallGuard: Send + Sync {
    fn before_call<'a>(
        &'a self,
        context: &'a ContextId,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<i64, String>> + Send + 'a>>;
}
#[derive(Debug, Clone, Serialize)]
pub struct ThermalReading {
    pub time: String,
    pub context_id: Option<ContextId>,
    pub millidegrees: i64,
}
pub struct CoreTempGuard {
    sensors: Vec<PathBuf>,
    readings: Mutex<Vec<ThermalReading>>,
}
#[derive(Debug, PartialEq, Eq)]
enum ThermalAction {
    Proceed,
    Pause,
    Stop,
}
fn thermal_action(temp: i64) -> ThermalAction {
    if temp >= 95_000 {
        ThermalAction::Stop
    } else if temp >= 90_000 {
        ThermalAction::Pause
    } else {
        ThermalAction::Proceed
    }
}
impl CoreTempGuard {
    pub fn discover(root: &Path) -> Result<Self, String> {
        let mut sensors = vec![];
        for entry in std::fs::read_dir(root).map_err(|_| "coretemp discovery failed")? {
            let path = entry.map_err(|_| "coretemp discovery failed")?.path();
            if std::fs::read_to_string(path.join("name"))
                .is_ok_and(|name| name.trim() == "coretemp")
            {
                sensors.push(path.join("temp1_input"));
            }
        }
        sensors.sort();
        if sensors.is_empty() {
            return Err("coretemp sensor required".into());
        }
        let guard = Self {
            sensors,
            readings: Mutex::new(vec![]),
        };
        guard.read(None)?;
        Ok(guard)
    }
    fn read(&self, context: Option<&ContextId>) -> Result<i64, String> {
        let mut max = None;
        for path in &self.sensors {
            let value = std::fs::read_to_string(path)
                .map_err(|_| "coretemp read failed")?
                .trim()
                .parse::<i64>()
                .map_err(|_| "invalid coretemp value")?;
            if !(0..=150_000).contains(&value) {
                return Err("invalid coretemp range".into());
            }
            max = Some(max.map_or(value, |old: i64| old.max(value)));
        }
        let value = max.ok_or("coretemp sensor required")?;
        let mut log = self
            .readings
            .lock()
            .map_err(|_| "temperature log unavailable")?;
        if log.len() >= 2048 {
            return Err("temperature log limit".into());
        }
        log.push(ThermalReading {
            time: iso_now(),
            context_id: context.cloned(),
            millidegrees: value,
        });
        Ok(value)
    }
    async fn wait_safe(&self, context: Option<&ContextId>) -> Result<i64, String> {
        loop {
            let value = self.read(context)?;
            match thermal_action(value) {
                ThermalAction::Proceed => return Ok(value),
                ThermalAction::Stop => return Err("temperature stop threshold reached".into()),
                ThermalAction::Pause => tokio::time::sleep(Duration::from_secs(15)).await,
            }
        }
    }
    pub async fn cooldown(&self) -> Result<(), String> {
        for _ in 0..3 {
            self.wait_safe(None).await?;
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
        self.wait_safe(None).await?;
        Ok(())
    }
    pub fn readings(&self) -> Result<Vec<ThermalReading>, String> {
        self.readings
            .lock()
            .map(|r| r.clone())
            .map_err(|_| "temperature log unavailable".into())
    }
}
impl ResearchCallGuard for CoreTempGuard {
    fn before_call<'a>(
        &'a self,
        context: &'a ContextId,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<i64, String>> + Send + 'a>> {
        Box::pin(self.wait_safe(Some(context)))
    }
}

fn endpoint(value: &str) -> Result<Url, String> {
    // Reject URL-normalized host aliases (127.1, numeric IPs, percent encoding).
    let authority = value
        .strip_prefix("http://")
        .ok_or("research endpoint must be local HTTP")?
        .split('/')
        .next()
        .ok_or("invalid endpoint")?;
    let raw_host = authority.split(':').next().ok_or("invalid endpoint")?;
    if !matches!(raw_host, "127.0.0.1" | "localhost") {
        return Err("research endpoint must be 127.0.0.1 or localhost".into());
    }
    let mut url = Url::parse(value).map_err(|_| "invalid endpoint")?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || url.port() == Some(0)
        || !matches!(url.host_str(), Some("127.0.0.1" | "localhost"))
    {
        return Err("invalid local endpoint".into());
    }
    // A literal address avoids DNS entirely, including localhost resolution.
    url.set_host(Some("127.0.0.1"))
        .map_err(|_| "invalid endpoint")?;
    Ok(url)
}
fn client() -> Result<Client, String> {
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|_| "research HTTP client unavailable".into())
}
async fn bounded_json(mut response: reqwest::Response) -> Result<serde_json::Value, String> {
    if !response.status().is_success() {
        return Err("research HTTP response rejected".into());
    }
    let mut bytes = vec![];
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "research response read failed")?
    {
        if bytes
            .len()
            .checked_add(chunk.len())
            .is_none_or(|n| n > RESPONSE_LIMIT)
        {
            return Err("research response too large".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalid research response JSON".into())
}
fn metadata(
    version: &serde_json::Value,
    tags: &serde_json::Value,
    model: &str,
) -> Result<ProviderProfile, String> {
    let version = version
        .get("version")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or("provider version missing")?;
    let matches: Vec<_> = tags
        .get("models")
        .and_then(|v| v.as_array())
        .ok_or("model metadata missing")?
        .iter()
        .filter(|v| v.get("name").and_then(|v| v.as_str()) == Some(model))
        .collect();
    if matches.len() != 1 {
        return Err("exact model metadata missing or ambiguous".into());
    }
    let digest = matches[0]
        .get("digest")
        .and_then(|v| v.as_str())
        .filter(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
        .ok_or("model digest missing")?;
    Ok(ProviderProfile {
        provider: "ollama-research-local".into(),
        provider_version: Some(version.into()),
        model: model.into(),
        model_version: Some(digest.into()),
    })
}
fn body(
    model: &str,
    settings: &InferenceSettings,
    system: &str,
    payload: &str,
) -> Result<serde_json::Value, String> {
    let seed = settings
        .seed
        .filter(|s| *s <= i64::MAX as u64)
        .ok_or("signed per-call seed required")?;
    if !settings.temperature.is_finite()
        || !(0.0..=2.0).contains(&settings.temperature)
        || !(1..=8192).contains(&settings.max_output_tokens)
    {
        return Err("invalid research settings".into());
    }
    Ok(
        serde_json::json!({"model":model,"messages":[{"role":"system","content":system},{"role":"user","content":payload}],
        "think":false,"stream":false,"options":{"temperature":settings.temperature,"seed":seed,"num_predict":settings.max_output_tokens}}),
    )
}

pub struct ResearchOllamaProvider {
    endpoint: Url,
    client: Client,
    profile: ProviderProfile,
    guard: Arc<dyn ResearchCallGuard>,
    telemetry: TelemetryStore,
    checks: Mutex<Vec<serde_json::Value>>,
}
impl ResearchOllamaProvider {
    /// Explicit network admission only. Never called by default build/tests.
    pub async fn connect(
        url: &str,
        model: &str,
        guard: Arc<dyn ResearchCallGuard>,
    ) -> Result<Self, String> {
        let endpoint = endpoint(url)?;
        let client = client()?;
        let profile = Self::fetch_profile(&endpoint, &client, model).await?;
        Ok(Self {
            endpoint,
            client,
            profile,
            guard,
            telemetry: TelemetryStore::default(),
            checks: Mutex::new(vec![]),
        })
    }
    async fn fetch_profile(
        url: &Url,
        client: &Client,
        model: &str,
    ) -> Result<ProviderProfile, String> {
        let version = bounded_json(
            client
                .get(url.join("api/version").map_err(|_| "invalid API path")?)
                .send()
                .await
                .map_err(|_| "metadata request failed")?,
        )
        .await?;
        let tags = bounded_json(
            client
                .get(url.join("api/tags").map_err(|_| "invalid API path")?)
                .send()
                .await
                .map_err(|_| "metadata request failed")?,
        )
        .await?;
        metadata(&version, &tags, model)
    }
    pub async fn recheck_metadata(&self) -> Result<(), String> {
        let observed =
            Self::fetch_profile(&self.endpoint, &self.client, &self.profile.model).await?;
        self.checks
            .lock()
            .map_err(|_| "metadata log unavailable")?
            .push(serde_json::json!({"time":iso_now(),"profile":observed}));
        if observed != self.profile {
            return Err("research model or provider version changed".into());
        }
        Ok(())
    }
    pub fn metadata_checks(&self) -> Result<Vec<serde_json::Value>, String> {
        self.checks
            .lock()
            .map(|v| v.clone())
            .map_err(|_| "metadata log unavailable".into())
    }
    pub fn telemetry(&self) -> TelemetryStore {
        self.telemetry.clone()
    }
}
impl FreshContextProvider for ResearchOllamaProvider {
    fn profile(&self) -> ProviderProfile {
        self.profile.clone()
    }
    fn open(
        &self,
        context: &ContextId,
        settings: &InferenceSettings,
    ) -> Result<EngineSession, String> {
        body(&self.profile.model, settings, "", "")?;
        Ok(EngineSession {
            profile: self.profile.clone(),
            settings: settings.clone(),
            engine: Box::new(ResearchOllamaEngine {
                endpoint: self.endpoint.clone(),
                client: self.client.clone(),
                model: self.profile.model.clone(),
                context: context.clone(),
                settings: settings.clone(),
                guard: self.guard.clone(),
                telemetry: self.telemetry.clone(),
                used: AtomicBool::new(false),
            }),
        })
    }
}
struct ResearchOllamaEngine {
    endpoint: Url,
    client: Client,
    model: String,
    context: ContextId,
    settings: InferenceSettings,
    guard: Arc<dyn ResearchCallGuard>,
    telemetry: TelemetryStore,
    used: AtomicBool,
}
#[derive(Deserialize)]
struct ChatReply {
    model: String,
    done: bool,
    done_reason: String,
    message: ChatMessage,
    eval_count: Option<u64>,
    prompt_eval_count: Option<u64>,
    total_duration: Option<u64>,
}
#[derive(Deserialize)]
struct ChatMessage {
    content: String,
}

#[async_trait]
impl InferenceEngine for ResearchOllamaEngine {
    async fn generate(&self, system: &str, payload: &str) -> Result<String, String> {
        if self.used.swap(true, Ordering::SeqCst) {
            return Err("research engine context already used".into());
        }
        let request = body(&self.model, &self.settings, system, payload)?;
        let index = self.telemetry.add(CallTelemetry {
            context_id: self.context.clone(),
            seed: self.settings.seed.ok_or("seed missing")?,
            request_sent: false,
            temperature_millidegrees: None,
            done_reason: None,
            eval_count: None,
            prompt_eval_count: None,
            total_duration_ns: None,
            completed: false,
        })?;
        let temperature = self.guard.before_call(&self.context).await?;
        self.telemetry.update(index, |r| {
            r.temperature_millidegrees = Some(temperature);
            r.request_sent = true;
        })?;
        let json = bounded_json(
            self.client
                .post(
                    self.endpoint
                        .join("api/chat")
                        .map_err(|_| "invalid API path")?,
                )
                .json(&request)
                .send()
                .await
                .map_err(|_| "research request failed")?,
        )
        .await?;
        let reply: ChatReply =
            serde_json::from_value(json).map_err(|_| "invalid research chat response")?;
        if !reply.done
            || reply.model != self.model
            || !matches!(reply.done_reason.as_str(), "stop" | "length")
        {
            return Err("incomplete or mismatched research response".into());
        }
        self.telemetry.update(index, |r| {
            r.done_reason = Some(reply.done_reason);
            r.eval_count = reply.eval_count;
            r.prompt_eval_count = reply.prompt_eval_count;
            r.total_duration_ns = reply.total_duration;
            r.completed = true;
        })?;
        Ok(reply.message.content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    struct Guard(bool);
    impl ResearchCallGuard for Guard {
        fn before_call<'a>(
            &'a self,
            _: &'a ContextId,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<i64, String>> + Send + 'a>>
        {
            Box::pin(async move {
                if self.0 {
                    Ok(38_000)
                } else {
                    Err("test thermal stop".into())
                }
            })
        }
    }
    fn settings() -> InferenceSettings {
        InferenceSettings {
            temperature: 0.7,
            seed: Some(123),
            max_output_tokens: 256,
        }
    }
    fn profile() -> ProviderProfile {
        ProviderProfile {
            provider: "ollama-research-local".into(),
            provider_version: Some("test-version".into()),
            model: "llama3.2:3b".into(),
            model_version: Some("a".repeat(64)),
        }
    }
    fn record(context: &str, truncated: bool) -> CallTelemetry {
        CallTelemetry {
            context_id: context.to_string().try_into().unwrap(),
            seed: 123,
            request_sent: true,
            temperature_millidegrees: Some(38_000),
            done_reason: Some(if truncated { "length" } else { "stop" }.into()),
            eval_count: Some(5),
            prompt_eval_count: Some(10),
            total_duration_ns: Some(99),
            completed: true,
        }
    }
    async fn mock(responses: Vec<(u16, String)>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let mut requests = vec![];
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = vec![];
                loop {
                    let mut chunk = [0; 2048];
                    let n = stream.read(&mut chunk).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.to_lowercase()
                                    .strip_prefix("content-length: ")
                                    .and_then(|s| s.parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                requests.push(String::from_utf8(bytes).unwrap());
                let response=format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nLocation: http://example.com/\r\n\r\n{body}",body.len());
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        (format!("http://{address}"), task)
    }
    fn admission() -> Vec<(u16, String)> {
        vec![
            (
                200,
                serde_json::json!({"version":"test-version"}).to_string(),
            ),
            (
                200,
                serde_json::json!({"models":[{"name":"llama3.2:3b","digest":"a".repeat(64)}]})
                    .to_string(),
            ),
        ]
    }
    fn reply(reason: &str) -> String {
        serde_json::json!({"model":"llama3.2:3b","done":true,"done_reason":reason,"message":{"content":"work\nFINAL: 323"},"eval_count":5,"prompt_eval_count":10,"total_duration":99}).to_string()
    }

    #[test]
    fn endpoint_rejects_remote_aliases_credentials_and_paths() {
        for value in [
            "https://localhost:11434",
            "http://example.com",
            "http://127.1",
            "http://2130706433",
            "http://[::1]",
            "http://user@localhost",
            "http://localhost/api",
            "http://localhost/?x=1",
            "http://localhost/#x",
            "http://localhost.evil",
            "http://127.0.0.1.evil",
        ] {
            assert!(endpoint(value).is_err(), "{value}");
        }
        assert_eq!(
            endpoint("http://localhost:11434").unwrap().host_str(),
            Some("127.0.0.1")
        );
    }
    #[test]
    fn request_uses_exact_settings_and_no_history() {
        let body = body("llama3.2:3b", &settings(), "approved system", "payload").unwrap();
        assert_eq!(body["options"]["seed"], 123);
        assert_eq!(body["options"]["num_predict"], 256);
        assert!((body["options"]["temperature"].as_f64().unwrap() - 0.7).abs() < 0.000001);
        assert_eq!(body["think"], false);
        assert_eq!(body["stream"], false);
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
        assert!(body.get("tools").is_none());
        let mut missing = settings();
        missing.seed = None;
        assert!(super::body("m", &missing, "", "").is_err());
        missing.seed = Some(u64::MAX);
        assert!(super::body("m", &missing, "", "").is_err());
    }
    #[test]
    fn final_parser_is_strict_and_supports_working() {
        for (raw, expected) in [
            ("work\nFINAL: 323\n\n", Some(323)),
            ("work\r\nFINAL: -03\r\n", Some(-3)),
            ("FINAL: -0", Some(0)),
            ("323", None),
            (" FINAL: 323", None),
            ("FINAL: 323 ", None),
            ("FINAL: +323", None),
            ("FINAL: ３", None),
            ("FINAL: 3\nextra", None),
            ("FINAL: 9223372036854775808", None),
            ("FINAL: 3\n ", None),
        ] {
            assert_eq!(parse_final_integer(raw), expected, "{raw}");
        }
    }
    #[test]
    fn truncation_cannot_pass_even_with_valid_final_line_and_missing_telemetry_fails() {
        let registry = TelemetryStore::default();
        registry.add(record("ok", false)).unwrap();
        registry.add(record("truncated", true)).unwrap();
        let scorer = FinalIntegerScorer::new(registry);
        assert_eq!(
            scorer
                .score(
                    "work\nFINAL: 003",
                    Some("3"),
                    &"ok".to_string().try_into().unwrap()
                )
                .unwrap(),
            TestOutcome::Passed
        );
        assert_eq!(
            scorer
                .score(
                    "FINAL: 3",
                    Some("3"),
                    &"truncated".to_string().try_into().unwrap()
                )
                .unwrap(),
            TestOutcome::Failed
        );
        assert_eq!(
            scorer
                .score("FINAL: 3", None, &"ok".to_string().try_into().unwrap())
                .unwrap(),
            TestOutcome::Inconclusive
        );
        assert!(scorer
            .score(
                "FINAL: 3",
                Some("3"),
                &"missing".to_string().try_into().unwrap()
            )
            .is_err());
    }
    #[tokio::test]
    async fn mock_transport_records_settings_truncation_and_single_use() {
        let mut responses = admission();
        responses.push((200, reply("length")));
        let (url, server) = mock(responses).await;
        let provider = ResearchOllamaProvider::connect(&url, "llama3.2:3b", Arc::new(Guard(true)))
            .await
            .unwrap();
        assert_eq!(provider.profile(), profile());
        let context = "mock-call".to_string().try_into().unwrap();
        let session = provider.open(&context, &settings()).unwrap();
        assert_eq!(
            session.engine.generate("system", "payload").await.unwrap(),
            "work\nFINAL: 323"
        );
        assert!(session.engine.generate("s", "p").await.is_err());
        let records = provider.telemetry().snapshot().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].temperature_millidegrees, Some(38_000));
        let scorer = FinalIntegerScorer::new(provider.telemetry());
        assert_eq!(
            scorer.score("FINAL: 323", Some("323"), &context).unwrap(),
            TestOutcome::Failed
        );
        let requests = server.await.unwrap();
        assert!(requests[0].starts_with("GET /api/version"));
        assert!(requests[2].starts_with("POST /api/chat"));
        let sent: serde_json::Value =
            serde_json::from_str(requests[2].split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(sent["options"]["num_predict"], 256);
        assert_eq!(sent["options"]["seed"], 123);
    }
    #[tokio::test]
    async fn mock_redirect_bad_json_and_oversized_metadata_fail_closed() {
        for (status, body) in [
            (302, "{}".into()),
            (500, "private server diagnostic".into()),
            (200, "not JSON".into()),
            (200, "x".repeat(RESPONSE_LIMIT + 1)),
        ] {
            let (url, server) = mock(vec![(status, body)]).await;
            let error = ResearchOllamaProvider::connect(&url, "llama3.2:3b", Arc::new(Guard(true)))
                .await
                .err()
                .unwrap();
            assert!(!error.contains("private"));
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn registered_experiment_uses_working_contract_scorer_and_unique_actual_seeds() {
        use crate::{
            dream_engine::{Cancellation, ControllerConfig, DreamController, SeedPolicy},
            lucid_pilot::{experiment, parse_registered},
        };
        let tasks = parse_registered(include_bytes!("../fixtures/dream_pilot_tasks.json")).unwrap();
        let mut responses = admission();
        responses.extend((0..15).map(|_| (200, reply("stop"))));
        let (url, server) = mock(responses).await;
        let provider = ResearchOllamaProvider::connect(&url, &tasks.model, Arc::new(Guard(true)))
            .await
            .unwrap();
        let scorer = FinalIntegerScorer::new(provider.telemetry());
        let controller = DreamController::new(
            &provider,
            ControllerConfig {
                base_system_prompt: tasks.base_system_prompt.clone(),
                settings: tasks.settings.clone(),
                call_timeout: Duration::from_secs(2),
                run_timeout: Duration::from_secs(10),
                max_input_bytes: 32_768,
                max_output_bytes: 16_384,
            },
        )
        .unwrap()
        .with_seed_policy(SeedPolicy::DistinctPerCall)
        .unwrap()
        .with_scorer(&scorer);
        let report = controller
            .lucid(
                &experiment(&tasks, &tasks.targets[0]).unwrap(),
                &Cancellation::default(),
            )
            .await
            .unwrap();
        assert_eq!(report.run.status, crate::dream::RunStatus::Completed);
        assert_eq!(report.run.inference_calls, 15);
        assert!(report
            .run
            .arms
            .iter()
            .flat_map(|a| &a.results)
            .all(|r| r.outcome == TestOutcome::Passed));
        let requests = server.await.unwrap();
        let telemetry = provider.telemetry().snapshot().unwrap();
        let seeds: std::collections::HashSet<_> = telemetry.iter().map(|t| t.seed).collect();
        assert_eq!(seeds.len(), 15);
        for (index, request) in requests[2..].iter().enumerate() {
            let body: serde_json::Value =
                serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
            let system = body["messages"][0]["content"].as_str().unwrap();
            assert!(system.contains("You may show working"));
            assert!(!system.contains("Return your final answer only."));
            assert_eq!(
                body["options"]["seed"].as_u64(),
                report.calls[index].provenance.seed
            );
            assert_eq!(body["options"]["num_predict"], 256);
            assert_eq!(
                telemetry[index].seed,
                report.calls[index].provenance.seed.unwrap()
            );
        }
    }
    #[tokio::test]
    async fn metadata_drift_stops_recheck() {
        let mut responses = admission();
        responses.extend([
            (200, "{\"version\":\"changed\"}".into()),
            admission()[1].clone(),
        ]);
        let (url, server) = mock(responses).await;
        let provider = ResearchOllamaProvider::connect(&url, "llama3.2:3b", Arc::new(Guard(true)))
            .await
            .unwrap();
        assert!(provider.recheck_metadata().await.is_err());
        assert_eq!(provider.metadata_checks().unwrap().len(), 1);
        server.await.unwrap();
    }
    #[tokio::test]
    async fn guard_stop_never_sends_a_generation_request() {
        let (url, server) = mock(admission()).await;
        let provider = ResearchOllamaProvider::connect(&url, "llama3.2:3b", Arc::new(Guard(false)))
            .await
            .unwrap();
        let session = provider
            .open(&"stopped".to_string().try_into().unwrap(), &settings())
            .unwrap();
        assert!(session.engine.generate("system", "payload").await.is_err());
        assert!(!provider.telemetry().snapshot().unwrap()[0].request_sent);
        assert_eq!(server.await.unwrap().len(), 2);
    }
    #[test]
    fn metadata_requires_exact_model_nonblank_version_and_digest() {
        assert!(metadata(
            &serde_json::json!({}),
            &serde_json::json!({"models":[]}),
            "m"
        )
        .is_err());
        assert!(metadata(
            &serde_json::json!({"version":"v"}),
            &serde_json::json!({"models":[{"name":"m","digest":"not-a-digest"}]}),
            "m"
        )
        .is_err());
        assert!(metadata(
            &serde_json::json!({"version":"v"}),
            &serde_json::json!({"models":[{"name":"different","digest":"a".repeat(64)}]}),
            "m"
        )
        .is_err());
    }
    #[tokio::test]
    async fn coretemp_discovery_thresholds_and_read_failures() {
        assert_eq!(thermal_action(89_999), ThermalAction::Proceed);
        assert_eq!(thermal_action(90_000), ThermalAction::Pause);
        assert_eq!(thermal_action(95_000), ThermalAction::Stop);
        let root = tempfile::tempdir().unwrap();
        assert!(CoreTempGuard::discover(root.path()).is_err());
        let path = root.path().join("hwmon-test");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("name"), "coretemp\n").unwrap();
        std::fs::write(path.join("temp1_input"), "38000\n").unwrap();
        let guard = CoreTempGuard::discover(root.path()).unwrap();
        let context = "thermal".to_string().try_into().unwrap();
        assert_eq!(guard.before_call(&context).await.unwrap(), 38_000);
        std::fs::write(path.join("temp1_input"), "95000").unwrap();
        assert!(guard.before_call(&context).await.is_err());
        assert_eq!(
            guard.readings().unwrap().last().unwrap().millidegrees,
            95_000
        );
        std::fs::write(path.join("temp1_input"), "invalid").unwrap();
        assert!(guard.before_call(&context).await.is_err());
    }
}
