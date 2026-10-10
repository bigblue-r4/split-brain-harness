//! Offline research-controller tests; no live model or network adapter.
use async_trait::async_trait;
use split_brain_harness::{
    backends::InferenceEngine, dream::*, dream_engine::*, types::TraceEntry,
};
use std::{
    collections::{HashSet, VecDeque},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

#[derive(Clone)]
enum Step {
    Answer(String),
    Error,
    Pending,
    CancelPending(Cancellation),
    CancelAnswer(Cancellation, String),
}
impl From<&str> for Step {
    fn from(value: &str) -> Self {
        Self::Answer(value.into())
    }
}
#[derive(Clone, Debug)]
struct Invocation {
    context: ContextId,
    system: String,
    payload: String,
    settings: InferenceSettings,
}
struct Scripted {
    script: Mutex<VecDeque<Step>>,
    calls: Arc<Mutex<Vec<Invocation>>>,
    opened: Mutex<Vec<ContextId>>,
    dropped: Arc<AtomicUsize>,
    mismatched_settings: bool,
}
impl Scripted {
    fn new(script: Vec<Step>) -> Self {
        Self {
            script: Mutex::new(script.into()),
            calls: Arc::default(),
            opened: Mutex::default(),
            dropped: Arc::default(),
            mismatched_settings: false,
        }
    }
    fn calls(&self) -> Vec<Invocation> {
        self.calls.lock().unwrap().clone()
    }
}
struct ScriptEngine {
    step: Step,
    calls: Arc<Mutex<Vec<Invocation>>>,
    context: ContextId,
    settings: InferenceSettings,
    used: AtomicUsize,
    dropped: Arc<AtomicUsize>,
}
impl Drop for ScriptEngine {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }
}
#[allow(clippy::double_must_use)]
#[async_trait]
impl InferenceEngine for ScriptEngine {
    async fn generate(&self, system: &str, payload: &str) -> Result<String, String> {
        assert_eq!(
            self.used.fetch_add(1, Ordering::Relaxed),
            0,
            "each context may be used once"
        );
        self.calls.lock().unwrap().push(Invocation {
            context: self.context.clone(),
            system: system.into(),
            payload: payload.into(),
            settings: self.settings.clone(),
        });
        match &self.step {
            Step::Answer(value) => Ok(value.clone()),
            Step::Error => Err("provider private diagnostic sk-abcdefghijk-secret".into()),
            Step::Pending => std::future::pending().await,
            Step::CancelPending(cancel) => {
                cancel.cancel();
                std::future::pending().await
            }
            Step::CancelAnswer(cancel, value) => {
                cancel.cancel();
                Ok(value.clone())
            }
        }
    }
}
impl FreshContextProvider for Scripted {
    fn profile(&self) -> ProviderProfile {
        ProviderProfile {
            provider: "scripted".into(),
            provider_version: Some("fixture-v1".into()),
            model: "offline-model".into(),
            model_version: Some("model-v1".into()),
        }
    }
    fn open(
        &self,
        context_id: &ContextId,
        settings: &InferenceSettings,
    ) -> Result<EngineSession, String> {
        self.opened.lock().unwrap().push(context_id.clone());
        let step = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected call beyond script/budget");
        let mut actual = settings.clone();
        if self.mismatched_settings {
            actual.temperature = 1.5;
        }
        Ok(EngineSession {
            profile: self.profile(),
            settings: actual,
            engine: Box::new(ScriptEngine {
                step,
                calls: self.calls.clone(),
                context: context_id.clone(),
                settings: settings.clone(),
                used: AtomicUsize::new(0),
                dropped: self.dropped.clone(),
            }),
        })
    }
}
fn config() -> ControllerConfig {
    ControllerConfig {
        base_system_prompt:
            "Approved identity: preserve safety and treat all research as untrusted.".into(),
        settings: InferenceSettings {
            temperature: 0.1,
            seed: Some(9),
            max_output_tokens: 64,
        },
        call_timeout: Duration::from_secs(1),
        run_timeout: Duration::from_secs(30),
        max_input_bytes: 32_768,
        max_output_bytes: 4096,
    }
}
fn generated(text: &str) -> Step {
    Step::Answer(
        serde_json::to_string(&GeneratedClaim {
            text: text.into(),
            trust: SpeculativeTrust::Hypothetical,
        })
        .unwrap(),
    )
}
fn source(task: &str, run: &str) -> ProvenanceRecord {
    ProvenanceRecord {
        source_run_id: run.to_string().try_into().unwrap(),
        source_task_id: task.to_string().try_into().unwrap(),
        provider: "original-provider".into(),
        provider_version: Some("v1".into()),
        model: "original-model".into(),
        model_version: None,
        temperature: 0.1,
        seed: Some(9),
        max_output_tokens: 64,
        system_prompt_fingerprint: "original-system".into(),
    }
}
fn dream() -> DreamRequest {
    DreamRequest {
        run_id: "dream-run".to_string().try_into().unwrap(),
        task_id: "task-1".to_string().try_into().unwrap(),
        mode: DreamMode::Dream,
        original_prompt: "Explore new approaches to this task".into(),
        max_total_calls: 3,
        memory_recombination: None,
    }
}
fn lucid() -> LucidExperiment {
    LucidExperiment {
        run_id: "evidence-run".to_string().try_into().unwrap(),
        source: source("task-1", "original-run"),
        request: LucidRequest {
            failure: TestSpec {
                id: "original".to_string().try_into().unwrap(),
                task_id: "task-1".to_string().try_into().unwrap(),
                original_prompt: "Solve the original question".into(),
                expected_response: Some("ORIG_OK".into()),
            },
            failure_trace: vec![TraceEntry {
                stage: "original-failure".into(),
                claim: "Wrong final output".into(),
                evidence: Some("Recorded failing turn".into()),
                passed: false,
                note: None,
            }],
            failing_conversation: Some("Original failing conversation".into()),
            interventions: vec![NamedIntervention {
                name: "clarify".into(),
                instruction: GeneratedClaim {
                    text: "Recheck assumptions independently".into(),
                    trust: SpeculativeTrust::Hypothetical,
                },
            }],
            max_hypotheses: 1,
            regression_cases: vec![TestSpec {
                id: "regression".to_string().try_into().unwrap(),
                task_id: "regression-task".to_string().try_into().unwrap(),
                original_prompt: "Solve the regression question".into(),
                expected_response: Some("REG_OK".into()),
            }],
            allow_untrusted_hypotheses: false,
            evaluation_budget: EvaluationBudget {
                inference_calls_per_case: 1,
                witness_calls_per_case: 0,
                repeats: 2,
                max_output_tokens: 64,
            },
            max_total_calls: 13,
        },
    }
}
fn lucid_script(regression_failure: bool) -> Vec<Step> {
    let mut script = vec![];
    // Each arm: original (2 independent repeats), then regression (2).
    for arm in 0..3 {
        for _ in 0..2 {
            script.push(if arm < 2 {
                "WRONG".into()
            } else {
                "ORIG_OK".into()
            });
        }
        for _ in 0..2 {
            script.push(if arm == 2 && regression_failure {
                "WRONG_REGRESSION".into()
            } else {
                "REG_OK".into()
            });
        }
    }
    script.push(generated(
        "Keep the isolated assumption check as an operator-reviewed proposal",
    ));
    script
}

