#!/usr/bin/env python3
"""Verify both native ABI directions against the immutable published v0.1.0 source."""
import io
import json
from pathlib import Path, PurePosixPath
import subprocess
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
LEGACY_TAG = "v0.1.0"
LEGACY_COMMIT = "e06f7570eaede6f46b4100ce7c0251e4cf510822"
EXT = ".dll" if sys.platform == "win32" else ".dylib" if sys.platform == "darwin" else ".so"
LIB = ("" if sys.platform == "win32" else "lib") + "demo_plugin" + EXT
HOST = "demo-host" + (".exe" if sys.platform == "win32" else "")


def run(*command, cwd=ROOT, capture=False):
    print("+", " ".join(map(str, command)), flush=True)
    return subprocess.run(list(map(str, command)), cwd=cwd, check=True,
                          stdout=subprocess.PIPE if capture else None).stdout


def build(source, target):
    run("cargo", "build", "--locked", "-p", "demo-host", "-p", "demo-plugin",
        "--target-dir", target, cwd=source)


def main():
    actual = run("git", "rev-parse", LEGACY_TAG + "^{commit}", capture=True).decode().strip()
    if actual != LEGACY_COMMIT:
        raise SystemExit(f"refusing changed legacy tag: {actual}; expected {LEGACY_COMMIT}")
    archive = run("git", "archive", "--format=tar", LEGACY_COMMIT, capture=True)
    output = ROOT / "target" / "legacy-v1"
    with tempfile.TemporaryDirectory(prefix="zplugin-legacy-v1-") as temporary:
        legacy = Path(temporary)
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r:") as tree:
            # No links, devices, absolute names, or parent traversal are needed
            # for this repository snapshot. Reject them before materializing it.
            for member in tree.getmembers():
                name = PurePosixPath(member.name)
                if name.is_absolute() or ".." in name.parts or not (member.isfile() or member.isdir()):
                    raise SystemExit(f"unsupported archive member: {member.name}")
            tree.extractall(legacy, filter="data")
        build(legacy, output / "released")
        build(ROOT, output / "current")
        run(output / "released" / "debug" / HOST, output / "current" / "debug" / LIB, "--self-test")
        run(output / "current" / "debug" / HOST, output / "released" / "debug" / LIB, "--self-test")
    print(json.dumps({"suite": "legacy-v1", "legacy_commit": LEGACY_COMMIT,
                      "directions": ["published-host/current-plugin", "current-host/published-plugin"],
                      "status": "passed"}))


if __name__ == "__main__":
    main()
