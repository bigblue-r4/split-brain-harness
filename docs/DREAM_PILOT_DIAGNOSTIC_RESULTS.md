# Ten-call diagnostic results

The diagnostic completed all ten calls without recorded errors. Different explicit
seeds A/B produced different answer bytes, while the two omitted-seed C calls
also differed. Repeated explicit-seed conditions A, B, D and E were internally
byte-identical; E was also byte-identical to A. These observations support further
investigation of seed handling. They do not establish the cause of the original
pilot's identical repeats or change its registered result: **no demonstrated
primary benefit**.

## Evidence and verification

Source artifacts, read offline:

- `~/archives/dream-pilot/diagnostic-2026-10-15/diagnostic.json`
- `~/archives/dream-pilot/diagnostic-2026-10-15/ollama-serve.log`
- `~/archives/dream-pilot/diagnostic-runner.log`
- Original pilot: `~/archives/dream-pilot/run-2026-10-14/report.json`

Diagnostic JSON SHA-256:
`783efde61b8ede37addb2192bcfa815e0d75dcfbb0c72132cc21dd46eedffd2e`.
Server log SHA-256:
`1bbaf3337becda8b54ea1aff63486425640a8e74d7d26d822ae1cd566a9b8512`.
Runner log SHA-256:
`60eab7d5a97b4d946ba122387774e79e5c88a1235e13eebae24af0f07d42ef03`.

