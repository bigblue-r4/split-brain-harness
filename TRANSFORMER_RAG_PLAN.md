# Split-Brain Harness Transformer / RAG Completion Plan

Date: 2026-06-21 HST
Project: `/home/evillab/Desktop/split-brain-harness`

## Observed current state

- Rust project: `split-brain-harness`.
- Current pipeline:
  1. Wrap raw input with embedded `soul.md` prompt.
  2. Ask a model to produce structured telemetry JSON.
  3. Run deterministic verification.
  4. Optionally run a second LLM verification pass.
- Existing backends:
  - `ollama-native`
  - `openai-compat`
  - `anthropic`
  - `local-embedded` stub only.
- Current tests pass: 23/23 when run with Rust toolchain path added.
- The default soul is already compile-time embedded with `include_str!("../soul.md")`.

## Goal

Make the split-brain reason tool usable as a model-side transformer/adaptor that can be added to Ollama or another model runner, with hard-coded reasoning/RAG context where useful.

Plain target shape:

```text
user input
  -> split-brain transformer layer
  -> hard-coded soul/context/RAG injection
  -> selected model backend
  -> structured telemetry JSON
  -> verifier
  -> final safe output
```

## Recommended architecture

Use three layers.

### 1. Core harness library

Keep the existing Rust library as the source of truth.

Responsibilities:

- Load embedded or external soul.
- Load hard-coded context packs.
- Build model prompts.
- Normalize model output into the existing JSON schema.
- Verify output.

Do not tie this layer to Ollama only.

### 2. Transformer/adaptor layer

Add a new module named `transformer`.

Suggested files:

```text
src/transformer.rs
src/context_pack.rs
src/rag.rs
src/backends/embedded.rs   # replace current stub later
```

Suggested public API:

```rust
pub struct SplitBrainTransformer {
    soul: Soul,
    context_pack: ContextPack,
    policy: TransformPolicy,
}

impl SplitBrainTransformer {
    pub fn transform_prompt(&self, input: &str) -> String;
    pub fn transform_system(&self) -> String;
    pub fn postprocess(&self, raw: &str) -> Result<TelemetryResult>;
}
```

This lets Ollama, OpenAI-compatible endpoints, Anthropic, or a future embedded model all use the same prompt construction and output cleanup.

### 3. Runner integrations

Support these in order:

1. **Ollama Modelfile wrapper** — fastest path.
2. **OpenAI-compatible proxy/server** — best general integration.
3. **True local embedded backend** — later, heavier work.

## Ollama path: fastest usable version

Create a generated Ollama `Modelfile` that hard-codes the split-brain system prompt.

Example target:

```text
FROM llama3.2:3b
PARAMETER temperature 0.1
PARAMETER num_predict 600
SYSTEM """
[embedded split-brain logic system prompt]

[embedded compact RAG/context pack]

Return only valid JSON matching the TelemetryResult schema.
"""
TEMPLATE """
{{ if .System }}<|system|>
{{ .System }}{{ end }}
<|user|>
<payload>
{{ .Prompt }}
</payload>
<|assistant|>
"""
```

Commands after implementation:

```bash
split-brain-harness export-ollama --base llama3.2:3b --name split-brain:latest
ollama create split-brain:latest -f Modelfile.split-brain
ollama run split-brain:latest "test input"
```

Add CLI command:

```bash
split-brain-harness export-ollama \
  --base llama3.2:3b \
  --output Modelfile.split-brain \
  --context context/core.toml
```

This does not require training. It is hard-coded prompt/RAG injection.

## Hard-coded RAG/context pack

Use a static context pack first, not vector search first.

Suggested folder:

```text
context/
  core.toml
  manipulation_patterns.toml
  coherence_rules.toml
  output_schema.toml
```

Suggested data model:

```toml
[name]
value = "split-brain-core-v1"

[[facts]]
id = "schema.telemetry"
text = "TelemetryResult has affective_telemetry, intent_matrix, and cognitive_state."

[[rules]]
id = "risk.urgency"
text = "High urgency with low manipulation_risk is suspicious and must be checked."
```