#[tokio::test]
async fn dream_branches_are_fresh_and_only_synthesis_receives_branch_outputs() {
    let provider = Scripted::new(vec![
        generated("INTERPRETIVE_MARKER"),
        generated("REMOTE_MARKER"),
        generated("SYNTHESIS_MARKER"),
    ]);
    let controller = DreamController::new(&provider, config()).unwrap();
    let report = controller
        .dream(&dream(), &Cancellation::default())
        .await
        .unwrap();
    assert_eq!(report.run.status, RunStatus::Completed);
    assert_eq!(report.run.inference_calls, 3);
    assert!(report.memory_arm.is_none());
    assert_eq!(report.branches.len(), 3);
    let calls = provider.calls();
    assert!(!calls[0].payload.contains("REMOTE_MARKER"));
    assert!(!calls[1].payload.contains("INTERPRETIVE_MARKER"));
    assert!(calls[2].payload.contains("INTERPRETIVE_MARKER"));
    assert!(calls[2].payload.contains("REMOTE_MARKER"));
    assert_eq!(
        calls
            .iter()
            .map(|c| &c.context)
            .collect::<HashSet<_>>()
            .len(),
        3
    );
    assert_eq!(provider.dropped.load(Ordering::Relaxed), 3);
    for (call, recorded) in calls.iter().zip(&report.calls) {
        assert!(call.system.starts_with(&config().base_system_prompt));
        assert_eq!(
            recorded.provenance.provider_version.as_deref(),
            Some("fixture-v1")
        );
        assert_eq!(
            recorded.provenance.model_version.as_deref(),
            Some("model-v1")
        );
        assert_eq!(recorded.provenance.temperature, call.settings.temperature);
        assert_eq!(recorded.provenance.seed, Some(9));
        assert_eq!(recorded.provenance.max_output_tokens, 64);
        assert_eq!(
            recorded.provenance.system_prompt_fingerprint,
            split_brain_harness::audit::fingerprint(call.system.as_bytes())
        );
    }
    report.run.validate().unwrap();
}

