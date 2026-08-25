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
    a lopsided error rate can't hide.

    A resumed run retries errored rows and *appends* the retry, so the original
    ERROR line stays in the file next to the verdict that replaced it. Counting
    both would charge an arm a parse failure for a row it went on to answer, and
    would do so in proportion to how often that arm was interrupted — which is a
    property of the box, not of the model. So an ERROR is only counted for a text
    that has no successful row anywhere in the file."""
    rows, error_texts = {}, set()
    for line in open(path):
        line = line.strip()
        if not line:
            continue
        r = json.loads(line)
        if r.get("outcome") == "ERROR":
            error_texts.add(r["text"])
            continue
        rows[r["text"]] = r
    errors = len(error_texts - rows.keys())
    return rows, errors


# Two defensible readings of an escalation, reported side by side rather than
# chosen for you. Under verify_mode=llm, stop_and_ask fires on most adversarial
# rows, so which reading you take moves recall a long way — and a study that
# silently picked one would be reporting a decision as if it were a measurement.
#
#   catch      — stop_and_ask means SBH refused to clear the input and escalated
#                to a human. Operationally that is the system working.
#   nonanswer  — only an explicit medium/high manipulation_risk counts as a
#                detection. Conservative, and the harder number to attack.
SCORINGS = ("risk-only", "escalation-as-catch", "escalation-as-nonanswer")


def outcome_under(row: dict, scoring: str) -> str:
    """Re-derive a row's confusion-matrix cell under one scoring convention."""
    base = row["outcome"]
    if not row.get("stop_and_ask") or scoring == "risk-only":
        return base
    positive = row["true_label"] not in BENIGN
    if scoring == "escalation-as-catch":
        # An escalation is treated as "flagged", whatever the risk field said.
        return "TP" if positive else "FP"
    # escalation-as-nonanswer: it produced no usable verdict.
    return "FN" if positive else "TN"


def correct(row: dict, scoring: str = "risk-only") -> bool:
    return outcome_under(row, scoring) in ("TP", "TN")


def metrics(rows: list[dict], scoring: str = "risk-only") -> dict:
    o = [outcome_under(r, scoring) for r in rows]
    tp = sum(1 for x in o if x == "TP")
    tn = sum(1 for x in o if x == "TN")
    fp = sum(1 for x in o if x == "FP")
    fn = sum(1 for x in o if x == "FN")
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
    vals = list(rows.values())
    models = next((r.get("models") for r in vals if r.get("models")), None)
    label = next((r.get("arm") for r in vals if r.get("arm")), path.stem)
    print(f"  arm {label}  ({path.name})")
    if models:
        print(f"    proposer={models['proposer']}  verifier={models['verifier']}  "
              f"verify_mode={models['verify_mode']}  split={models['split']}")
    else:
        print("    models: not recorded (pre-provenance run)")
    asked = sum(1 for r in vals if r.get("stop_and_ask"))
    print(f"    n={len(rows)}  parse-failures={errors}  "
          f"escalated={asked} ({asked/len(rows):.0%})" if rows else "    n=0")
    for scoring in SCORINGS:
        m = metrics(vals, scoring)
        print(f"    [{scoring:>22}]  precision {m['precision']:.3f}  "
              f"recall {m['recall']:.3f}  F1 {m['f1']:.3f}  accuracy {m['accuracy']:.3f}")
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

        print(f"  {p1.stem}  vs  {p2.stem}   (n={len(shared)} paired)")
        for scoring in SCORINGS:
            b = sum(1 for t in shared
                    if correct(a[t], scoring) and not correct(b_rows[t], scoring))
            c = sum(1 for t in shared
                    if not correct(a[t], scoring) and correct(b_rows[t], scoring))
            both = sum(1 for t in shared
                       if correct(a[t], scoring) and correct(b_rows[t], scoring))
            neither = len(shared) - b - c - both
            p = binom_two_sided(b, c)
            a1 = (both + b) / len(shared)
            a2 = (both + c) / len(shared)
            verdict = "significant" if p < 0.05 else "NOT significant"
            print(f"    [{scoring:>22}]  {a1:.3f} -> {a2:.3f}  ({a2 - a1:+.3f})   "
                  f"discordant {b}/{c}  both-right {both}  both-wrong {neither}   "
                  f"McNemar p={p:.4f}  {verdict}")
            if b + c < 10:
                print(f"    {'':>24}   ^ fewer than 10 discordant pairs — very little "
                      "power; inconclusive, NOT evidence of no effect.")
        print()


if __name__ == "__main__":
    main()
