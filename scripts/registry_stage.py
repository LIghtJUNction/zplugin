"""Clean, local registry projections of committed Git-only Cargo workspaces.

No Cargo, registry, publication or remote writes occur in this module. Callers
supply exact-version registry documents. Development refs always remain on the
developer commit; archives must identify the separate clean projection HEAD.
"""
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib
from urllib.parse import urlsplit

FORMAT = "cargo-registry-projection-v1"
REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"
STAGE = Path("target/registry-stage")
RECEIPT = Path("target/release-projection.json")
PATCH = Path("target/release-projection.patch")
KINDS = ("dependencies", "build-dependencies", "dev-dependencies")
VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?\Z")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def repository(url):
    require(isinstance(url, str), "repository URL must be a string")
    parsed = urlsplit(url)
    require(parsed.scheme == "https" and parsed.netloc.lower() == "github.com"
            and not parsed.query and not parsed.fragment,
            "registry projection requires an unqualified HTTPS GitHub repository")
    path = parsed.path.rstrip("/").removesuffix(".git")
    require(re.fullmatch(r"/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", path),
            "invalid repository path")
    return "https://github.com" + path.lower()


def locked_git_packages(lock_text):
    packages = tomllib.loads(lock_text)["package"]
    result = []
    for package in packages:
        source = package.get("source", "")
        if not source.startswith("git+"):
            continue
        url, separator, commit = source[4:].rpartition("#")
        require(separator and re.fullmatch(r"[0-9a-f]{40}", commit),
                "Git lock source must contain its full commit")
        repository(url)  # rejects branch/tag/rev query selectors and credentials
        name, version = package["name"], package["version"]
        require(re.fullmatch(r"[A-Za-z0-9_-]+", name) and VERSION.fullmatch(version),
                "invalid locked Git package identity")
        prerelease = version.partition("+")[0].partition("-")[2]
        require(all(not (p.isdigit() and len(p) > 1 and p.startswith("0"))
                    for p in prerelease.split(".")),
                "numeric prerelease identifiers must not contain leading zeroes")
        require(all(int(n) < 2**64 for n in VERSION.fullmatch(version).groups()[:3]),
                "locked version components must fit u64")
        require("checksum" not in package, "Git lock entry unexpectedly has a registry checksum")
        require(sum(p["name"] == name and p["version"] == version for p in packages) == 1,
                f"{name} {version}: ambiguous source or duplicate locked package")
        result.append({"name": name, "version": version, "source": source,
                       "git": url, "commit": commit})
    return result


def registry_record(package, crate, version, index):
    """Bind the exact committed tuple to one non-yanked crates.io identity."""
    name, number = package["name"], package["version"]
    require(isinstance(crate, dict) and isinstance(version, dict) and isinstance(index, dict),
            f"{name} {number}: registry version is not available")
    crate, version = crate.get("crate", {}), version.get("version", {})
    require(crate.get("id") == name and version.get("crate") == name
            and version.get("num") == number,
            f"{name} {number}: registry package/version identity mismatch")
    require(repository(crate.get("repository")) == repository(package["git"]),
            f"{name}: registry repository differs from the locked Git source")
    checksum = version.get("checksum", "")
    require(isinstance(checksum, str) and re.fullmatch(r"[0-9a-f]{64}", checksum),
            f"{name} {number}: invalid registry checksum")
    require(version.get("yanked") is False, f"{name} {number}: registry version is yanked or unverified")
    require(index.get("name") == name and index.get("vers") == number
            and index.get("cksum") == checksum and index.get("yanked") is False,
            f"{name} {number}: sparse index identity/checksum/yank mismatch")
    return dict(package, registry=REGISTRY, checksum=checksum,
                repository=repository(crate["repository"]), yanked=False)


def dependency_tables(data):
    for kind in KINDS:
        if kind in data:
            yield (kind,), data[kind]
    workspace = data.get("workspace", {})
    if "dependencies" in workspace:
        yield ("workspace", "dependencies"), workspace["dependencies"]
    for target, tables in data.get("target", {}).items():
        for kind in KINDS:
            if kind in tables:
                yield ("target", target, kind), tables[kind]


