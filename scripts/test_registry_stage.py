"""Pure Python/local-Git fixtures for registry staging; no Cargo or live registry."""
import copy
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import unittest
from unittest.mock import patch

import registry_stage as stage
import release

URL = "https://github.com/LIghtJUNction/zsys.git"
SOURCE = "git+" + URL + "#" + "a" * 40
CHECKSUM = "b" * 64
LOCK = '''version = 4
[[package]]
name = "zplugin"
version = "0.1.0"
dependencies = ["zsys"]
[[package]]
name = "zsys"
version = "0.1.1"
source = "''' + SOURCE + '''"
[[package]]
name = "syn"
version = "2.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "''' + "c" * 64 + '''"
'''
MANIFEST = '''[package]
name = "zplugin"
version = "0.1.0"
[dependencies]
zsys = { git = "''' + URL + '''", default-features = false, features = ["dylib"], optional = true }
macros = { package = "zplugin-macros", path = "macros", version = "=0.1.0", optional = true }
syn = "2"
[features]
std = ["dep:zsys"]
'''


def api(path):
    if path == "crates/zsys":
        return {"crate": {"id": "zsys", "repository": URL.removesuffix(".git")}}
    if path == "crates/zsys/0.1.1":
        return {"version": {"crate": "zsys", "num": "0.1.1", "yanked": False, "checksum": CHECKSUM}}
    raise AssertionError("unexpected registry call: " + path)


def index(name, version):
    assert (name, version) == ("zsys", "0.1.1")
    return {"name": name, "vers": version, "cksum": CHECKSUM, "yanked": False}


def records():
    package = stage.locked_git_packages(LOCK)[0]
    return [stage.registry_record(package, api("crates/zsys"), api("crates/zsys/0.1.1"), index("zsys", "0.1.1"))]