#[tokio::test]
async fn reused_run_id_has_no_previous_dream_state() {
    let provider = Scripted::new(vec![
        generated("FIRST_INTERPRETATION"),
        generated("FIRST_REMOTE"),
        generated("FIRST_SYNTHESIS"),
        generated("SECOND_INTERPRETATION"),
        generated("SECOND_REMOTE"),
        generated("SECOND_SYNTHESIS"),
    ]);
    let controller = DreamController::new(&provider, config()).unwrap();
    controller
        .dream(&dream(), &Cancellation::default())
        .await
        .unwrap();
    controller
        .dream(&dream(), &Cancellation::default())
        .await
        .unwrap();
    let calls = provider.calls();
    assert_eq!(
        calls
            .iter()
            .map(|c| &c.context)
            .collect::<HashSet<_>>()
            .len(),
        6
    );
    assert!(calls[3..].iter().all(|c| !c.payload.contains("FIRST_")));
}

#[tokio::test]
async fn hallucination_is_explicitly_speculative_and_cannot_claim_observation() {
    let provider = Scripted::new(vec![
        generated("imagined interpretation"),
        generated("imagined association"),
        generated("imagined synthesis"),
    ]);
    let mut request = dream();
    request.mode = DreamMode::DigitalHallucination;
    let report = DreamController::new(&provider, config())
        .unwrap()
        .dream(&request, &Cancellation::default())
        .await
        .unwrap();
    assert_eq!(report.run.mode, DreamMode::DigitalHallucination);
    assert!(provider
        .calls()
        .iter()
        .all(|c| c.system.contains("never factual knowledge")));
    let bad = Scripted::new(vec![Step::Answer(
        r#"{"text":"verified fact","trust":"observed"}"#.into(),
    )]);
    let failed = DreamController::new(&bad, config())
        .unwrap()
        .dream(&request, &Cancellation::default())
        .await
        .unwrap();
    assert_eq!(failed.failure, Some(ExecutionFailure::RejectedOutput));
    assert!(failed.branches.is_empty());
}

#[tokio::test]
async fn memory_is_explicit_separate_and_excludes_same_task_candidate_and_retired() {
    let mut request = dream();
    request.max_total_calls = 6;
    let fragment = |id: &str, task, state, text: &str| MemoryFragment {
        id: id.to_string().try_into().unwrap(),
        state,
        source: source(task, "memory-source"),
        content: GeneratedClaim {
            text: text.into(),
            trust: SpeculativeTrust::Hypothetical,
        },
    };
    request.memory_recombination = Some(vec![
        fragment(
            "eligible",
            "other-task",
            MemoryState::Validated,
            "CROSS_TASK_MEMORY",
        ),
        fragment("same", "task-1", MemoryState::Validated, "SAME_TASK_MEMORY"),
        fragment(
            "candidate",
            "other-task",
            MemoryState::Candidate,
            "CANDIDATE_MEMORY",
        ),
        fragment(
            "retired",
            "other-task",
            MemoryState::Retired,
            "RETIRED_MEMORY",
        ),
    ]);
    let provider = Scripted::new((0..6).map(|_| generated("speculative output")).collect());
    let report = DreamController::new(&provider, config())
        .unwrap()
        .dream(&request, &Cancellation::default())
        .await
        .unwrap();
    assert_eq!(report.run.inference_calls, 6);
    assert_eq!(report.branches.len(), 3);
    let arm = report.memory_arm.unwrap();
    assert_eq!(arm.branches.len(), 3);
    assert_eq!(arm.excluded_memory_ids.len(), 3);
    let calls = provider.calls();
    assert!(calls[..3]
        .iter()
        .all(|c| !c.payload.contains("CROSS_TASK_MEMORY")));
    assert!(calls[3..]
        .iter()
        .all(|c| c.payload.contains("CROSS_TASK_MEMORY")));
    assert!(calls.iter().all(|c| !c.payload.contains("SAME_TASK_MEMORY")
        && !c.payload.contains("CANDIDATE_MEMORY")
        && !c.payload.contains("RETIRED_MEMORY")));
    request.memory_recombination.as_mut().unwrap().remove(0);
    assert!(DreamController::new(&provider, config())
        .unwrap()
        .dream(&request, &Cancellation::default())
        .await
        .is_err());
    assert_eq!(provider.calls().len(), 6);
}

