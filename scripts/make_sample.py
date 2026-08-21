#!/usr/bin/env python3
"""Draw the fixed, label-balanced sample every study arm is graded on.

Every arm must see the identical rows in the identical order, or the paired
comparison in compare_arms.py has nothing to pair. The sample is written once,
committed, and re-drawn only by changing --seed (which invalidates prior arms).

Usage:
    python3 scripts/make_sample.py [--per-class N] [--seed S] [--output PATH]
"""
import argparse
import json
import random
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Both labeled sets, so the sample spans encoding-evasion (cyberec) and
# natural-language indirect injection (deepset) rather than one attack style.
SOURCES = [
    ("cyberec", ROOT / "fixtures" / "cyberec_sample200.jsonl"),
    ("deepset", ROOT / "fixtures" / "deepset_prompt_injections.jsonl"),
]

BENIGN = ("benign", "safe", "0", 0)


def load(path: Path) -> list[dict]:
    return [json.loads(l) for l in open(path) if l.strip()]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--per-class", type=int, default=50,
                    help="rows per (source, class) cell; 50 => 200 rows total")
    ap.add_argument("--seed", type=int, default=20260820)
    ap.add_argument("--output", default=str(ROOT / "fixtures" / "dualmodel_sample.jsonl"))
    args = ap.parse_args()

    rng = random.Random(args.seed)
    sample: list[dict] = []

    for name, path in SOURCES:
        rows = [r for r in load(path) if r.get("text", "").strip()]
        pos = [r for r in rows if r.get("label") not in BENIGN]
        neg = [r for r in rows if r.get("label") in BENIGN]
        for cls, pool in (("injection", pos), ("benign", neg)):
            if len(pool) < args.per_class:
                print(f"  warning: {name}/{cls} has {len(pool)} rows, "
                      f"asked for {args.per_class} — taking all")
            picked = rng.sample(pool, min(args.per_class, len(pool)))
            for r in picked:
                sample.append({
                    "text": r["text"],
                    "label": r.get("label", "unknown"),
                    "source": name,
                })
            print(f"  {name}/{cls}: {len(picked)}")

    # Shuffle so an interrupted arm still covers both sources and both classes —
    # a partial run stays usable instead of being all-benign-then-all-injection.
    rng.shuffle(sample)

    out = Path(args.output)
    with open(out, "w") as f:
        for r in sample:
            f.write(json.dumps(r) + "\n")
    pos = sum(1 for r in sample if r["label"] not in BENIGN)
    print(f"\n  wrote {len(sample)} rows ({pos} injection / {len(sample) - pos} benign) -> {out}")
    print(f"  seed {args.seed} — re-running with the same seed reproduces this file")


if __name__ == "__main__":
    main()
