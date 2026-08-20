#!/usr/bin/env python3
"""Compare study arms row-by-row, with a paired significance test.

Two detection rates side by side do not establish that two models on the two
hemispheres beat one. At n in the low hundreds the difference between 48% and
53% is comfortably inside noise, and the arms are *paired* — the same inputs
graded twice — so the honest test is McNemar's on the discordant pairs, not a
comparison of the two headline rates.

McNemar's exact test is a two-sided binomial test on b vs c, where
  b = rows arm 1 got right and arm 2 got wrong
  c = rows arm 2 got right and arm 1 got wrong
Concordant rows carry no information about which arm is better and are excluded
by construction. Pure stdlib — no scipy, keeping the repo's dependency posture.

Usage:
    python3 scripts/compare_arms.py fixtures/dualmodel_armB.jsonl \
                                    fixtures/dualmodel_armC.jsonl [more...]
"""
import json
import math
import sys
from itertools import combinations
from pathlib import Path

BENIGN = ("benign", "safe", "0", 0)


def load(path: Path) -> dict[str, dict]:
    """Rows keyed by input text. Errored rows are dropped — a row that never
    produced a verdict is not evidence either way — but the count is reported so
    a lopsided error rate can't hide."""
    rows, errors = {}, 0
    for line in open(path):
        line = line.strip()
        if not line:
            continue
        r = json.loads(line)
        if r.get("outcome") == "ERROR":
            errors += 1
            continue
        rows[r["text"]] = r
    return rows, errors


def correct(row: dict) -> bool:
    return row["outcome"] in ("TP", "TN")


def metrics(rows: list[dict]) -> dict:
    tp = sum(1 for r in rows if r["outcome"] == "TP")
    tn = sum(1 for r in rows if r["outcome"] == "TN")
    fp = sum(1 for r in rows if r["outcome"] == "FP")
    fn = sum(1 for r in rows if r["outcome"] == "FN")
    prec = tp / (tp + fp) if tp + fp else 0.0
    rec = tp / (tp + fn) if tp + fn else 0.0
    f1 = 2 * prec * rec / (prec + rec) if prec + rec else 0.0
    return {
        "tp": tp, "tn": tn, "fp": fp, "fn": fn,
        "precision": prec, "recall": rec, "f1": f1,
        "accuracy": (tp + tn) / len(rows) if rows else 0.0,
    }


def binom_two_sided(b: int, c: int) -> float:
    """Exact two-sided binomial p for b successes in n=b+c at p=0.5."""
    n = b + c
    if n == 0:
        return 1.0
    k = min(b, c)
    tail = sum(math.comb(n, i) for i in range(0, k + 1)) / (2 ** n)
    return min(1.0, 2 * tail)


def describe(path: Path, rows: dict, errors: int) -> None:
    m = metrics(list(rows.values()))
    models = next((r.get("models") for r in rows.values() if r.get("models")), None)
    label = next((r.get("arm") for r in rows.values() if r.get("arm")), path.stem)
    print(f"  arm {label}  ({path.name})")
    if models:
        print(f"    proposer={models['proposer']}  verifier={models['verifier']}  "
              f"verify_mode={models['verify_mode']}  split={models['split']}")
    else:
        print("    models: not recorded (pre-provenance run)")
    print(f"    n={len(rows)}  errors={errors}")
    print(f"    precision {m['precision']:.3f}  recall {m['recall']:.3f}  "
          f"F1 {m['f1']:.3f}  accuracy {m['accuracy']:.3f}")
    times = [r["elapsed_s"] for r in rows.values() if r.get("elapsed_s")]
    if times:
        times.sort()
        print(f"    median {times[len(times)//2]:.1f}s/row  total {sum(times)/3600:.1f}h")
    print()


def main() -> None:
    paths = [Path(p) for p in sys.argv[1:]]
    if len(paths) < 2:
        print(__doc__)
        sys.exit(1)

    loaded = {}
    print("\n=== Arms ===\n")
    for p in paths:
        rows, errors = load(p)
        loaded[p] = rows
        describe(p, rows, errors)

    print("=== Paired comparisons (McNemar's exact test) ===\n")
    for p1, p2 in combinations(paths, 2):
        a, b_rows = loaded[p1], loaded[p2]
        shared = sorted(set(a) & set(b_rows))
        if not shared:
            print(f"  {p1.stem} vs {p2.stem}: no shared rows — nothing to pair\n")
            continue

        b = sum(1 for t in shared if correct(a[t]) and not correct(b_rows[t]))
        c = sum(1 for t in shared if not correct(a[t]) and correct(b_rows[t]))
        both = sum(1 for t in shared if correct(a[t]) and correct(b_rows[t]))
        neither = len(shared) - b - c - both
        p = binom_two_sided(b, c)

        a1 = (both + b) / len(shared)
        a2 = (both + c) / len(shared)
        print(f"  {p1.stem}  vs  {p2.stem}   (n={len(shared)} paired)")
        print(f"    accuracy {a1:.3f} vs {a2:.3f}   delta {a2 - a1:+.3f}")
        print(f"    both right {both}  both wrong {neither}  "
              f"only-{p1.stem} {b}  only-{p2.stem} {c}")
        print(f"    McNemar exact p = {p:.4f}  "
              f"({'significant at 0.05' if p < 0.05 else 'NOT significant at 0.05'})")
        if b + c < 10:
            print("    note: fewer than 10 discordant pairs — the test has very "
                  "little power here; treat as inconclusive, not as evidence of no effect.")
        print()


if __name__ == "__main__":
    main()