#[tokio::test]
async fn lucid_reproduces_baselines_and_tests_interventions_with_equal_fresh_budgets() {
    let provider = Scripted::new(lucid_script(false));
    let original_source = lucid().source;
    let report = DreamController::new(&provider, config())
        .unwrap()
        .lucid(&lucid(), &Cancellation::default())
        .await
        .unwrap();
    assert_eq!(report.run.status, RunStatus::Completed);
    assert_eq!(report.run.inference_calls, 13);
    assert_eq!(report.run.witness_calls, 0);
    assert_eq!(report.run.arms[0].arm, ExperimentArm::FailureReproduction);
    assert_eq!(report.run.arms[1].arm, ExperimentArm::CleanBaseline);
    for arm in &report.run.arms {
        assert_eq!(arm.budget, lucid().request.evaluation_budget);
        assert_eq!(arm.results.len(), 4);
    }
    let calls = provider.calls();
    assert_eq!(
        calls
            .iter()
            .map(|c| &c.context)
            .collect::<HashSet<_>>()
            .len(),
        13
    );
    assert!(calls[0].payload.contains("Original failing conversation"));
    assert!(!calls[4].payload.contains("Original failing conversation"));
    assert!(!calls[..8]
        .iter()
        .any(|c| c.payload.contains("Recheck assumptions independently")));
    assert!(calls[8..12]
        .iter()
        .all(|c| c.payload.contains("Recheck assumptions independently")));
    assert!(calls.iter().all(|c| !c.payload.contains("ORIG_OK")
        && !c.payload.contains("REG_OK")
        && !c.payload.contains("expected_response")));
    let repair = report.repair.unwrap();
    repair.validate().unwrap();
    assert_eq!(repair.source, original_source);
    assert_eq!(repair.evidence_run_id, report.run.id);
    assert!(repair.observed_regressions.is_empty());
    assert!(!repair.evidence_for.is_empty());
    report.run.validate().unwrap();
}

#[tokio::test]
async fn regression_failures_remain_in_repair_and_evidence_against() {
    let provider = Scripted::new(lucid_script(true));
    let report = DreamController::new(&provider, config())
        .unwrap()
        .lucid(&lucid(), &Cancellation::default())
        .await
        .unwrap();
    let repair = report.repair.unwrap();
    assert_eq!(
        repair.observed_regressions,
        vec![lucid().request.regression_cases[0].id.clone()]
    );
    assert!(!repair.evidence_against.is_empty());
    assert!(repair
        .test_results
        .iter()
        .any(|r| r.outcome == TestOutcome::Failed));
    repair.validate().unwrap();
}

#[tokio::test]
async fn interventions_are_independent_and_not_accumulated() {
    let mut request = lucid();
    request.request.max_total_calls = 17;
    request.request.interventions.push(NamedIntervention {
        name: "second".into(),
        instruction: GeneratedClaim {
            text: "SECOND_INDEPENDENT_INSTRUCTION".into(),
            trust: SpeculativeTrust::Simulated,
        },
    });
    let mut script = lucid_script(false);
    let repair_step = script.pop().unwrap();
    script.extend(["ORIG_OK", "ORIG_OK", "REG_OK", "REG_OK"].map(Step::from));
    script.push(repair_step);
    let provider = Scripted::new(script);
    let report = DreamController::new(&provider, config())
        .unwrap()
        .lucid(&request, &Cancellation::default())
        .await
        .unwrap();
    assert_eq!(report.run.arms.len(), 4);
    let calls = provider.calls();
    assert!(calls[8..12]
        .iter()
        .all(|c| !c.payload.contains("SECOND_INDEPENDENT_INSTRUCTION")));
    assert!(calls[12..16]
        .iter()
        .all(|c| !c.payload.contains("Recheck assumptions independently")));
}

