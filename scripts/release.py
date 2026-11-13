#!/usr/bin/env python3
"""Reproducible, resumable zplugin workspace releases (Python standard library only).

The same local candidate commit is tested on every runner before any remote write.
A single atomic Git push creates the version commit and immutable annotated tag.
Registry uploads are intentionally separate, checksum-verified, resumable steps.
"""

import argparse
import hashlib
import glob
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import time
import tomllib
from urllib.error import HTTPError
from urllib.request import Request, urlopen

REPOSITORY = "LIghtJUNction/zplugin"
PACKAGES = ("zplugin-macros", "zplugin")  # dependency order
SEMVER = re.compile(
    r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
    r"(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?\Z"
)
TAG_MARKER = "zplugin-release-v1"
USER_AGENT = "zplugin-release (https://github.com/LIghtJUNction/zplugin)"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def run(*args, capture=True, check=True, env=None):
    result = subprocess.run(args, text=True, encoding="utf-8", check=False,
                            stdout=subprocess.PIPE if capture else None, env=env)
    require(not check or result.returncode == 0,
            f"command failed ({result.returncode}): {' '.join(map(str, args))}")
    return result.stdout.strip() if capture and check else result


def git(*args):
    return run("git", *args)


def semver(value):
    require(isinstance(value, str), "version must be a string")
    match = SEMVER.fullmatch(value)
    require(match is not None, f"invalid SemVer: {value!r}; use MAJOR.MINOR.PATCH (no v prefix)")
    numbers = tuple(int(n) for n in match.groups()[:3])
    require(all(n < 2**64 for n in numbers), "SemVer components must fit u64")
    parts = match[4].split(".") if match[4] else []
    require(all(not (p.isdigit() and len(p) > 1 and p.startswith("0")) for p in parts),
            "numeric prerelease identifiers must not contain leading zeroes")
    # Build metadata does not affect SemVer precedence.
    return (*numbers, not bool(parts), tuple((0, int(p)) if p.isdigit() else (1, p) for p in parts))


def next_version(current, bump, requested=""):
    old = semver(current)
    require(bump in {"patch", "minor", "major", "custom"}, "unsupported bump mode")
    if bump == "custom":
        require(bool(requested), "custom requires the version input")
        candidate = requested
    else:
        require(not requested, "version is accepted only when bump=custom")
        a, b, c = old[:3]
        candidate = {"patch": f"{a}.{b}.{c + 1}", "minor": f"{a}.{b + 1}.0",
                     "major": f"{a + 1}.0.0"}[bump]
    require(semver(candidate) >= old, f"version must not decrease: {current} -> {candidate}")
    require(semver(candidate) > old or candidate == current,
            "build metadata alone is not a version increment")
    return candidate


def request_settings(path):
    def unique(pairs):
        data = dict(pairs)
        require(len(data) == len(pairs), "duplicate release request fields")
        return data
    data = json.loads(Path(path).read_text(encoding="utf-8"), object_pairs_hook=unique)
    require(isinstance(data, dict) and set(data) == {"bump", "version", "dry_run"},
            "release request must contain exactly bump, version, and dry_run")
    require(isinstance(data["bump"], str) and data["bump"] in {"patch", "minor", "major", "custom"}, "invalid release bump")
    require(isinstance(data["version"], str) and type(data["dry_run"]) is bool,
            "release version must be a string and dry_run a boolean")
    return data


def root_version(ref=None):
    data = tomllib.loads(git("show", f"{ref}:Cargo.toml") if ref else Path("Cargo.toml").read_text())
    version = data.get("workspace", {}).get("package", {}).get("version")
    return version or data["package"]["version"]


def replace_section_version(text, section_name, version):
    pattern = r"(?ms)^\[" + re.escape(section_name) + r"\][ \t]*\n(.*?)(?=^\[|\Z)"
    match = re.search(pattern, text)
    if not match:
        return text
    body = re.sub(r'(?m)^version\s*=\s*"[^"]+"', f'version = "{version}"', match[1])
    return text[:match.start(1)] + body + text[match.end(1):]


