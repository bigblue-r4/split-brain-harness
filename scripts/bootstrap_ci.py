#!/usr/bin/env python3
"""Paired effect sizes with bootstrap confidence intervals, for the study arms.

A p-value answers "could this be nothing?" and stops. It says nothing about how
big the effect is or how precisely it was measured — and this study ran eighteen
tests, so p-values alone also invite reading the smallest one as the headline.

For each pair of arms this reports the paired accuracy difference as a point
estimate with a percentile bootstrap 95% CI, resampling the *paired rows* (not
the arms independently) so the pairing that gives the design its power is
preserved. Also reports the discordant counts behind each difference, since a
difference resting on seven discordant pairs and one resting on forty are not
the same claim.

Parse-failure rates get a Wilson score interval — normal-approximation intervals
are useless at a rate of zero, which is exactly the number of interest here.

Pure stdlib, seeded, no scipy — same posture as compare_arms.py.

Usage:
    python3 scripts/bootstrap_ci.py fixtures/dualmodel_arm{B,C,D,E}.jsonl
"""
import random
import sys
from itertools import combinations
from math import sqrt
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from compare_arms import SCORINGS, correct, load  # noqa: E402

RESAMPLES = 10000
SEED = 20260825


def wilson(k: int, n: int, z: float = 1.96) -> tuple[float, float]:
    """Wilson score interval. Degrades gracefully at k=0 and k=n, where the
    normal approximation returns a zero-width interval and lies."""
    if n == 0:
        return (0.0, 0.0)
    p = k / n
    d = 1 + z * z / n
    centre = (p + z * z / (2 * n)) / d
    half = z * sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / d
    return (max(0.0, centre - half), min(1.0, centre + half))


def paired_diff_ci(pairs: list[tuple[bool, bool]], rng: random.Random):
    """Bootstrap the paired accuracy difference (arm2 - arm1)."""
    n = len(pairs)
    if n == 0:
        return 0.0, (0.0, 0.0)
    point = sum(y - x for x, y in pairs) / n
    diffs = []
    for _ in range(RESAMPLES):
        s = 0
        for _ in range(n):
            x, y = pairs[rng.randrange(n)]
            s += y - x
        diffs.append(s / n)
    diffs.sort()
    lo = diffs[int(0.025 * RESAMPLES)]
    hi = diffs[int(0.975 * RESAMPLES) - 1]
    return point, (lo, hi)


def main() -> None:
    paths = [Path(a) for a in sys.argv[1:]]
    if len(paths) < 2:
        print(__doc__)
        sys.exit(1)
    arms = {p.stem.replace("dualmodel_arm", ""): load(p) for p in paths}

    print("\n=== Parse-failure rate (Wilson 95% CI) ===\n")
    for name, (rows, errors) in arms.items():
        n = len(rows) + errors
        lo, hi = wilson(errors, n)
        print(f"  arm {name:3}  {errors:2}/{n}  = {errors/n:.3f}   95% CI [{lo:.3f}, {hi:.3f}]")

    print(f"\n=== Paired accuracy difference, bootstrap 95% CI ({RESAMPLES} resamples, seed {SEED}) ===")
    for (n1, (r1, _)), (n2, (r2, _)) in combinations(arms.items(), 2):
        shared = sorted(set(r1) & set(r2))
        print(f"\n  arm {n1} -> arm {n2}   (n={len(shared)} paired)")
        for scoring in SCORINGS:
            rng = random.Random(SEED)
            pairs = [(correct(r1[t], scoring), correct(r2[t], scoring)) for t in shared]
            point, (lo, hi) = paired_diff_ci(pairs, rng)
            b = sum(1 for x, y in pairs if x and not y)
            c = sum(1 for x, y in pairs if y and not x)
            crosses = " (CI crosses zero)" if lo <= 0 <= hi else ""
            print(f"    [{scoring:>23}]  {point:+.3f}  95% CI [{lo:+.3f}, {hi:+.3f}]"
                  f"   discordant {b}/{c}{crosses}")


if __name__ == "__main__":
    main()