def _table_path(header):
    node = tomllib.loads(header + "\n__projection_section__ = 1\n")
    path = []
    while "__projection_section__" not in node:
        require(len(node) == 1, "ambiguous TOML section")
        key, node = next(iter(node.items()))
        path.append(key)
    return tuple(path)


def _value(value):
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=False)
    if type(value) is bool:
        return str(value).lower()
    if isinstance(value, list):
        return "[" + ", ".join(_value(item) for item in value) + "]"
    raise ValueError("unsupported Git dependency field; refusing a lossy TOML rewrite")


def project_manifest(text, records):
    data = tomllib.loads(text)
    require(not {"patch", "replace"}.intersection(data), "registry stage refuses patch/replace overrides")
    expected = copy.deepcopy(data)
    replacements = {}
    expected_tables = dict(dependency_tables(expected))
    for section, table in dependency_tables(data):
        for alias, dependency in table.items():
            if not isinstance(dependency, dict) or "git" not in dependency:
                continue
            require(not {"path", "registry", "registry-index", "workspace", "rev", "branch",
                         "tag", "version"}.intersection(dependency),
                    f"{alias}: projection accepts Git-only development locations; do not relax an extra selector")
            name = dependency.get("package", alias)
            matches = [r for r in records if r["name"] == name
                       and repository(r["git"]) == repository(dependency["git"])]
            require(len(matches) == 1, f"{alias}: expected one committed Git package, found {len(matches)}")
            record = matches[0]
            projected = {key: value for key, value in dependency.items() if key != "git"}
            projected["version"] = "=" + record["version"]
            expected_tables[section][alias] = projected
            replacements[section + (alias,)] = (dependency, projected)
    if not replacements:
        return text
    lines, section, changed = [], (), set()
    for line in text.splitlines(keepends=True):
        header = re.fullmatch(r"(\[[^\n]+\])[ \t]*(?:#[^\n]*)?\r?\n?", line)
        if header and not header[1].startswith("[["):
            section = _table_path(header[1])
        for path, (old, new) in replacements.items():
            if section == path:
                field = re.fullmatch(r"([ \t]*)git[ \t]*=[ \t]*([^\n]+?)(\r?\n)?", line)
                if field:
                    require(tomllib.loads(line).get("git") == old["git"], "Git field parse mismatch")
                    line = field[1] + "version = " + _value(new["version"]) + (field[3] or "")
                    changed.add(path)
                    break
            if section != path[:-1]:
                continue
            assignment = re.fullmatch(r"([ \t]*(?:[A-Za-z0-9_-]+|\"(?:[^\"\\]|\\.)*\"|'[^']*')[ \t]*=[ \t]*)\{(.*)\}([ \t]*(?:#[^\n]*)?)(\r?\n)?", line)
            if not assignment:
                continue
            parsed = tomllib.loads(assignment[1] + "{" + assignment[2] + "}")
            if parsed != {path[-1]: old}:
                continue
            fields = ", ".join(key + " = " + _value(value) for key, value in new.items())
            line = assignment[1] + "{ " + fields + " }" + assignment[3] + (assignment[4] or "")
            changed.add(path)
            break
        lines.append(line)
    result = "".join(lines)
    require(changed == set(replacements), "unsupported Git dependency formatting; no partial projection is allowed")
    require(tomllib.loads(result) == expected, "projection changed fields other than Git locations")
    return result