class ProjectionTests(unittest.TestCase):
    def test_exact_projection_preserves_features_alias_paths_and_unrelated_lock(self):
        manifest = MANIFEST.replace("zsys = {", "system = { package = \"zsys\",")
        original = tomllib.loads(manifest)
        lock_original = tomllib.loads(LOCK)
        changes = stage.project({"Cargo.toml": manifest}, LOCK, records())
        actual = tomllib.loads(changes["Cargo.toml"])
        expected = copy.deepcopy(original)
        del expected["dependencies"]["system"]["git"]
        expected["dependencies"]["system"]["version"] = "=0.1.1"
        self.assertEqual(actual, expected)
        locked = tomllib.loads(changes["Cargo.lock"])
        expected_lock = copy.deepcopy(lock_original)
        expected_lock["package"][1].update(source=stage.REGISTRY, checksum=CHECKSUM)
        self.assertEqual(locked, expected_lock)
        self.assertEqual(tomllib.loads(manifest), original)
        self.assertEqual(tomllib.loads(LOCK), lock_original)

    def test_build_metadata_hyphens_are_not_prerelease_numbers(self):
        version = "1.2.3+build-01"
        try:
            package = stage.locked_git_packages(LOCK.replace('version = "0.1.1"', 'version = "' + version + '"'))[0]
        except ValueError as error:
            self.fail("valid build metadata was mistaken for a prerelease: " + str(error))
        self.assertEqual(package["version"], version)
        with self.assertRaisesRegex(ValueError, "prerelease"):
            stage.locked_git_packages(LOCK.replace('version = "0.1.1"', 'version = "1.2.3-01+build-02"'))

    def test_target_workspace_and_table_form_dependencies_preserve_inheritance(self):
        text = '''[workspace.dependencies]
sys = { package = "zsys", git = "''' + URL + '''", features = ["dylib"] }
[dependencies]
sys = { workspace = true, optional = true }
[target.'cfg(windows)'.build-dependencies.system]
package = "zsys"
git = "''' + URL + '''"
default-features = false
features = ["dylib"]
'''
        result = tomllib.loads(stage.project_manifest(text, records()))
        self.assertEqual(result["dependencies"], tomllib.loads(text)["dependencies"])
        self.assertEqual(result["workspace"]["dependencies"]["sys"]["version"], "=0.1.1")
        table = result["target"]["cfg(windows)"]["build-dependencies"]["system"]
        self.assertNotIn("git", table)
        self.assertEqual(table["version"], "=0.1.1")
        self.assertIs(table["default-features"], False)
        self.assertEqual(table["features"], ["dylib"])

    def test_extra_source_selectors_are_not_silently_relaxed(self):
        for key, value in [("rev", '"abc"'), ("tag", '"v1"'), ("branch", '"main"'),
                           ("path", '"../zsys"'), ("registry", '"private"'),
                           ("registry-index", '"https://example.invalid"'),
                           ("version", '"1"'), ("workspace", "true")]:
            text = MANIFEST.replace('optional = true }', f'{key} = {value}, optional = true }}', 1)
            with self.subTest(key=key), self.assertRaises(ValueError):
                stage.project({"Cargo.toml": text}, LOCK, records())

    def test_missing_ambiguous_or_qualified_git_lock_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "expected one"):
            stage.project_manifest(MANIFEST.replace('package]\n', 'package]\n', 1).replace(URL, URL.replace("zsys", "other")), records())
        duplicate = LOCK + '\n[[package]]\nname = "zsys"\nversion = "0.1.1"\nsource = "' + stage.REGISTRY + '"\nchecksum = "' + CHECKSUM + '"\n'
        with self.assertRaisesRegex(ValueError, "ambiguous"):
            stage.locked_git_packages(duplicate)
        with self.assertRaises(ValueError):
            stage.locked_git_packages(LOCK.replace(URL, URL + "?branch=main"))
        with self.assertRaises(ValueError):
            stage.locked_git_packages(LOCK.replace("a" * 40, "a" * 7))

    def test_qualified_transitive_lock_reference_is_rewritten_exactly(self):
        old = 'zsys 0.1.1 (' + SOURCE + ')'
        lock = LOCK.replace('dependencies = ["zsys"]', "dependencies = [" + json.dumps(old) + "]")
        changes = stage.project({"Cargo.toml": MANIFEST}, lock, records())
        reference = tomllib.loads(changes["Cargo.lock"])["package"][0]["dependencies"]
        self.assertEqual(reference, ["zsys 0.1.1 (" + stage.REGISTRY + ")"])

    def test_registry_identity_repository_checksum_and_yank_fail_closed(self):
        package = stage.locked_git_packages(LOCK)[0]
        crate, version, sparse = api("crates/zsys"), api("crates/zsys/0.1.1"), index("zsys", "0.1.1")
        cases = []
        for field, value in [("crate", "foreign"), ("num", "0.2.0"),
                             ("checksum", "invalid"), ("yanked", True)]:
            item = copy.deepcopy(version)
            item["version"][field] = value
            cases.append((crate, item, sparse))
        foreign = copy.deepcopy(crate)
        foreign["crate"]["repository"] = "https://github.com/other/zsys"
        cases.append((foreign, version, sparse))
        for field, value in [("name", "foreign"), ("vers", "0.2.0"),
                             ("cksum", "d" * 64), ("yanked", True)]:
            item = dict(sparse, **{field: value})
            cases.append((crate, version, item))
        cases.extend([(None, version, sparse), (crate, None, sparse), (crate, version, None)])
        for documents in cases:
            with self.subTest(documents=documents), self.assertRaises(ValueError):
                stage.registry_record(package, *documents)

    def test_patch_and_lossy_or_unmapped_format_is_rejected(self):
        with self.assertRaises(ValueError):
            stage.project_manifest(MANIFEST + '\n[patch.crates-io]\n', records())
        with self.assertRaises(ValueError):
            stage.project({"Cargo.toml": MANIFEST}, LOCK, [])
        # Dotted assignment is intentionally unsupported rather than partly rewritten.
        text = '[dependencies]\nzsys.git = "' + URL + '"\n'
        with self.assertRaisesRegex(ValueError, "unsupported"):
            stage.project_manifest(text, records())


class StageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name) / "source"
        self.root.mkdir()
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "Test")
        self.git("config", "user.email", "test@example.invalid")
        (self.root / ".gitignore").write_text("/target/\n")
        (self.root / "Cargo.toml").write_text(MANIFEST)
        (self.root / "Cargo.lock").write_text(LOCK)
        (self.root / "README.md").write_text("source remains unchanged\n")
        self.git("add", ".")
        self.git("commit", "-qm", "fixture source")
        self.developer = self.git("rev-parse", "HEAD")
        self.original = {p: (self.root / p).read_bytes() for p in ("Cargo.toml", "Cargo.lock", "README.md")}

    def tearDown(self):
        self.temp.cleanup()

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.root), *args], text=True).strip()

    def prepare(self):
        return stage.prepare(self.root, api, index)

    def test_clean_projection_is_deterministic_and_developer_is_unchanged(self):
        first = self.prepare()
        self.assertNotEqual(first["projection_commit"], self.developer)
        self.assertEqual(first["developer_commit"], self.developer)
        self.assertEqual(stage._git(self.root / stage.STAGE, "remote"), "")
        self.assertEqual(stage._git(self.root / stage.STAGE, "show", "-s", "--format=%P", "HEAD"), self.developer)
        self.assertEqual(self.git("rev-parse", "HEAD"), self.developer)
        self.assertEqual(self.git("status", "--porcelain"), "")
        for path, value in self.original.items():
            self.assertEqual((self.root / path).read_bytes(), value)
        shutil.rmtree(self.root / stage.STAGE)
        (self.root / stage.RECEIPT).unlink()
        (self.root / stage.PATCH).unlink()
        self.assertEqual(self.prepare(), first)

    def test_committed_blob_reader_preserves_crlf_bytes(self):
        self.git("config", "core.autocrlf", "false")
        payload = b"first\r\nsecond\r\n"
        (self.root / "line-endings.txt").write_bytes(payload)
        self.git("add", "line-endings.txt")
        self.git("commit", "-qm", "CRLF source fixture")
        self.assertEqual(stage._git(self.root, "show", "HEAD:line-endings.txt", strip=False).encode(), payload)

    def test_dirty_developer_is_rejected_before_stage_creation(self):
        (self.root / "README.md").write_text("uncommitted\n")
        with self.assertRaisesRegex(ValueError, "clean developer"):
            self.prepare()
        self.assertFalse((self.root / stage.STAGE).exists())

    def test_stage_mutation_and_foreign_developer_are_rejected(self):
        self.prepare()
        path = self.root / stage.STAGE / "Cargo.toml"
        old = path.read_bytes()
        path.write_bytes(old + b"\n")
        with self.assertRaisesRegex(ValueError, "dirty"):
            stage.load(self.root)
        path.write_bytes(old)
        self.git("commit", "--allow-empty", "-qm", "another developer")
        with self.assertRaisesRegex(ValueError, "different developer"):
            stage.load(self.root)

    def test_changed_registry_identity_cannot_reuse_stage(self):
        self.prepare()
        def changed_api(path):
            result = api(path)
            if "version" in result:
                result["version"]["checksum"] = "d" * 64
            return result
        def changed_index(name, version):
            return dict(index(name, version), cksum="d" * 64)
        with self.assertRaisesRegex(ValueError, "identity changed"):
            stage.prepare(self.root, changed_api, changed_index)

    def archive(self, proof, *, vcs_commit=None, dirty=False, target_git=False, bad_checksum=False):
        manifest = '[package]\nname = "zplugin"\nversion = "0.1.0"\n[dependencies]\nzsys = "=0.1.1"\n'
        if target_git:
            manifest += '\n[target.\'cfg(windows)\'.build-dependencies]\nfoo = { git = "https://github.com/owner/repo.git" }\n'
        lock = (self.root / stage.STAGE / "Cargo.lock").read_bytes()
        if bad_checksum:
            lock = lock.replace(CHECKSUM.encode(), ("d" * 64).encode())
        vcs = {"git": {"sha1": vcs_commit or proof["projection_commit"], "dirty": dirty}, "path_in_vcs": ""}
        members = {"Cargo.toml": manifest.encode(),
                   "Cargo.toml.orig": (self.root / stage.STAGE / "Cargo.toml").read_bytes(),
                   "Cargo.lock": lock, ".cargo_vcs_info.json": json.dumps(vcs).encode()}
        path = self.root / "target/package/zplugin-0.1.0.crate"
        path.parent.mkdir(exist_ok=True)
        with tarfile.open(path, "w:gz") as archive:
            for name, data in members.items():
                info = tarfile.TarInfo("zplugin-0.1.0/" + name)
                info.size = len(data)
                archive.addfile(info, io.BytesIO(data))

    def hashes(self):
        previous = Path.cwd()
        try:
            os.chdir(self.root)
            with patch.object(release, "PACKAGES", ("zplugin",)):
                return release.package_hashes("0.1.0")
        finally:
            os.chdir(previous)

    def test_archive_binds_projection_head_original_manifest_and_lock(self):
        proof = self.prepare()
        self.archive(proof)
        data = self.hashes()["zplugin"]
        self.assertEqual(data["developer_commit"], self.developer)
        self.assertEqual(data["projection_commit"], proof["projection_commit"])
        self.assertEqual(data["projection_lock_sha256"], proof["projection_lock_sha256"])
        for kwargs in ({"vcs_commit": self.developer}, {"dirty": True},
                       {"target_git": True}, {"bad_checksum": True}):
            with self.subTest(kwargs=kwargs):
                self.archive(proof, **kwargs)
                with self.assertRaises(ValueError):
                    self.hashes()


    def test_publish_uses_stage_but_tag_and_resume_keep_developer_identity(self):
        proof = self.prepare()
        previous = Path.cwd()
        data = {"zplugin": {"path": "target/package/zplugin-0.1.0.crate", "sha256": CHECKSUM}}
        (self.root / "target/release-packages.json").write_text(json.dumps(data))
        commands = []
        original_run = release.run

        def run(*args, **kwargs):
            if args[0] != "cargo":
                return original_run(*args, **kwargs)
            commands.append(args)
            self.assertEqual(Path(kwargs["cwd"]), self.root / stage.STAGE)
            self.assertEqual(Path(kwargs["env"]["CARGO_TARGET_DIR"]), self.root / "target")
            self.assertEqual(stage.load(self.root), proof)
            self.assertEqual(args, ("cargo", "publish", "--locked", "-p", "zplugin", "--all-features"))
            return subprocess.CompletedProcess(args, 0)

        try:
            os.chdir(self.root)
            with patch.dict(os.environ, {"CARGO_REGISTRY_TOKEN": "synthetic-test-token"}), \
                 patch.object(release, "PACKAGES", ("zplugin",)), \
                 patch.object(release, "tag_provenance", return_value=self.developer) as tag, \
                 patch.object(release, "api", side_effect=api), \
                 patch.object(release, "index_version", side_effect=index), \
                 patch.object(release, "package_hashes", return_value=data), \
                 patch.object(release, "run", side_effect=run), \
                 patch.object(release, "wait_index") as wait:
                with patch.object(release, "registry_matches", side_effect=[False, True]) as matches:
                    release.publish("0.1.0")
                    self.assertEqual(matches.call_args_list[0].args, ("zplugin", "0.1.0", CHECKSUM))
                    self.assertEqual(matches.call_args_list[1].kwargs, {"attempts": 24})
                self.assertEqual(len(commands), 1)
                with patch.object(release, "registry_matches", return_value=True):
                    release.publish("0.1.0")
                self.assertEqual(len(commands), 1, "an identical published checksum must not be uploaded again")
                self.assertEqual(tag.call_args.args, ("v0.1.0",))
                self.assertEqual(wait.call_count, 2)
            self.assertEqual(self.git("rev-parse", "HEAD"), self.developer)
            self.assertEqual(self.git("status", "--porcelain"), "")
        finally:
            os.chdir(previous)

    def test_missing_registry_version_does_not_create_partial_stage(self):
        with self.assertRaises(ValueError):
            stage.prepare(self.root, lambda path: None, index)
        self.assertFalse((self.root / stage.STAGE).exists())
        self.assertEqual(self.git("status", "--porcelain"), "")


if __name__ == "__main__":
    unittest.main()
