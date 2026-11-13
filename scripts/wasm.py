#!/usr/bin/env python3
"""Compile a real wasm32 guest independently and run it in the native sandbox host."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]


def run(*command, toolchain=None, capture=False):
    env = dict(os.environ)
    if toolchain:
        env["RUSTUP_TOOLCHAIN"] = toolchain
    print("+", " ".join(map(str, command)), flush=True)
    result = subprocess.run(list(map(str, command)), cwd=ROOT, env=env, check=True,
                            timeout=300, text=True, encoding="utf-8",
                            stdout=subprocess.PIPE if capture else None)
    return result.stdout.strip() if capture else ""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--guest-toolchain", help="independently select the guest compiler")
    parser.add_argument("--host-toolchain", help="independently select the native host compiler")
    options = parser.parse_args()
    started = time.monotonic()
    label = re.sub(r"[^A-Za-z0-9_.-]", "_", os.environ.get("RUSTUP_TOOLCHAIN", "pinned"))
    for name, value in [("guest", options.guest_toolchain), ("host", options.host_toolchain)]:
        if value:
            label += "-" + name + "-" + re.sub(r"[^A-Za-z0-9_.-]", "_", value)
    output = ROOT / "target" / ("wasm-" + label)
    output.mkdir(parents=True, exist_ok=True)
    guest = output / "demo.wasm"
    sdk_target = output / "sdk-build"
    sdk = sdk_target / "wasm32-unknown-unknown" / "release"
    guest_rust = run("rustc", "-Vv", toolchain=options.guest_toolchain, capture=True)
    host_rust = run("rustc", "-Vv", toolchain=options.host_toolchain, capture=True)
    run("rustfmt", "--edition=2024", "--check", ROOT / "examples/wasm/guest.rs",
        toolchain=options.guest_toolchain)
    run("cargo", "build", "--locked", "--release", "-p", "zplugin", "--lib",
        "--no-default-features", "--target=wasm32-unknown-unknown", "--target-dir", sdk_target,
        toolchain=options.guest_toolchain)
    run("rustc", "--edition=2024", "--crate-type=cdylib", "--crate-name=zplugin_wasm_demo",
        "--target=wasm32-unknown-unknown", "-Copt-level=3", "-Cpanic=abort",
        "-Ccodegen-units=1", "-Clto=thin", "-Clink-arg=-zstack-size=65536",
        "--extern", "zplugin=" + str(sdk / "libzplugin.rlib"),
        "-L", "dependency=" + str(sdk / "deps"),
        ROOT / "examples/wasm/guest.rs", "-o", guest,
        toolchain=options.guest_toolchain)
    host_output = run("cargo", "run", "--locked", "--release", "-p", "zplugin", "--example", "wasm_host",
                      "--no-default-features", "--features", "wasm,std", "--", guest, "--self-test",
                      toolchain=options.host_toolchain, capture=True)
    print(host_output, flush=True)
    host_evidence = json.loads(host_output.splitlines()[-1])
    evidence = {"suite": "wasm-independent-build", "status": "passed",
                "platform": platform.platform(), "guest_rustc": guest_rust,
                "host_rustc": host_rust, "guest_target": "wasm32-unknown-unknown",
                "guest_linear_stack_bytes": 65_536,
                "guest_bytes": guest.stat().st_size,
                "guest_sha256": hashlib.sha256(guest.read_bytes()).hexdigest(),
                "host_checks": host_evidence,
                "elapsed_seconds": round(time.monotonic() - started, 3)}
    artifact = ROOT / "artifacts" / ("wasm-" + label + ".json")
    artifact.parent.mkdir(exist_ok=True)
    artifact.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(evidence, indent=2), flush=True)


if __name__ == "__main__":
    main()