def project(manifests, lock_text, records):
    """Return only changed text files. Inputs are immutable committed bytes."""
    packages = locked_git_packages(lock_text)
    require(len(records) == len(packages), "registry records do not cover the committed Git lock")
    by_source = {}
    for package in packages:
        matches = [r for r in records if all(r.get(k) == v for k, v in package.items())]
        require(len(matches) == 1, "registry record does not bind exactly one committed Git tuple")
        record = matches[0]
        require(record.get("registry") == REGISTRY and record.get("yanked") is False
                and repository(record.get("repository")) == repository(package["git"])
                and re.fullmatch(r"[0-9a-f]{64}", record.get("checksum", "")),
                "invalid stored registry projection record")
        by_source[(package["name"], package["version"], package["source"])] = record
    changes = {}
    for path, text in manifests.items():
        projected = project_manifest(text, records)
        if projected != text:
            changes[path] = projected
    expected = copy.deepcopy(tomllib.loads(lock_text))
    chunks = lock_text.split("[[package]]")
    for i in range(1, len(chunks)):
        package = tomllib.loads("[[package]]" + chunks[i])["package"][0]
        key = (package["name"], package["version"], package.get("source", ""))
        if key in by_source:
            record = by_source[key]
            pattern = r'(?m)^source[ \t]*=[ \t]*' + re.escape(json.dumps(package["source"])) + r"[ \t]*$"
            chunks[i], count = re.subn(pattern, "source = " + json.dumps(REGISTRY) +
                                      "\nchecksum = " + json.dumps(record["checksum"]), chunks[i])
            require(count == 1, "unsupported lock source formatting")
    projected_lock = "[[package]]".join(chunks)
    for package in expected["package"]:
        key = (package["name"], package["version"], package.get("source", ""))
        if key in by_source:
            package.update(source=REGISTRY, checksum=by_source[key]["checksum"])
        for index, dependency in enumerate(package.get("dependencies", [])):
            for record in records:
                old = f'{record["name"]} {record["version"]} ({record["source"]})'
                new = f'{record["name"]} {record["version"]} ({REGISTRY})'
                if dependency == old:
                    package["dependencies"][index] = new
                    projected_lock = projected_lock.replace(json.dumps(old), json.dumps(new))
    require(tomllib.loads(projected_lock) == expected, "lock projection changed unrelated fields")
    require(not any(p.get("source", "").startswith("git+") for p in expected["package"])
            and not any("(git+" in d for p in expected["package"] for d in p.get("dependencies", [])),
            "unmapped Git source remains in the registry lock")
    if projected_lock != lock_text:
        changes["Cargo.lock"] = projected_lock
    return changes


def _git(root, *args, strip=True, input=None, env=None):
    result = subprocess.run(["git", "-C", str(root), *args],
                            input=input.encode("utf-8") if input is not None else None,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, check=False)
    require(result.returncode == 0, "Git staging command failed: " + " ".join(args)
            + "\n" + result.stderr.decode("utf-8", errors="replace"))
    text = result.stdout.decode("utf-8")
    return text.strip() if strip else text


def _committed(root):
    require(Path(_git(root, "rev-parse", "--show-toplevel")).resolve() == root, "expected the repository root")
    require(not _git(root, "status", "--porcelain"), "registry projection requires a clean developer checkout")
    developer = _git(root, "rev-parse", "HEAD")
    paths = _git(root, "ls-tree", "-r", "--name-only", developer).splitlines()
    manifests = {p: _git(root, "show", f"{developer}:{p}", strip=False)
                 for p in paths if p == "Cargo.toml" or p.endswith("/Cargo.toml")}
    require("Cargo.toml" in manifests and "Cargo.lock" in paths, "committed manifest/lock required")
    lock = _git(root, "show", f"{developer}:Cargo.lock", strip=False)
    return developer, manifests, lock


def environment(root):
    return dict(os.environ, CARGO_TARGET_DIR=str(Path(root).resolve() / "target"))


def load(root):
    """Read and verify a stage before packaging, publishing, or checksum reuse."""
    root = Path(root).resolve()
    developer, manifests, lock = _committed(root)
    receipt = json.loads((root / RECEIPT).read_text(encoding="utf-8"))
    require(receipt["format"] == FORMAT and receipt["developer_commit"] == developer
            and receipt["developer_tree"] == _git(root, "rev-parse", "HEAD^{tree}")
            and receipt["developer_lock_sha256"] == sha256(lock.encode()),
            "registry stage belongs to a different developer candidate")
    changes = project(manifests, lock, receipt["dependencies"])
    stage = root / STAGE
    require(not stage.is_symlink() and stage.resolve() == stage,
            "registry stage path must be a local owned directory")
    require(Path(_git(stage, "rev-parse", "--show-toplevel")).resolve() == stage, "stage is not its own Git checkout")
    commit = _git(stage, "rev-parse", "HEAD")
    require(commit == receipt["projection_commit"]
            and _git(stage, "rev-parse", "HEAD^{tree}") == receipt["projection_tree"]
            and not _git(stage, "status", "--porcelain"), "registry stage is dirty or has changed")
    expected_parents = developer if changes else ""
    if changes:
        require(_git(stage, "show", "-s", "--format=%P", commit) == expected_parents,
                "projection must have exactly the developer candidate as its parent")
    else:
        require(commit == developer, "unchanged projection must keep the developer commit")
    changed_paths = _git(stage, "diff", "--name-only", "--no-renames", developer, commit).splitlines()
    require(sorted(changed_paths) == sorted(changes), "projection changed files outside manifest/lock locations")
    for path, text in changes.items():
        require(_git(stage, "show", f"{commit}:{path}", strip=False) == text
                and (stage / path).read_bytes() == text.encode(),
                "projection contents differ from the committed lock: " + path)
    require((stage / "Cargo.lock").is_file()
            and sha256((stage / "Cargo.lock").read_bytes()) == receipt["projection_lock_sha256"],
            "projection lock bytes changed")
    return receipt


