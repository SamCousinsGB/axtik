"""Synthetic artifact/evidence fixtures; these are not native-host observations."""
import copy
import importlib.util
import io
import json
import pathlib
import struct
import subprocess
import tarfile
import tempfile
import unittest
import zipfile

spec = importlib.util.spec_from_file_location("portable_release", pathlib.Path(__file__).with_name("finalize-portable-release.py"))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)
VERSION = "0.2.2"
EXTENSION = "apx-local.apx-local-apex"


def native(target):
    if target.startswith("win32-"):
        data = bytearray(512)
        data[:2] = b"MZ"
        struct.pack_into("<I", data, 0x3c, 128)
        data[128:132] = b"PE\0\0"
        struct.pack_into("<HH", data, 132, 0x8664 if target.endswith("x64") else 0xaa64, 1)
        struct.pack_into("<HHH", data, 148, 240, 2, 0x20b)
        struct.pack_into("<I", data, 152 + 60, 512)
        struct.pack_into("<I", data, 152 + 108, 16)
        struct.pack_into("<III", data, 392 + 12, 4096, 0, 512)
    elif target.startswith("linux-"):
        data = bytearray(128)
        data[:7] = b"\x7fELF\x02\x01\x01"
        struct.pack_into("<HH", data, 16, 2, 62 if target.endswith("x64") else 183)
        struct.pack_into("<Q", data, 32, 64)
        struct.pack_into("<HH", data, 54, 56, 1)
        struct.pack_into("<I", data, 64, 1)
        struct.pack_into("<Q", data, 64 + 32, len(data))
    else:
        data = bytearray(32)
        struct.pack_into("<IIII", data, 0, 0xfeedfacf, 0x01000007 if target.endswith("x64") else 0x0100000c, 0, 2)
    return bytes(data)


def write_json(file, value):
    file.parent.mkdir(parents=True, exist_ok=True)
    file.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def engine_receipt(target, data):
    actual, file_format, linkage = release.native_identity(data)
    platform, arch = target.split("-")
    return {"schemaVersion": 1, "target": actual, "version": VERSION, "observedVersion": f"apx {VERSION}",
            "sha256": release.sha(data), "bytes": len(data), "host": {"platform": platform, "arch": arch},
            "format": file_format, "linkage": linkage, "dependencies": []}


def host_receipt(target, engine_hash, vsix_hash):
    platform, arch = target.split("-")
    checks = [{"name": name, "passed": True} for name in sorted(release.HOST_CHECKS)]
    host = {"schemaVersion": 1, "extensionId": EXTENSION, "version": VERSION,
            "extensionRoot": f"/synthetic/installed/{EXTENSION}-{VERSION}", "engineSha256": engine_hash,
            "extensionHost": "v24.0.0", "vscodeVersion": "1.144.0", "platform": platform, "arch": arch, "results": checks}
    return {**copy.deepcopy(host), "target": target, "kind": "installed-vsix", "verified": True,
            "installExitCode": 0, "hostExitCode": 0, "vsixSha256": vsix_hash, "host": host}


def corpus_receipt(engine_hash, environment_hash):
    tests = [{"id": f"Probe.test{index}", "path": "Probe.cls", "status": "passed",
              "assertions": {"executed": 1, "passed": 1, "failed": 0},
              "setupAssertions": {"executed": 0, "passed": 0, "failed": 0}} for index in range(322)]
    identity = {"files": [{"path": "/synthetic/Probe.cls", "sha256": "a" * 64}],
                "directories": [{"path": "/synthetic", "sha256": "b" * 64, "includeIgnored": False}]}
    pin = {"revision": "revision", "gitTree": "tree", "repository": "repo", "tests": copy.deepcopy(tests)}
    report = {"engineSha256": engine_hash, "engineVersion": VERSION, "revision": "revision", "gitTree": "tree",
        "repository": "repo", "tests": tests, "sourceUnchanged": True, "conservation": {"verified": True},
        "environment": {"sha256": environment_hash}, "diff": {"regressions": []},
        "gate": {"allTestsPassed": True, "regressionFree": True, "negativeControlsVerified": True, "negativeMutationsKilled": True},
        "mutations": [{"id": test["id"], "kind": "assertion-inversion", "killed": True, "status": "failed", "exitCode": 1,
                       "message": "System.AssertException: Assertion Failed"} for test in tests],
        "validation": {"schemaVersion": 2, "status": "preview_passed", "exitCode": 4, "inputs": identity["files"],
            "inputDirectories": identity["directories"], "inputFingerprint": release.sha(json.dumps(identity, separators=(",", ":")).encode()),
            "summary": {"tests": 322, "passed": 322, "failed": 0, "blocked": 0, "errored": 0}}}
    return report, pin


class PortableReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)
        self.engines = self.root / "engines"
        self.packages = self.root / "packages"
        self.packages.mkdir()
        self.evidence = self.root / "evidence"
        self.source = self.root / "source"
        self.source.mkdir()
        self.info = {}
        for target in release.TARGETS:
            data = native(target)
            executable = "apx.exe" if target.startswith("win32-") else "apx"
            folder = self.engines / target
            folder.mkdir(parents=True)
            (folder / executable).write_bytes(data)
            write_json(folder / "engine.json", engine_receipt(target, data))
            self.info[target] = release.verify_engine(self.engines, target, VERSION)
        self.manifest = {"schemaVersion": 2, "version": VERSION, "target": "universal", "targets": list(release.TARGETS),
                         "vsix": f"apx-local-apex-{VERSION}-universal.vsix", "plugin": f"apx-sf-plugin-{VERSION}.tgz",
                         "engines": list(self.info.values())}
        write_json(self.packages / "packaging-manifest.json", self.manifest)
        self.make_packages()
        environment = b"{}\n"
        for target in release.TARGETS:
            report, pin = corpus_receipt(self.info[target]["sha256"], release.sha(environment))
            write_json(self.evidence / target / "corpus.json", report)
            write_json(self.evidence / target / "installed-vsix.json", host_receipt(target, self.info[target]["sha256"], self.vsix_hash))
        for name in release.base.ROOT_FILES:
            (self.source / name).write_text(name + "\n", encoding="utf-8")
        (self.source / ".gitignore").write_text("", encoding="utf-8")
        write_json(self.source / "package.json", {"version": VERSION})
        write_json(self.source / "corpus/apex-recipes.pin.json", pin)
        (self.source / "corpus/apex-recipes.environment.json").write_bytes(environment)

    def make_packages(self, extra_zip=None):
        common = {"package.json": json.dumps({"version": VERSION}).encode(),
                  "node_modules/@apx/runner/package.json": json.dumps({"version": VERSION}).encode(),
                  "node_modules/@apx/runner/lib/index.js": b"// synthetic runner"}
        for target, info in self.info.items():
            name = "apx.exe" if target.startswith("win32-") else "apx"
            common[f"bin/{target}/{name}"] = native(target)
            common[f"bin/{target}/engine.json"] = json.dumps(info["receipt"]).encode()
        zip_entries = {"extension.vsixmanifest": b'<PackageManifest><Metadata><Identity Id="apx-local-apex"/></Metadata></PackageManifest>',
            "extension/package.json": json.dumps({"version": VERSION, "publisher": "apx-local", "name": "apx-local-apex", "extensionKind": ["workspace"]}).encode(),
            "extension/dist/extension.js": b"// synthetic extension"}
        zip_entries.update({"extension/sf-plugin/" + name: data for name, data in common.items()})
        zip_entries.update({"extension/" + name: data for name, data in common.items() if name.startswith("bin/")})
        if extra_zip:
            zip_entries.update(extra_zip)
        with zipfile.ZipFile(self.packages / self.manifest["vsix"], "w", zipfile.ZIP_DEFLATED) as archive:
            for name, data in zip_entries.items():
                info = zipfile.ZipInfo(name)
                info.create_system = 3
                info.external_attr = (0o100755 if name.endswith("/apx") else 0o100644) << 16
                archive.writestr(info, data)
        with tarfile.open(self.packages / self.manifest["plugin"], "w:gz") as archive:
            for name, data in common.items():
                info = tarfile.TarInfo("package/" + name)
                info.size = len(data)
                info.mode = 0o755 if name.endswith("/apx") else 0o644
                archive.addfile(info, io.BytesIO(data))
        self.vsix_hash = release.sha((self.packages / self.manifest["vsix"]).read_bytes())

    def test_all_six_native_architectures_and_eighteen_archive_copies_are_required(self):
        release.verify_package_manifest(self.manifest, VERSION, self.info)
        _, _, extension = release.verify_packages(self.packages, self.manifest, VERSION, self.info)
        self.assertEqual(extension, EXTENSION)
        for target in release.TARGETS:
            self.assertEqual(release.native_identity(native(target))[0], target)
        for change in (lambda m: m["targets"].pop(), lambda m: m["engines"].append(m["engines"][0])):
            manifest = copy.deepcopy(self.manifest)
            change(manifest)
            with self.assertRaisesRegex(ValueError, "six targets"):
                release.verify_package_manifest(manifest, VERSION, self.info)

    def test_engine_receipt_cannot_mislabel_architecture_version_or_hash(self):
        target = "win32-x64"
        receipt_file = self.engines / target / "engine.json"
        original = release.read_json(receipt_file)
        for change, expected in [({"sha256": "0" * 64}, "hash"), ({"version": "0.2.0"}, "version"),
                                 ({"host": {"platform": "linux", "arch": "x64"}}, "target host")]:
            write_json(receipt_file, {**original, **change})
            with self.assertRaisesRegex(ValueError, expected):
                release.verify_engine(self.engines, target, VERSION)
        write_json(receipt_file, original)
        (self.engines / target / "apx.exe").write_bytes(native("win32-arm64"))
        with self.assertRaisesRegex(ValueError, "architecture"):
            release.verify_engine(self.engines, target, VERSION)

    def test_linux_dynamic_loader_is_not_a_portable_static_engine(self):
        data = bytearray(native("linux-x64"))
        struct.pack_into("<I", data, 64, 3)
        with self.assertRaisesRegex(ValueError, "dynamic interpreter"):
            release.native_identity(data)

    def test_platform_dependency_inspection_rejects_extra_runtime_requirements(self):
        data = bytearray(native("win32-x64"))
        struct.pack_into("<II", data, 152 + 120, 432, 40)
        struct.pack_into("<I", data, 432 + 12, 480)
        data[480:480 + 13] = b"kernel32.dll\0"
        self.assertEqual(release.native_dependencies(data), ["kernel32.dll"])
        data[480:480 + 16] = b"vcruntime140.dll\0"
        with self.assertRaisesRegex(ValueError, "non-system DLL"):
            release.native_dependencies(data)
        for library, allowed in [(b"/usr/lib/libSystem.B.dylib", True), (b"@rpath/extra.dylib", False)]:
            size = ((24 + len(library) + 1 + 7) // 8) * 8
            data = bytearray(native("darwin-arm64")) + bytearray(size)
            struct.pack_into("<II", data, 16, 1, size)
            struct.pack_into("<III", data, 32, 0xc, size, 24)
            data[56:56 + len(library)] = library
            if allowed:
                self.assertEqual(release.native_dependencies(data), [library.decode()])
            else:
                with self.assertRaisesRegex(ValueError, "non-system dylib"):
                    release.native_dependencies(data)

    def test_host_receipt_rejects_partial_wrong_target_and_wrong_package(self):
        target = "darwin-arm64"
        receipt = host_receipt(target, self.info[target]["sha256"], self.vsix_hash)
        release.verify_host(receipt, target, VERSION, EXTENSION, self.info[target]["sha256"], self.vsix_hash)
        mutations = [
            (lambda h: h.update(vsixSha256="0" * 64), "another VSIX"),
            (lambda h: h.update(verified=False), "complete successfully"),
            (lambda h: h.update(arch="x64"), "platform or version"),
            (lambda h: h["results"].pop(), "missing or duplicated"),
            (lambda h: h["host"]["results"].append(h["host"]["results"][0]), "missing or duplicated"),
            (lambda h: h["host"].update(engineSha256="0" * 64), "engine hash"),
            (lambda h: h.update(extensionRoot="/development/source"), "installed extension"),
        ]
        for mutate, message in mutations:
            with self.subTest(message=message):
                changed = copy.deepcopy(receipt)
                mutate(changed)
                with self.assertRaisesRegex(ValueError, message):
                    release.verify_host(changed, target, VERSION, EXTENSION, self.info[target]["sha256"], self.vsix_hash)

    def test_each_target_requires_its_own_complete_corpus_and_host_evidence(self):
        observed = release.verify_evidence(self.evidence, self.source, VERSION, self.info, EXTENSION, self.vsix_hash)
        self.assertEqual(set(observed), set(release.TARGETS))
        file = self.evidence / "linux-arm64/corpus.json"
        report = release.read_json(file)
        report["mutations"].pop()
        write_json(file, report)
        with self.assertRaisesRegex(ValueError, "Negative mutation evidence"):
            release.verify_evidence(self.evidence, self.source, VERSION, self.info, EXTENSION, self.vsix_hash)
        file.unlink()
        with self.assertRaises(FileNotFoundError):
            release.verify_evidence(self.evidence, self.source, VERSION, self.info, EXTENSION, self.vsix_hash)

    def test_archive_rejects_unsafe_paths_and_changed_native_copy(self):
        for name in ("extension/../outside.txt", "extension/runtime.js:hidden"):
            with self.subTest(name=name):
                self.make_packages({name: b"bad"})
                with self.assertRaisesRegex(ValueError, "Unsafe archive"):
                    release.verify_packages(self.packages, self.manifest, VERSION, self.info)
        self.make_packages({"extension/sf-plugin/bin/darwin-x64/apx": native("darwin-arm64")})
        with self.assertRaisesRegex(ValueError, "engine hash differs"):
            release.verify_packages(self.packages, self.manifest, VERSION, self.info)

    def commit_source(self):
        def git(*args):
            return subprocess.check_output(["git", "-C", str(self.source), *args], stderr=subprocess.PIPE)
        git("init", "-q")
        git("add", ".")
        git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "Synthetic source fixture")
        return git("rev-parse", "HEAD").decode().strip()

    def test_finalization_requires_committed_source_and_is_reproducible(self):
        commit = self.commit_source()
        output = self.root / "output"
        result = release.finalize(self.source, self.packages, self.engines, self.evidence, output)
        self.assertEqual(result["sourceCommit"], commit)
        self.assertEqual(result["nativeArchiveCopiesVerified"], 18)
        archive = (output / f"apx-{VERSION}-source.zip").read_bytes()
        release.finalize(self.source, self.packages, self.engines, self.evidence, output)
        self.assertEqual(archive, (output / f"apx-{VERSION}-source.zip").read_bytes())
        evidence = release.read_json(output / "release-evidence.json")
        self.assertIn("no Salesforce org comparison", evidence["qualification"])
        self.assertNotIn("/synthetic/", json.dumps(evidence))
        (self.source / "README.md").write_text("uncommitted change", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "uncommitted or untracked"):
            release.finalize(self.source, self.packages, self.engines, self.evidence, self.root / "rejected-output")
        self.assertFalse((self.root / "rejected-output").exists())

    def test_source_archive_cannot_omit_committed_files(self):
        self.commit_source()
        (self.source / "corpus/apex-recipes.environment.json").unlink()
        with self.assertRaisesRegex(ValueError, "omits committed files"):
            release.verify_committed_source(self.source)


if __name__ == "__main__":
    unittest.main()
