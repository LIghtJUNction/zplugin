#!/usr/bin/env python3
"""Run the sandbox benchmark in three fresh processes and retain every batch sample."""
import argparse
import csv
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def command(*arguments, capture=False):
    result = subprocess.run(list(map(str, arguments)), cwd=ROOT, check=True, timeout=300,
                            text=True, encoding="utf-8",
                            stdout=subprocess.PIPE if capture else None)
    return result.stdout if capture else ""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--guest", type=Path, default=ROOT / "target/wasm-pinned/demo.wasm")
    parser.add_argument("--output", type=Path, default=ROOT / "artifacts/wasm-bench")
    parser.add_argument("--run-only", action="store_true")
    parser.add_argument("--smoke", action="store_true")
    arguments = parser.parse_args()
    guest = arguments.guest.resolve()
    if not guest.is_file():
        raise SystemExit(f"Missing {guest}; build the independently compiled guest with python scripts/wasm.py")
    if not arguments.run_only:
        command("cargo", "build", "--locked", "--release", "-p", "zplugin", "--example", "wasm_bench",
                "--no-default-features", "--features", "wasm,std")
    executable = ROOT / "target/release/examples" / ("wasm_bench.exe" if os.name == "nt" else "wasm_bench")
    cpu = None
    if hasattr(os, "sched_getaffinity"):
        cpu = min(os.sched_getaffinity(0))
        os.sched_setaffinity(0, {cpu})
    if arguments.smoke:
        print(command(executable, guest, "--smoke", capture=True))
        return
    cpu_name = platform.processor()
    if Path("/proc/cpuinfo").exists():
        cpu_name = next((line.split(":", 1)[1].strip() for line in
                         Path("/proc/cpuinfo").read_text().splitlines()
                         if line.startswith("model name")), cpu_name)
    metadata = {
        "suite": "wasm-benchmark", "processes": 3, "samples_per_case_per_process": 31,
        "quantiles": "batch averages, not individual-call latency percentiles",
        "quantile_method": "sorted[floor(p * (samples - 1))]",
        "source_commit": command("git", "rev-parse", "HEAD", capture=True).strip(),
        "working_tree": command("git", "status", "--porcelain", capture=True).strip(),
        "rustc": command("rustc", "-Vv", capture=True).strip(),
        "system": platform.platform(), "machine": platform.machine(), "cpu": cpu_name,
        "affinity_cpu": cpu, "guest_bytes": guest.stat().st_size,
        "guest_sha256": hashlib.sha256(guest.read_bytes()).hexdigest(),
        "profile": "release,opt-level=3,lto=thin,codegen-units=1,panic=unwind",
        "rustflags": os.environ.get("RUSTFLAGS", ""),
        "allocation_scope": "Rust global allocator; excludes allocator metadata and OS memory; peak counts requested live layouts, not temporary realloc internals",
    }
    output = arguments.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    summaries = []
    for repeat in range(1, 4):
        result = command(executable, guest, capture=True)
        path = output / f"wasm-{platform.system().lower()}-{platform.machine()}-run{repeat}.csv"
        path.write_text(result, encoding="utf-8")
        rows = list(csv.DictReader(line for line in result.splitlines() if not line.startswith("#")))
        for name in dict.fromkeys(row["name"] for row in rows):
            case = [row for row in rows if row["name"] == name]
            assert len(case) == 31, (name, len(case))
            values = sorted(float(row["ns_per_op"]) for row in case)
            total = sum(int(row["iterations"]) for row in case)
            summaries.append({
                "run": repeat, "name": name, "p50_ns": values[15], "p95_ns": values[28],
                "min_ns": values[0], "max_ns": values[-1],
                "allocations_per_op": sum(int(row["allocations"]) for row in case) / total,
                "allocated_bytes_per_op": sum(int(row["allocated_bytes"]) for row in case) / total,
                "net_live_bytes": sum(int(row["net_live_bytes"]) for row in case),
                "max_peak_extra_bytes": max(int(row["peak_extra_bytes"]) for row in case),
                "guest_memory_bytes": int(case[0]["guest_memory_bytes"]),
            })
        print(f"Saved {path}", flush=True)
    metadata["runs"] = summaries
    (output / "evidence.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    buffer = io.StringIO()
    writer = csv.DictWriter(buffer, fieldnames=list(summaries[0]))
    writer.writeheader()
    writer.writerows(summaries)
    (output / "summary.csv").write_text(buffer.getvalue(), encoding="utf-8")
    print("name,median_p50_ns,min_p50_ns,max_p50_ns,allocations_per_op,allocated_bytes_per_op,peak_extra_bytes,net_live_bytes")
    for name in dict.fromkeys(row["name"] for row in summaries):
        case = [row for row in summaries if row["name"] == name]
        values = [row["p50_ns"] for row in case]
        print(f"{name},{statistics.median(values):.3f},{min(values):.3f},{max(values):.3f},"
              f"{case[0]['allocations_per_op']:.3f},{case[0]['allocated_bytes_per_op']:.3f},"
              f"{max(row['max_peak_extra_bytes'] for row in case)},"
              f"{sum(row['net_live_bytes'] for row in case)}")


if __name__ == "__main__":
    main()
