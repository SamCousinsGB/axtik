import importlib.util
import copy
import os
import pathlib
import tempfile
import unittest
import zipfile

spec = importlib.util.spec_from_file_location("release_artifacts", pathlib.Path(__file__).with_name("release_artifacts.py"))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


def mutation_fixture(identity, status="failed"):
    """Synthetic schema-2 evidence for finalizer rejection tests, not a live observation."""
    failed = status == "failed"
    message = "Assertion Failed: synthetic probe" if failed else "Passed; no test-body assertions executed"
    target = {"id": identity, "path": "Probe.cls", "line": 1, "sha256": ("a" if failed else "b") * 64}
    witness = {"schemaVersion": 2, "command": "test", "status": "failed" if failed else "preview_passed", "exitCode": 1 if failed else 4,
        "summary": {"tests": 1, "passed": 0 if failed else 1, "failed": 1 if failed else 0, "blocked": 0, "errored": 0},
        "tests": [{"id": identity, "path": "Probe.cls", "line": 1, "status": status, "message": message,
                   "assertions": {"executed": 1 if failed else 0, "passed": 0, "failed": 1 if failed else 0},
                   "setupAssertions": {"executed": 0, "passed": 0, "failed": 0}}],
        "inputs": [{"path": "Probe.cls", "sha256": target["sha256"]}],
        "diagnostics": [{"code": "APX_ASSERTION", "severity": "error", "path": "Probe.cls", "line": 2, "column": 1,
                         "endLine": 2, "endColumn": 20, "message": message}] if failed else []}
    return {"id": identity, "kind": "assertion-inversion", "evidenceSchemaVersion": 2, "target": target, "witness": witness,
            "killed": failed, "status": status, "exitCode": witness["exitCode"], "message": message}


def weakness_fixture(pin):
    return {"schemaVersion": 1, **{key: pin[key] for key in ("repository", "revision", "gitTree")},
            "survivingAssertionInversions": ["Probe.test321"], "entrySentinels": ["Probe.test0"], "zeroAssertionTests": ["Probe.test0"]}


def provenance_fixture():
    return {"schemaVersion": 1, "catalogueVersion": "apx-capabilities-v1", "conformance": "unqualified",
        "compiler": {"coverage": "bounded-semantic-subset", "conformance": "unqualified"},
        "inventory": {"scope": "reached-native-dispatch", "complete": False, "limitations": ["Synthetic test fixture; not platform evidence."]},
        "contracts": [{"id": name, "implementation": "partial-local-model", "basis": "local-implementation", "conformance": "unqualified", "independentEvidence": []}
                      for name in ("apx.native-method.v1", "apx.native-constructor.v1", "apx.soql.v1", "apx.sosl.v1", "apx.dml.v1")]}


def test_provenance_fixture():
    return {"schemaVersion": 1, "conformance": "unqualified", "inventoryComplete": False, "truncated": False, "operations": [], "assumptions": []}