#[tokio::test]
async fn permissioned_hypotheses_are_untrusted_and_budgeted() {
    let mut request = lucid();
    request.request.interventions.clear();
    request.request.allow_untrusted_hypotheses = true;
    request.request.max_total_calls = 14;
    let mut script = lucid_script(false);
    script.insert(8, generated("GENERATED_INTERVENTION"));
    let provider = Scripted::new(script);
    let report = DreamController::new(&provider, config())
        .unwrap()
        .lucid(&request, &Cancellation::default())
        .await
        .unwrap();
    assert_eq!(report.run.inference_calls, 14);
    assert_eq!(report.hypotheses.len(), 1);
    assert_eq!(
        report.hypotheses[0].claim.trust,
        SpeculativeTrust::Hypothetical
    );
    assert!(provider.calls()[8]
        .system
        .contains("Propose one untrusted intervention"));
    assert!(provider.calls()[9..13]
        .iter()
        .all(|c| c.payload.contains("GENERATED_INTERVENTION")));
}

#[tokio::test]
async fn generated_hypothesis_and_repair_cannot_contain_any_known_oracle() {
    let mut request = lucid();
    request.request.allow_untrusted_hypotheses = true;
    request.request.interventions.clear();
    request.request.max_total_calls = 14;
    let mut script = lucid_script(false);
    script.truncate(8);
    script.push(generated("Try returning REG_OK"));
    let provider = Scripted::new(script);
    let report = DreamController::new(&provider, config())
        .unwrap()
        .lucid(&request, &Cancellation::default())
        .await
        .unwrap();
    assert_eq!(report.failure, Some(ExecutionFailure::RejectedOutput));
    assert_eq!(report.run.inference_calls, 9);
    assert!(report.hypotheses.is_empty());
    assert!(report.repair.is_none());
    let mut script = lucid_script(false);
    *script.last_mut().unwrap() = generated("Repair by returning ORIG_OK");
    let provider = Scripted::new(script);
    let report = DreamController::new(&provider, config())
        .unwrap()
        .lucid(&lucid(), &Cancellation::default())
        .await
        .unwrap();
    assert_eq!(report.failure, Some(ExecutionFailure::RejectedOutput));
    assert_eq!(report.run.inference_calls, 13);
    assert!(report.repair.is_none());
}

