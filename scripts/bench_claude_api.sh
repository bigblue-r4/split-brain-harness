#!/usr/bin/env bash
# Benchmark Claude (Anthropic API) as the telemetry engine vs the local
# split-brain:latest baseline, using the harness's built-in bench comparator.
#
# Usage:
#   ANTHROPIC_API_KEY=sk-ant-... ./scripts/bench_claude_api.sh [fixture] [model]
#
#   fixture: llm_sec_eval (default) | mt_bench | trustai | deepset | cyberec200
#   model:   claude-opus-4-8 (default) — any current Anthropic model ID
#
# Cost ballpark at Opus 4.8 ($5/M in, $25/M out): llm_sec_eval (150 q) or
# mt_bench (80 q) ≈ a few dollars. trustai (1405 q) / deepset (546 q) cost
# proportionally more — start small.
set -euo pipefail
cd "$(dirname "$0")/.."

[ -n "${ANTHROPIC_API_KEY:-}" ] || { echo "Set ANTHROPIC_API_KEY first (console.anthropic.com -> API keys)"; exit 1; }

FIXTURE="${1:-llm_sec_eval}"
MODEL="${2:-claude-opus-4-8}"

case "$FIXTURE" in
  llm_sec_eval) Q=fixtures/llm_sec_eval_questions.jsonl;  B=fixtures/llm_sec_eval_sbh_results.jsonl ;;
  mt_bench)     Q=fixtures/mt_bench_questions.jsonl;      B=fixtures/mt_bench_sbh_results_v2.jsonl ;;
  trustai)      Q=fixtures/trustai_jailbreaks.jsonl;      B=fixtures/trustai_sbh_results.jsonl ;;
  deepset)      Q=fixtures/deepset_prompt_injections.jsonl; B=fixtures/deepset_sbh_results.jsonl ;;
  cyberec200)   Q=fixtures/cyberec_sample200.jsonl;       B=fixtures/cyberec_sbh_results.jsonl ;;
  *) echo "unknown fixture: $FIXTURE"; exit 1 ;;
esac

OUT="fixtures/${FIXTURE}_claude_$(date +%Y%m%d).jsonl"
echo "Benchmarking $MODEL on $Q vs local baseline $B -> $OUT"

SBH_BACKEND=anthropic \
SBH_API_KEY="$ANTHROPIC_API_KEY" \
SBH_MODEL="$MODEL" \
./target/release/split-brain-harness bench "$Q" --baseline "$B" --output "$OUT"
