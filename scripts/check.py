#!/usr/bin/env python3
"""Reproducible local/CI checks, including independently built native plugins."""
import argparse
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
EXT = {"Linux": ".so", "Darwin": ".dylib", "Windows": ".dll"}[platform.system()]
LIB = ("" if os.name == "nt" else "lib") + "demo_plugin" + EXT
EXE = ".exe" if os.name == "nt" else ""
START = time.monotonic()
LABEL = re.sub(r"[^A-Za-z0-9_.-]", "_", os.environ.get("RUSTUP_TOOLCHAIN", "pinned"))
NATIVE_FEATURES = ["", "alloc", "std", "macros", "alloc,macros", "std,macros", "hot-reload", "hot-reload,macros"]
WASM_FEATURES = ["wasm", "wasm,std", "wasm,macros", "wasm,hot-reload"]


def feature_selection(rustc_version, native_only=False):
    """Wasm's engine needs Rust 1.86; native callers retain the 1.85 MSRV."""
    version = re.search(r"(?m)^release: (\d+)\.(\d+)\.(\d+)", rustc_version)
    if version is None:
        raise ValueError("rustc -Vv did not contain a release version")
    wasm = not native_only and tuple(map(int, version.groups())) >= (1, 86, 0)
    flags = ["--all-features"] if wasm else ["--features", "std,macros,hot-reload"]
    return flags, NATIVE_FEATURES + (WASM_FEATURES if wasm else []), wasm


def run(*args, env=None, capture=False):
    print("+", " ".join(map(str, args)), flush=True)
    result = subprocess.run(list(map(str, args)), cwd=ROOT, env=env, text=True, encoding="utf-8",
                            stdout=subprocess.PIPE if capture else None,
                            stderr=subprocess.STDOUT if capture else None)
    if result.returncode:
        if capture:
            print(result.stdout)
        raise SystemExit(result.returncode)
    return result.stdout if capture else ""


def build_plugin(target, destination, features="", toolchain=None, release=False):
    env = dict(os.environ)
    if toolchain:
        env["RUSTUP_TOOLCHAIN"] = toolchain
    args = ["cargo", "build", "--locked", "-j2", "-p", "demo-plugin", "--target-dir", target]
    if features:
        args += ["--features", features]
    if release:
        args += ["--release"]
    run(*args, env=env)
    shutil.copy2(target / ("release" if release else "debug") / LIB, destination)


