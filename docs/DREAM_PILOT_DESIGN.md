# Phase 2a design proposal — no live execution

Status: design for review on `codex/dream-engine-live-pilot`, based on `ac71af7`.
This step changes documents and a preregistered task fixture only. No adapter,
runner or controller implementation is added yet, and no local model is contacted.
Implementation follows design approval; preregistration must be committed before
any live run, and Chris/Claude must approve the run separately.

## Research-only adapter

Proposed `src/research_ollama.rs` owns `ResearchOllamaProvider` and a one-use
`ResearchOllamaEngine` implementing the existing `InferenceEngine`. Leave
`crates/sbh-llm/src/ollama.rs` and `OllamaNativeEngine` untouched to preserve
benchmark behavior. No serve, CLI, decision-log or production store integration.
Use existing reqwest, serde, Tokio and FNV helpers; no new dependency.

Construction is explicitly async: validate endpoint, then retrieve `/api/version`
and `/api/tags`, select the exact configured model and freeze version plus model
digest in `ProviderProfile`. Fail closed on missing/blank metadata or model.
Use no model download/pull API. Recheck metadata before each experiment and at
the end; a changed digest/version invalidates the run and stops further calls.
Metadata reads do not invoke generation and are logged separately from inference.

Parse endpoint using `reqwest::Url`; require plain HTTP, literal host exactly
`127.0.0.1` or `localhost`, no credentials, query, fragment or non-root path,
and a valid explicit/default port. Reject remote hosts, IPv6 and URL host aliases.
Pin localhost resolution to 127.0.0.1, disable environment proxies and redirects,
so local validation cannot redirect/DNS-resolve generation off-machine. Set a
bounded connection timeout and request timeout; scrub server/error bodies.

Every `open(context_id, requested_settings)` creates a new engine with immutable
settings and no conversation history. Sharing an HTTP connection pool is allowed;
sharing conversation state is not. Enforce one invocation per engine. POST
`/api/chat` with exactly system/user messages, `stream:false`, `think:false`, and
`options:{temperature,seed,num_predict:max_output_tokens}`. No tools, implicit
defaults for these settings, or accumulated messages. Check HTTP success,
bounded response bytes, completed response, model identity and string content;
record truncation/done_reason rather than silently claiming a complete answer.