At compile time, default packs can be embedded with `include_str!`.
At runtime, external packs can be loaded with `SBH_CONTEXT_PATH`.

Recommended rule:

- Keep the context pack short.
- Prefer stable definitions, schemas, and verification rules.
- Do not put private user data in hard-coded model files.

## Optional real RAG later

After the hard-coded pack works, add real retrieval.

Recommended local stack:

- `sled` or `sqlite` for local storage.
- `fastembed` or Ollama embeddings for vectors.
- Top-k retrieval with token budget.
- Retrieved snippets inserted into the transformer prompt under `<retrieved_context>`.

But this is phase 2. The first version should be static and deterministic.

## Completion phases

### Phase 1 — stabilize current harness

- Add `cargo fmt` and `cargo clippy` checks.
- Add integration test with a mock backend.
- Add clear error when Ollama is unreachable.
- Add CLI `--config-check`.
- Add documented PATH note for local Rust toolchain.

Done when:

- `cargo test` passes.
- `cargo fmt --check` passes.
- README documents Ollama setup.

### Phase 2 — add transformer layer

- Add `src/transformer.rs`.
- Move prompt construction out of `harness.rs` into transformer.
- Add unit tests for generated system and payload prompt.
- Add `TransformPolicy` with:
  - JSON-only mode.
  - max context chars.
  - include/exclude verifier context.

Done when:

- Existing backend behavior is unchanged.
- Tests prove prompt output is deterministic.

### Phase 3 — add hard-coded context/RAG pack

- Add `src/context_pack.rs`.
- Add default embedded context pack.
- Add `SBH_CONTEXT_PATH` runtime override.
- Insert context into prompt as `<context_pack>...</context_pack>`.
- Add tests for embedded and external context loading.

Done when:

- Binary works with no external files.
- External context can override or extend default context.

### Phase 4 — add Ollama export

- Add CLI subcommand `export-ollama`.
- Generate Modelfile from base model + embedded system prompt + context pack.
- Add README section for `ollama create`.
- Add smoke test that generated Modelfile contains required schema and prompt sections.

Done when:

- User can create an Ollama model named `split-brain:latest`.
- `ollama run split-brain:latest` returns JSON matching schema.

### Phase 5 — add OpenAI-compatible proxy mode

Add a small HTTP server mode:

```bash
split-brain-harness serve --listen 127.0.0.1:8088
```

Endpoints:

```text
POST /v1/chat/completions
POST /analyze
GET  /health
```

Purpose:

- Let any OpenAI-compatible client use the transformer.
- Keep split-brain verification outside the base model.

Done when:

- LM Studio, OpenWebUI, or other OpenAI-compatible clients can call it.

### Phase 6 — true local embedded backend

Only after the above works.

Options:

1. `llama.cpp` binding through `llama-cpp-2` or external process.
2. Candle-based Rust inference.
3. Keep using Ollama as the local model runner.

Recommendation: keep Ollama as the local runner unless there is a strong reason to embed weights directly. True embedded inference adds build and GPU complexity.

## Suggested implementation order

1. Add transformer module.
2. Add context pack module.
3. Add Ollama Modelfile export.
4. Add mock backend integration tests.
5. Add OpenAI-compatible server.
6. Consider true embedded inference last.

## Risks

- Hard-coding too much context can reduce model quality and increase prompt brittleness.
- Ollama Modelfile system prompts are not private; do not embed secrets.
- True embedded inference will complicate install size, GPU support, and build reproducibility.
- Verifier should remain outside the base model when possible, because independent verification is the point of the split-brain design.

## Recommendation

Build this as a transformer/adaptor, not as model fine-tuning.

Use Ollama Modelfile export for the first working version, then add an OpenAI-compatible proxy for general use. Keep the verifier in the Rust harness so the base model cannot silently bypass it.
