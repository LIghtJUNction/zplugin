"""Rust compiler diagnostics use UTF-8 even on Windows' legacy text locale."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import check


class FeatureSelectionTests(unittest.TestCase):
    def test_native_msrv_does_not_enable_wasm_engine(self):
        flags, configurations, wasm = check.feature_selection("rustc 1.85.0\nrelease: 1.85.0\n")
        self.assertEqual(flags, ["--features", "std,macros,hot-reload"])
        self.assertEqual(len(configurations), 8)
        self.assertFalse(wasm)

    def test_wasm_msrv_and_stable_cover_all_features(self):
        for release in ["1.86.0", "1.100.0", "1.101.0-nightly"]:
            with self.subTest(release=release):
                flags, configurations, wasm = check.feature_selection("release: " + release)
                self.assertEqual(flags, ["--all-features"])
                self.assertEqual(len(configurations), 12)
                self.assertIn("wasm", configurations)
                self.assertTrue(wasm)

    def test_native_only_is_explicit_even_on_new_compiler(self):
        flags, configurations, wasm = check.feature_selection("release: 1.100.0", native_only=True)
        self.assertNotIn("--all-features", flags)
        self.assertNotIn("wasm", configurations)
        self.assertFalse(wasm)

    def test_unrecognized_rustc_output_fails_closed(self):
        with self.assertRaisesRegex(ValueError, "release version"):
            check.feature_selection("not a compiler version")


class EncodingTests(unittest.TestCase):
    def test_capture_uses_compiler_encoding_not_windows_locale(self):
        code = "import sys; sys.stdout.buffer.write('接口'.encode('utf-8'))"
        with patch("locale.getencoding", return_value="cp1252"):
            self.assertEqual(check.run(sys.executable, "-c", code, capture=True), "接口")

    def test_compile_fail_reads_non_ascii_diagnostic(self):
        real_run = subprocess.run

        def compiler(_command, **kwargs):
            code = "import sys; sys.stdout.buffer.write('字段: unsupported field'.encode('utf-8')); sys.exit(1)"
            return real_run([sys.executable, "-c", code], **kwargs)

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixtures = root / "tests/ui"
            fixtures.mkdir(parents=True)
            (fixtures / "unicode.rs").write_text("// error: unsupported field\nfn main() {}\n", encoding="utf-8")
            with patch.object(check, "ROOT", root), patch("locale.getencoding", return_value="cp1252"), patch.object(check.subprocess, "run", side_effect=compiler):
                self.assertEqual(check.compile_fail(), 1)


if __name__ == "__main__":
    unittest.main()
