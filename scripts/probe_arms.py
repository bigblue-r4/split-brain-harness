#!/usr/bin/env python3
"""Time each study arm on a handful of real bench rows.

Wall-clock per row decides the sample size, so measure it before committing to a
run rather than extrapolating from the single-model logs — those rows made one
LLM call, and every arm here makes two.

Usage: python3 scripts/probe_arms.py [--rows N]
"""
import json
import os
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SBH = ROOT / "target" / "release" / "split-brain-harness"
FIXTURE = ROOT / "fixtures" / "cyberec_sample200.jsonl"

# (arm, proposer, verifier)
ARMS = [
    ("B", "llama3.2:3b", "llama3.2:3b"),
    ("C", "llama3.2:3b", "qwen3.5"),
    ("D", "qwen3.5", "llama3.2:3b"),
]

BASE_ENV = {
    "SBH_BACKEND": "ollama-native",
    "SBH_VERIFY": "llm",
    "SBH_REFINE_ITERS": "1",
    "SBH_TIMEOUT_SECONDS": "1800",
}


def main() -> None:
    n = 3
    if "--rows" in sys.argv:
        n = int(sys.argv[sys.argv.index("--rows") + 1])

    rows = [json.loads(l) for l in open(FIXTURE) if l.strip()][:n]
    print(f"probe: {len(rows)} rows x {len(ARMS)} arms\n", flush=True)

    for arm, proposer, verifier in ARMS:
        env = {**os.environ, **BASE_ENV, "SBH_MODEL": proposer}
        if verifier != proposer:
            env["SBH_VERIFIER_MODEL"] = verifier
        else:
            env.pop("SBH_VERIFIER_MODEL", None)

        times, split_seen = [], None
        for i, row in enumerate(rows, 1):
            t0 = time.time()
            p = subprocess.run(
                [str(SBH), "analyze", "--raw", row["text"]],
                capture_output=True, text=True, env=env, timeout=3600,
            )
            dt = time.time() - t0
            times.append(dt)
            try:
                split_seen = json.loads(p.stdout)["models"]
            except Exception:
                split_seen = {"error": (p.stderr or p.stdout)[:120]}
            print(f"  arm {arm} [{proposer} -> {verifier}] row {i}: {dt:.1f}s", flush=True)

        med = statistics.median(times)
        print(f"  arm {arm} MEDIAN {med:.1f}s/row  models={json.dumps(split_seen)}")
        for size in (100, 150):
            print(f"    projected {size} rows: {med * size / 3600:.1f}h")
        print(flush=True)


if __name__ == "__main__":
    main()
