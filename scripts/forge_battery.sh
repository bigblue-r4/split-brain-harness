#!/usr/bin/env bash
# forge_battery.sh — Stress test the Ephemeral Tool Forge across diverse capability types.
#
# Usage:
#   ./scripts/forge_battery.sh [--model <model>] [--backend <backend>] [--retries <n>]
#
# Outputs:
#   - Live progress to stderr
#   - Summary table to stdout
#   - Full JSONL log to forge_battery_results.jsonl

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BINARY="$REPO_DIR/target/release/split-brain-harness"
RESULTS_FILE="$REPO_DIR/forge_battery_results.jsonl"
MEMORY_FILE="/tmp/sbh-battery-memory.json"

# ── defaults ──────────────────────────────────────────────────────────────────
MODEL="${SBH_MODEL:-claude-haiku-4-5-20251001}"
BACKEND="${SBH_BACKEND:-anthropic}"
MAX_RETRIES=3
PAUSE_SECS=2   # between runs to avoid rate limiting

while [[ $# -gt 0 ]]; do
  case "$1" in
    --model)    MODEL="$2";       shift 2 ;;
    --backend)  BACKEND="$2";     shift 2 ;;
    --retries)  MAX_RETRIES="$2"; shift 2 ;;
    *) echo "unknown flag: $1" >&2; exit 1 ;;
  esac
done

export PATH="$HOME/.wasmtime/bin:$HOME/.cargo/bin:$PATH"
export SBH_BACKEND="$BACKEND"
export SBH_MODEL="$MODEL"
export SBH_MEMORY_PATH="$MEMORY_FILE"
: "${SBH_API_KEY:?SBH_API_KEY must be set}"

# ── build if needed ────────────────────────────────────────────────────────────
if [[ ! -f "$BINARY" ]]; then
  echo "building release binary..." >&2
  cargo build --release --manifest-path "$REPO_DIR/Cargo.toml" >&2
fi

# ── test cases ─────────────────────────────────────────────────────────────────
# Format: "capability" TAB "input"
declare -a CASES=(
  "word count	the quick brown fox jumps over the lazy dog"
  "character count	hello world"
  "reverse string	racecar"
  "uppercase	hello from the ephemeral tool forge"
  "line count	alpha\nbeta\ngamma\ndelta\nepsilon"
  "sort lines	banana\napple\ncherry\ndate\nelderberry"
  "unique lines	alpha\nbeta\nalpha\ngamma\nbeta\ndelta"
  "sum numbers	10\n20\n30\n40\n50"
  "palindrome check	racecar"
  "hex dump	hello"
  "rot13 encode	Hello World"
  "csv column sum	name,score\nalice,90\nbob,85\ncarol,92"
  "log line filter	2024-01-01 INFO started\n2024-01-01 ERROR crash\n2024-01-01 INFO done"
  "count vowels	the quick brown fox"
  "title case	the quick brown fox jumps"
)

# ── run battery ────────────────────────────────────────────────────────────────
> "$RESULTS_FILE"   # truncate

PASSED=0
FAILED=0
TOTAL=${#CASES[@]}
declare -a SUMMARY_ROWS=()

printf '\n%s\n' "━━━ Ephemeral Tool Forge Battery ━━━" >&2
printf 'model=%s  backend=%s  retries=%s  cases=%s\n\n' \
  "$MODEL" "$BACKEND" "$MAX_RETRIES" "$TOTAL" >&2

for i in "${!CASES[@]}"; do
  IFS=$'\t' read -r CAPABILITY INPUT_RAW <<< "${CASES[$i]}"
  # Expand \n escape sequences in input
  INPUT="$(printf '%b' "$INPUT_RAW")"

  IDX=$((i + 1))
  printf '[%2d/%d] %-30s ' "$IDX" "$TOTAL" "\"$CAPABILITY\"" >&2

  START_TS=$(date +%s%3N)

  REPORT=$("$BINARY" forge \
    --capability "$CAPABILITY" \
    --max-retries "$MAX_RETRIES" \
    "$INPUT" 2>/dev/null) || true

  END_TS=$(date +%s%3N)
  WALL_MS=$(( END_TS - START_TS ))

  # Parse report fields
  SUCCEEDED=$(echo "$REPORT" | jq -r '.succeeded // false')
  ACCEPTED=$(echo  "$REPORT" | jq -r '.accepted  // false')
  ATTEMPTS=$(echo  "$REPORT" | jq -r '.attempts | length')
  OUTPUT=$(echo    "$REPORT" | jq -r '.output   // ""')
  TIER_AFTER=$(echo "$REPORT" | jq -r '.reputation_after.tier // "unknown"')

  if [[ "$SUCCEEDED" == "true" ]]; then
    STATUS="PASS"
    ICON="✓"
    PASSED=$(( PASSED + 1 ))
  else
    STATUS="FAIL"
    ICON="✗"
    FAILED=$(( FAILED + 1 ))
  fi

  printf '%s  attempts=%-2s  wall=%5sms  tier=%-10s  output=%.60s\n' \
    "$ICON" "$ATTEMPTS" "$WALL_MS" "$TIER_AFTER" "$OUTPUT" >&2

  # Append to JSONL log
  jq -n \
    --arg  cap        "$CAPABILITY" \
    --arg  input      "$INPUT" \
    --arg  status     "$STATUS" \
    --argjson succeeded "$SUCCEEDED" \
    --argjson accepted  "$ACCEPTED" \
    --argjson attempts  "$ATTEMPTS" \
    --arg  output     "$OUTPUT" \
    --arg  tier_after "$TIER_AFTER" \
    --argjson wall_ms   "$WALL_MS" \
    --arg  model      "$MODEL" \
    --arg  backend    "$BACKEND" \
    '{capability:$cap, input:$input, status:$status,
      succeeded:$succeeded, accepted:$accepted,
      attempts:$attempts, output:$output,
      tier_after:$tier_after, wall_ms:$wall_ms,
      model:$model, backend:$backend}' \
    >> "$RESULTS_FILE"

  SUMMARY_ROWS+=("$STATUS|$CAPABILITY|$ATTEMPTS|${WALL_MS}ms|$TIER_AFTER|$OUTPUT")

  # Rate-limit pause (skip after last case)
  if (( IDX < TOTAL )); then
    sleep "$PAUSE_SECS"
  fi
done

# ── summary table ──────────────────────────────────────────────────────────────
printf '\n%s\n' "━━━ Summary ━━━"
printf '%-6s  %-30s  %-8s  %-8s  %-12s  %s\n' \
  "STATUS" "CAPABILITY" "ATTEMPTS" "TIME" "REPUTATION" "OUTPUT (truncated)"
printf '%s\n' "$(printf '─%.0s' {1..100})"

for row in "${SUMMARY_ROWS[@]}"; do
  IFS='|' read -r ST CAP ATT TIME TIER OUT <<< "$row"
  printf '%-6s  %-30s  %-8s  %-8s  %-12s  %.50s\n' \
    "$ST" "$CAP" "$ATT" "$TIME" "$TIER" "$OUT"
done

printf '%s\n' "$(printf '─%.0s' {1..100})"
printf 'PASSED: %d / %d    FAILED: %d / %d\n' "$PASSED" "$TOTAL" "$FAILED" "$TOTAL"
printf 'Full results: %s\n' "$RESULTS_FILE"
