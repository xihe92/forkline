#!/usr/bin/env python3
"""Fail if any criterion benchmark regressed by more than a threshold.

Run the benchmarks twice: first on the base revision with
`cargo bench -p forkline-engine --bench engine -- --save-baseline base`,
then on the change with `-- --baseline base`. Criterion writes the relative
change of each benchmark to target/criterion/**/change/estimates.json; this
script reads them.

    scripts/bench-check.py [threshold_percent=10]
"""
import glob
import json
import sys

threshold = float(sys.argv[1]) / 100 if len(sys.argv) > 1 else 0.10
files = sorted(glob.glob("target/criterion/**/change/estimates.json", recursive=True))
if not files:
    sys.exit("no criterion change estimates found; run the benchmarks against a saved baseline first")

worst = []
for path in files:
    name = path.removeprefix("target/criterion/").removesuffix("/change/estimates.json")
    change = json.load(open(path))["mean"]["point_estimate"]
    flag = "REGRESSION" if change > threshold else ""
    print(f"{name:<50} {change * 100:+7.2f}% {flag}")
    if change > threshold:
        worst.append((name, change))

if worst:
    sys.exit(f"{len(worst)} benchmark(s) regressed by more than {threshold * 100:.0f}%")
print(f"no benchmark regressed by more than {threshold * 100:.0f}%")