The official [chat API](https://docs.ollama.com/api/chat) documents the messages,
runtime options and thinking controls; [tags](https://docs.ollama.com/api/tags)
supplies model digests. The implementation will verify the version response
against Ollama's official API schema in offline fixtures. Settings are requested
and recorded; the client cannot prove the server honored them or reset hidden state.

## Distinct seeds without weakening settings checks

Add an explicit controller seed policy, Fixed by default and DistinctPerCall only
for this pilot. Require a configured base seed. Derive a deterministic run base
from length-delimited run ID and base seed using existing FNV; reduce to 63 bits
for Ollama's signed integer range. For call ordinal k, seed = (run_base + k) mod
2^63. With bounded calls, this is injective within a run, including across arms,
cases, repeats, samples and any repair call. Run IDs are unique in the task file
execution manifest. Distinct seeds across different runs are not claimed to prove
statistical independence, and a seed does not guarantee backend determinism.

The controller computes the seed BEFORE `open`, passes a copied settings record,
and compares the returned session's FULL settings to that per-call request.
Thus comparison to the fixed configuration changes only for seed; temperature,
token ceiling and profile equality remain enforced, and even the derived seed
cannot be replaced by an adapter. Preserve fixed-seed behavior/tests by default.
Call provenance records actual requested/session seed, not the configured base.
Arm-level aggregate provenance has seed None in distinct mode; call records are
the authoritative per-invocation settings. Record policy/base in runner manifest.
Stable ordinal/stage mapping is logged; failed calls stay charged, no silent retry
or reseeding after seeing an answer. No per-call seed may be silently omitted.

## Preregistered scorer and transport diagnostics

Introduce an opt-in controller `AnswerScorer` interface taking raw answer, optional
oracle and actual call context ID, returning `Result<TestOutcome, ExecutionFailure>`
so missing required telemetry can fail the experiment explicitly. The default remains the current exact-trimmed
scorer with identical behavior; only the pilot installs `FinalIntegerScorer`.
Apply the configured scorer to each sample for diagnostic best-of-N and to the
final sample for TestResult/repair eligibility. Record scorer ID in experiment
manifests and result evidence; do not silently transform the model output.

FinalIntegerScorer accepts only the last non-empty line matching
`^FINAL: (-?\d+)$` with ASCII digits, no leading/trailing whitespace on that line,
and a checked signed 64-bit integer equal to the oracle. Ignore empty trailing
lines; handle CRLF normally. Earlier lines may show working. Missing/malformed
lines or integer overflow fail, missing oracle remains inconclusive. All arms
share this contract and the 256-token ceiling. Placebo retains content-free
`Answer the question.`; baseline gets no strategy.

Because InferenceEngine returns only text, the research provider also retains
bounded per-context response telemetry (done_reason, token counts, timing and
pre-call temperature) in a private runner-owned registry. It validates done/model
and records truncation without converting it into a transport failure. The pilot
scorer reads the exact call's completed registry entry and returns Failed for
truncation even if a FINAL line is present. Missing telemetry fails the experiment
rather than fabricating a score. Raw answers remain in reports; telemetry is
exported separately and truncations/format failures reported per arm. No global
registry or production engine changes. Offline tests cover CRLF/trailing blank
lines, signed values/leading zeros, extra text after FINAL, missing/malformed
lines, overflow and truncated-but-valid FINAL output, plus unchanged default tests.

## Pilot runner proposal

`examples/lucid_pilot.rs`, invoked with `--tasks`, `--output-dir`, optionally
`--endpoint` and `--model`; defaults `http://127.0.0.1:11434`, `llama3.2:3b`.
No top-level CLI subcommand. Require a task file and explicit new output directory;
refuse overwriting existing run artifacts. Example argument parsing uses std.
Read/validate strict JSON matching `fixtures/dream_pilot_tasks.json`, maximum
10 target plus 4 regression cases, exact preregistered settings/arms, task IDs
unique, nonblank oracles and the fixed FINAL-line output/scoring contract. Changed cases or
settings require a new preregistration, not an unnoticed command-line override.

Preserve the existing caller-approved safety/system identity. The fixture's base
prompt is fixed before outputs are observed. Model-generated hypotheses, memory
retrieval and Witness evaluation are disabled. Run five arms through Lucid:
reproduction, baseline, two named interventions and placebo. One sample per
repeat; three repeats. Each target is one independent Lucid experiment with no
attached regression cases. Four regression tasks are separate Lucid experiments
with the same five arms/settings; this avoids rerunning all regressions for each
target and exceeding the budget. Pool them only in the preregistered regression
guard metric. This measures population-level guard cases, not per-target repair
regressions. Do not apply any returned repair.

No preliminary live failure screening. Trace/source records identify a
preregistered fixture candidate, not a fabricated observed model failure:
provider `preregistered-fixture`, model `not-model-output`, unknown versions,
source run `prereg:<case-id>`, and a note that failure is unestablished. The fresh
reproduction arm provides actual failure measurement. Report all targets including
ones reproduction passes; never select/replace cases after seeing outputs.

Run targets then regressions in fixture order; controller's existing arm order
is fixed. Record that time/temperature/order confounding remains a pilot limitation.
Sequential requests only; cool down 45 seconds between experiments. Per-call
timeout 60 seconds, per-experiment timeout 600 seconds (existing controller cap),
global elapsed cap 150 minutes including pauses, hard inference ceiling 224.
On cancellation/error/timeout, retain partial results and stop, without retries.
Discover readable coretemp sensors under `/sys/class/hwmon/hwmon*/` by `name`;
read `temp1_input` before every call and record millidegree readings. Use the
maximum if multiple coretemp packages are discovered. Stop at >=95C, pause at
>=90C and poll every 15 seconds until below 90C, including during cooldown.
Unreadable/missing telemetry stops generation; elapsed deadline includes pauses.
The provider's async generation method invokes this runner-supplied pre-call
guard, so internal Lucid repair calls are covered too. Temperature polling and
thermal waiting occur inside the call deadline; no generation is sent on guard
failure. Sensor discovery/name/temp files are the only extra filesystem reads;
no model/config/git file reads occur in the runner (git revision is injected at
compile time). No thermal hardware settings are changed.

Persist a manifest (git revision, prereg/task file fingerprints, metadata,
endpoint/model/settings/seed policy/start time), per-case report JSON immediately
after each experiment, aggregate report JSON and a short Markdown summary.
Update artifacts atomically, restrict output files/directory permissions on Unix,
and include aborted/incomplete status, call counts, latency and token diagnostics
where available. Default build/test never sends HTTP to real Ollama. Only a later
explicit runner invocation generates live outputs.

## Offline verification plan and review gates

- Pure request-builder tests: exact temperature/seed/token limit, think/stream
  flags, exactly two messages, endpoint rejections, metadata parsing.
- Mock local HTTP tests: redirect/HTTP error/invalid JSON/missing model or version,
  response length bounds, truncation telemetry, one-use engines; no live Ollama needed in CI.
- Controller tests: unique per-call seeds across repeats/arms/roles, reproducible
  mapping, provenance seed equals sent seed, fixed default unchanged, mismatched
  temperature/tokens/profile/derived seed still rejected.
- Runner offline tests: strict fixture admission, budgets 210+14, preregistered FINAL-line scoring,
  fixed primary contrast, expected-pass/ceiling regression guard, thermal discovery/
  pre-call guards/thresholds, abort/partial output, overwrite refusal.
- Run fmt, offline locked workspace tests and strict all-target Clippy after
  implementation; send implementation diff for review before committing.

Review this design plus `DREAM_PILOT_PREREG.md` and the frozen cases first. Then
implement offline only. Commit the reviewed preregistration/implementation before
requesting permission for any live run. No pushing, serving, production writes,
or external communications beyond the authorized Claude mailbox are involved.
