#!/usr/bin/env python3
"""Run sbh bench against an unlabeled adversarial JSONL.

For datasets where all inputs are expected to be adversarial (injection,
jailbreak, social engineering). Reports detection rate (% scored medium/high).

Usage:
    python3 scripts/run_bench_adversarial.py <input.jsonl> [--output <out.jsonl>] [--limit N]
"""
import json
import os
import subprocess
import sys
import time
from pathlib import Path

SBH = Path(__file__).parent.parent / "target" / "debug" / "split-brain-harness"


def sbh_analyze(text: str) -> dict:
    result = subprocess.run(
        [str(SBH), "analyze", "--raw", text],
        capture_output=True, text=True, timeout=180,
    )
    if result.returncode != 0:
        raise RuntimeError(result.stderr[:300])
    return json.loads(result.stdout)


def main():
    args = sys.argv[1:]
    if not args or args[0].startswith("--"):
        print("Usage: run_bench_adversarial.py <input.jsonl> [--output <out.jsonl>] [--limit N]")
        sys.exit(1)

    input_path = Path(args[0])
    out_path = None
    if "--output" in args:
        out_path = Path(args[args.index("--output") + 1])
    limit = None
    if "--limit" in args:
        limit = int(args[args.index("--limit") + 1])

    resume = "--resume" in args

    rows = []
    with open(input_path) as f:
        for line in f:
            line = line.strip()
            if line:
                rows.append(json.loads(line))
    if limit:
        rows = rows[:limit]

    # Load already-completed texts when resuming
    done_texts: set[str] = set()
    prior_results: list[dict] = []
    if resume and out_path and out_path.exists():
        with open(out_path) as f:
            for line in f:
                line = line.strip()
                if line:
                    r = json.loads(line)
                    done_texts.add(r["text"])
                    prior_results.append(r)
        print(f"  resume: skipping {len(done_texts)} already-done inputs", flush=True)

    total = len(rows)
    print(f"sbh adversarial bench: {total} inputs from {input_path.name}", flush=True)

    results = list(prior_results)
    n_low  = sum(1 for r in prior_results if r["risk"] == "low")
    n_med  = sum(1 for r in prior_results if r["risk"] == "medium")
    n_high = sum(1 for r in prior_results if r["risk"] == "high")
    errors = 0

    out_file = open(out_path, "a" if resume else "w") if out_path else None

    for i, row in enumerate(rows, 1):
        text = row.get("text", "")
        if not text:
            continue
        if text in done_texts:
            continue

        t0 = time.time()
        try:
            sbh = sbh_analyze(text)
            elapsed = time.time() - t0
            # Treat model parse failures as errors — they don't carry signal
            tone = sbh["telemetry"]["affective_telemetry"].get("structural_tone", [])
            if "parse_failure" in tone or sbh["verification"].get("stop_and_ask"):
                raise RuntimeError("parse_failure — model returned non-JSON")
            risk = sbh["telemetry"]["intent_matrix"]["manipulation_risk"]
            flags = sbh["verification"]["consistency_flags"]

            if risk == "low":
                n_low += 1
                miss = "  ← MISSED"
            elif risk == "medium":
                n_med += 1
                miss = ""
            else:
                n_high += 1
                miss = ""

            print(
                f"  [{i:>4}/{total}] {risk:<6}  {elapsed:.1f}s  {text[:70]}{miss}",
                flush=True,
            )
            for flag in flags:
                print(f"             ⚑ {flag}", flush=True)

            entry = {
                "text": text,
                "risk": risk,
                "flags": flags,
                "elapsed_s": round(elapsed, 2),
                "source": row.get("source", input_path.stem),
            }
            results.append(entry)
            if out_file:
                out_file.write(json.dumps(entry) + "\n")

        except Exception as e:
            elapsed = time.time() - t0
            errors += 1
            print(f"  [{i:>4}/{total}] ERROR  {elapsed:.1f}s  {e}", flush=True)

    if out_file:
        out_file.close()

    detected = n_med + n_high
    detection_rate = detected / (total - errors) if (total - errors) > 0 else 0.0

    bar = "=" * 60
    print()
    print(bar)
    print(f"  {input_path.name}  —  {total} inputs  |  {errors} errors")
    print()
    print(f"  Risk distribution:")
    print(f"    high:   {n_high:>4}  ({100*n_high/(total-errors):.1f}%)" if total - errors else "    high:      0")
    print(f"    medium: {n_med:>4}  ({100*n_med/(total-errors):.1f}%)" if total - errors else "    medium:    0")
    print(f"    low:    {n_low:>4}  ({100*n_low/(total-errors):.1f}%)  ← missed" if total - errors else "    low:       0")
    print()
    print(f"  Detection rate (medium+high): {detection_rate:.3f}  ({detected}/{total-errors})")

    if n_low > 0:
        missed = [r for r in results if r["risk"] == "low"]
        print(f"\n  Missed inputs ({len(missed)}):")
        for r in missed[:10]:
            print(f"    [low] {r['text'][:90]}")

    if out_path:
        print(f"\n  output: {out_path}")
    print(bar)


if __name__ == "__main__":
    main()
