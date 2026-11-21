"""Registry-stage release contracts; Cargo and registry calls are mocked."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest
from unittest.mock import patch

import release

GIT_URL = "https://github.com/LIghtJUNction/zsys.git"
GIT_SOURCE = "git+" + GIT_URL + "#" + "a" * 40
REGISTRY_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"
CHECKSUM = "b" * 64


class PackageProjectionTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.previous = Path.cwd()
        self.root = Path(self.directory.name).resolve() / "source"
        self.root.mkdir()
        os.chdir(self.root)
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "Test")
        self.git("config", "user.email", "test@example.invalid")
        Path(".gitignore").write_text("/target/\n")
        Path("Cargo.toml").write_text(
            '[package]\nname = "zplugin"\nversion = "0.1.0"\n'
            '[dependencies]\nzsys = { git = "' + GIT_URL + '", default-features = false, features = ["dylib"] }\n')
        Path("Cargo.lock").write_text(
            'version = 4\n[[package]]\nname = "zplugin"\nversion = "0.1.0"\n'
            'dependencies = ["zsys"]\n[[package]]\nname = "zsys"\nversion = "0.1.1"\n'
            'source = "' + GIT_SOURCE + '"\n')
        self.git("add", ".")
        self.git("commit", "-qm", "development Git source")
        self.original = {name: Path(name).read_bytes() for name in ("Cargo.toml", "Cargo.lock")}
        self.head = self.git("rev-parse", "HEAD")

    def tearDown(self):
        os.chdir(self.previous)
        self.directory.cleanup()

    def git(self, *args):
        return subprocess.check_output(["git", *args], text=True).strip()

    def api(self, path):
        if path == "crates/zsys":
            return {"crate": {"id": "zsys", "repository": GIT_URL.removesuffix(".git")}}
        if path == "crates/zsys/0.1.1":
            return {"version": {"crate": "zsys", "num": "0.1.1",
                                "checksum": CHECKSUM, "yanked": False}}
        self.fail("unexpected registry path: " + path)

    def check_packages(self, invariant):
        commands = []
        real_run = release.run

        def run(*args, **kwargs):
            if args[0] != "cargo":
                return real_run(*args, **kwargs)
            commands.append(args)
            stage = Path(kwargs.get("cwd") or Path.cwd())
            manifest = tomllib.loads((stage / "Cargo.toml").read_text())
            lock = tomllib.loads((stage / "Cargo.lock").read_text())
            invariant(manifest, lock)
            self.assertNotEqual(stage.resolve(), self.root.resolve())
            self.assertEqual(self.head, self.git("rev-parse", "HEAD"))
            self.assertEqual("", self.git("status", "--porcelain"))
            for name, data in self.original.items():
                self.assertEqual(data, Path(name).read_bytes())
            self.assertIn("--locked", args)
            self.assertIn("--all-features", args)
            self.assertEqual(Path(kwargs["env"]["CARGO_TARGET_DIR"]), self.root / "target")
            return subprocess.CompletedProcess(args, 0)

        index = {"name": "zsys", "vers": "0.1.1", "cksum": CHECKSUM, "yanked": False}
        with patch.object(release, "run", side_effect=run), \
             patch.object(release, "api", side_effect=self.api), \
             patch.object(release, "index_version", return_value=index, create=True), \
             patch.object(release, "package_hashes", return_value={}):
            release.packages("0.1.0")
        self.assertEqual([c[1] for c in commands], ["package", "publish"])
        self.assertIn("--dry-run", commands[1])

    def test_packages_project_manifest_before_cargo(self):
        def invariant(manifest, lock):
            dep = manifest["dependencies"]["zsys"]
            self.assertNotIn("git", dep, "registry packaging must not use the development Git manifest")
            self.assertEqual(dep["version"], "=0.1.1")
            self.assertEqual(dep["features"], ["dylib"])
            self.assertIs(dep["default-features"], False)
        self.check_packages(invariant)

    def test_packages_project_lock_before_cargo(self):
        def invariant(manifest, lock):
            dep = next(p for p in lock["package"] if p["name"] == "zsys")
            self.assertEqual(dep["source"], REGISTRY_SOURCE,
                             "registry packaging must not retain the development Git lock tuple")
            self.assertEqual(dep["checksum"], CHECKSUM)
            self.assertEqual(dep["version"], "0.1.1")
        self.check_packages(invariant)


if __name__ == "__main__":
    unittest.main()