def set_version(version, source):
    """Edit package versions and exact internal requirements, preserving unrelated pins."""
    semver(version)
    old = root_version()
    workspace = tomllib.loads(Path("Cargo.toml").read_text()).get("workspace", {})
    files = {Path("Cargo.toml")}
    for member in workspace.get("members", []):
        files.update(Path(p) for p in glob.glob(str(Path(member) / "Cargo.toml")))
    manifests = [(p, tomllib.loads(p.read_text())) for p in sorted(files)]
    workspace_names = {d["package"]["name"] for _, d in manifests if "package" in d}
    for path, data in manifests:
        text = path.read_text(encoding="utf-8")
        text = replace_section_version(text, "workspace.package", version)
        if "package" in data:
            text = replace_section_version(text, "package", version)
        # Workspace-owned dependencies are written as single-line inline tables.
        for name in workspace_names:
            pattern = r'(?m)^(\s*' + re.escape(name) + r'\s*=\s*\{[^\n]*?\bversion\s*=\s*")[^"]+("[^\n]*\})'
            text = re.sub(pattern, lambda m: m[1] + "=" + version + m[2], text)
        path.write_text(text, encoding="utf-8")
    lock = Path("Cargo.lock")
    if lock.exists():
        chunks = lock.read_text(encoding="utf-8").split("[[package]]")
        for i in range(1, len(chunks)):
            package = tomllib.loads("[[package]]" + chunks[i])["package"][0]
            if package["name"] in workspace_names and "source" not in package:
                chunks[i] = re.sub(r'(?m)^version = "[^"]+"$', f'version = "{version}"', chunks[i], count=1)
        text = "[[package]]".join(chunks)
        # Ambiguous-name lock entries sometimes carry an explicit path-package version.
        for name in workspace_names:
            text = text.replace(f'"{name} {old}"', f'"{name} {version}"')
        lock.write_text(text, encoding="utf-8")
    changelog = Path("CHANGELOG.md")
    if changelog.exists():
        text = changelog.read_text(encoding="utf-8")
        if not re.search(r"(?m)^## \[?" + re.escape(version) + r"\]?(?:\s|$)", text):
            date = git("show", "-s", "--format=%cs", source)
            pending = re.search(r"(?mi)^## (?:\[Unreleased\]|Unreleased)[^\n]*$", text)
            if pending:
                text = text[:pending.start()] + f"## [{version}] - {date}" + text[pending.end():]
            else:
                entry = f"## [{version}] - {date}\n\nValidated workspace release; see the tagged commit, CI run, and release checksums.\n\n"
                marker = re.search(r"(?m)^## ", text)
                offset = marker.start() if marker else len(text)
                text = text[:offset] + entry + text[offset:]
            changelog.write_text(text, encoding="utf-8")
    readme = Path("README.md")
    if readme.exists():
        text = re.sub(r'(?m)(^zplugin\s*=\s*(?:\{\s*version\s*=\s*)")=?' + re.escape(old) + r'(")',
                      lambda m: m[1] + version + m[2], readme.read_text(encoding="utf-8"))
        readme.write_text(text, encoding="utf-8")


