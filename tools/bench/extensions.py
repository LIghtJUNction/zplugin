#!/usr/bin/env python3
"""Build then measure the task ABI in three fresh processes on one CPU."""
import argparse
import csv
import os
from pathlib import Path
import platform
import statistics
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--build-only", action="store_true")
    parser.add_argument("--run-only", action="store_true")
    parser.add_argument("--smoke", action="store_true")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    target = root / "artifacts/benchmark-target"
    output = (args.output or root / "docs/benchmarks/v0.2.0").resolve()
    if not args.run_only:
        subprocess.run([
            "cargo", "build", "--locked", "--release", "-j2", "--target-dir", str(target),
            "-p", "async-plugin", "-p", "zplugin-bench", "--features", "async-plugin/instrument",
        ], cwd=root, check=True)
    if args.build_only:
        return
    suffix = ".dll" if os.name == "nt" else ".dylib" if platform.system() == "Darwin" else ".so"
    plugin = target / "release" / f"{'' if os.name == 'nt' else 'lib'}async_plugin{suffix}"
    executable = target / "release" / ("extensions.exe" if os.name == "nt" else "extensions")
    cpu = None
    if hasattr(os, "sched_getaffinity"):
        cpu = min(os.sched_getaffinity(0))
        os.sched_setaffinity(0, {cpu})
    command = [str(executable), str(plugin)]
    if args.smoke:
        print(subprocess.check_output(command + ["--smoke"], cwd=root, text=True, encoding="utf-8"))
        return
    metadata = subprocess.check_output(["rustc", "-Vv"], text=True, encoding="utf-8")
    source = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    changed = subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True).strip().replace("\n", "; ")
    cpu_name = platform.processor()
    if Path("/proc/cpuinfo").exists():
        cpu_name = next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")), cpu_name)
    preamble = (f"# source={source}; dirty={changed or 'none'}\n"
                f"# system={platform.platform()}; cpu={cpu_name}; affinity_cpu={cpu}\n"
                "# profile=release,opt-level=3,lto=thin,codegen-units=1,panic=unwind\n"
                f"# RUSTFLAGS={os.environ.get('RUSTFLAGS', '')}\n"
                + "".join(f"# {line}\n" for line in metadata.splitlines()))
    output.mkdir(parents=True, exist_ok=True)
    runs = []
    for repeat in range(1, 4):
        result = subprocess.check_output(command, cwd=root, text=True, encoding="utf-8", timeout=120)
        path = output / f"extensions-{platform.system().lower()}-{platform.machine()}-run{repeat}.csv"
        path.write_text(preamble + f"# repeat={repeat}\n" + result, encoding="utf-8")
        runs.append({row["name"]: row for row in csv.DictReader(line for line in result.splitlines() if not line.startswith("#"))})
        print(f"Saved {path}", flush=True)
    print("name,median_of_p50_ns,min_p50_ns,max_p50_ns,total_allocations_per_op")
    for name in runs[0]:
        values = [float(run[name]["p50_ns"]) for run in runs]
        print(f"{name},{statistics.median(values):.3f},{min(values):.3f},{max(values):.3f},{runs[0][name]['total_allocations_per_op']}")


if __name__ == "__main__":
    main()
