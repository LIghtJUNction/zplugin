#!/usr/bin/env python3
"""Generate from Rust, compile C/C++/Zig cdylibs, then load them in a Rust host."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import time
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[1]
VERSION = "0.17.1"
# Official https://ziglang.org/download/index.json, stable release 2026-11-02.
CHECKSUMS = {
    "x86_64-linux": "3581863e3cbcfd0fce0efc682ce7e9e93da96668a977791d356a26d2a7e9b17a",
    "aarch64-linux": "a1546d5646eb184437918d6635ca6b762a0e889eac09cef720ee7a1ed3fe7795",
    "x86_64-macos": "9af07b20319fbd0c92ee46ad23e81d6a782b1aaea2cfb8dcbff35f1d2c7ddbb7",
    "aarch64-macos": "75f1bc7a1e9852f17ec9a0c1895f767c4ec56d46bc8a834920eece1d0359dd8d",
    "x86_64-windows": "b03f477a1943673e02aca0f570e8030dffd536ce1f45cbc4d3326ec31159562e",
    "aarch64-windows": "4789895bdb1e312a2ea73b2a7c0a0cc8d2ab62ba6976e834a7c54cf443377c15",
}
OS = platform.system()
EXE = ".exe" if OS == "Windows" else ""
EXT = {"Linux": ".so", "Darwin": ".dylib", "Windows": ".dll"}[OS]


def run(*args, capture=False, env=None):
    print("+", " ".join(map(str, args)), flush=True)
    result = subprocess.run(list(map(str, args)), cwd=ROOT, check=True,
                            stdout=subprocess.PIPE if capture else None,
                            text=True, encoding="utf-8", env=env)
    return result.stdout.strip() if capture else None


def zig_target():
    machine = platform.machine().lower()
    architecture = {"amd64": "x86_64", "arm64": "aarch64"}.get(machine, machine)
    return architecture + "-" + {"Linux": "linux", "Darwin": "macos", "Windows": "windows"}[OS]


def install_zig():
    target = zig_target()
    checksum = CHECKSUMS.get(target)
    if checksum is None:
        raise SystemExit("No verified Zig archive for " + target)
    name = "zig-" + target + "-" + VERSION
    directory = ROOT / "target" / "foreign-toolchain"
    binary = directory / name / ("zig" + EXE)
    if binary.exists():
        return binary
    directory.mkdir(parents=True, exist_ok=True)
    archive = directory / (name + (".zip" if OS == "Windows" else ".tar.xz"))
    url = "https://ziglang.org/download/" + VERSION + "/" + archive.name
    print("Downloading", url, flush=True)
    with urllib.request.urlopen(url, timeout=60) as response, archive.open("wb") as output:
        shutil.copyfileobj(response, output)
    with archive.open("rb") as source:
        actual = hashlib.file_digest(source, "sha256").hexdigest()
    if actual != checksum:
        raise SystemExit("Zig archive checksum mismatch; refusing to execute it")
    if OS == "Windows":
        with zipfile.ZipFile(archive) as source:
            source.extractall(directory)
    else:
        with tarfile.open(archive) as source:
            source.extractall(directory, filter="data")
    return binary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--zig", default=os.environ.get("ZIG"))
    parser.add_argument("--install-zig", action="store_true", help="install checksum-pinned compiler into target/")
    args = parser.parse_args()
    # Limit child compiler parallelism on large shared Linux hosts. This affects
    # this test process and its children only; it does not change system policy.
    if hasattr(os, "sched_getaffinity"):
        available = sorted(os.sched_getaffinity(0))
        os.sched_setaffinity(0, available[:4])
    start = time.monotonic()
    zig = args.zig or (install_zig() if args.install_zig else shutil.which("zig"))
    if not zig:
        raise SystemExit("Set ZIG or pass --zig PATH; alternatively use --install-zig")
    zig_version = run(zig, "version", capture=True)
    if zig_version != VERSION:
        raise SystemExit(f"Expected Zig {VERSION}, found {zig_version}")
    output = ROOT / "target" / "foreign"
    output.mkdir(parents=True, exist_ok=True)
    run("cargo", "build", "--locked", "-p", "zplugin-foreign-tests")
    host = ROOT / "target" / "debug" / ("zplugin-foreign-tests" + EXE)
    run(host, "--header", output / "foreign.h")
    # Both includes must work together, in either C or C++ translation units.
    # plugin.cpp imports plugin.c, which includes this same generated header.
    shared = ["-dynamiclib"] if OS == "Darwin" else ["-shared"]
    pic = [] if OS == "Windows" else ["-fPIC"]
    env = dict(os.environ, ZIG_GLOBAL_CACHE_DIR=str(output / "zig-cache"))
    libraries = []
    compilers = {}
    for compiler, standard, source, name in [
        ("cc", "c11", "plugin.c", "native_c"),
        ("c++", "c++17", "plugin.cpp", "native_cpp"),
    ]:
        library = output / (name + EXT)
        # Exercise independent platform C/C++ compilers when available. Zig's
        # C/C++ driver supplies a reproducible fallback (including Windows).
        native = None if OS == "Windows" else shutil.which(compiler)
        driver = [native] if native else [zig, compiler]
        compilers[standard] = run(*driver, "--version", capture=True).splitlines()[0]
        run(*driver, "-std=" + standard, "-O2", "-Wall", "-Wextra", "-Werror", "-pedantic",
            *shared, *pic, "-I", output, ROOT / "examples" / "foreign" / source, "-o", library, env=env)
        libraries.append(library)
    library = output / ("native_zig" + EXT)
    bindings = output / "foreign.zig"
    translated = run(zig, "translate-c", "-j4", "-lc", "-I", output,
                     ROOT / "examples" / "foreign" / "zig_api.h", capture=True, env=env)
    bindings.write_text(translated + "\n", encoding="utf-8")
    run(zig, "build-lib", "-j4", "-dynamic", "-O", "ReleaseSafe", "-lc", "--dep", "cabi",
        "-Mroot=" + str(ROOT / "examples" / "foreign" / "plugin.zig"),
        "-Mcabi=" + str(bindings), "-femit-bin=" + str(library), env=env)
    libraries.append(library)
    run(host, *libraries)
    evidence = {
        "result": "pass", "platform": platform.platform(), "rustc": run("rustc", "-Vv", capture=True),
        "zig": zig_version, "languages": ["C11", "C++17", "Zig"],
        "c_compilers": compilers,
        "contracts": ["generated schema", "stable struct/enum", "borrowed buffers", "owned bytes",
                      "owned resources", "reverse callback", "pending/wake/ready", "cancel", "library lifetime"],
        "elapsed_seconds": round(time.monotonic() - start, 2),
    }
    artifact = ROOT / "artifacts" / ("foreign-" + OS.lower() + ".json")
    artifact.parent.mkdir(exist_ok=True)
    artifact.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(evidence, indent=2))


if __name__ == "__main__":
    main()
