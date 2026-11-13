"""The native-language runner fails promptly and keeps compiler UTF-8 output."""
import subprocess
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

import foreign


class ForeignRunnerTests(unittest.TestCase):
    def test_capture_uses_utf8_and_records_success(self):
        code = "import sys; sys.stdout.buffer.write('接口'.encode('utf-8'))"
        with patch("locale.getencoding", return_value="cp1252"):
            self.assertEqual(foreign.run(sys.executable, "-c", code, capture=True), "接口")

    def test_failed_command_is_not_reported_as_success(self):
        with self.assertRaises(subprocess.CalledProcessError) as error:
            foreign.run(sys.executable, "-c", "raise SystemExit(7)", capture=True)
        self.assertEqual(error.exception.returncode, 7)

    def test_stalled_command_has_bounded_runtime(self):
        with self.assertRaises(subprocess.TimeoutExpired):
            foreign.run(sys.executable, "-c", "import time; time.sleep(60)", timeout=0.1)

    def test_windows_architecture_is_case_insensitive(self):
        with patch.object(foreign, "OS", "Windows"), patch.object(foreign.platform, "machine", return_value="AMD64"):
            self.assertEqual(foreign.zig_target(), "x86_64-windows")

    def test_compiler_pin_matches_platform(self):
        for platform, version in [("Darwin", "0.15.2"), ("Linux", "0.17.1"), ("Windows", "0.17.1")]:
            with self.subTest(platform=platform), patch.object(foreign, "OS", platform):
                self.assertEqual(foreign.expected_zig_version(), version)

    def test_macos_links_zig_pic_object_with_native_toolchain(self):
        with patch.object(foreign, "OS", "Darwin"), patch.object(foreign.shutil, "which", return_value="/usr/bin/cc"), patch.object(foreign, "run") as run:
            foreign.build_zig_plugin("zig", Path("foreign.zig"), Path("plugin.dylib"), {})
        compile_args = run.call_args_list[0].args
        self.assertEqual(compile_args[:4], ("zig", "build-obj", "-fPIC", "-fcompiler-rt"))
        self.assertIn("-femit-bin=plugin.o", compile_args)
        self.assertEqual(run.call_args_list[1].args, ("/usr/bin/cc", "-dynamiclib", Path("plugin.o"), "-o", Path("plugin.dylib")))

    def test_other_platforms_keep_zig_shared_library_linker(self):
        with patch.object(foreign, "OS", "Linux"), patch.object(foreign, "run") as run:
            foreign.build_zig_plugin("zig", Path("foreign.zig"), Path("plugin.so"), {})
        self.assertEqual(run.call_count, 1)
        self.assertEqual(run.call_args.args[:3], ("zig", "build-lib", "-dynamic"))


if __name__ == "__main__":
    unittest.main()
