# Seed-collapse audit and future research seed policy

The seed-collapse mechanism is established at source level for **Ollama v0.24.0
with llamarunner**, the path used by the saved pilot and diagnostic. Large JSON
seed integers lose precision during request decoding before conversion to a
32-bit sampler seed. Other versions and execution engines have not been audited
for equivalent behavior. The registered pilot decision remains **no demonstrated
primary benefit**; this is not a reanalysis or an additional live experiment.

## Pinned source chain

References use Ollama tag `v0.24.0`, commit
`c28ddc0a7b273cd286b680a6db0bef0c17bc0ec0`. Line numbers were checked against
raw files at that commit, rather than a moving branch or rendered-text offsets.

1. [server/routes.go:2178–2182](https://github.com/ollama/ollama/blob/c28ddc0a7b273cd286b680a6db0bef0c17bc0ec0/server/routes.go#L2178-L2182)
   binds ChatHandler's request with `ShouldBindJSON`. The chat request's options
   field is `map[string]any` at
   [api/types.go:147–168](https://github.com/ollama/ollama/blob/c28ddc0a7b273cd286b680a6db0bef0c17bc0ec0/api/types.go#L147-L168).
2. The dependency is Gin v1.10.0
   ([go.mod:8](https://github.com/ollama/ollama/blob/c28ddc0a7b273cd286b680a6db0bef0c17bc0ec0/go.mod#L8)).
   Its JSON binding defaults `EnableDecoderUseNumber` to false and invokes
   UseNumber only when enabled
   ([binding/json.go:19,42–52](https://github.com/gin-gonic/gin/blob/v1.10.0/binding/json.go#L19-L52)).
   A read-only scan of all Go files in the pinned Ollama source archive found no
   override or UseNumber invocation. Standard JSON decoding therefore represents
   numeric values in these interface-valued options as float64, rather than
   retaining the incoming integer's precision.
3. Request options reach model option conversion through
   [server/routes.go:2372](https://github.com/ollama/ollama/blob/c28ddc0a7b273cd286b680a6db0bef0c17bc0ec0/server/routes.go#L2372),
   [168](https://github.com/ollama/ollama/blob/c28ddc0a7b273cd286b680a6db0bef0c17bc0ec0/server/routes.go#L168)
   and [128–140](https://github.com/ollama/ollama/blob/c28ddc0a7b273cd286b680a6db0bef0c17bc0ec0/server/routes.go#L128-L140).
   Options.Seed is an int at
   [api/types.go:587](https://github.com/ollama/ollama/blob/c28ddc0a7b273cd286b680a6db0bef0c17bc0ec0/api/types.go#L587).
   Options.FromMap's reflect.Int branch converts float64 to int64 at
   [976,1003–1009](https://github.com/ollama/ollama/blob/c28ddc0a7b273cd286b680a6db0bef0c17bc0ec0/api/types.go#L976-L1009).
   Conversion cannot recover bits already lost in decoding.
4. The audited runner narrows the integer to uint32 at
   [runner/llamarunner/runner.go:655](https://github.com/ollama/ollama/blob/c28ddc0a7b273cd286b680a6db0bef0c17bc0ec0/runner/llamarunner/runner.go#L655).
   The native sampler assignment is C.uint32_t at
   [llama/llama.go:658](https://github.com/ollama/ollama/blob/c28ddc0a7b273cd286b680a6db0bef0c17bc0ec0/llama/llama.go#L658).

This gives the effective conversion chain: JSON integer -> float64 -> Go integer
-> uint32 sampler seed. The precision loss and narrowing are separate operations.

## Which runner executed the model

Saved logs, read without starting a service:

- `~/archives/dream-pilot/ollama-serve.log`: model command at line 82 lacks
  `--ollama-engine`; line 88 identifies the go runner.
- `~/archives/dream-pilot/diagnostic-2026-10-15/ollama-serve.log`: model command
  at line 356 likewise lacks that flag; line 365 identifies the go runner.

The go-runner log message is emitted by llamarunner at
[runner.go:965](https://github.com/ollama/ollama/blob/c28ddc0a7b273cd286b680a6db0bef0c17bc0ec0/runner/llamarunner/runner.go#L965).
Model launch adds `--ollama-engine` only for the other engine selection at
[llm/server.go:357–359](https://github.com/ollama/ollama/blob/c28ddc0a7b273cd286b680a6db0bef0c17bc0ec0/llm/server.go#L357-L359).
The earlier flag-bearing launches in both logs are discovery launches without
the model argument; they are not the sampled model execution above.
The separate ollamarunner passes Seed to NewSampler without this explicit
uint32 cast at
[runner/ollamarunner/runner.go:897–902](https://github.com/ollama/ollama/blob/c28ddc0a7b273cd286b680a6db0bef0c17bc0ec0/runner/ollamarunner/runner.go#L897-L902).
Its full behavior is outside this audit; do not generalize the llamarunner result.

## Reconciliation with the saved evidence

The pilot's three consecutive 64-bit seeds round to one float64 value in every
one of its 70 arm/task cells. Target-01 baseline illustrates the loss:

```text
6887878990344476218 -> 6887878990344476672
6887878990344476219 -> 6887878990344476672
6887878990344476220 -> 6887878990344476672
```

The resulting equal integers then narrow identically to uint32. This source path
explains why distinct recorded requests did not yield distinct effective sampler
seeds. Merely truncating the original unrounded integers would not explain it:
their low 32 and low 31 bits were distinct within every cell.

Diagnostic E's seed 4294967297 is exactly representable in float64 but narrows
to uint32 value 1, matching A. Its byte-identical output is consistent with the
same audited path. The log audit still does not provide per-request received
parameters; this conclusion is based on source and offline numeric conversion,
not newly observed live server instrumentation. No sampling-independence or
universal determinism claim follows from fixing seed representation.

## Future-only implementation

DistinctPerCall now keeps the low 32 bits of the deterministic run/base-seed hash
plus the call ordinal: `(run_base + ordinal) mod 2^32`. Adding consecutive indices
modulo 2^32 is injective over any interval shorter than 2^32, even across wraparound.
Research runs are capped at 10,000 calls, far below that period. Run identity and
base seed still determine the starting value; replaying the same inputs gives
the same sequence. No cross-experiment uniqueness guarantee is added.

Every derived seed is <2^32, hence exactly representable in float64 and unchanged
by uint32 conversion. The research provider rejects absent seeds and values
>=2^32 rather than silently narrowing them. Full requested/session settings
checks remain intact, and the JSON request, telemetry and provenance keep the
same seed. Fixed-policy semantics for other providers are unchanged; the research
provider's stricter boundary also applies to explicitly supplied fixed seeds.

The already-approved diagnostic is a deliberate exception, using its own fixed
request builder to retain condition C's omission and E's high-bit probe. Its
matrix and historical results are unchanged; that builder cannot relax the
research provider's admission rules.

Offline tests cover 225 call positions (224 plus an extra repair reservation),
the full 10,000-call controller limit, multiple base seeds, wraparound, float64
round-trip and uint32 identity, provider boundary/missing-seed rejection, and
mock HTTP equality between sent seed, provenance and telemetry. No new live call,
model service change or study was performed. Historical preregistration, fixture
and results documents remain unchanged. A future rerun needs a new preregistered
study and Chris's separate go; the original pilot decision is not amended.