def candidate(version, source, expected=""):
    require(re.fullmatch(r"[0-9a-f]{40}", source), "invalid source commit")
    require(git("rev-parse", "HEAD") == source, "checkout is not the selected source commit")
    require(not git("status", "--porcelain"), "candidate preparation needs a clean checkout")
    set_version(version, source)
    git("add", "--", "Cargo.toml", ":(glob)**/Cargo.toml", "Cargo.lock", "CHANGELOG.md", "README.md")
    tree = git("write-tree")
    commit = source
    if tree != git("rev-parse", f"{source}^{{tree}}"):
        stamp = str(int(git("show", "-s", "--format=%ct", source)) + 1) + " +0000"
        env = dict(os.environ, GIT_AUTHOR_NAME="github-actions[bot]",
                   GIT_AUTHOR_EMAIL="41898282+github-actions[bot]@users.noreply.github.com",
                   GIT_COMMITTER_NAME="github-actions[bot]",
                   GIT_COMMITTER_EMAIL="41898282+github-actions[bot]@users.noreply.github.com",
                   GIT_AUTHOR_DATE=stamp, GIT_COMMITTER_DATE=stamp)
        commit = run("git", "commit-tree", tree, "-p", source, "-m", f"release: v{version}", env=env)
        git("reset", "--hard", commit)
    require(not expected or commit == expected, "candidate commit differs from the validated release plan")
    require(not git("status", "--porcelain"), "candidate contains uncommitted files")
    return commit


def api(path):
    try:
        with urlopen(Request("https://crates.io/api/v1/" + path,
                             headers={"User-Agent": USER_AGENT}), timeout=30) as response:
            return json.load(response)
    except HTTPError as error:
        if error.code == 404:
            return None
        raise ValueError(f"crates.io returned HTTP {error.code} for {path}") from error


def registry_preflight(version, resume=False):
    for name in PACKAGES:
        data = api("crates/" + name)
        if data is None:
            print(f"{name}: registry name is currently available")
            continue
        repository = (data["crate"].get("repository") or "").rstrip("/").removesuffix(".git")
        require(repository.lower() == f"https://github.com/{REPOSITORY}".lower(),
                f"{name} belongs to another repository ({repository}); refusing publication")
        owners = api(f"crates/{name}/owners")
        require(owners is not None and any(u.get("login", "").lower() == "lightjunction" for u in owners["users"]),
                f"{name}: LIghtJUNction is not a verified registry owner")
        existing = next((v for v in data["versions"] if v["num"] == version), None)
        require(existing is None or resume, f"{name} {version} already exists without a resumable tag")
        require(existing is None or not existing.get("yanked"), f"{name} {version} is yanked")


def tag_exists(tag):
    result = run("git", "show-ref", "--verify", "--quiet", f"refs/tags/{tag}", check=False)
    require(result.returncode in (0, 1), "cannot inspect release tags")
    return result.returncode == 0


def tag_provenance(tag):
    require(git("cat-file", "-t", f"refs/tags/{tag}") == "tag", "refusing to adopt a non-annotated release tag")
    message = git("for-each-ref", "--format=%(contents)", f"refs/tags/{tag}")
    fields = dict(line.split(": ", 1) for line in message.splitlines() if ": " in line)
    commit = git("rev-parse", f"refs/tags/{tag}^{{commit}}")
    require(message.splitlines()[0] == TAG_MARKER and fields.get("Release-Commit") == commit,
            "existing tag lacks zplugin release provenance; never move or overwrite it")
    require(fields.get("Version") == tag.removeprefix("v") and root_version(commit) == fields["Version"],
            "existing tag version disagrees with its manifest")
    return commit


def emit(**values):
    print(json.dumps(values, sort_keys=True))
    if path := os.getenv("GITHUB_OUTPUT"):
        with open(path, "a", encoding="utf-8") as output:
            for key, value in values.items():
                output.write(f"{key}={str(value).lower() if type(value) is bool else value}\n")


def plan(settings):
    require(os.getenv("GITHUB_REPOSITORY", REPOSITORY) == REPOSITORY, "releases are restricted to the owning repository")
    require(os.getenv("GITHUB_REF", "refs/heads/main") == "refs/heads/main", "release from main only")
    source = git("rev-parse", "HEAD")
    git("fetch", "--no-recurse-submodules", "origin", "main", "--tags")
    requested = settings["version"]
    if requested:
        semver(requested)
    resume = settings["bump"] == "custom" and bool(requested) and tag_exists("v" + requested)
    version = requested if resume else next_version(root_version(), settings["bump"], requested)
    tag = "v" + version
    require(not tag_exists(tag) or resume, "existing tag requires custom with its exact version")
    if resume:
        require(settings["bump"] == "custom", "an existing release may only be resumed with custom and its exact version")
        commit = tag_provenance(tag)
        require(run("git", "merge-base", "--is-ancestor", commit, "origin/main", check=False).returncode == 0,
                "release tag is not in main history")
        source = commit
    else:
        require(git("rev-parse", "origin/main") == source, "main advanced since dispatch; run again from the current head")
        require(version != root_version() or not git("tag", "--list", "v*"),
                "unchanged version is only allowed for an unpublished first release or exact-tag resume")
        commit = candidate(version, source)
    registry_preflight(version, resume)
    emit(version=version, tag=tag, source=source, candidate=commit,
         resume=resume, dry_run=settings["dry_run"])