The write-up independently recomputed every answer SHA-256 and strict FINAL
parse, checked saved seed fields against requests (including C's omitted field),
compared all ten message pairs and answers, and checked hashes of the saved
system/payload strings. No model call or service change was made for this analysis.

Implementation: `8d71765471dc6c28b840c7ba153cef98c6ad1b34`.
Plan: `d439e4a40644ab3aea63ef7fe6df49292ef3e738`.
Ollama: `0.24.0`; model: `llama3.2:3b`; digest:
`a80c4f17acd55265feec403c7aef86be0c25983ab279d83f3bcd3abbcb5b8b72`.
Start: `2026-10-15T23:14:09Z`; final metadata check:
`2026-10-15T23:15:45Z` (96 seconds between these timestamps). Metadata before
and after matched the pilot. All responses were complete with `done_reason=stop`;
no truncations or retries were recorded. Temperatures were 42–59C.

## Per-call observations

All calls used the same reconstructed target-01 clean-baseline system/user
messages, `think:false`, `stream:false` and a 256-token ceiling. Temperature 0.7
is serialized from f32 as `0.699999988079071`; D's value is `1.0`.
Hash prefixes below are twelve hexadecimal characters of the independently
verified full answer SHA-256 values retained in the JSON.

| Call | Condition | Seed field | Temperature | Answer hash prefix | Parsed FINAL | eval_count |
|---:|---|---:|---:|---|---:|---:|
| 1 | A | 1 | 0.7 | 54bb2c8c13c8 | 323 | 35 |
| 2 | B | 2147483646 | 0.7 | 1443ae5a405a | 323 | 34 |
| 3 | C | omitted | 0.7 | ba1d7bedec04 | 646 | 67 |
| 4 | D | 1 | 1.0 | f57845ec8556 | unparsed | 36 |
| 5 | E | 4294967297 | 0.7 | 54bb2c8c13c8 | 323 | 35 |
| 6 | A | 1 | 0.7 | 54bb2c8c13c8 | 323 | 35 |
| 7 | B | 2147483646 | 0.7 | 1443ae5a405a | 323 | 34 |
| 8 | C | omitted | 0.7 | e4dd403c595c | 323 | 35 |
| 9 | D | 1 | 1.0 | f57845ec8556 | unparsed | 36 |
| 10 | E | 4294967297 | 0.7 | 54bb2c8c13c8 | 323 | 35 |

D's answer ends with `FINAL: 323.`. The trailing period makes it malformed under
the fixed parser, despite the visible arithmetic answer being 323. There were
two malformed FINAL lines, no missing responses and no truncated responses.
Eight answers parsed successfully; seven had the oracle value 323 and one had
646. These are diagnostic observations, not additional pilot measurements.

## Comparisons and bounded interpretation

- A differs byte-for-byte from B, although both end with valid 323. The seed
  condition has an observable output contrast at the recorded temperature.
- C's two answers differ both in bytes and FINAL value (646 versus 323). This
  demonstrates output variation with the same recorded messages/options when
  seed is omitted. A universally fixed response for those requests is not
  observed; the result does not prove a particular sampler or rule out every
  cache mechanism.
- A, B, D and E each repeat identically for their two calls. This is consistent
  with reproducibility for these explicit-seed conditions, not proof of general
  determinism or independent sampling.
- D differs from A with seed 1 held fixed and requested temperature changed to
  1.0. Its FINAL format fails. This is a condition contrast, not evidence that
  higher temperature improves answers.
- E equals A in the full answer bytes, not just the parsed number. Their seeds
  differ by 2^32, so this is consistent with low-32-bit seed reduction. Two
  conditions yielding the same text do not prove truncation or its location.
- **None of the ten diagnostic answers is byte-identical to the pilot's saved
  target-01 clean-baseline answer.** This comparison used the actual answer
  strings, not only their hashes or parsed integers.

The original pilot's repeat seeds remain distinct after masking to 32 bits and
also after masking to 31 bits, in all 70 arm/task cells. None exceeds the signed
63-bit range. Therefore bit truncation alone cannot have mapped each pilot
cell's three seeds to one value; E matching A does not by itself explain those
70 identical-repeat cells.

## Float64 candidate mechanism — arithmetic verified, backend path unverified

Converting the original pilot's integers to IEEE-754 binary64 in an offline
calculation maps all three seeds to the same floating-point value in **70/70**
cells. For example, target-01 baseline seeds:

```text
6887878990344476218 -> 6887878990344476672
6887878990344476219 -> 6887878990344476672
6887878990344476220 -> 6887878990344476672
```

If a backend decodes JSON numeric options into float64 before converting seed
to an integer, that path could collapse each cell's consecutive large seeds.
The small diagnostic seeds do not have that precision problem. Such a path is
consistent with both datasets, but it has **not been checked against Ollama
0.24.0 source** and is not established as the cause. Other backend paths remain
unexamined. A separate offline audit of the exact version's options decoding and
seed conversion is the next proposed check; no source audit is claimed here.

Recommendation only, not implemented: a future seed policy should keep seeds
within the exactly representable integer range (at most 2^53) and ensure distinct
values modulo 2^32 within each experiment. This would avoid these two candidate
representational collisions, not guarantee independent outputs or backend
compliance. It requires separate review and does not amend the completed pilot.

## Log evidence and limits

The server startup log reports `OLLAMA_DEBUG:DEBUG`, and subprocess entries show
`OLLAMA_DEBUG=1`. This is a documented configuration difference from the pilot's
INFO logging. A case-insensitive search for `seed|temperature|sampl` returned
**zero matches in both server and runner logs**. Thus neither log evidences the
seed/temperature actually received for any of the ten calls. The JSON records
client request settings, not proof of receipt, decoding or sampling behavior.

Only the reconstructed system prompt was checked against the pilot's saved
fingerprint. The pilot did not record a user-payload fingerprint, so historical
payload equality cannot be verified. The diagnostic's own ten message pairs
are identical, and its saved payload SHA-256 is internally verified, but this
does not fill the missing historical evidence. Debug configuration and execution
time also differ between runs.

The 90C thermal pause occurs inside the 60-second per-call deadline. A sustained
pause could abort the diagnostic rather than wait indefinitely. It did not occur
here: recorded temperatures stayed below 90C. Ten calls on one prompt with two
repeats per condition provide a narrow behavior check, not a diagnosis or a new
benefit trial. No source fix, seed-policy change, new pilot or live call is
authorized by this write-up. Registered files and original artifacts are unchanged.
