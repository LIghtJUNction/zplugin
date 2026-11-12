"""Rust compiler diagnostics use UTF-8 even on Windows' legacy text locale."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import check


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
