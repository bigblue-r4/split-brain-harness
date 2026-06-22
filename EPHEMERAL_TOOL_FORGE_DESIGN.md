# Ephemeral Tool Forge Design

Date: 2026-06-21 HST
Project: split-brain-harness

## Core concept

The model should not directly create, execute, or persist arbitrary tools.

Instead, the model may request a capability. A separate supervisor decides whether a temporary tool should be generated, verified, executed, measured, and destroyed.

Plain shape:

```text
user task
  -> split-brain reasoning layer
  -> capability request
  -> execution supervisor
  -> Rust/WASM tool generation
  -> verification gate
  -> sandboxed execution
  -> result returned
  -> environment destroyed
  -> capability memory updated
```

## Rule zero

The model never runs code.

The model may only emit a structured capability request.

Example:

```json
{
  "kind": "capability_request",
  "capability": "stream_parse_logs",
  "input_contract": "UTF-8 log lines from stdin",
  "output_contract": "JSON array of matching events",
  "constraints": {
    "no_network": true,
    "read_only_input": true,
    "max_runtime_ms": 1000,
    "max_memory_mb": 64
  },
  "reason": "Existing text reasoning is inefficient for repeated regex parsing."
}
```

## Separate reasoning from building

### Reasoning layer

Allowed:

- identify that a tool may help.
- describe capability needed.
- define input/output contracts.
- define constraints.
- explain why existing tools are insufficient.

Not allowed:

- execute code.
- decide its own permissions.
- bypass verification.
- persist generated binaries.

### Execution supervisor

Allowed:

- approve or reject capability requests.
- generate or ask another model to generate Rust source.
- verify source.
- compile to sandbox target.
- run in isolated environment.
- collect metrics.
- destroy environment.
- update capability memory.

## Capability manifest

Every generated tool needs a manifest.

Example:

```json
{
  "manifest_version": 1,
  "capability_id": "stream_parse_logs.regex.v1",
  "problem_signature": "vendor_logs_v2",
  "tool_kind": "ephemeral_rust_wasm",
  "input_contract": {
    "source": "stdin",
    "format": "utf8_lines",
    "max_bytes": 10485760
  },
  "output_contract": {
    "format": "json",
    "schema": "events[]"
  },
  "permissions": {
    "network": false,
    "filesystem_write": false,
    "filesystem_read": "sandbox/input_only",
    "process_spawn": false,
    "env_access": false
  },
  "limits": {
    "runtime_ms": 1000,
    "memory_mb": 64,
    "stdout_bytes": 1048576,
    "stderr_bytes": 65536
  },
  "verification_required": [
    "static_analysis",
    "dependency_scan",
    "policy_check",
    "unit_tests",
    "resource_estimate"
  ],
  "destroy_after_run": true
}
```

## Recommended execution target

Recommended first implementation:

```text
Rust -> WASM/WASI -> verification -> sandboxed execution
```

Why:

- no arbitrary native syscalls by default.
- permission model is clearer than native binaries.
- fast startup.
- easy disposal.
- reproducible execution.

Useful technologies:

- WebAssembly.
- WASI Preview 2.
- Wasmtime.
- wasm-tools.
- cargo-component.

Later options:

- Firecracker microVMs.
- gVisor.
- Bubblewrap.
- Linux namespaces/seccomp.

Recommendation: start with WASM/WASI before microVMs.

## Verification stage

Never execute first-pass generated code.

Required checks:

1. Source policy scan.
2. Dependency scan.
3. Static analysis.
4. Formatting/lint.
5. Unit tests.
6. Resource estimate.
7. Manifest permission match.
8. Deterministic build if possible.

Reject code containing or requesting:

- `unsafe` blocks.
- dynamic library loading.
- network access.
- filesystem writes outside sandbox.
- process spawning.
- shell execution.
- environment variable scraping.
- absolute host paths.
- hidden persistence.
- self-modifying behavior.
- dependency build scripts unless explicitly allowed.

Suggested Rust restrictions:

```text
#![forbid(unsafe_code)]
```

Suggested dependency rule:

- default: no third-party dependencies.
- allowlist only for audited crates.
- deny crates with build scripts unless reviewed.

## Execution lifecycle

```text
1. Receive capability request.
2. Check if task is worth tool generation.
3. Match against capability memory.
4. Generate source from trusted template/pattern.
5. Create manifest.
6. Verify source and manifest.
7. Compile Rust to WASM/WASI.
8. Run unit tests in sandbox.
9. Execute against user input in sandbox.
10. Capture output and metrics.
11. Destroy sandbox.
12. Store only fingerprint, pattern, constraints, and metrics.
```

## Destroy means destroy

Do not only kill the process.

Destroy:

- temporary filesystem.
- memory space.
- temporary credentials.
- mounted input copies.
- generated source unless policy allows retaining a redacted template.
- generated binary.
- logs containing secrets.