def compile_fail():
    case_dir = ROOT / "target" / ("ui-case-" + LABEL)
    case_dir.mkdir(parents=True, exist_ok=True)
    (case_dir / "src").mkdir(exist_ok=True)
    (case_dir / "Cargo.toml").write_text(
        '[package]\nname="zplugin-ui-case"\nversion="0.0.0"\nedition="2024"\n'
        '[workspace]\n[dependencies]\nzplugin={path=' + json.dumps(str(ROOT)) +
        ',features=["macros","std"]}\n', encoding="utf-8")
    fixtures = sorted((ROOT / "crates/zplugin-macros/tests/ui").glob("*.rs"))
    fixtures += sorted((ROOT / "tests/ui").glob("*.rs"))
    for path in fixtures:
        source = path.read_text(encoding="utf-8")
        expected = source.splitlines()[0].removeprefix("// error: ")
        (case_dir / "src/main.rs").write_text(source, encoding="utf-8")
        result = subprocess.run(["cargo", "check", "--offline", "--manifest-path", str(case_dir / "Cargo.toml"),
                                 "--target-dir", str(ROOT / ("target/ui-build-" + LABEL))], cwd=ROOT, text=True, encoding="utf-8",
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        if result.returncode == 0 or expected not in result.stdout:
            print(result.stdout)
            raise SystemExit(f"compile-fail contract failed: {path.name}; wanted {expected!r}")
        print("compile-fail PASS", path.name, flush=True)
    return len(fixtures)


def dynamic():
    output = ROOT / ("target/contracts-" + LABEL)
    target = ROOT / ("target/contract-build-" + LABEL)
    output.mkdir(parents=True, exist_ok=True)
    build_plugin(target, output / ("v1" + EXT))
    build_plugin(target, output / ("v2" + EXT), "v2")
    build_plugin(target, output / ("fail_init" + EXT), "fail-init")
    for case in ["missing", "null", "abi", "magic", "short", "version", "short_interface", "signature", "missing_method", "busy"]:
        run("rustc", "--edition=2024", "--crate-type=cdylib", "--crate-name", "fixture_" + case,
            ROOT / "tests/fixtures/bad_plugin.rs", "--cfg", f'fixture="{case}"',
            "-o", output / (case + EXT))
    run("cargo", "run", "--locked", "-p", "demo-host", "--", output / ("v1" + EXT), "--self-test")
    run("cargo", "run", "--locked", "-p", "zplugin-contract-tests", "--", output)


def cross_version():
    # Both directions and optimized/unoptimized builds use separate target dirs.
    output = ROOT / "target/cross-version"
    output.mkdir(parents=True, exist_ok=True)
    stable = os.environ.get("ZPLUGIN_STABLE_TOOLCHAIN", "stable")
    versions = ["1.85.0", stable]
    for index, compiler in enumerate(versions):
        build_plugin(output / ("plugin-" + str(index)), output / ("plugin-" + str(index) + EXT),
                     toolchain=compiler, release=index == 1)
    for index, compiler in enumerate(versions):
        env = dict(os.environ, RUSTUP_TOOLCHAIN=compiler)
        target = output / ("host-" + str(index))
        run("cargo", "build", "--locked", "-j2", "-p", "demo-host", "--target-dir", target, env=env)
        for plugin in range(len(versions)):
            run(target / "debug" / ("demo-host" + EXE), output / ("plugin-" + str(plugin) + EXT), "--self-test", env=env)
    print("PASS: four independently compiled host/plugin combinations; debug and release")
    run(sys.executable, "scripts/async.py", "--cross-version")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cross-version", action="store_true")
    parser.add_argument("--native-only", action="store_true", help="omit the optional Wasm engine")
    args = parser.parse_args()
    compiler = run("rustc", "-Vv", capture=True).strip()
    print(compiler, flush=True)
    if args.cross_version:
        cross_version()
        return
    run("cargo", "fmt", "--all", "--check")
    flags, configurations, wasm = feature_selection(compiler, args.native_only)
    run("cargo", "clippy", "--locked", "--workspace", "--all-targets", *flags, "--", "-D", "warnings")
    run("cargo", "doc", "--locked", "--workspace", "--no-deps", *flags,
        env=dict(os.environ, RUSTDOCFLAGS=os.environ.get("RUSTDOCFLAGS", "") + " -D warnings"))
    run("cargo", "test", "--locked", "--workspace", *flags)
    run("cargo", "test", "--locked", "-p", "zplugin", "--no-default-features")
    tree = run("cargo", "tree", "--locked", "-p", "zplugin", "--no-default-features",
               "--edges", "normal", "--prefix", "none", capture=True)
    if len(tree.strip().splitlines()) != 1:
        raise SystemExit("default runtime unexpectedly contains dependencies:\n" + tree)
    for features in configurations:
        command = ["cargo", "check", "--locked", "-p", "zplugin", "--no-default-features"]
        if features:
            command += ["--features", features]
        run(*command)
    ui = compile_fail()
    dynamic()
    run(sys.executable, "scripts/async.py")
    run(sys.executable, "-m", "unittest", "discover", "-s", "scripts", "-p", "test_*.py")
    evidence = {"platform": platform.platform(), "rustc": compiler,
                "compile_fail_cases": ui, "feature_configurations": len(configurations),
                "wasm_enabled": wasm,
                "default_runtime_dependency_count": 0,
                "elapsed_seconds": round(time.monotonic() - START, 2), "result": "pass"}
    artifact = ROOT / ("artifacts/check-" + LABEL + ".json")
    artifact.parent.mkdir(exist_ok=True)
    artifact.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(evidence, indent=2))


if __name__ == "__main__":
    main()
