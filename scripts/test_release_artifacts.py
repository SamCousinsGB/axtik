import importlib.util
import os
import pathlib
import tempfile
import unittest
import zipfile

spec = importlib.util.spec_from_file_location("release_artifacts", pathlib.Path(__file__).with_name("release_artifacts.py"))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseArtifactTests(unittest.TestCase):
    def test_source_is_reproducible_and_excludes_workspace_state_without_git(self):
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            source = base / "source"
            source.mkdir()
            for name in release.ROOT_FILES:
                (source / name).write_text(name, encoding="utf-8")
            for name in ["crates/runtime.rs", "packages/vscode/src/extension.ts", "packages/vscode/test/driver/package.json", "packages/vscode/node_modules/secret", "crates/target/build.exe", "corpus/.env", ".git/config", "unlisted/secret"]:
                file = source / name
                file.parent.mkdir(parents=True, exist_ok=True)
                file.write_text("fixture", encoding="utf-8")
            first = base / "first.zip"
            second = base / "second.zip"
            release.write_source_zip(source, first)
            os.utime(source / "crates/runtime.rs", (1800000000, 1800000000))
            release.write_source_zip(source, second)
            self.assertEqual(first.read_bytes(), second.read_bytes())
            with zipfile.ZipFile(first) as archive:
                names = archive.namelist()
                self.assertIn("apx/packages/vscode/test/driver/package.json", names)
                self.assertIn("apx/crates/runtime.rs", names)
                self.assertFalse(any("secret" in name or "node_modules" in name or "/target/" in name or "/.git/" in name or "/.env" in name for name in names))

    def test_corpus_gate_rejects_missing_mutations_despite_passing_summary(self):
        tests = [{"id": f"Probe.test{i}", "path": "Probe.cls", "status": "passed"} for i in range(322)]
        pin = {"revision": "revision", "gitTree": "tree", "repository": "repo", "tests": tests}
        report = {"engineSha256": "hash", "engineVersion": "0.2.0", "revision": "revision", "gitTree": "tree", "repository": "repo", "tests": tests, "sourceUnchanged": True, "conservation": {"verified": True}, "environment": {"sha256": "env"}, "gate": {"allTestsPassed": True, "regressionFree": True, "negativeMutationsKilled": True}, "mutations": [{"id": test["id"], "killed": True} for test in tests], "validation": {"status": "preview_passed", "exitCode": 4, "summary": {"tests": 322, "passed": 322, "failed": 0, "blocked": 0, "errored": 0}}}
        release.verify_corpus(report, pin, "hash", "0.2.0", "env")
        report["mutations"].pop()
        with self.assertRaisesRegex(ValueError, "Negative mutation"):
            release.verify_corpus(report, pin, "hash", "0.2.0", "env")
        report["tests"] = tests[:-1] + [tests[0]]
        with self.assertRaisesRegex(ValueError, "identity conservation"):
            release.verify_corpus(report, pin, "hash", "0.2.0", "env")


if __name__ == "__main__":
    unittest.main()