Keep only:

- manifest summary.
- problem signature.
- solution pattern.
- verification result.
- performance metrics.
- failure reasons.

## Capability memory

Do not store generated binaries as trusted tools.

Store fingerprints and patterns.

Example:

```json
{
  "problem_signature": "vendor_logs_v2",
  "solution_pattern": "regex_stream_parser",
  "input_shape": "utf8_lines",
  "output_shape": "json_events",
  "constraints": {
    "no_network": true,
    "read_only": true
  },
  "performance": {
    "runtime_ms_p50": 80,
    "runtime_ms_p95": 140,
    "success_rate": 0.99,
    "verification_score": 0.97
  },
  "last_verified_at": "2026-06-21T00:00:00Z"
}
```

Next time:

1. Match current task to prior problem signature.
2. Regenerate code from stored solution pattern.
3. Reverify.
4. Execute in fresh sandbox.
5. Update metrics.

This avoids trusting persistent binaries.

## Tool reputation system

Track generated tools by capability and pattern, not by binary.

```text
Capability -> Solution patterns -> Runs -> Metrics
```

Metrics:

- execution time.
- memory usage.
- failure rate.
- verification score.
- policy violations.
- parse success rate.
- user correction rate.
- fallback frequency.

Use reputation to choose patterns, not to skip verification.

High reputation means:

- better candidate pattern.
- faster approval path.
- more confidence in expected performance.

High reputation does not mean:

- execute without sandbox.
- execute without verification.
- persist binary forever.

## Multi-agent split-brain roles

Add specialized internal roles:

### Planner

Decides whether a tool is needed.

Output: capability request only.

### Tool architect

Converts capability request into a manifest and implementation plan.

### Code generator

Writes minimal Rust source.

### Security verifier

Checks source, manifest, dependencies, and permissions.

### Test generator

Creates unit tests and fixture tests.

### Execution supervisor

Compiles, runs, limits, destroys.

### Auditor

Writes capability memory and reputation metrics.

No single role should both generate and approve execution.

## Cost controls

Prevent infinite tool creation.

Add budgets:

```json
{
  "max_tools_per_task": 2,
  "max_regenerations": 1,
  "max_compile_seconds": 20,
  "max_total_runtime_seconds": 60,
  "max_memory_mb": 128,
  "require_user_approval_after_failures": 2
}
```

Use existing tools or direct reasoning when:

- task is one-off and simple.
- generation cost exceeds expected benefit.
- no clear input/output contract exists.
- permissions required are too broad.

## Suggested repository additions

```text
src/capability.rs          # request + manifest types
src/tool_forge.rs          # supervisor state machine
src/tool_memory.rs         # capability fingerprints/reputation
src/sandbox.rs             # WASM/WASI runner abstraction
src/policy.rs              # static policy checks
src/bin/sbh-forge.rs       # optional separate supervisor binary
fixtures/capabilities/     # test manifests and requests
```

## Suggested API types

```rust
pub struct CapabilityRequest {
    pub capability: String,
    pub input_contract: String,
    pub output_contract: String,
    pub constraints: CapabilityConstraints,
    pub reason: String,
}

pub struct CapabilityManifest {
    pub capability_id: String,
    pub problem_signature: String,
    pub permissions: Permissions,
    pub limits: ResourceLimits,
    pub verification_required: Vec<VerificationKind>,
    pub destroy_after_run: bool,
}

pub struct ToolRunReport {
    pub accepted: bool,
    pub verification_passed: bool,
    pub executed: bool,
    pub output: Option<String>,
    pub metrics: ToolMetrics,
    pub destroyed: bool,
    pub memory_update: Option<CapabilityMemoryRecord>,
}
```

## Suggested phases

### Phase 1 — design-only integration

- Add capability request schema.
- Add model prompt section that allows capability requests.
- Add tests that model output can be parsed as a request.
- No code generation yet.

### Phase 2 — mock tool forge

- Supervisor accepts/rejects requests.
- Uses hand-written mock implementations.
- Stores capability memory and metrics.
- No generated code yet.

### Phase 3 — generated Rust source, no execution

- Generate Rust source.
- Run policy checks.
- Run tests.
- Do not execute against real user data.

### Phase 4 — WASM/WASI execution

- Compile to WASM.
- Execute in Wasmtime with no network and limited filesystem.
- Destroy environment.
- Store fingerprint metrics only.

### Phase 5 — reputation and regeneration

- Match tasks to stored problem signatures.
- Regenerate from patterns.
- Reverify every time.
- Track performance.

## Main recommendation

Implement this as a separate execution supervisor beside the split-brain harness.

Do not place tool creation inside the model itself. The model should request capabilities; the supervisor should decide, verify, run, destroy, and remember only safe fingerprints.
