"""Release contract tests: untrusted inputs, immutable refs, exact source and ownership."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import release


class VersionTests(unittest.TestCase):
    def test_stable_and_prerelease_semver(self):
        self.assertLess(release.semver("1.0.0-alpha.9"), release.semver("1.0.0-alpha.10"))
        self.assertLess(release.semver("1.0.0-rc.1"), release.semver("1.0.0"))
        self.assertEqual(release.semver("1.0.0+a"), release.semver("1.0.0+b"))
        self.assertEqual(release.next_version("0.1.9", "patch"), "0.1.10")
        self.assertEqual(release.next_version("0.1.9", "minor"), "0.2.0")
        self.assertEqual(release.next_version("0.1.9", "major"), "1.0.0")
        self.assertEqual(release.next_version("0.1.9", "custom", "1.0.0-rc.1"), "1.0.0-rc.1")

    def test_rejects_ambiguous_or_injected_versions(self):
        for value in ["v1.0.0", "01.0.0", "1.0", "1.0.0-01", "1.0.0-", "1.0.0+",
                      "1.0.0\n", "1.0.0; touch owned", "$(id)", "1.0.0-α", "18446744073709551616.0.0"]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                release.semver(value)
        for bump, value in [("patch", "1.0.0"), ("custom", ""), ("custom", "0.0.1"),
                            ("custom", "0.1.0+metadata"), ("shell", "")]:
            with self.subTest(bump=bump, value=value), self.assertRaises(ValueError):
                release.next_version("0.1.0", bump, value)

    def test_request_is_exactly_typed_data(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "request.json"
            valid = {"bump": "custom", "version": "0.1.0", "dry_run": True}
            path.write_text(json.dumps(valid))
            self.assertEqual(release.request_settings(path), valid)
            for data in [dict(valid, dry_run="false"), dict(valid, extra=True), dict(valid, version=1)]:
                path.write_text(json.dumps(data))
                with self.assertRaises(ValueError):
                    release.request_settings(path)
            path.write_text('{"bump":"patch","bump":"major","version":"","dry_run":false}')
            with self.assertRaises(ValueError):
                release.request_settings(path)


class GitReleaseTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.old_directory = Path.cwd()
        self.repository = Path(self.directory.name) / "repository"
        self.repository.mkdir()
        os.chdir(self.repository)
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "Test")
        self.git("config", "user.email", "test@example.invalid")
        Path("Cargo.toml").write_text('''[workspace]
members = ["macros"]
[workspace.package]
version = "0.1.0"
[package]
name = "zplugin"
version.workspace = true
[dependencies]
zplugin-macros = { path = "macros", version = "=0.1.0", optional = true }
''')
        Path("macros").mkdir()
        Path("macros/Cargo.toml").write_text('''[package]
name = "zplugin-macros"
version.workspace = true
[dependencies]
syn = "2"
''')
        Path("Cargo.lock").write_text('''version = 4
[[package]]
name = "zplugin"
version = "0.1.0"
dependencies = ["zplugin-macros"]
[[package]]
name = "zplugin-macros"
version = "0.1.0"
[[package]]
name = "syn"
version = "2.0.100"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "preserved"
''')
        Path("CHANGELOG.md").write_text("# Changes\n\n## Unreleased\n\nInitial implementation.\n")
        Path("README.md").write_text('zplugin = { version = "0.1.0", features = ["std"] }\n')
        self.git("add", ".")
        self.git("commit", "-qm", "initial")
        self.source = self.git("rev-parse", "HEAD")
        self.remote = str(Path(self.directory.name) / "remote.git")
        subprocess.run(["git", "init", "--bare", "-q", self.remote], check=True)
        self.git("remote", "add", "origin", self.remote)
        self.git("push", "-q", "origin", "main")

    def tearDown(self):
        os.chdir(self.old_directory)
        self.directory.cleanup()

    def git(self, *args):
        return subprocess.check_output(["git", *args], text=True).strip()

    def test_candidate_commit_is_reproducible_and_pins_are_preserved(self):
        first = release.candidate("0.2.0", self.source)
        self.assertEqual(release.root_version(), "0.2.0")
        self.assertIn('version = "=0.2.0"', Path("Cargo.toml").read_text())
        lock = Path("Cargo.lock").read_text()
        self.assertIn('version = "2.0.100"', lock)
        self.assertIn('checksum = "preserved"', lock)
        self.assertIn('version = "0.2.0"', Path("README.md").read_text())
        self.assertNotIn("Unreleased", Path("CHANGELOG.md").read_text())
        self.assertEqual(release.changelog_notes("0.2.0"), "Initial implementation.")
        self.git("reset", "--hard", self.source)
        second = release.candidate("0.2.0", self.source, first)
        self.assertEqual(first, second)
        self.assertEqual(self.git("status", "--porcelain"), "")

    def test_rejects_dirty_checkout(self):
        Path("README.md").write_text("uncommitted user work")
        with self.assertRaisesRegex(ValueError, "clean checkout"):
            release.candidate("0.2.0", self.source)
        self.assertEqual(Path("README.md").read_text(), "uncommitted user work")

    def test_changelog_promotes_bracketed_pending_and_preserves_history(self):
        Path("CHANGELOG.md").write_text("# Changes\n\n## [Unreleased]\n\n- New task ABI.\n\n## [0.1.0]\n\n- Original.\n")
        self.git("add", "CHANGELOG.md")
        self.git("commit", "-qm", "pending changes")
        source = self.git("rev-parse", "HEAD")
        release.candidate("0.2.0", source)
        self.assertEqual(release.changelog_notes("0.2.0"), "- New task ABI.")
        self.assertEqual(release.changelog_notes("0.1.0"), "- Original.")
        with self.assertRaisesRegex(ValueError, "missing"):
            release.changelog_notes("0.3.0")

    def test_rejects_unexpected_candidate_commit(self):
        with self.assertRaisesRegex(ValueError, "differs"):
            release.candidate("0.2.0", self.source, "f" * 40)
        self.assertEqual(self.git("ls-remote", "origin", "refs/heads/main").split()[0], self.source)

    def test_atomic_publish_refs_and_same_run_retry(self):
        candidate = release.candidate("0.2.0", self.source)
        with patch.object(release, "registry_preflight"):
            release.push_release("0.2.0", self.source, candidate, False)
            self.assertEqual(release.tag_provenance("v0.2.0"), candidate)
            # Diagnostic outputs are ignored just as in the real repository.
            self.git("clean", "-fdq", "target")
            release.push_release("0.2.0", self.source, candidate, False)
        self.assertEqual(self.git("ls-remote", "origin", "refs/heads/main").split()[0], candidate)
        self.assertIn("refs/tags/v0.2.0", self.git("ls-remote", "origin", "refs/tags/v0.2.0"))

    def test_foreign_tag_is_never_overwritten(self):
        candidate = release.candidate("0.2.0", self.source)
        self.git("tag", "v0.2.0", self.source)
        with patch.object(release, "registry_preflight"), self.assertRaises(ValueError):
            release.push_release("0.2.0", self.source, candidate, True)
        self.assertEqual(self.git("rev-parse", "v0.2.0"), self.source)
        self.assertEqual(self.git("ls-remote", "origin", "refs/heads/main").split()[0], self.source)

    def test_concurrent_main_advance_blocks_ref_updates(self):
        candidate = release.candidate("0.2.0", self.source)
        self.git("checkout", "--detach", self.source)
        Path("concurrent.txt").write_text("other author's work")
        self.git("add", ".")
        self.git("commit", "-qm", "concurrent")
        other = self.git("rev-parse", "HEAD")
        self.git("push", "-q", "origin", "HEAD:main")
        self.git("checkout", "--detach", candidate)
        with patch.object(release, "registry_preflight"), self.assertRaisesRegex(ValueError, "main advanced"):
            release.push_release("0.2.0", self.source, candidate, False)
        self.assertEqual(self.git("ls-remote", "origin", "refs/heads/main").split()[0], other)
        self.assertFalse(release.tag_exists("v0.2.0"))


class RegistryTests(unittest.TestCase):
    def test_foreign_owner_or_repository_blocks_publication(self):
        data = {"crate": {"repository": "https://example.invalid/other"}, "versions": []}
        with patch.object(release, "api", return_value=data), self.assertRaisesRegex(ValueError, "another repository"):
            release.registry_preflight("0.1.0")
        data["crate"]["repository"] = "https://github.com/LIghtJUNction/zplugin"
        with patch.object(release, "api", side_effect=[data, {"users": [{"login": "other"}]}]), self.assertRaisesRegex(ValueError, "owner"):
            release.registry_preflight("0.1.0")

    def test_registry_collision_and_checksum_mismatch_fail_closed(self):
        data = {"crate": {"repository": "https://github.com/LIghtJUNction/zplugin"}, "versions": [{"num": "0.1.0"}]}
        with patch.object(release, "api", side_effect=[data, {"users": [{"login": "LIghtJUNction"}]}]), self.assertRaisesRegex(ValueError, "already exists"):
            release.registry_preflight("0.1.0")
        with patch.object(release, "api", return_value={"version": {"checksum": "foreign", "yanked": False}}), self.assertRaisesRegex(ValueError, "checksum differs"):
            release.registry_matches("zplugin", "0.1.0", "expected")

    def test_missing_publish_token_stops_before_registry_request(self):
        with patch.dict(os.environ, {}, clear=True), patch.object(release, "api") as api:
            with self.assertRaisesRegex(ValueError, "CARGO_REGISTRY_TOKEN"):
                release.publish("0.1.0")
            api.assert_not_called()

    def test_confirmed_identical_upload_is_idempotent(self):
        with patch.object(release, "api", return_value={"version": {"checksum": "same", "yanked": False}}):
            self.assertTrue(release.registry_matches("zplugin", "0.1.0", "same"))
        with patch.object(release, "api", return_value={"version": {"checksum": "same", "yanked": True}}), self.assertRaisesRegex(ValueError, "yanked"):
            release.registry_matches("zplugin", "0.1.0", "same")


if __name__ == "__main__":
    unittest.main()