class ReleaseArtifactTests(unittest.TestCase):
    def corpus_fixture(self, survivor=False):
        tests = [{"id": f"Probe.test{i}", "path": "Probe.cls", "line": 1, "status": "passed"} for i in range(322)]
        pin = {"revision": "revision", "gitTree": "tree", "repository": "repo", "tests": copy.deepcopy(tests)}
        report = {
            "engineSha256": "hash", "engineVersion": "0.2.0", "revision": "revision", "gitTree": "tree",
            "repository": "repo", "tests": tests, "sourceUnchanged": True, "conservation": {"verified": True},
            "environment": {"sha256": "env"}, "diff": {"regressions": []},
            "gate": {"allTestsPassed": True, "regressionFree": True, "negativeControlsVerified": True,
                     "negativeMutationsKilled": not survivor, "weaknessBaselineRespected": True},
            "mutationEvidenceSchemaVersion": 2, "weaknessBaseline": weakness_fixture(pin),
            "weaknessComparison": {"verified": True, "added": {key: [] for key in ("survivingAssertionInversions", "entrySentinels", "zeroAssertionTests")}},
            "mutations": [mutation_fixture(test["id"]) for test in tests],
            "validation": {"status": "preview_passed", "exitCode": 4,
                           "summary": {"tests": 322, "passed": 322, "failed": 0, "blocked": 0, "errored": 0}},
        }
        identity = {"files": [{"path": "/fixture/Probe.cls", "sha256": "a" * 64}],
                    "directories": [{"path": "/fixture", "sha256": "b" * 64, "includeIgnored": False}]}
        report["validation"].update({"schemaVersion": 2, "inputs": identity["files"],
            "inputDirectories": identity["directories"],
            "inputFingerprint": release.sha(release.json.dumps(identity, separators=(",", ":")).encode())})
        for test in tests:
            test.update({"assertions": {"executed": 1, "passed": 1, "failed": 0},
                         "setupAssertions": {"executed": 0, "passed": 0, "failed": 0}})
        if survivor:
            report["mutations"][-1] = mutation_fixture(tests[-1]["id"], "passed")
            report["mutations"][-1]["wiringCheck"] = {**mutation_fixture(tests[-1]["id"]), "kind": "entry-sentinel-after-survivor"}
        return report, pin

    def verify(self, report, pin):
        return release.verify_corpus(report, pin, "hash", "0.2.0", "env", weakness_fixture(pin))

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
        report, pin = self.corpus_fixture()
        audit = self.verify(report, pin)
        self.assertTrue(audit["allPrimaryMutationsKilled"])
        self.assertEqual(audit["primaryControlsKilled"], 322)
        report["mutations"].pop()
        with self.assertRaisesRegex(ValueError, "Negative mutation"):
            self.verify(report, pin)
        report["tests"][-1] = report["tests"][0]
        with self.assertRaisesRegex(ValueError, "identity conservation"):
            self.verify(report, pin)

    def test_survivor_and_distinct_wiring_control_are_exposed_without_relabeling(self):
        report, pin = self.corpus_fixture(survivor=True)
        report["mutations"][0]["kind"] = "entry-sentinel"
        before = copy.deepcopy(report)
        audit = self.verify(report, pin)
        self.assertEqual(report, before)
        self.assertEqual(audit["testsWithVerifiedNegativeControls"], 322)
        self.assertEqual(audit["primaryControlsKilled"], 321)
        self.assertEqual(audit["assertionInversions"], 321)
        self.assertEqual(audit["assertionInversionsKilled"], 320)
        self.assertEqual(audit["survivingAssertionInversionCount"], 1)
        self.assertEqual(audit["survivingAssertionInversions"], ["Probe.test321"])
        self.assertEqual(audit["primaryEntrySentinelTests"], ["Probe.test0"])
        self.assertEqual(audit["additionalEntrySentinelsVerified"], 1)
        self.assertFalse(audit["allPrimaryMutationsKilled"])
        self.assertIn("1 primary assertion inversion(s) survived", release.qualification_text(audit))
        self.assertIn("coverage remains unverified", release.qualification_text(audit))

    def test_survivor_requires_separate_observed_assertion_failure(self):
        invalid = [None, {"kind": "entry-sentinel"}, {"killed": False}, {"status": "blocked"},
                   {"status": "harness-error"}, {"status": "passed"}, {"exitCode": 4}, {"exitCode": 2},
                   {"message": "DML exception"}, {"message": None}]
        for change in invalid:
            with self.subTest(change=change):
                report, pin = self.corpus_fixture(survivor=True)
                if change is None:
                    del report["mutations"][-1]["wiringCheck"]
                else:
                    report["mutations"][-1]["wiringCheck"].update(change)
                with self.assertRaisesRegex(ValueError, "verified additional wiring control"):
                    self.verify(report, pin)

    def test_fallback_cannot_hide_blocked_harness_or_other_primary_failures(self):
        invalid = [{"status": "blocked"}, {"status": "harness-error"}, {"status": "failed", "exitCode": 1},
                   {"exitCode": 1}, {"kind": "entry-sentinel"}, {"killed": None}]
        for change in invalid:
            with self.subTest(change=change):
                report, pin = self.corpus_fixture(survivor=True)
                report["mutations"][-1].update(change)
                with self.assertRaisesRegex(ValueError, "Unverified primary"):
                    self.verify(report, pin)

    def test_killed_flags_cannot_hide_non_assertion_failures(self):
        invalid = [{"status": "blocked"}, {"status": "harness-error"}, {"status": "passed"},
                   {"exitCode": 2}, {"message": "Harness failed"}, {"message": None}]
        for change in invalid:
            with self.subTest(change=change):
                report, pin = self.corpus_fixture()
                report["mutations"][0].update(change)
                with self.assertRaisesRegex(ValueError, "observed assertion failure"):
                    self.verify(report, pin)

    def test_structured_mutation_witness_rejects_wrong_test_diagnostic_counts_and_source(self):
        mutations = [
            lambda m: m.pop("evidenceSchemaVersion"),
            lambda m: m.pop("witness"),
            lambda m: m["witness"]["tests"][0].update(id="Different.test"),
            lambda m: m["witness"]["tests"][0].update(path="Different.cls"),
            lambda m: m["witness"]["tests"].append(copy.deepcopy(m["witness"]["tests"][0])),
            lambda m: m["witness"]["tests"][0].pop("assertions"),
            lambda m: m["witness"]["tests"][0]["assertions"].update(executed=2),
            lambda m: m["witness"]["tests"][0]["assertions"].update(executed=0, failed=0),
            lambda m: m["witness"]["tests"][0]["assertions"].update(executed=True, failed=True),
            lambda m: m["witness"]["summary"].update(failed=0),
            lambda m: m["witness"].update(exitCode=2),
            lambda m: m["witness"].update(diagnostics=[]),
            lambda m: m["witness"]["diagnostics"][0].update(code="APX_EXCEPTION"),
            lambda m: m["witness"]["diagnostics"][0].update(message="Unrelated assertion"),
            lambda m: m["witness"]["diagnostics"][0].update(path="Missing.cls"),
            lambda m: m["witness"]["diagnostics"][0].update(line=0),
            lambda m: m["witness"]["diagnostics"][0].update(severity="warning"),
            lambda m: m["witness"]["diagnostics"].append({**m["witness"]["diagnostics"][0], "code": "APX_EXCEPTION"}),
            lambda m: m["witness"]["inputs"][0].update(sha256="c" * 64),
            lambda m: m["witness"].update(inputs=[]),
        ]
        for change in mutations:
            report, pin = self.corpus_fixture()
            change(report["mutations"][0])
            with self.assertRaisesRegex(ValueError, "observed assertion failure"):
                self.verify(report, pin)
        report, pin = self.corpus_fixture()
        mutation = report["mutations"][0]
        message = "System.DmlException: assertion record missing"
        mutation["message"] = mutation["witness"]["tests"][0]["message"] = message
        mutation["witness"]["diagnostics"][0].update(message=message, code="APX_EXCEPTION")
        with self.assertRaisesRegex(ValueError, "observed assertion failure"):
            self.verify(report, pin)
        report, pin = self.corpus_fixture()
        mutation = report["mutations"][0]
        mutation["witness"]["diagnostics"][0]["path"] = "Helper.cls"
        mutation["witness"]["inputs"].append({"path": "Helper.cls", "sha256": "c" * 64})
        self.verify(report, pin)

    def test_historical_evidence_remains_unqualified_and_cannot_be_promoted(self):
        report, pin = self.corpus_fixture()
        del report["mutationEvidenceSchemaVersion"]
        with self.assertRaisesRegex(ValueError, "legacy receipts remain historical"):
            self.verify(report, pin)

    def test_new_candidates_require_unqualified_provenance_for_original_and_mutant_tests(self):
        report, pin = self.corpus_fixture()
        report["engineVersion"] = "0.2.3"
        verify = lambda value: release.verify_corpus(value, pin, "hash", "0.2.3", "env", weakness_fixture(pin))
        with self.assertRaisesRegex(ValueError, "Corpus provenance"):
            verify(report)
        report["validation"]["provenance"] = provenance_fixture()
        for test in report["tests"]:
            test["provenance"] = test_provenance_fixture()
        with self.assertRaisesRegex(ValueError, "Mutation provenance"):
            verify(report)
        for mutation in report["mutations"]:
            mutation["witness"]["provenance"] = provenance_fixture()
            mutation["witness"]["tests"][0]["provenance"] = test_provenance_fixture()
        verify(report)
        for change in [
            lambda r: r["validation"]["provenance"].update(conformance="qualified"),
            lambda r: r["validation"]["provenance"]["inventory"].update(complete=True),
            lambda r: r["validation"]["provenance"]["compiler"].update(conformance="qualified"),
            lambda r: r["validation"]["provenance"]["contracts"][0].update(independentEvidence=["invented"]),
            lambda r: r["validation"]["provenance"]["contracts"].pop(),
            lambda r: r["tests"][0]["provenance"].update(inventoryComplete=True),
            lambda r: r["tests"][0]["provenance"].update(conformance="qualified"),
            lambda r: r["mutations"][0]["witness"]["provenance"].update(conformance="qualified"),
            lambda r: r["mutations"][0]["witness"]["tests"][0]["provenance"].update(inventoryComplete=True),
            lambda r: r["mutations"][0]["witness"]["tests"][0]["provenance"].update(operations=[{"contractId": "invented", "operation": "x", "phase": "test", "returned": 1, "failed": 0, "blocked": 0}]),
        ]:
            changed = copy.deepcopy(report)
            change(changed)
            with self.assertRaisesRegex(ValueError, "provenance"):
                verify(changed)

    def test_new_weaker_controls_require_reviewed_identity_baseline(self):
        report, pin = self.corpus_fixture()
        report["mutations"][1]["kind"] = "entry-sentinel"
        with self.assertRaisesRegex(ValueError, "New corpus weakness.*entrySentinels"):
            self.verify(report, pin)
        report, pin = self.corpus_fixture()
        report["tests"][1].update(assertions={"executed": 0, "passed": 0, "failed": 0}, message="No assertions executed")
        with self.assertRaisesRegex(ValueError, "New corpus weakness.*zeroAssertionTests"):
            self.verify(report, pin)
        report, pin = self.corpus_fixture()
        report["mutations"][1] = mutation_fixture("Probe.test1", "passed")
        report["mutations"][1]["wiringCheck"] = {**mutation_fixture("Probe.test1"), "kind": "entry-sentinel-after-survivor"}
        report["gate"]["negativeMutationsKilled"] = False
        with self.assertRaisesRegex(ValueError, "New corpus weakness.*survivingAssertionInversions"):
            self.verify(report, pin)
        report["weaknessBaseline"]["survivingAssertionInversions"].append("Probe.test1")
        with self.assertRaisesRegex(ValueError, "differs from reviewed source"):
            self.verify(report, pin)

    def test_contradictory_gates_and_regression_evidence_are_rejected(self):
        for survivor in [False, True]:
            report, pin = self.corpus_fixture(survivor=survivor)
            report["gate"]["negativeMutationsKilled"] = survivor
            with self.assertRaisesRegex(ValueError, "gate contradicts primary"):
                self.verify(report, pin)
        for gate in ["allTestsPassed", "regressionFree", "negativeControlsVerified"]:
            report, pin = self.corpus_fixture(survivor=True)
            report["gate"][gate] = False
            with self.assertRaisesRegex(ValueError, "gates are incomplete"):
                self.verify(report, pin)
        report, pin = self.corpus_fixture(survivor=True)
        report["diff"]["regressions"] = ["Probe.test0"]
        with self.assertRaisesRegex(ValueError, "regression evidence contradicts"):
            self.verify(report, pin)
        report, pin = self.corpus_fixture(survivor=True)
        report["tests"][0]["status"] = "blocked"
        with self.assertRaisesRegex(ValueError, "every pinned test to execute and pass"):
            self.verify(report, pin)

    def test_report_requires_protocol_fingerprint_and_consistent_assertion_verdicts(self):
        for mutate, error in [
            (lambda r: r["validation"].update(schemaVersion=1), "protocol 2"),
            (lambda r: r["validation"]["inputs"][0].update(sha256="c" * 64), "fingerprint"),
            (lambda r: r["validation"]["inputDirectories"][0].pop("includeIgnored"), "policy"),
            (lambda r: r["tests"][0].pop("assertions"), "counters"),
            (lambda r: r["tests"][0]["assertions"].update(executed=2), "contradict"),
            (lambda r: r["tests"][0]["assertions"].update(executed=0, passed=0), "warning"),
        ]:
            with self.subTest(error=error):
                report, pin = self.corpus_fixture()
                mutate(report)
                with self.assertRaisesRegex(ValueError, error):
                    self.verify(report, pin)
        report, pin = self.corpus_fixture()
        report["tests"][0].update(assertions={"executed": 0, "passed": 0, "failed": 0},
                                  message="No assertions were observed; execution only")
        self.verify(report, pin)


if __name__ == "__main__":
    unittest.main()
