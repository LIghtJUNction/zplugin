#!/usr/bin/env python3
"""Build the benchmark, rotate baseline order, and retain all three raw runs."""

import csv
import os
from pathlib import Path
import statistics
import subprocess
import sys


def main():
    root = Path(__file__).resolve().parents[2]
    output = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else root / "docs/benchmarks"
    output.mkdir(parents=True, exist_ok=True)
    target = root / "artifacts/benchmark-target"
    subprocess.run(
        [
            "cargo", "build", "--locked", "--release", "-j2",
            "--target-dir", str(target), "-p", "demo-plugin", "-p", "zplugin-bench",
            "--features", "demo-plugin/instrument",
        ],
        cwd=root,
        check=True,
    )
    cpu = None
    if hasattr(os, "sched_getaffinity"):
        cpu = min(os.sched_getaffinity(0))
        os.sched_setaffinity(0, {cpu})
    extension = ".dll" if sys.platform == "win32" else ".dylib" if sys.platform == "darwin" else ".so"
    prefix = "" if sys.platform == "win32" else "lib"
    executable = target / "release" / ("zplugin-bench.exe" if sys.platform == "win32" else "zplugin-bench")
    plugin = target / "release" / f"{prefix}demo_plugin{extension}"
    runs = []
    machine = "linux-x86_64" if sys.platform == "linux" and os.uname().machine == "x86_64" else sys.platform
    for repeat, rotation in enumerate((0, 2, 4), 1):
        environment = dict(os.environ, ZPLUGIN_BENCH_ROTATION=str(rotation))
        result = subprocess.check_output([str(executable), str(plugin)], cwd=root, env=environment, text=True)
        raw = f"# affinity_cpu={cpu},repeat={repeat}\n{result}"
        path = output / f"{machine}-run{repeat}.csv"
        path.write_text(raw, encoding="utf-8")
        runs.append({row["name"]: row for row in csv.DictReader(line for line in result.splitlines() if not line.startswith("#"))})
        print(f"Saved {path.relative_to(root) if path.is_relative_to(root) else path}")
    print("name,median_of_p50_ns,min_p50_ns,max_p50_ns")
    for name in runs[0]:
        values = [float(run[name]["p50_ns"]) for run in runs]
        print(f"{name},{statistics.median(values):.3f},{min(values):.3f},{max(values):.3f}")


if __name__ == "__main__":
    main()
