"""Independent Wasm guests must find transitive SDK libraries in every Cargo layout."""
import json
from pathlib import Path
import unittest

import wasm


def artifact(name, *filenames, fresh=False):
    return {"reason": "compiler-artifact", "target": {"name": name},
            "filenames": list(map(str, filenames)), "fresh": fresh}


def stream(*messages):
    return "\n".join(json.dumps(message) for message in messages)


class SdkArtifactTests(unittest.TestCase):
    def test_package_build_directories_include_transitive_libraries(self):
        root = Path("build output") / "wasm32-unknown-unknown" / "release"
        zsys = root / "build" / "zsys" / "123" / "out"
        zplugin = root / "build" / "zplugin" / "456" / "out"
        sdk, directories = wasm.sdk_artifacts(stream(
            artifact("zsys", zsys / "libzsys-123.rlib", zsys / "libzsys-123.rmeta"),
            artifact("zplugin", root / "libzplugin.rlib", zplugin / "libzplugin-456.rmeta"),
            {"reason": "build-finished", "success": True},
        ))
        self.assertEqual(sdk, root / "libzplugin.rlib")
        self.assertEqual(set(directories), {root, zsys, zplugin})

    def test_legacy_deps_layout_and_fresh_artifacts(self):
        root = Path("target") / "wasm32-unknown-unknown" / "release"
        deps = root / "deps"
        sdk, directories = wasm.sdk_artifacts(stream(
            artifact("zsys", deps / "libzsys-123.rlib", deps / "libzsys-123.rmeta", fresh=True),
            artifact("zplugin", root / "libzplugin.rlib", deps / "libzplugin-456.rmeta", fresh=True),
            artifact("build-script-build", root / "build-script-build"),
        ))
        self.assertEqual(sdk, root / "libzplugin.rlib")
        self.assertEqual(set(directories), {root, deps})
        self.assertEqual(len(directories), 2)

    def test_missing_sdk_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "did not report"):
            wasm.sdk_artifacts(stream(artifact("zsys", "libzsys.rlib")))

    def test_ambiguous_sdk_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "multiple"):
            wasm.sdk_artifacts(stream(
                artifact("zplugin", Path("first") / "libzplugin.rlib"),
                artifact("zplugin", Path("second") / "libzplugin.rlib"),
            ))


if __name__ == "__main__":
    unittest.main()
