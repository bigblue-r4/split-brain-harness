#!/usr/bin/env bash
# Run all three adversarial benchmarks with the Anthropic/haiku backend.
# Supports --resume to skip already-done entries if the session was interrupted.
# Usage: ./scripts/run_all_benches.sh [--resume]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$SCRIPT_DIR/.."

RESUME_FLAG=""
if [[ "${1:-}" == "--resume" ]]; then
  RESUME_FLAG="--resume"
  echo ">>> resume mode: existing output files will be appended to"
fi

export SBH_BACKEND="${SBH_BACKEND:-ollama-native}"
export SBH_MODEL="${SBH_MODEL:-llama3.2:3b}"
export PATH="$HOME/.wasmtime/bin:$HOME/.cargo/bin:$PATH"

echo ">>> backend: $SBH_BACKEND  model: $SBH_MODEL"
echo

# 1. CyberEC (labeled — has injection/benign labels)
echo "=== [1/3] cyberec ==="
python3 "$SCRIPT_DIR/run_bench_labeled.py" \
  "$ROOT/fixtures/cyberec_sample200.jsonl" \
  --output "$ROOT/fixtures/cyberec_sbh_results.jsonl" \
  $RESUME_FLAG
echo

# 2. Deepset (labeled)
echo "=== [2/3] deepset ==="
python3 "$SCRIPT_DIR/run_bench_labeled.py" \
  "$ROOT/fixtures/deepset_prompt_injections.jsonl" \
  --output "$ROOT/fixtures/deepset_sbh_results.jsonl" \
  $RESUME_FLAG
echo

# 3. TrustAI (adversarial/unlabeled)
echo "=== [3/3] trustai ==="
python3 "$SCRIPT_DIR/run_bench_adversarial.py" \
  "$ROOT/fixtures/trustai_jailbreaks.jsonl" \
  --output "$ROOT/fixtures/trustai_sbh_results.jsonl" \
  $RESUME_FLAG
echo

echo ">>> all benches done"