def package_hashes(version):
    paths = {}
    for name in PACKAGES:
        path = Path("target/package") / f"{name}-{version}.crate"
        require(path.is_file(), f"missing validated archive: {path}")
        with tarfile.open(path, "r:gz") as archive:
            prefix = f"{name}-{version}/"
            manifest = tomllib.loads(archive.extractfile(prefix + "Cargo.toml").read().decode())
            require(manifest["package"]["name"] == name and manifest["package"]["version"] == version,
                    "package archive identity mismatch")
            vcs = json.load(archive.extractfile(prefix + ".cargo_vcs_info.json"))
            require(vcs["git"]["sha1"] == git("rev-parse", "HEAD") and not vcs["git"].get("dirty", False),
                    f"{name}: archive provenance differs from the clean validated commit")
            require("workspace" not in manifest and "patch" not in manifest,
                    f"{name}: published manifest retains workspace or patch overrides")
            for dependency in manifest.get("dependencies", {}).values():
                require(not isinstance(dependency, dict) or not {"path", "git"}.intersection(dependency),
                        "published package contains a path or Git dependency")
        paths[name] = {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
    return paths


def packages(version):
    run("cargo", "package", "--locked", "-p", PACKAGES[0], "-p", PACKAGES[1], "--all-features", capture=False)
    data = package_hashes(version)
    Path("target/release-packages.json").write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    run("cargo", "publish", "--locked", "--dry-run", "-p", PACKAGES[0], "-p", PACKAGES[1], "--all-features", capture=False)
    require(data == package_hashes(version), "dry-run changed validated package bytes")


def record(stage, **values):
    path = Path("target/release-state.json")
    path.parent.mkdir(exist_ok=True)
    data = json.loads(path.read_text()) if path.exists() else {}
    data.update(stage=stage, **values)
    path.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    print(f"Release stage: {stage}")


def push_release(version, source, commit, resume):
    require(git("rev-parse", "HEAD") == commit and not git("status", "--porcelain"), "release checkout is not clean or exact")
    tag = "v" + version
    git("fetch", "--no-recurse-submodules", "origin", "main", "--tags")
    registry_preflight(version, resume or tag_exists(tag))
    if tag_exists(tag):
        message = git("for-each-ref", "--format=%(contents)", f"refs/tags/{tag}")
        same_run = f"Release-Run: {os.getenv('GITHUB_RUN_ID', 'local')}" in message.splitlines()
        require((resume or same_run) and tag_provenance(tag) == commit, "tag collision; refusing to overwrite")
        record("tagged", version=version, commit=commit, tag=tag)
        return
    require(not resume and git("rev-parse", "origin/main") == source,
            "main advanced during validation or resumable tag disappeared; no release refs were changed")
    require(run("git", "merge-base", "--is-ancestor", source, commit, check=False).returncode == 0,
            "candidate is not a descendant of the source")
    git("config", "user.name", "github-actions[bot]")
    git("config", "user.email", "41898282+github-actions[bot]@users.noreply.github.com")
    message = (f"{TAG_MARKER}\n\nVersion: {version}\nRelease-Source: {source}\n"
               f"Release-Commit: {commit}\nRelease-Run: {os.getenv('GITHUB_RUN_ID', 'local')}")
    git("tag", "-a", tag, "-m", message)
    # No force updates; either both refs succeed or neither changes.
    run("git", "push", "--atomic", "origin", "HEAD:refs/heads/main", f"refs/tags/{tag}", capture=False)
    record("tagged", version=version, commit=commit, tag=tag)


def registry_matches(name, version, checksum, attempts=1):
    for attempt in range(attempts):
        data = api(f"crates/{name}/{version}")
        if data is not None:
            release = data["version"]
            require(not release.get("yanked"), f"{name} {version} is yanked")
            require(release.get("checksum") == checksum,
                    f"{name} {version}: registry checksum differs; never overwrite or adopt foreign package bytes")
            return True
        if attempt + 1 < attempts:
            time.sleep(5)
    return False


def wait_index(name, version, checksum):
    path = f"{name[:2]}/{name[2:4]}/{name}"
    for attempt in range(24):
        try:
            request = Request("https://index.crates.io/" + path, headers={"User-Agent": USER_AGENT, "Cache-Control": "no-cache"})
            with urlopen(request, timeout=30) as response:
                versions = [json.loads(line) for line in response if line.strip()]
            found = next((v for v in versions if v["vers"] == version), None)
            if found:
                require(found["cksum"] == checksum and not found.get("yanked"), "sparse index package mismatch")
                return
        except HTTPError as error:
            if error.code != 404:
                raise
        if attempt < 23:
            time.sleep(5)
    raise ValueError(f"{name} {version} is not visible in the index; rerun/custom {version} to resume")


def publish(version):
    require(bool(os.getenv("CARGO_REGISTRY_TOKEN", "").strip()),
            "CARGO_REGISTRY_TOKEN is missing from GitHub Actions secrets; no registry upload attempted")
    tag = "v" + version
    require(tag_provenance(tag) == git("rev-parse", "HEAD"), "publish requires the exact verified tag commit")
    data = package_hashes(version)
    require(data == json.loads(Path("target/release-packages.json").read_text()), "validated package bytes changed")
    for name in PACKAGES:
        checksum = data[name]["sha256"]
        record("publishing", version=version, crate=name, packages=data)
        if not registry_matches(name, version, checksum):
            result = run("cargo", "publish", "--locked", "-p", name, "--all-features", capture=False, check=False)
            require(registry_matches(name, version, checksum, attempts=24),
                    f"cargo publish {name} exited {result.returncode}; publication unconfirmed. "
                    f"Use bump=custom, version={version} to resume the immutable tag.")
        wait_index(name, version, checksum)
        record("published-" + name, version=version, packages=data)
    record("registry-complete", version=version, packages=data)


def changelog_notes(version):
    text = Path("CHANGELOG.md").read_text(encoding="utf-8")
    heading = re.search(r"(?m)^## \[?" + re.escape(version) + r"\]?(?:\s[^\n]*)?\n", text)
    require(heading is not None, "release version is missing from CHANGELOG.md")
    end = re.search(r"(?m)^## ", text[heading.end():])
    body = text[heading.end():heading.end() + end.start()] if end else text[heading.end():]
    return body.strip()


def finish(version, validation_url):
    data = package_hashes(version)
    for name in PACKAGES:
        require(registry_matches(name, version, data[name]["sha256"]), "cannot create a release before registry verification")
    tag = "v" + version
    commit = tag_provenance(tag)
    notes = Path("target/release-notes.md")
    rows = "\n".join(f"| [{name}](https://crates.io/crates/{name}/{version}) | `{info['sha256']}` |" for name, info in data.items())
    notes.write_text(
        f"## zplugin {version}\n\nNative plugin ABI primitives and an opt-in untrusted Wasm sandbox.\n\n"
        "### Changes\n\n" + changelog_notes(version) + "\n\n"
        f"Validated commit: `{commit}`. [Validation run]({validation_url}).\n\n"
        "Linux, macOS and Windows compile and execute the Rust sync/async and C/C++/Zig host/plugin contract tests. "
        "The same native hosts execute independently compiled Wasm plugins; the optional engine is checked on Rust 1.86 and no_std targets. "
        "The same candidate commit passes feature checks, doctests, compile-fail diagnostics and package verification. "
        "See the repository performance report for measured numbers and environment limits.\n\n"
        f"Install: `cargo add zplugin@{version} --features std,macros,hot-reload`.\n\n"
        f"For untrusted Wasm plugins: `cargo add zplugin@{version} --features wasm`. "
        "The default native runtime remains zero-dependency; the optional Wasm backend uses Wasmi.\n\n"
        "| Package | SHA-256 |\n| --- | --- |\n" + rows + "\n\n"
        "See [CHANGELOG.md](https://github.com/LIghtJUNction/zplugin/blob/" + tag + "/CHANGELOG.md) "
        "[the native ABI](https://github.com/LIghtJUNction/zplugin/blob/" + tag + "/docs/abi.md), "
        "and [Wasm isolation and limits](https://github.com/LIghtJUNction/zplugin/blob/" + tag + "/docs/wasm.md).\n",
        encoding="utf-8")
    existing = run("gh", "release", "view", tag, "--repo", REPOSITORY, "--json", "tagName,isDraft,url", check=False)
    if existing.returncode == 0:
        release = json.loads(existing.stdout)
        require(release["tagName"] == tag and not release["isDraft"], "existing release is inconsistent; inspect it manually")
    else:
        args = ["gh", "release", "create", tag, "--repo", REPOSITORY, "--verify-tag", "--title", f"zplugin {version}", "--notes-file", str(notes)]
        if not semver(version)[3]:
            args.extend(["--prerelease", "--latest=false"])
        run(*args, capture=False)
    url = run("gh", "release", "view", tag, "--repo", REPOSITORY, "--json", "url", "--jq", ".url")
    record("complete", version=version, commit=commit, release_url=url, packages=data)
    if path := os.getenv("GITHUB_STEP_SUMMARY"):
        with open(path, "a", encoding="utf-8") as output:
            output.write(notes.read_text() + f"\n[GitHub Release]({url})\n")
    print(url)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    planning = commands.add_parser("plan")
    planning.add_argument("--request")
    planning.add_argument("--bump", default="patch")
    planning.add_argument("--version", default="")
    planning.add_argument("--dry-run", choices=("true", "false"), default="true")
    prepare = commands.add_parser("candidate")
    prepare.add_argument("--version", required=True)
    prepare.add_argument("--source", required=True)
    prepare.add_argument("--expected", default="")
    package = commands.add_parser("packages")
    package.add_argument("--version", default="")
    push = commands.add_parser("push")
    push.add_argument("--version", required=True)
    push.add_argument("--source", required=True)
    push.add_argument("--commit", required=True)
    push.add_argument("--resume", choices=("true", "false"), default="false")
    upload = commands.add_parser("publish")
    upload.add_argument("--version", required=True)
    release = commands.add_parser("finish")
    release.add_argument("--version", required=True)
    release.add_argument("--validation-url", required=True)
    args = parser.parse_args()
    if args.command == "plan":
        settings = request_settings(args.request) if args.request else {"bump": args.bump, "version": args.version, "dry_run": args.dry_run == "true"}
        plan(settings)
    elif args.command == "candidate":
        emit(candidate=candidate(args.version, args.source, args.expected))
    elif args.command == "packages":
        packages(args.version or root_version())
    elif args.command == "push":
        push_release(args.version, args.source, args.commit, args.resume == "true")
    elif args.command == "publish":
        publish(args.version)
    else:
        finish(args.version, args.validation_url)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError) as error:
        print(f"::error::{error}", file=sys.stderr)
        if Path("target/release-state.json").exists():
            print("Partial release status is preserved in target/release-state.json. "
                  "Use bump=custom and the same version to resume its immutable tag.", file=sys.stderr)
        raise SystemExit(1) from error
