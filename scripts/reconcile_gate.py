#!/usr/bin/env python3
"""Measure the Reconcile adjudicator gate from artifacts already on disk.

VerifyMode::Reconcile makes a third LLM call when the gate opens. This reports how
often the OLD trigger opened (it never did) and how often the CURRENT one does.

    old:  injection_fingerprint || flag_density >= 0.5
    new:  a check in an INTENT dimension fired, || the two old disjuncts

Both are recoverable from a bench artifact without re-running anything: `flags`
records the fired checks, because `CheckOutcome::fired()` is defined as
`detail.is_some()` and `consistency_flags` collects every `detail`.

CORRECTION (2026-08-27): an earlier version of this script counted the
"obfuscation detected" string toward flag_density. That string is inserted into
consistency_flags by `stage_obfuscation` in harness.rs AFTER verify() has returned,
so the gate never sees it. It is excluded here. This did not change the old gate's
result — 0 either way, and correct counting puts it further from firing, since the
real per-row maximum is 2 fired checks rather than 3.

Usage: python3 scripts/reconcile_gate.py fixtures/dualmodel_arm*.jsonl
"""
import json
import os
import sys

# flag-text signature -> dimension, and whether that dimension is an intent signal
# (Dimension::is_intent_signal in verifier.rs). Coherence and RiskValue are quality
# signals: they say the input is garbled or the risk value unparseable.
CHECKS = [
    ("high emotional_intensity", "affective", True),
    ("conflicts with manipulation_risk=low", "tone", True),
    ("urgency may be manufactured", "urgency", True),
    ("input may be too incoherent", "coherence", False),
    ("is not a recognized value", "risk_value", False),
    ("manipulation_risk=high but", "risk_signal", True),
    ("hidden_payload:", "scope_creep", True),
    ("value_alignment delta", "value_alignment", True),
]
NOT_A_CHECK = "obfuscation detected"  # added post-verify; invisible to the gate
TOTAL_CHECKS = 8


def classify(flags):
    """-> (fired dimensions, count of real fired checks, any intent signal)."""
    fired, intent = set(), False
    for f in flags:
        if NOT_A_CHECK in f:
            continue
        for sig, dim, is_intent in CHECKS:
            if sig in f:
                fired.add(dim)
                intent = intent or is_intent
                break
    return fired, len(fired), intent


def main(paths):
    if not paths:
        print(__doc__)
        return 1
    hdr = f"{'arm':<5} {'rows':>5} {'tone':>5} {'urg':>5} {'OLD gate':>9} {'NEW gate':>9}"
    print(hdr)
    print("-" * len(hdr))
    t_rows = t_tone = t_urg = t_old = t_new = 0
    for path in paths:
        arm = os.path.basename(path).replace("dualmodel_arm", "").replace(".jsonl", "")
        rows = [json.loads(l) for l in open(path) if l.strip()]
        tone = urg = old = new = 0
        for r in rows:
            dims, n, intent = classify(r.get("flags") or [])
            tone += "tone" in dims
            urg += "urgency" in dims
            o = ("tone" in dims and "urgency" in dims) or n >= TOTAL_CHECKS * 0.5
            old += o
            new += o or intent
        t_rows += len(rows); t_tone += tone; t_urg += urg; t_old += old; t_new += new
        print(f"{arm:<5} {len(rows):>5} {tone:>5} {urg:>5} {old:>9} {new:>9}")
    print("-" * len(hdr))
    print(f"{'ALL':<5} {t_rows:>5} {t_tone:>5} {t_urg:>5} {t_old:>9} {t_new:>9}")
    print(f"\nold trigger: {t_old}/{t_rows} = {t_old / t_rows * 100:.2f}%")
    print(f"new trigger: {t_new}/{t_rows} = {t_new / t_rows * 100:.2f}%")
    if t_old == 0:
        print(
            "\nThe old trigger never opened: it wanted high urgency AND adversarial tone\n"
            "AND an asserted low risk (these models report high risk whenever they report\n"
            "urgency), or four of eight checks when the observed maximum is two.\n"
            "Run scripts/gate_candidates.py to score triggers against the labels."
        )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
