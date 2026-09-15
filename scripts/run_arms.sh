#!/usr/bin/env bash
# Run the dual-model study arms over the fixed sample.
#
# Arms B/C/D/E all run at SBH_VERIFY=llm and SBH_REFINE_ITERS=1: one propose call
# and one verify call per row. Refinement is pinned off deliberately — it fires a
# variable number of extra calls depending on which flags trip, which would make
# both the cost and the behaviour differ between arms for reasons unrelated to
# which model verified.
#
# Arm A (the published baseline) is deterministic-verify and already on disk;
# it is a reference point, not the control. The control is arm B.
#
# Arm E (qwen3.5 on both hemispheres) is the same-model control for arm D, added
# after D beat B: without it, "qwen3.5 is the better proposer" explains D's whole
# gain and diversity gets credit it has not earned. D - E is the dual-model effect
# at qwen3.5's proposer, the mirror of C - B at llama3.2's.
#
# Usage: ./scripts/run_arms.sh [--resume] [arm ...]     e.g. ./scripts/run_arms.sh B C
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SAMPLE="$ROOT/fixtures/dualmodel_sample.jsonl"

RESUME=""
ARMS=()
for a in "$@"; do
  if [[ "$a" == "--resume" ]]; then RESUME="--resume"; else ARMS+=("$a"); fi
done
[[ ${#ARMS[@]} -eq 0 ]] && ARMS=(B C D E)

export SBH_BACKEND=ollama-native
export SBH_VERIFY=llm
export SBH_REFINE_ITERS=1
export SBH_TIMEOUT_SECONDS=1800

for arm in "${ARMS[@]}"; do
  case "$arm" in
    B) PROPOSER=llama3.2:3b; VERIFIER=llama3.2:3b ;;
    C) PROPOSER=llama3.2:3b; VERIFIER=qwen3.5     ;;
    D) PROPOSER=qwen3.5;     VERIFIER=llama3.2:3b ;;
    E) PROPOSER=qwen3.5;     VERIFIER=qwen3.5     ;;
    # B2 is arm B re-run unchanged: the test-retest replicate. Temperature is 0.1,
    # not 0, so the same configuration does not give the same answer twice — and
    # until B vs B2 is measured, every difference in this study is being read
    # against an unknown noise floor. Not in the default arm list: it is a
    # calibration run, not one of the four design arms.
    B2) PROPOSER=llama3.2:3b; VERIFIER=llama3.2:3b ;;
    # B3 is arm B again, under the mission soul (souls/soul-mission.md via
    # SBH_SOUL_PATH). Same models as B and B2 on purpose: the only thing that
    # differs is the system prompt, so B vs B3 isolates the soul edit and B vs B2
    # says how much of any difference is just temperature 0.1 talking.
    B3) PROPOSER=llama3.2:3b; VERIFIER=llama3.2:3b ;;
    # B4 is the tightened mission (souls/soul-mission-v2.md): the same four
    # commitments as B3 but folded into the existing objective paragraph, 257
    # added characters instead of 786. B3 cost ~2-3 points against both B and
    # B2 while changing no rule, so this tests whether the cost was the length
    # rather than the content.
    B4) PROPOSER=llama3.2:3b; VERIFIER=llama3.2:3b ;;
    *) echo "unknown arm: $arm (want B, C, D, E, B2, B3 or B4)"; exit 1 ;;
  esac

  export SBH_MODEL="$PROPOSER"
  if [[ "$VERIFIER" == "$PROPOSER" ]]; then
    unset SBH_VERIFIER_MODEL
  else
    export SBH_VERIFIER_MODEL="$VERIFIER"
  fi

  OUT="$ROOT/fixtures/dualmodel_arm${arm}.jsonl"
  echo "=== arm $arm: proposer=$PROPOSER verifier=$VERIFIER -> $(basename "$OUT") ==="
  python3 "$ROOT/scripts/run_bench_labeled.py" "$SAMPLE" \
    --output "$OUT" --arm "$arm" $RESUME
  echo
done

echo ">>> arms done — compare with:"
echo "    python3 scripts/compare_arms.py fixtures/dualmodel_arm{B,C,D,E}.jsonl"