def prepare(root, api, index_version):
    """Create a deterministic clean clone. The injected readers may use the registry."""
    root = Path(root).resolve()
    developer, manifests, lock = _committed(root)
    records = []
    for package in locked_git_packages(lock):
        name, version = package["name"], package["version"]
        records.append(registry_record(package, api(f"crates/{name}"),
                                       api(f"crates/{name}/{version}"), index_version(name, version)))
    changes = project(manifests, lock, records)
    stage = root / STAGE
    require(stage.parent.resolve() == stage.parent and not stage.parent.is_symlink(),
            "registry stage parent must be a local owned directory")
    if stage.exists():
        existing = load(root)
        require(existing["dependencies"] == records, "registry identity changed since stage preparation")
        return existing
    require(not (root / RECEIPT).exists(), "projection receipt exists without its stage")
    stage.parent.mkdir(parents=True, exist_ok=True)
    _git(root, "clone", "--no-hardlinks", "--no-checkout", str(root), str(stage))
    _git(stage, "config", "core.autocrlf", "false")
    _git(stage, "remote", "remove", "origin")  # this checkout never publishes refs
    _git(stage, "checkout", "--detach", developer)
    for path, text in changes.items():
        destination = stage / path
        require(not destination.is_symlink(), "cannot project a symlink manifest")
        destination.write_bytes(text.encode())
    if changes:
        _git(stage, "add", "--", *sorted(changes))
        tree = _git(stage, "write-tree")
        stamp = int(_git(root, "show", "-s", "--format=%ct", developer)) + 1
        identity = "registry-projection"
        env = dict(os.environ, GIT_AUTHOR_NAME=identity, GIT_COMMITTER_NAME=identity,
                   GIT_AUTHOR_EMAIL="registry-projection@example.invalid",
                   GIT_COMMITTER_EMAIL="registry-projection@example.invalid",
                   GIT_AUTHOR_DATE=f"@{stamp} +0000", GIT_COMMITTER_DATE=f"@{stamp} +0000")
        commit = _git(stage, "-c", "commit.gpgsign=false", "commit-tree", tree, "-p", developer,
                      input=f"registry projection of {developer}\n", env=env)
        _git(stage, "reset", "--hard", commit)
    else:
        commit, tree = developer, _git(stage, "rev-parse", "HEAD^{tree}")
    receipt = {
        "format": FORMAT,
        "developer_commit": developer,
        "developer_tree": _git(root, "rev-parse", "HEAD^{tree}"),
        "developer_lock_sha256": sha256(lock.encode()),
        "projection_commit": commit,
        "projection_tree": tree,
        "projection_lock_sha256": sha256((stage / "Cargo.lock").read_bytes()),
        "stage_path": STAGE.as_posix(),
        "stage_refs_published": False,
        "dependencies": records,
        "changed_files": [{"path": p, "sha256": sha256(t.encode()),
                           "git_blob": _git(stage, "rev-parse", f"{commit}:{p}")}
                          for p, t in sorted(changes.items())],
        "validation_scope": "registry projection; developer and registry dependency sources are distinct",
    }
    (root / RECEIPT).write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    (root / PATCH).write_text(_git(stage, "diff", "--full-index", developer, commit, strip=False),
                             encoding="utf-8")
    return load(root)
