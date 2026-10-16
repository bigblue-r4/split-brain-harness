# Phase 2a results: no demonstrated primary benefit

The registered decision is **no demonstrated primary benefit**. Decompose scored
18/30 target answers versus placebo's 24/30: a gain of -6 correct answers, or
-20 percentage points. It improved on only one target, below the preregistered
minimum of three, and scored below baseline's 24/30. The regression guard passed
in aggregate (both decompose and placebo 9/12), which does not rescue the failed
primary criteria. Verify is secondary and does not replace the primary contrast.

All three repeats produced byte-identical answer strings in every one of the
70 arm-by-task cells. Keep the registered denominators/decision, but do not treat
these as three informative independent replications: there is only one distinct
observed answer per cell. This is a major validity limitation, not a reason to
change the registered analysis or reinterpret the result as positive.

## Evidence and run identity

Source: `~/archives/dream-pilot/run-2026-10-14/report.json`, cross-checked against
`summary.md`. The report's SHA-256 is
`f667e103236b861e06867f502dc833801543c37c11c3afd1455bcb8fd630944b`.
Scores, repeat identity, recorded seed matches, format failures and truncations
were recomputed offline from the report. No diagnostic/model calls were made
for this write-up.

- Registration: `2fea927176ecaa3337afc716d0685d792ad5b323`.
- Implementation: `cbb17f4b669d751a6b54110d8c10dadb5bfa3163`.
- Ollama version: `0.24.0`; model: `llama3.2:3b`.
- Model digest: `a80c4f17acd55265feec403c7aef86be0c25983ab279d83f3bcd3abbcb5b8b72`.
- Status: completed, 14/14 experiments; 210 measurement calls plus one optional
  repair-description call = 211 attempted/sent requests. All telemetry entries
  have completed responses. No repair was applied.
- Recorded start: `2026-10-14T21:41:26Z`; final metadata check:
  `2026-10-14T22:31:38Z` (50 minutes 12 seconds between these timestamps).
- Recorded temperatures: 51–81C, below the 90C pause and 95C stop thresholds.
  Fifteen recorded metadata rechecks completed without aborting the run.

## Per-task exact registered scores

Each cell is correct answers out of three scheduled repeats. A score of 3/3
represents one answer repeated identically, not three distinct solutions. The
oracle is the integer compared to the strict last-line FINAL parse; malformed
or truncated answers remain incorrect as registered.

| Case | Oracle | Reproduction | Baseline | Decompose | Verify | Placebo |
|---|---:|---:|---:|---:|---:|---:|
| target-01: 17 x 19 | 323 | 3/3 | 3/3 | 0/3 | 0/3 | 3/3 |
| target-02: 27 x 14 | 378 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 |
| target-03: 48 x 25 - 37 | 1163 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 |
| target-04: 37 + 48 + 56 - 29 | 112 | 3/3 | 3/3 | 3/3 | 0/3 | 3/3 |
| target-05: marbles | 29 | 3/3 | 3/3 | 3/3 | 0/3 | 3/3 |
| target-06: three quarters of 52 | 39 | 3/3 | 3/3 | 0/3 | 3/3 | 3/3 |
| target-07: 97 remainder 8 | 1 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 |
| target-08: i in mississippi | 4 | 0/3 | 0/3 | 3/3 | 0/3 | 0/3 |
| target-09: r in strawberry | 3 | 0/3 | 0/3 | 0/3 | 0/3 | 0/3 |
| target-10: t in committee | 2 | 3/3 | 3/3 | 0/3 | 3/3 | 3/3 |
| **Targets** | | **24/30** | **24/30** | **18/30** | **15/30** | **24/30** |
| regression-01: 2 + 2 | 4 | 3/3 | 3/3 | 0/3 | 3/3 | 3/3 |
| regression-02: 9 x 3 | 27 | 3/3 | 3/3 | 3/3 | 3/3 | 0/3 |
| regression-03: a in cat | 1 | 0/3 | 3/3 | 3/3 | 3/3 | 3/3 |
| regression-04: 12 - 5 | 7 | 3/3 | 3/3 | 3/3 | 0/3 | 3/3 |
| **Regression guard** | | **9/12** | **12/12** | **9/12** | **9/12** | **9/12** |

Decompose improved over placebo on target-08 and lost on targets 01, 06 and 10.
The remaining six target contrasts tied. Only targets 08 and 09 failed the
reproduction arm; no tasks were screened out after these observations. Equal
regression totals conceal different failed tasks: decompose failed regression-01,
whereas placebo failed regression-02.

The sole repair-description call followed target-08. Its content admission was
rejected, no repair was returned, and the completed measurements were retained
under the pre-run clarification. The trace records a generic content-admission
rejection; it does not identify whether schema, privacy, size or oracle admission
was the particular cause. No further cause is inferred here.

## Identical repeats and recorded settings

Within each of 14 tasks x five arms, all three `final_answer` strings are
byte-identical after decoding JSON and encoding UTF-8, including working and
formatting. This is stronger than merely obtaining the same integer/outcome.
There is no observed within-cell output variation in any of the 70 cells.
For descriptive distinct-answer counts, the target totals correspond to
reproduction 8/10, baseline 8/10, decompose 6/10, verify 5/10 and placebo 8/10;
these do not replace the registered 30-answer denominators or establish that
the ten tasks themselves are statistically independent.

All 211 recorded call seeds are distinct, and each call's provenance seed equals
the corresponding telemetry seed. Within each arm's repeats, seed values differ;
temperature is recorded as 0.7 and the token ceiling as 256. The adapter's
request construction carries these settings, but these saved artifacts are not
a packet capture or proof that the backend honored them. No cause of the output
invariance has been established. Seed handling, sampling and caching behavior
require separate diagnosis; this report attributes the pattern to none of them.

## Format failures and truncation

Counts cover all 42 measured responses per arm (targets plus regression guard).
The diagnostic repair description is excluded from these counts.

| Arm | Malformed/missing FINAL | Truncated (`done_reason=length`) |
|---|---:|---:|
| Reproduction | 6 | 0 |
| Baseline | 0 | 0 |
| Decompose | 3 | 0 |
| Verify | 12 | 3 |
| Placebo | 3 | 0 |
| **Total** | **24** | **3** |

The three truncations were verify on target-05, in all three repeats. They also
had malformed/missing FINAL lines, so the two totals overlap; do not count 27
distinct failing responses. These counts preserve the registered scorer, rather
than extracting a favorable number from working or ignoring truncation.

## Limits and next question

This small fixed-order, single-model, single-machine pilot found no demonstrated
primary benefit under its registered rules. It does not establish that Lucid can
never help, or that any particular sampling/seed implementation is defective.
Most candidate targets already passed reproduction, leaving substantial ceiling
effects. Regression tasks were standalone population guards, not validation of
each individual repair. The non-varying repeats supply no observed replication
gain; thresholds are descriptive, with no significance or generalization claim.
Formatting/truncation penalties are part of the registered outcome, not purely
an arithmetic-ability measure. Order/time effects and shared approved identity
remain limitations. No live Witness or memory arm was involved.

The next proposed action is the separately reviewed, ten-call maximum diagnostic
in [DREAM_PILOT_DIAGNOSTIC_PLAN.md](DREAM_PILOT_DIAGNOSTIC_PLAN.md). It is not a
rerun of this pilot, does not change its result, and needs Chris's explicit go.
