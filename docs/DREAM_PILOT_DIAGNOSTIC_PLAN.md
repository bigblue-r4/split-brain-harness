# Proposed determinism diagnostic — not authorized to run

Status: offline proposal for Claude/Chris review. **At most ten generation calls**;
no calls or diagnostic implementation have been made for this proposal. Ollama
remains stopped as reported by Claude. Require Chris's explicit live-run go after
review of the diagnostic implementation. Do not change registered pilot files,
re-run its task set, or treat diagnostic outputs as additional pilot samples.

## Question and fixed input

Determine whether a small set of deliberately different seed/temperature options
produces any observable output variation on the same prompt. This is a behavior
check, not a causal identification of the backend internals or a benefit test.

Use the original pilot's target-01 **clean-baseline** system and user payload,
including its working/FINAL contract and no intervention, trace or conversation.
Before generation, reconstruct that payload offline from the committed fixture
and controller, and verify the system-prompt fingerprint against the saved
baseline call. Freeze/hash both exact UTF-8 strings in the diagnostic manifest.
Do not edit the prompt if outputs turn out to be identical.

Keep model name/digest, Ollama version, token cap 256, `think:false`, `stream:false`,
two-message fresh context, and every other option identical. Record server
metadata before and after. Use sequential fresh one-use engines and no warmup
generation; metadata requests do not count as generation. Do not pull/update the
model, restart services, change sampling defaults or alter cache settings midway.

## Ten-call matrix and order

For the separately authorized live run, Claude starts Ollama with `OLLAMA_DEBUG=1`
from startup and never changes it midway. Save its server log in the new private
output directory and record the environment in the manifest. This differs from
the pilot's INFO-level configuration. Offline inspection of the pilot server log
by Claude found no option/seed warnings; INFO logging is weak evidence because it
does not show per-request sampling parameters. After the diagnostic, inspect both
server/runner logs offline and report whether received seed and temperature are
visible for each request. Missing log detail is missing evidence, not proof of
option omission or a backend defect. Logs are evidence, not a diagnosis.

| Condition | Temperature | Seed field in actual request | Repetitions |
|---|---:|---|---:|
| A | 0.7 | 1 | 2 |
| B | 0.7 | 2147483646 | 2 |
| C | 0.7 | omitted entirely | 2 |
| D | 1.0 | 1 | 2 |
| E | 0.7 | 4294967297 (= 1 + 2^32) | 2 |

Fixed order: A, B, C, D, E, A, B, C, D, E. A/B tests widely separated lower seed
bits rather than only consecutive large seeds. A/E holds the low 32 bits equal
while changing a high bit; it is a diagnostic contrast, not an assumption that
truncation occurs. C omits seed rather than sending null, zero or a fabricated
recorded seed. D changes temperature while holding A's seed fixed.

The current research provider correctly requires a seed for the registered pilot.
A separately reviewed diagnostic request builder must permit explicit omission
only for this experiment and retain `seed: null` in diagnostic metadata to denote
absence; actual JSON must lack `options.seed`. Do not weaken the pilot's required
seed policy or spoof its telemetry. Test all five request shapes offline before
asking for execution approval. Save request bodies (synthetic inputs only) so
reported settings are not inferred solely from provenance.

## Evidence and interpretation fixed in advance

Save raw assistant answer UTF-8, SHA-256, exact request JSON, options, context ID,
model/version/digest, response model/done_reason, token counts, timings and
temperatures for every attempt, including failures. Compare full answer bytes
and parsed FINAL integers separately. Earlier working can vary while the final
answer remains the same; report both, without changing the pilot's scorer.

- Variation within a condition demonstrates observed variability in that
  condition; it does not prove independent sampling.
- Different consistent A/B outputs show an observable seed-condition contrast;
  identical A/B outputs do not prove that seeds are ignored.
- Different A/E outputs would motivate examining high-bit handling; identical
  A/E outputs do not establish 32-bit truncation. No backend cause is diagnosed
  from ten calls alone.
- Differences at C or D identify conditions worth investigating, not proof that
  seed or temperature alone caused the original invariance.
- If all ten outputs are identical, report that fact as a bounded negative
  diagnostic, not proof of greedy sampling, caching or option noncompliance.
  If transport/metadata/thermal failures prevent ten completions, report partial
  evidence and stop. Do not choose a new prompt, seed or additional calls.

Only after reviewing these results should an offline source/configuration audit
of the exact Ollama version be considered to distinguish implementation causes.
Do not speculate that a specific bug exists or retroactively adjust pilot n.

## Limits and stop rules

Hard cap ten attempted generation calls, no retries or repair generation. Reuse
the preregistered thermal rules: coretemp before each call, pause at >=90C,
stop at >=95C or unreadable telemetry. Keep 60s per-call deadlines and cancellation;
use a 20-minute overall cap including pauses and a 45-second cooldown between
the two five-call blocks. Stop on transport/provider error, timeout, cancellation,
metadata drift, or incomplete response. A completed truncated response is kept
and flagged, never retried. Planning assumption from the pilot's recorded
~11.5s mean server duration is a few minutes for ten calls, but condition changes
may alter length/latency; this is not a throughput guarantee.

Write a new explicitly supplied private output directory; preserve the original
pilot archive read-only. No push, model publication, serve/CLI/decision-log
changes or production memory writes. Return the diagnostic diff for review
before any commit or live action. This document is a proposal, not a GO.
