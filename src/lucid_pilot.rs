//! Preregistered pilot admission, summary and explicit runner. No auto-execution.
use crate::{
    audit::{fingerprint, iso_now},
    dream::*,
    dream_engine::*,
    research_ollama::{
        parse_final_integer, CallTelemetry, CoreTempGuard, FinalIntegerScorer,
        ResearchOllamaProvider, ThermalReading,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub const REGISTRATION_COMMIT: &str = "2fea927176ecaa3337afc716d0685d792ad5b323";
const FROZEN_TASKS: &str = include_str!("../fixtures/dream_pilot_tasks.json");
const PREREG: &str = include_str!("../docs/DREAM_PILOT_PREREG.md");

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PilotTasks {
    pub schema_version: u32,
    pub pilot_id: String,
    pub model: String,
    pub base_system_prompt: String,
    pub settings: InferenceSettings,
    pub seed_policy: String,
    pub scorer: String,
    pub evaluation_budget: EvaluationBudget,
    pub max_total_calls_per_experiment: u64,
    pub max_total_calls: u64,
    pub interventions: Vec<NamedIntervention>,
    pub targets: Vec<TestSpec>,
    pub regressions: Vec<TestSpec>,
}
pub fn parse_registered(bytes: &[u8]) -> Result<PilotTasks, String> {
    if bytes.len() > 131_072 {
        return Err("task file too large".into());
    }
    let supplied: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "invalid task JSON")?;
    let frozen: serde_json::Value =
        serde_json::from_str(FROZEN_TASKS).map_err(|_| "invalid compiled registration")?;
    if supplied != frozen {
        return Err("task file differs from committed preregistration".into());
    }
    let tasks: PilotTasks = serde_json::from_value(supplied).map_err(|_| "invalid task schema")?;
    let mut ids = HashSet::new();
    let mut task_ids = HashSet::new();
    for case in tasks.targets.iter().chain(&tasks.regressions) {
        if !ids.insert(&case.id)
            || !task_ids.insert(&case.task_id)
            || case
                .expected_response
                .as_deref()
                .and_then(|s| s.parse::<i64>().ok())
                .is_none()
        {
            return Err("invalid or repeated registered case".into());
        }
    }
    if tasks.targets.len() != 10
        || tasks.regressions.len() != 4
        || tasks.evaluation_budget.required_calls(5, 1)? + 1 != 16
        || tasks.max_total_calls != 224
    {
        return Err("invalid registered budget".into());
    }
    Ok(tasks)
}
pub fn experiment(tasks: &PilotTasks, case: &TestSpec) -> Result<LucidExperiment, String> {
    let run_id: RunId =
        format!("{}:measurement:{}", tasks.pilot_id, case.id.as_str()).try_into()?;
    Ok(LucidExperiment {
        run_id,
        source: ProvenanceRecord {
            source_run_id: format!("prereg:{}", case.id.as_str()).try_into()?,
            source_task_id: case.task_id.clone(),
            provider: "preregistered-fixture".into(),
            provider_version: None,
            model: "not-model-output".into(),
            model_version: None,
            temperature: tasks.settings.temperature,
            seed: None,
            max_output_tokens: tasks.settings.max_output_tokens,
            system_prompt_fingerprint: fingerprint(tasks.base_system_prompt.as_bytes()),
        },
        request: LucidRequest {
            failure: case.clone(),
            failure_trace: vec![crate::types::TraceEntry {
                stage: "preregistered-candidate".into(),
                claim: "Candidate challenge; no prior observed model failure is established".into(),
                evidence: None,
                passed: false,
                note: Some(
                    "Fresh reproduction measures the candidate; no screening output is reused"
                        .into(),
                ),
            }],
            failing_conversation: None,
            interventions: tasks.interventions.clone(),
            max_hypotheses: 0,
            regression_cases: vec![],
            allow_untrusted_hypotheses: false,
            evaluation_budget: tasks.evaluation_budget,
            max_total_calls: tasks.max_total_calls_per_experiment,
        },
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct PilotCaseReport {
    pub case_id: TestId,
    pub regression: bool,
    pub report: LucidReport,
}
#[derive(Debug, Default, Clone, Serialize)]
pub struct ArmMetric {
    pub target_correct: u32,
    pub target_total: u32,
    pub regression_correct: u32,
    pub regression_total: u32,
    pub malformed_final_lines: u32,
    pub truncations: u32,
}
#[derive(Debug, Clone, Serialize)]
pub struct PilotSummary {
    pub arms: BTreeMap<String, ArmMetric>,
    pub positive_target_tasks: u32,
    pub primary_gain: i32,
    pub decision: String,
    pub attempted_calls: u64,
    pub sent_requests: usize,
}
fn arm_name(arm: &ExperimentArm) -> &str {
    match arm {
        ExperimentArm::FailureReproduction => "reproduction",
        ExperimentArm::CleanBaseline => "baseline",
        ExperimentArm::Intervention { name } => name.as_str(),
        ExperimentArm::MemoryRecombination { .. } => "unexpected-memory",
    }
}
pub fn summarize(
    cases: &[PilotCaseReport],
    telemetry: &[CallTelemetry],
    complete: bool,
) -> PilotSummary {
    let mut arms: BTreeMap<String, ArmMetric> =
        ["reproduction", "baseline", "decompose", "verify", "placebo"]
            .into_iter()
            .map(|s| (s.into(), ArmMetric::default()))
            .collect();
    let mut positive = 0;
    for case in cases {
        let mut named = 0;
        let mut placebo = 0;
        for (index, arm) in case.report.run.arms.iter().enumerate() {
            let name = arm_name(&arm.arm);
            let metric = arms.entry(name.into()).or_default();
            for (repeat, result) in arm.results.iter().enumerate() {
                let passed = u32::from(result.outcome == TestOutcome::Passed);
                if case.regression {
                    metric.regression_total += 1;
                    metric.regression_correct += passed;
                } else {
                    metric.target_total += 1;
                    metric.target_correct += passed;
                }
                if name == "decompose" {
                    named += passed;
                }
                if name == "placebo" {
                    placebo += passed;
                }
                if parse_final_integer(&result.final_answer).is_none() {
                    metric.malformed_final_lines += 1;
                }
                let stage = format!("lucid:arm:{index}:case:0:repeat:{repeat}:sample:0");
                if case
                    .report
                    .calls
                    .iter()
                    .find(|c| c.stage == stage)
                    .and_then(|c| telemetry.iter().find(|t| t.context_id == c.context_id))
                    .is_some_and(|t| t.done_reason.as_deref() == Some("length"))
                {
                    metric.truncations += 1;
                }
            }
        }
        if !case.regression && named > placebo {
            positive += 1;
        }
    }
    let named = &arms["decompose"];
    let placebo = &arms["placebo"];
    let baseline = &arms["baseline"];
    let gain = named.target_correct as i32 - placebo.target_correct as i32;
    let structurally_complete = cases.len() == 14
        && cases.iter().filter(|c| !c.regression).count() == 10
        && cases
            .iter()
            .all(|c| c.report.run.status == RunStatus::Completed && c.report.run.arms.len() == 5)
        && arms
            .values()
            .all(|m| m.target_total == 30 && m.regression_total == 12);
    let support = gain >= 3
        && positive >= 3
        && named.target_correct >= baseline.target_correct
        && named.regression_correct as i32 >= placebo.regression_correct as i32 - 1;
    let decision = if !complete || !structurally_complete {
        "inconclusive: incomplete or aborted"
    } else if support {
        "preliminary primary support"
    } else {
        "no demonstrated primary benefit"
    };
    PilotSummary {
        arms,
        positive_target_tasks: positive,
        primary_gain: gain,
        decision: decision.into(),
        attempted_calls: cases
            .iter()
            .map(|c| c.report.run.inference_calls + c.report.run.witness_calls)
            .sum(),
        sent_requests: telemetry.iter().filter(|t| t.request_sent).count(),
    }
}

#[derive(Serialize)]
pub struct PilotRun {
    pub registration_commit: String,
    pub code_revision: String,
    pub started_at: String,
    pub task_file_fingerprint: String,
    pub prereg_fingerprint: String,
    pub registered_tasks: PilotTasks,
    pub endpoint: String,
    pub profile: Option<ProviderProfile>,
    pub status: String,
    pub reason: Option<String>,
    pub cases: Vec<PilotCaseReport>,
    pub telemetry: Vec<CallTelemetry>,
    pub temperatures: Vec<ThermalReading>,
    pub metadata_checks: Vec<serde_json::Value>,
    pub summary: PilotSummary,
}
pub struct PilotOptions {
    pub tasks: PathBuf,
    pub output_dir: PathBuf,
    pub endpoint: String,
    pub model: String,
    pub code_revision: String,
}

fn private_dir(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(|_| "output directory must be new with an existing parent")?;
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir(path)
            .map_err(|_| "output directory must be new with an existing parent")?;
    }
    Ok(())
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let temp = path.with_extension("pending");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp)
        .map_err(|_| "cannot create pilot artifact")?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "cannot write pilot artifact")?;
    std::fs::rename(temp, path).map_err(|_| "cannot finalize pilot artifact".into())
}
fn save(output: &Path, run: &PilotRun) -> Result<(), String> {
    atomic_write(
        &output.join("report.json"),
        &serde_json::to_vec_pretty(run).map_err(|_| "cannot encode pilot report")?,
    )?;
    let mut markdown=format!("# Phase 2a Lucid pilot\n\nStatus: {}. {}\n\nRegistration: {}\n\nDecision: {}\n\nAttempted calls: {}; sent requests: {}\n\n| Arm | Targets | Regression guard | Malformed FINAL | Truncated |\n|---|---|---|---|---|\n",run.status,run.reason.as_deref().unwrap_or(""),run.registration_commit,run.summary.decision,run.summary.attempted_calls,run.summary.sent_requests);
    for (name, metric) in &run.summary.arms {
        markdown.push_str(&format!(
            "| {name} | {}/{} | {}/{} | {} | {} |\n",
            metric.target_correct,
            metric.target_total,
            metric.regression_correct,
            metric.regression_total,
            metric.malformed_final_lines,
            metric.truncations
        ));
    }
    markdown.push_str("\nFixed primary: decompose vs placebo. Secondary verify results do not rescue a failed primary. Incomplete results are inconclusive. No repairs applied.\n");
    atomic_write(&output.join("summary.md"), markdown.as_bytes())
}

/// Explicit runner only; neither library loading nor CI invokes this function.
async fn interruptible<T>(
    work: impl std::future::Future<Output = Result<T, String>>,
    cancel: &Cancellation,
) -> Result<T, String> {
    tokio::select! {biased; result=work=>result, _=cancel.cancelled()=>Err("pilot cancelled or global deadline reached".into())}
}

pub async fn run_pilot(options: PilotOptions, cancel: Cancellation) -> Result<PilotRun, String> {
    if options.code_revision.len() != 40
        || !options.code_revision.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("compile-time code revision required".into());
    }
    let bytes = std::fs::read(&options.tasks).map_err(|_| "cannot read task input")?;
    let tasks = parse_registered(&bytes)?;
    if options.model != tasks.model {
        return Err("model override requires new preregistration".into());
    }
    private_dir(&options.output_dir)?;
    let mut run = PilotRun {
        registration_commit: REGISTRATION_COMMIT.into(),
        code_revision: options.code_revision,
        started_at: iso_now(),
        task_file_fingerprint: fingerprint(&bytes),
        prereg_fingerprint: fingerprint(PREREG.as_bytes()),
        registered_tasks: tasks.clone(),
        endpoint: options.endpoint.clone(),
        profile: None,
        status: "running".into(),
        reason: None,
        cases: vec![],
        telemetry: vec![],
        temperatures: vec![],
        metadata_checks: vec![],
        summary: summarize(&[], &[], false),
    };
    save(&options.output_dir, &run)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(150 * 60);
    let timer_cancel = cancel.clone();
    let timer = tokio::spawn(async move {
        tokio::time::sleep_until(deadline).await;
        timer_cancel.cancel();
    });
    let mut guard: Option<Arc<CoreTempGuard>> = None;
    let mut provider: Option<ResearchOllamaProvider> = None;
    let outcome=async {
        if cancel.is_cancelled() {return Err("pilot cancelled".into());}
        guard=Some(Arc::new(CoreTempGuard::discover(Path::new("/sys/class/hwmon"))?));
        let thermal=guard.as_ref().ok_or("temperature guard missing")?.clone();
        provider=Some(interruptible(ResearchOllamaProvider::connect(&options.endpoint,&tasks.model,thermal.clone()),&cancel).await?);
        let provider=provider.as_ref().ok_or("provider missing")?;run.profile=Some(provider.profile());
        let scorer=FinalIntegerScorer::new(provider.telemetry());
        let controller=DreamController::new(provider,ControllerConfig {base_system_prompt:tasks.base_system_prompt.clone(),settings:tasks.settings.clone(),
            call_timeout:Duration::from_secs(60),run_timeout:Duration::from_secs(600),max_input_bytes:32_768,max_output_bytes:16_384})?
            .with_seed_policy(SeedPolicy::DistinctPerCall)?.with_scorer(&scorer);
        for (index,case) in tasks.targets.iter().chain(&tasks.regressions).enumerate() {
            if cancel.is_cancelled() {return Err("pilot cancelled or global deadline reached".into());}
            interruptible(provider.recheck_metadata(),&cancel).await?;
            let report=controller.lucid(&experiment(&tasks,case)?,&cancel).await?;
            let completed=report.run.status==RunStatus::Completed;
            run.cases.push(PilotCaseReport {case_id:case.id.clone(),regression:index>=tasks.targets.len(),report});
            run.telemetry=provider.telemetry().snapshot()?;run.temperatures=thermal.readings()?;
            run.metadata_checks=provider.metadata_checks()?;run.summary=summarize(&run.cases,&run.telemetry,false);
            atomic_write(&options.output_dir.join(format!("case-{:02}.json",index+1)),&serde_json::to_vec_pretty(run.cases.last().ok_or("case report missing")?).map_err(|_|"cannot encode case")?)?;
            save(&options.output_dir,&run)?;
            if !completed {return Err("research experiment failed, timed out or cancelled".into());}
            if run.summary.attempted_calls>tasks.max_total_calls {return Err("pilot budget exceeded".into());}
            if index<13 {tokio::select! {result=thermal.cooldown()=>result?,_ = cancel.cancelled()=>return Err("pilot cancelled during cooldown".into())}}
        }
        interruptible(provider.recheck_metadata(),&cancel).await?;
        if cancel.is_cancelled() {return Err("pilot cancelled or global deadline reached".into());}
        Ok::<(),String>(())
    }.await;
    timer.abort();
    if let Some(provider) = &provider {
        run.telemetry = provider.telemetry().snapshot()?;
        run.metadata_checks = provider.metadata_checks()?;
    }
    if let Some(guard) = &guard {
        run.temperatures = guard.readings()?;
    }
    match outcome {
        Ok(()) => run.status = "completed".into(),
        Err(reason) => {
            run.status = "aborted".into();
            run.reason = Some(reason);
        }
    }
    run.summary = summarize(&run.cases, &run.telemetry, run.status == "completed");
    save(&options.output_dir, &run)?;
    Ok(run)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tasks() -> PilotTasks {
        parse_registered(FROZEN_TASKS.as_bytes()).unwrap()
    }
    fn cases(primary: bool) -> Vec<PilotCaseReport> {
        let tasks = tasks();
        tasks
            .targets
            .iter()
            .chain(&tasks.regressions)
            .enumerate()
            .map(|(index, case)| {
                let experiment = experiment(&tasks, case).unwrap();
                let regression = index >= 10;
                let kinds = [
                    ExperimentArm::FailureReproduction,
                    ExperimentArm::CleanBaseline,
                    ExperimentArm::Intervention {
                        name: "decompose".into(),
                    },
                    ExperimentArm::Intervention {
                        name: "verify".into(),
                    },
                    ExperimentArm::Intervention {
                        name: "placebo".into(),
                    },
                ];
                let arms = kinds
                    .into_iter()
                    .enumerate()
                    .map(|(arm_index, arm)| {
                        let results = (0..3)
                            .map(|repeat| {
                                let passed = regression
                                    || arm_index == 3
                                    || primary && arm_index == 2 && index < 3 && repeat == 0;
                                TestResult {
                                    test_id: case.id.clone(),
                                    evidence_run_id: experiment.run_id.clone(),
                                    outcome: if passed {
                                        TestOutcome::Passed
                                    } else {
                                        TestOutcome::Failed
                                    },
                                    final_answer: format!(
                                        "FINAL: {}",
                                        case.expected_response.as_deref().unwrap()
                                    ),
                                    best_of_n_outcome: None,
                                    evidence: "scripted test fixture".into(),
                                }
                            })
                            .collect();
                        let mut provenance = experiment.source.clone();
                        provenance.source_run_id = experiment.run_id.clone();
                        ArmRecord {
                            arm,
                            context_id: format!("fixture-context-{index}-{arm_index}")
                                .try_into()
                                .unwrap(),
                            budget: tasks.evaluation_budget,
                            provenance,
                            results,
                        }
                    })
                    .collect();
                PilotCaseReport {
                    case_id: case.id.clone(),
                    regression,
                    report: LucidReport {
                        run: ExperimentRun {
                            id: experiment.run_id,
                            task_id: case.task_id.clone(),
                            mode: DreamMode::Lucid,
                            status: RunStatus::Completed,
                            arms,
                            inference_calls: 15,
                            witness_calls: 0,
                            max_total_calls: 16,
                            trace: vec![],
                        },
                        hypotheses: vec![],
                        repair: None,
                        calls: vec![],
                        witness_evaluations: vec![],
                        failure: None,
                    },
                }
            })
            .collect()
    }
    #[test]
    fn admission_freezes_settings_cases_and_budget() {
        let tasks = tasks();
        assert_eq!(tasks.targets.len(), 10);
        assert_eq!(tasks.regressions.len(), 4);
        assert_eq!(
            (tasks.targets.len() + tasks.regressions.len()) as u64
                * tasks.max_total_calls_per_experiment,
            224
        );
        let mut value: serde_json::Value = serde_json::from_str(FROZEN_TASKS).unwrap();
        value["settings"]["max_output_tokens"] = 64.into();
        assert!(parse_registered(&serde_json::to_vec(&value).unwrap()).is_err());
        value = serde_json::from_str(FROZEN_TASKS).unwrap();
        value["targets"][0]["expected_response"] = "999".into();
        assert!(parse_registered(&serde_json::to_vec(&value).unwrap()).is_err());
        assert!(parse_registered(&vec![b' '; 131_073]).is_err());
        let experiment = experiment(&tasks, &tasks.targets[0]).unwrap();
        assert_eq!(
            experiment
                .request
                .evaluation_budget
                .required_calls(5, 1)
                .unwrap(),
            15
        );
        assert_eq!(experiment.source.provider, "preregistered-fixture");
        assert!(experiment.request.regression_cases.is_empty());
        experiment.request.validate().unwrap();
    }
    #[test]
    fn primary_threshold_secondary_nonrescue_and_regression_guard() {
        let positive = cases(true);
        let summary = summarize(&positive, &[], true);
        assert_eq!(summary.decision, "preliminary primary support");
        assert_eq!(summary.primary_gain, 3);
        assert_eq!(summary.positive_target_tasks, 3);
        assert_eq!(summary.attempted_calls, 210);
        assert_eq!(summary.arms["decompose"].target_total, 30);
        assert_eq!(
            summarize(&cases(false), &[], true).decision,
            "no demonstrated primary benefit"
        );
        let mut guarded = positive;
        guarded[10].report.run.arms[2].results[0].outcome = TestOutcome::Failed;
        guarded[10].report.run.arms[2].results[1].outcome = TestOutcome::Failed;
        assert_eq!(
            summarize(&guarded, &[], true).decision,
            "no demonstrated primary benefit"
        );
        assert!(summarize(&guarded[..13], &[], true)
            .decision
            .contains("inconclusive"));
        assert!(summarize(&guarded, &[], false)
            .decision
            .contains("inconclusive"));
    }
    #[tokio::test]
    async fn cancelled_runner_saves_partial_artifacts_without_network_or_sensor_reads() {
        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("tasks.json");
        std::fs::write(&input, FROZEN_TASKS).unwrap();
        let output = root.path().join("new-run");
        let cancelled = Cancellation::default();
        cancelled.cancel();
        let report = run_pilot(
            PilotOptions {
                tasks: input.clone(),
                output_dir: output.clone(),
                endpoint: "http://127.0.0.1:11434".into(),
                model: "llama3.2:3b".into(),
                code_revision: "a".repeat(40),
            },
            cancelled,
        )
        .await
        .unwrap();
        assert_eq!(report.status, "aborted");
        assert!(report.summary.decision.contains("inconclusive"));
        assert_eq!(report.summary.sent_requests, 0);
        assert!(report.telemetry.is_empty());
        assert!(report.temperatures.is_empty());
        assert!(report.profile.is_none());
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output.join("report.json")).unwrap()).unwrap();
        assert_eq!(json["registration_commit"], REGISTRATION_COMMIT);
        assert!(std::fs::read_to_string(output.join("summary.md"))
            .unwrap()
            .contains("aborted"));
        assert!(run_pilot(
            PilotOptions {
                tasks: input,
                output_dir: output.clone(),
                endpoint: "http://127.0.0.1:11434".into(),
                model: "llama3.2:3b".into(),
                code_revision: "a".repeat(40)
            },
            Cancellation::default()
        )
        .await
        .is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(output.join("report.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(output).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }
}
