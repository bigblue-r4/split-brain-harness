#!/usr/bin/env python3
"""Run sbh bench against a labeled JSONL and compute precision/recall/F1.

For labeled datasets (e.g. deepset/prompt-injections) where each row has a
{label} field of "benign" or "injection"/"jailbreak"/etc.

Usage:
    python3 scripts/run_bench_labeled.py <input.jsonl> [--output <out.jsonl>] [--limit N]

Positive class: any label that is NOT "benign" (injection, jailbreak, attack, …)
sbh positive:   manipulation_risk = medium or high

Prints: confusion matrix, precision, recall, F1.
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
        print("Usage: run_bench_labeled.py <input.jsonl> [--output <out.jsonl>] [--limit N]")
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
    print(f"sbh labeled bench: {total} inputs from {input_path.name}", flush=True)

    results = list(prior_results)
    tp = sum(1 for r in prior_results if r["outcome"] == "TP")
    fp = sum(1 for r in prior_results if r["outcome"] == "FP")
    tn = sum(1 for r in prior_results if r["outcome"] == "TN")
    fn = sum(1 for r in prior_results if r["outcome"] == "FN")
    errors = 0

    out_file = open(out_path, "a" if resume else "w") if out_path else None

    for i, row in enumerate(rows, 1):
        if row.get("text", "") in done_texts:
            continue
        text = row.get("text", "")
        if not text:
            continue
        true_label = row.get("label", "unknown")
        is_positive = true_label not in ("benign", "safe", "0", 0)

        t0 = time.time()
        try:
            sbh = sbh_analyze(text)
            elapsed = time.time() - t0
            # Treat model parse failures as errors — they don't carry signal
            tone = sbh["telemetry"]["affective_telemetry"].get("structural_tone", [])
            if "parse_failure" in tone or sbh["verification"].get("stop_and_ask"):
                raise RuntimeError("parse_failure — model returned non-JSON")
            risk = sbh["telemetry"]["intent_matrix"]["manipulation_risk"]
            sbh_positive = risk in ("medium", "high")

            if is_positive and sbh_positive:
                outcome, tp = "TP", tp + 1
            elif is_positive and not sbh_positive:
                outcome, fn = "FN", fn + 1
            elif not is_positive and sbh_positive:
                outcome, fp = "FP", fp + 1
            else:
                outcome, tn = "TN", tn + 1

            flag = "  ←" if outcome in ("FP", "FN") else ""
            print(
                f"  [{i:>4}/{total}] {outcome}  sbh={risk:<6}  true={true_label:<10}  "
                f"{elapsed:.1f}s  {text[:60]}{flag}",
                flush=True,
            )

            entry = {
                "text": text,
                "true_label": true_label,
                "risk": risk,
                "outcome": outcome,
                "flags": sbh["verification"]["consistency_flags"],
                "elapsed_s": round(elapsed, 2),
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

    # Metrics
    precision = tp / (tp + fp) if (tp + fp) > 0 else 0.0
    recall    = tp / (tp + fn) if (tp + fn) > 0 else 0.0
    f1        = 2 * precision * recall / (precision + recall) if (precision + recall) > 0 else 0.0
    accuracy  = (tp + tn) / (tp + tn + fp + fn) if (tp + tn + fp + fn) > 0 else 0.0

    bar = "=" * 60
    print()
    print(bar)
    print(f"  {input_path.name}  —  {total} inputs  |  {errors} errors")
    print()
    print(f"  Confusion matrix (positive = non-benign, sbh = medium/high)")
    print(f"    TP (correctly flagged):    {tp:>4}")
    print(f"    TN (correctly passed):     {tn:>4}")
    print(f"    FP (false alarm):          {fp:>4}")
    print(f"    FN (missed threat):        {fn:>4}")
    print()
    print(f"  Precision:  {precision:.3f}   ({tp}/{tp+fp} sbh-positives correct)")
    print(f"  Recall:     {recall:.3f}   ({tp}/{tp+fn} true threats caught)")
    print(f"  F1:         {f1:.3f}")
    print(f"  Accuracy:   {accuracy:.3f}")

    if fp > 0:
        fp_rows = [r for r in results if r["outcome"] == "FP"]
        print(f"\n  False positives ({len(fp_rows)}):")
        for r in fp_rows[:10]:
            print(f"    [FP] {r['text'][:80]}")

    if fn > 0:
        fn_rows = [r for r in results if r["outcome"] == "FN"]
        print(f"\n  False negatives / missed threats ({len(fn_rows)}):")
        for r in fn_rows[:10]:
            print(f"    [FN] true={r['true_label']}  {r['text'][:80]}")

    if out_path:
        print(f"\n  output: {out_path}")
    print(bar)


if __name__ == "__main__":
    main()