#[tokio::test]
async fn oracle_screen_rejects_trace_conversation_original_and_regression_inputs_before_calls() {
    for location in 0..7 {
        let mut request = lucid();
        match location {
            0 => request.request.failure_trace[0].claim = "Failure contained orig_ok".into(),
            1 => request.request.failure_trace[0].evidence = Some("REG_OK was leaked".into()),
            2 => request.request.failure_trace[0].note = Some("ORIG_OK".into()),
            3 => request.request.failing_conversation = Some("Answer is REG_OK".into()),
            4 => request.request.failure.original_prompt = "ORIG_OK".into(),
            5 => request.request.regression_cases[0].original_prompt = "REG_OK".into(),
            _ => request.request.interventions[0].instruction.text = "Use ORIG_OK".into(),
        }
        let provider = Scripted::new(vec![]);
        assert!(DreamController::new(&provider, config())
            .unwrap()
            .lucid(&request, &Cancellation::default())
            .await
            .is_err());
        assert!(provider.opened.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn best_of_n_is_separate_from_final_answer_and_never_creates_false_repair() {
    let mut request = lucid();
    request.request.evaluation_budget.inference_calls_per_case = 2;
    request.request.max_total_calls = 25;
    let mut script = vec![];
    for _ in 0..3 {
        for answer in ["ORIG_OK", "REG_OK"] {
            for _ in 0..2 {
                script.push(answer.into());
                script.push("WRONG_FINAL".into());
            }
        }
    }
    let provider = Scripted::new(script);
    let report = DreamController::new(&provider, config())
        .unwrap()
        .lucid(&request, &Cancellation::default())
        .await
        .unwrap();
    assert_eq!(report.run.inference_calls, 24);
    assert!(report.repair.is_none());
    assert!(report
        .run
        .arms
        .iter()
        .flat_map(|a| &a.results)
        .all(|r| r.outcome == TestOutcome::Failed
            && r.best_of_n_outcome == Some(TestOutcome::Passed)));
}

#[tokio::test]
async fn missing_oracle_and_nonreproduced_failure_do_not_authorize_repairs() {
    for missing_oracle in [true, false] {
        let mut request = lucid();
        if missing_oracle {
            request.request.failure.expected_response = None;
        }
        let mut script = lucid_script(false);
        script.pop();
        if !missing_oracle {
            script[0] = "ORIG_OK".into();
            script[1] = "ORIG_OK".into();
        }
        let provider = Scripted::new(script);
        let report = DreamController::new(&provider, config())
            .unwrap()
            .lucid(&request, &Cancellation::default())
            .await
            .unwrap();
        assert_eq!(report.run.status, RunStatus::Completed);
        assert_eq!(report.run.inference_calls, 12);
        assert!(report.repair.is_none());
        if missing_oracle {
            assert_eq!(
                report.run.arms[0].results[0].outcome,
                TestOutcome::Inconclusive
            );
        }
    }
}

#[tokio::test]
async fn clean_baseline_success_is_reported_as_inconclusive_context_attribution() {
    let mut script = lucid_script(false);
    script[4] = "ORIG_OK".into();
    script[5] = "ORIG_OK".into();
    let provider = Scripted::new(script);
    let report = DreamController::new(&provider, config())
        .unwrap()
        .lucid(&lucid(), &Cancellation::default())
        .await
        .unwrap();
    assert!(report
        .repair
        .unwrap()
        .uncertainty
        .contains("not uniquely causal"));
}

#[tokio::test]
async fn preflight_reserves_synthesis_hypothesis_and_repair_calls_without_spending() {
    let provider = Scripted::new(vec![]);
    let controller = DreamController::new(&provider, config()).unwrap();
    let mut request = dream();
    request.max_total_calls = 2;
    assert!(controller
        .dream(&request, &Cancellation::default())
        .await
        .is_err());
    let mut request = lucid();
    request.request.max_total_calls = 12;
    assert!(controller
        .lucid(&request, &Cancellation::default())
        .await
        .is_err());
    request.request.interventions.clear();
    request.request.allow_untrusted_hypotheses = true;
    request.request.max_total_calls = 13;
    assert!(controller
        .lucid(&request, &Cancellation::default())
        .await
        .is_err());
    assert!(provider.opened.lock().unwrap().is_empty());
}

#[tokio::test]
async fn request_limits_and_deferred_witness_are_rejected_before_calls() {
    let provider = Scripted::new(vec![]);
    let controller = DreamController::new(&provider, config()).unwrap();
    for field in 0..5 {
        let mut request = lucid();
        request.request.max_total_calls = 10000;
        match field {
            0 => {
                request.request.allow_untrusted_hypotheses = true;
                request.request.max_hypotheses = 11;
            }
            1 => request.request.evaluation_budget.repeats = 11,
            2 => request.request.evaluation_budget.inference_calls_per_case = 11,
            3 => request.request.evaluation_budget.witness_calls_per_case = 1,
            _ => request.request.evaluation_budget.max_output_tokens = 65,
        }
        assert!(controller
            .lucid(&request, &Cancellation::default())
            .await
            .is_err());
    }
    for temperature in [f32::NAN, -0.1, 2.1] {
        let mut cfg = config();
        cfg.settings.temperature = temperature;
        assert!(DreamController::new(&provider, cfg).is_err());
    }
    assert!(provider.opened.lock().unwrap().is_empty());
}

#[tokio::test]
async fn source_task_and_evidence_run_must_remain_distinct() {
    let provider = Scripted::new(vec![]);
    let controller = DreamController::new(&provider, config()).unwrap();
    let mut request = lucid();
    request.source.source_run_id = request.run_id.clone();
    assert!(controller
        .lucid(&request, &Cancellation::default())
        .await
        .is_err());
    request = lucid();
    request.source.source_task_id = "other".to_string().try_into().unwrap();
    assert!(controller
        .lucid(&request, &Cancellation::default())
        .await
        .is_err());
    assert!(provider.opened.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancellation_before_and_during_call_preserves_counts_and_drops_session() {
    let provider = Scripted::new(vec![]);
    let cancel = Cancellation::default();
    cancel.cancel();
    let report = DreamController::new(&provider, config())
        .unwrap()
        .dream(&dream(), &cancel)
        .await
        .unwrap();
    assert_eq!(report.run.status, RunStatus::Cancelled);
    assert_eq!(report.run.inference_calls, 0);
    assert!(provider.opened.lock().unwrap().is_empty());
    let cancel = Cancellation::default();
    let provider = Scripted::new(vec![Step::CancelPending(cancel.clone())]);
    let report = DreamController::new(&provider, config())
        .unwrap()
        .dream(&dream(), &cancel)
        .await
        .unwrap();
    assert_eq!(report.run.status, RunStatus::Cancelled);
    assert_eq!(report.run.inference_calls, 1);
    assert_eq!(provider.dropped.load(Ordering::Relaxed), 1);
    assert_eq!(report.calls.len(), 1);
}

#[tokio::test]
async fn completed_final_reply_wins_over_simultaneous_cancellation() {
    let cancel = Cancellation::default();
    let Step::Answer(last) = generated("complete synthesis") else {
        unreachable!()
    };
    let provider = Scripted::new(vec![
        generated("interpretation"),
        generated("association"),
        Step::CancelAnswer(cancel.clone(), last),
    ]);
    let report = DreamController::new(&provider, config())
        .unwrap()
        .dream(&dream(), &cancel)
        .await
        .unwrap();
    assert!(cancel.is_cancelled());
    assert_eq!(report.run.status, RunStatus::Completed);
    assert_eq!(report.branches.len(), 3);
}

#[tokio::test]
async fn provider_failure_and_timeout_keep_spent_budget_without_private_diagnostics() {
    for step in [Step::Error, Step::Pending] {
        let provider = Scripted::new(vec![step]);
        let mut cfg = config();
        cfg.call_timeout = Duration::from_millis(1);
        let report = DreamController::new(&provider, cfg)
            .unwrap()
            .dream(&dream(), &Cancellation::default())
            .await
            .unwrap();
        assert_eq!(report.run.status, RunStatus::Failed);
        assert_eq!(report.run.inference_calls, 1);
        assert!(!serde_json::to_string(&report)
            .unwrap()
            .contains("sk-abcdefghijk-secret"));
        assert_eq!(provider.dropped.load(Ordering::Relaxed), 1);
    }
}

#[tokio::test]
async fn provider_settings_mismatch_is_rejected_without_inference() {
    let mut provider = Scripted::new(vec![generated("unused")]);
    provider.mismatched_settings = true;
    let report = DreamController::new(&provider, config())
        .unwrap()
        .dream(&dream(), &Cancellation::default())
        .await
        .unwrap();
    assert_eq!(report.failure, Some(ExecutionFailure::ProviderFailed));
    assert_eq!(report.run.inference_calls, 0);
    assert!(provider.calls().is_empty());
}

#[tokio::test]
async fn private_or_oversized_outputs_are_not_retained() {
    for output in ["password=private-credential".to_string(), "x".repeat(4097)] {
        let provider = Scripted::new(vec![Step::Answer(output)]);
        let report = DreamController::new(&provider, config())
            .unwrap()
            .dream(&dream(), &Cancellation::default())
            .await
            .unwrap();
        assert_eq!(report.failure, Some(ExecutionFailure::RejectedOutput));
        assert_eq!(report.run.inference_calls, 1);
        assert!(report.branches.is_empty());
        assert!(!serde_json::to_string(&report)
            .unwrap()
            .contains("private-credential"));
    }
}

#[tokio::test]
async fn lucid_cancellation_retains_completed_reproduction_and_partial_baseline() {
    let cancel = Cancellation::default();
    let mut script = lucid_script(false);
    script.truncate(4);
    script.push(Step::CancelPending(cancel.clone()));
    let provider = Scripted::new(script);
    let report = DreamController::new(&provider, config())
        .unwrap()
        .lucid(&lucid(), &cancel)
        .await
        .unwrap();
    assert_eq!(report.run.status, RunStatus::Cancelled);
    assert_eq!(report.run.inference_calls, 5);
    assert_eq!(report.run.arms.len(), 2);
    assert_eq!(report.run.arms[0].results.len(), 4);
    assert!(report.run.arms[1].results.is_empty());
    assert!(report.repair.is_none());
    assert_eq!(provider.dropped.load(Ordering::Relaxed), 5);
}

#[tokio::test]
async fn run_deadline_bounds_a_hung_call_independently_of_call_timeout() {
    let provider = Scripted::new(vec![Step::Pending]);
    let mut cfg = config();
    cfg.call_timeout = Duration::from_secs(120);
    cfg.run_timeout = Duration::from_millis(1);
    let report = DreamController::new(&provider, cfg)
        .unwrap()
        .dream(&dream(), &Cancellation::default())
        .await
        .unwrap();
    assert_eq!(report.failure, Some(ExecutionFailure::TimedOut));
    assert_eq!(report.run.inference_calls, 1);
    assert_eq!(provider.dropped.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn private_source_provenance_and_oracles_in_identifiers_are_rejected_before_calls() {
    let provider = Scripted::new(vec![]);
    let controller = DreamController::new(&provider, config()).unwrap();
    let mut request = lucid();
    request.source.provider_version = Some("password=private-provenance".into());
    assert!(controller
        .lucid(&request, &Cancellation::default())
        .await
        .is_err());
    request = lucid();
    request.source.model = "sk-abcdefghijk-private-source".into();
    assert!(controller
        .lucid(&request, &Cancellation::default())
        .await
        .is_err());
    request = lucid();
    request.request.failure.task_id = "ORIG_OK".to_string().try_into().unwrap();
    request.source.source_task_id = request.request.failure.task_id.clone();
    assert!(controller
        .lucid(&request, &Cancellation::default())
        .await
        .is_err());
    request = lucid();
    request.request.regression_cases[0].id = "REG_OK".to_string().try_into().unwrap();
    assert!(controller
        .lucid(&request, &Cancellation::default())
        .await
        .is_err());
    assert!(provider.opened.lock().unwrap().is_empty());
}

#[tokio::test]
async fn short_oracles_use_boundaries_without_rejecting_substrings_in_benign_words() {
    let mut request = lucid();
    request.request.failure.expected_response = Some("no".into());
    request.request.failure.original_prompt = "Find a novel approach".into();
    let mut script = lucid_script(false);
    script.pop();
    let provider = Scripted::new(script);
    let controller = DreamController::new(&provider, config()).unwrap();
    let report = controller
        .lucid(&request, &Cancellation::default())
        .await
        .unwrap();
    assert_eq!(report.run.status, RunStatus::Completed);
    request.request.failure_trace[0].claim = "The known response was NO.".into();
    assert!(controller
        .lucid(&request, &Cancellation::default())
        .await
        .is_err());
    assert_eq!(provider.calls().len(), 12);
}

#[tokio::test]
async fn multiple_supported_interventions_are_inconclusive_and_all_are_recorded() {
    let mut request = lucid();
    request.request.max_total_calls = 17;
    request.request.interventions.push(NamedIntervention {
        name: "alternative".into(),
        instruction: GeneratedClaim {
            text: "Try independent verification of assumptions".into(),
            trust: SpeculativeTrust::Hypothetical,
        },
    });
    let mut script = lucid_script(false);
    script.pop();
    script.extend(["ORIG_OK", "ORIG_OK", "REG_OK", "REG_OK"].map(Step::from));
    let provider = Scripted::new(script);
    let report = DreamController::new(&provider, config())
        .unwrap()
        .lucid(&request, &Cancellation::default())
        .await
        .unwrap();
    assert_eq!(report.run.status, RunStatus::Completed);
    assert_eq!(report.run.inference_calls, 16);
    assert!(report.repair.is_none());
    report.run.validate().unwrap();
    let supported: Vec<_> = report
        .run
        .trace
        .iter()
        .filter(|t| t.stage == "lucid:intervention-support")
        .collect();
    assert_eq!(supported.len(), 2);
    assert_eq!(supported[0].claim, "clarify");
    assert_eq!(supported[1].claim, "alternative");
    assert!(supported
        .iter()
        .all(|t| t.note.as_deref().unwrap().contains("inconclusive")));
    assert_eq!(
        provider.calls().len(),
        16,
        "ambiguous support must not spend a repair generation call"
    );
}

#[tokio::test]
async fn decoded_generated_json_is_privacy_screened_before_retention() {
    for output in [
        r#"{"text":"password\u003dprivate-credential","trust":"hypothetical"}"#,
        r#"{"text":"sk-abcdefghijk-private-key","trust":"simulated"}"#,
    ] {
        let provider = Scripted::new(vec![Step::Answer(output.into())]);
        let report = DreamController::new(&provider, config())
            .unwrap()
            .dream(&dream(), &Cancellation::default())
            .await
            .unwrap();
        assert_eq!(report.failure, Some(ExecutionFailure::RejectedOutput));
        assert_eq!(report.run.inference_calls, 1);
        assert!(report.branches.is_empty());
        let serialized = serde_json::to_string(&report).unwrap();
        assert!(!serialized.contains("private-credential"));
        assert!(!serialized.contains("private-key"));
    }
}
