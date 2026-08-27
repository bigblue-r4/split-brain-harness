#!/usr/bin/env python3
"""Measure the Reconcile adjudicator gate from artifacts already on disk.

VerifyMode::Reconcile adds a third LLM call, but only when

    disagreement.injection_fingerprint || disagreement.flag_density >= 0.5

Both sides of that OR are recoverable from a bench artifact without re-running
anything, because `flags` records exactly the fired consistency checks:
`CheckOutcome::fired()` is defined as `detail.is_some()`, and `consistency_flags`
is the list of every `detail`. So fired-set == recorded-flag-set, exactly.

  flag_density >= 0.5  ->  len(flags) >= 4   (TOTAL_CHECKS = 8)
  injection_fingerprint -> a Tone-dimension check AND an Urgency-dimension check
                           both fired. Each of those two checks already requires
                           manipulation_risk == Low to fire at all, so the third
                           conjunct is implied and needs no separate lookup.

Usage: python3 scripts/reconcile_gate.py fixtures/dualmodel_arm*.jsonl
"""
import json
import os
import sys

TONE_FLAG = "conflicts with manipulation_risk=low"
URGENCY_FLAG = "urgency may be manufactured"
TOTAL_CHECKS = 8
DENSITY_MIN_FLAGS = int(TOTAL_CHECKS * 0.5)  # flag_density >= 0.5


def gate(flags):
    """Return (tone, urgency, fingerprint, density, fires) for one row."""
    tone = any(TONE_FLAG in f for f in flags)
    urgency = any(URGENCY_FLAG in f for f in flags)
    fingerprint = tone and urgency
    density = len(flags) >= DENSITY_MIN_FLAGS
    return tone, urgency, fingerprint, density, (fingerprint or density)


def main(paths):
    if not paths:
        print(__doc__)
        return 1
    hdr = f"{'arm':<5} {'rows':>5} {'tone':>5} {'urg':>5} {'fingerprint':>12} {'density':>8} {'GATE':>6}"
    print(hdr)
    print("-" * len(hdr))
    tot = [0] * 5
    n_tot = 0
    for path in paths:
        arm = os.path.basename(path).replace("dualmodel_arm", "").replace(".jsonl", "")
        rows = [json.loads(l) for l in open(path) if l.strip()]
        acc = [0] * 5
        for r in rows:
            for i, v in enumerate(gate(r.get("flags") or [])):
                acc[i] += bool(v)
        n_tot += len(rows)
        tot = [a + b for a, b in zip(tot, acc)]
        print(
            f"{arm:<5} {len(rows):>5} {acc[0]:>5} {acc[1]:>5} {acc[2]:>12} {acc[3]:>8} {acc[4]:>6}"
        )
    print("-" * len(hdr))
    print(f"{'ALL':<5} {n_tot:>5} {tot[0]:>5} {tot[1]:>5} {tot[2]:>12} {tot[3]:>8} {tot[4]:>6}")
    rate = tot[4] / n_tot if n_tot else 0.0
    print(f"\nadjudicator fire rate: {tot[4]}/{n_tot} = {rate * 100:.2f}%")
    if tot[4] == 0:
        print(
            "\nThe gate never opens on this corpus, so Reconcile makes exactly the same\n"
            "calls as Llm here. Note this is about REACHABILITY only: even when the gate\n"
            "does open, the verdict is not read by any decision — see\n"
            "verifier.rs::reconcile_verdict_cannot_change_the_decision."
        )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
