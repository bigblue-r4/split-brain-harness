#!/usr/bin/env python3
"""Re-run MT-Bench questions through sbh analyze and write results JSONL.

Usage:
    python3 scripts/run_mt_bench.py [--output fixtures/mt_bench_sbh_results.jsonl]

Uses turn 0 of each question (same as original benchmark run).
Requires sbh binary on PATH or cargo-built in target/debug/.
"""
import json
import os
import subprocess
import sys
import time
from pathlib import Path

QUESTIONS  = Path(__file__).parent.parent / "fixtures" / "mt_bench_questions.jsonl"
DEFAULT_OUT = Path(__file__).parent.parent / "fixtures" / "mt_bench_sbh_results.jsonl"

SBH = (
    Path(__file__).parent.parent / "target" / "debug" / "split-brain-harness"
)

def sbh_analyze(text: str) -> dict:
    result = subprocess.run(
        [str(SBH), "analyze", "--raw", text],
        capture_output=True, text=True, timeout=180,
    )
    if result.returncode != 0:
        raise RuntimeError(f"sbh exited {result.returncode}: {result.stderr[:200]}")
    return json.loads(result.stdout)

def main():
    out_path = Path(sys.argv[sys.argv.index("--output") + 1]) if "--output" in sys.argv else DEFAULT_OUT

    questions = []
    with open(QUESTIONS) as f:
        for line in f:
            questions.append(json.loads(line))

    print(f"Running {len(questions)} questions → {out_path}", flush=True)

    results = []
    for i, q in enumerate(questions, 1):
        text = q["turns"][0]
        t0 = time.time()
        try:
            sbh = sbh_analyze(text)
            elapsed = time.time() - t0
            risk = sbh["telemetry"]["intent_matrix"]["manipulation_risk"]
            flag = "  ← FLAGGED" if risk != "low" else ""
            print(f"  [{i:2}/{len(questions)}] {risk:<6}  {elapsed:.1f}s  {text[:70]}{flag}", flush=True)
            results.append({
                "question_id": q.get("question_id", i),
                "category":    q.get("category", ""),
                "turn":        0,
                "text":        text,
                "sbh":         sbh,
                "elapsed_s":   round(elapsed, 2),
            })
        except Exception as e:
            elapsed = time.time() - t0
            print(f"  [{i:2}/{len(questions)}] ERROR  {elapsed:.1f}s  {e}", flush=True)
            results.append({
                "question_id": q.get("question_id", i),
                "category":    q.get("category", ""),
                "turn":        0,
                "text":        text,
                "sbh":         None,
                "error":       str(e),
                "elapsed_s":   round(elapsed, 2),
            })

    with open(out_path, "w") as f:
        for r in results:
            f.write(json.dumps(r) + "\n")

    # Summary
    ok = [r for r in results if r.get("sbh")]
    risks = {}
    for r in ok:
        risk = r["sbh"]["telemetry"]["intent_matrix"]["manipulation_risk"]
        risks[risk] = risks.get(risk, 0) + 1
    flagged = [r for r in ok if r["sbh"]["telemetry"]["intent_matrix"]["manipulation_risk"] != "low"]

    print()
    print("=" * 60)
    print(f"  {len(ok)}/{len(questions)} completed  |  errors: {len(questions)-len(ok)}")
    for k, v in sorted(risks.items()):
        print(f"  {k:<8} {v}")
    print(f"  flagged: {len(flagged)}")
    if flagged:
        print()
        print("  Flagged inputs:")
        for r in flagged:
            risk = r["sbh"]["telemetry"]["intent_matrix"]["manipulation_risk"]
            flags = r["sbh"]["verification"]["consistency_flags"]
            print(f"    [{risk}] {r['text'][:80]}")
            for flag in flags:
                print(f"           ⚑ {flag}")
    print("=" * 60)
    print(f"  Results written to {out_path}")

if __name__ == "__main__":
    main()
