#!/usr/bin/env python3
"""Build independently linked Rust async plugins and exercise their native ABI."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
EXT = ".dll" if sys.platform == "win32" else ".dylib" if sys.platform == "darwin" else ".so"
LIB = ("" if sys.platform == "win32" else "lib") + "async_plugin" + EXT
EXE = "async-host" + (".exe" if sys.platform == "win32" else "")


def run(*command, toolchain=None):
    env = dict(os.environ)
    if toolchain:
        env["RUSTUP_TOOLCHAIN"] = toolchain
    print("+", " ".join(map(str, command)), flush=True)
    subprocess.run(list(map(str, command)), cwd=ROOT, env=env, check=True)


def plugin(target, destination, *, v2=False, toolchain=None, release=False):
    args = ["cargo", "build", "--locked", "-p", "async-plugin", "--target-dir", target]
    if v2:
        args += ["--features", "v2"]
    if release:
        args += ["--release"]
    run(*args, toolchain=toolchain)
    shutil.copy2(target / ("release" if release else "debug") / LIB, destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cross-version", action="store_true")
    options = parser.parse_args()
    label = os.environ.get("RUSTUP_TOOLCHAIN", "default").replace("/", "_").replace("\\", "_")
    output = ROOT / "target" / ("async-" + label)
    output.mkdir(parents=True, exist_ok=True)
    if options.cross_version:
        stable = os.environ.get("ZPLUGIN_STABLE_TOOLCHAIN", "stable")
        versions = ["1.85.0", stable]
        for index, compiler in enumerate(versions):
            plugin(output / ("plugin-" + str(index)), output / ("plugin-" + str(index) + EXT),
                   toolchain=compiler, release=index == 1)
        for index, compiler in enumerate(versions):
            target = output / ("host-" + str(index))
            run("cargo", "build", "--locked", "-p", "async-host", "--target-dir", target, toolchain=compiler)
            for other in range(len(versions)):
                run(target / "debug" / EXE, output / ("plugin-" + str(other) + EXT))
        checks = 36
    else:
        target = output / "plugin-build"
        plugin(target, output / ("v1" + EXT))
        plugin(target, output / ("v2" + EXT), v2=True)
        run("cargo", "run", "--locked", "-p", "async-host", "--", output / ("v1" + EXT), output / ("v2" + EXT))
        checks = 10
    print(json.dumps({"suite": "async-build", "cross_version": options.cross_version, "checks": checks, "status": "passed"}))


if __name__ == "__main__":
    main()
