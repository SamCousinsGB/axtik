"""Finalize local release artifacts from frozen, independently checked evidence.

Uses Python's standard library; never builds, downloads, publishes, or modifies
the upstream corpus. Archives source through an explicit allowlist, including
untracked source files. Repeated identical source bytes produce identical ZIPs.
"""
import argparse
import hashlib
import json
import pathlib
import re
import shutil
import stat
import tarfile
import zipfile

ROOT_FILES = {
    ".gitattributes", ".gitignore", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "package.json",
    "package-lock.json", "README.md", "LICENSE", "NOTICE", "THIRD_PARTY_NOTICES.txt",
    "CONTRIBUTING.md", "SECURITY.md",
}
ROOT_DIRS = {".github", "crates", "docs", "fixtures", "packages", "packaging", "scripts", "corpus", "third-party"}
EXCLUDED = {"node_modules", "target", ".git", ".packaging", ".tmp", ".corpus-history", "dist", "lib", "__pycache__", ".sf", ".sfdx", ".apx-cache"}
BINARY_SUFFIXES = {".exe", ".dll", ".pdb", ".vsix", ".tgz", ".zip", ".pyc", ".class"}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def read_json(path):
    return json.loads(pathlib.Path(path).read_text(encoding="utf-8-sig"))


def source_files(root):
    root = pathlib.Path(root).resolve()
    output = []

    def collect(folder):
        for entry in sorted(folder.iterdir()):
            if entry.name in EXCLUDED or entry.name == ".env" or entry.name.startswith(".env."):
                continue
            require(not entry.is_symlink(), f"Source symlink is not allowed: {entry}")
            if entry.is_dir():
                collect(entry)
            elif entry.is_file() and entry.suffix.lower() not in BINARY_SUFFIXES:
                require(entry.stat().st_size <= 64 * 1024 * 1024, f"Source file exceeds archive bound: {entry}")
                output.append(entry.relative_to(root).as_posix())

    for name in sorted(ROOT_FILES):
        path = root / name
        require(path.is_file() and not path.is_symlink(), f"Required source file missing: {name}")
        output.append(name)
    for name in sorted(ROOT_DIRS):
        path = root / name
        if path.exists():
            require(path.is_dir() and not path.is_symlink(), f"Invalid source directory: {name}")
            collect(path)
    return sorted(output)


def write_source_zip(root, destination):
    root = pathlib.Path(root).resolve()
    inventory = []
    with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name in source_files(root):
            data = (root / name).read_bytes()
            info = zipfile.ZipInfo("apx/" + name, date_time=(1980, 1, 1, 0, 0, 0))
            info.create_system = 3
            info.external_attr = (stat.S_IFREG | 0o644) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, data, compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)
            inventory.append({"path": name, "bytes": len(data), "sha256": sha(data)})
    return inventory


def safe_members(names):
    require(len(names) == len(set(names)), "Archive contains duplicate paths")
    for name in names:
        path = pathlib.PurePosixPath(name)
        require(not path.is_absolute() and ".." not in path.parts and "\\" not in name, f"Unsafe archive path: {name}")


def verify_packages(output, version, target, engine_hash):
    executable = "apx.exe" if target.startswith("win32-") else "apx"
    vsix = output / f"apx-local-apex-{version}-{target}.vsix"
    plugin = output / f"apx-sf-plugin-{version}.tgz"
    with zipfile.ZipFile(vsix) as archive:
        names = archive.namelist()
        safe_members(names)
        require(archive.testzip() is None, "VSIX CRC validation failed")
        for path in [f"extension/bin/{target}/{executable}", f"extension/sf-plugin/bin/{target}/{executable}"]:
            require(sha(archive.read(path)) == engine_hash, f"VSIX engine differs: {path}")
        for path in ["extension/package.json", "extension/sf-plugin/package.json", "extension/sf-plugin/node_modules/@apx/runner/package.json"]:
            require(json.loads(archive.read(path))["version"] == version, f"VSIX package version differs: {path}")
        for path in ["extension/dist/extension.js", "extension/sf-plugin/node_modules/@apx/runner/lib/index.js", "extension/sf-plugin/node_modules/@salesforce/sf-plugins-core/package.json"]:
            require(path in names, f"VSIX runtime file missing: {path}")
        require(not any(set(pathlib.PurePosixPath(name).parts) & {".git", ".sf", ".sfdx", ".tmp", ".env"} for name in names), "VSIX contains workspace state")
        require(not any("/test/driver/" in name for name in names), "VSIX contains development test driver")
    with tarfile.open(plugin, "r:gz") as archive:
        safe_members(archive.getnames())
        require(not any(member.issym() or member.islnk() for member in archive.getmembers()), "Plugin archive must contain physical dependencies, not links")
        native = archive.extractfile(f"package/bin/{target}/{executable}")
        require(native is not None and sha(native.read()) == engine_hash, "Plugin engine differs")
        for name in ["package/package.json", "package/node_modules/@apx/runner/package.json"]:
            require(json.load(archive.extractfile(name))["version"] == version, f"Plugin version differs: {name}")
        require(archive.getmember("package/node_modules/@apx/runner/lib/index.js").isfile(), "Plugin runner missing")
    return vsix, plugin


def verify_corpus(report, pin, engine_hash, version, environment_hash, weakness_baseline):
    require(report["engineSha256"].lower() == engine_hash and report["engineVersion"] == version, "Corpus evidence belongs to another engine")
    require(report["revision"] == pin["revision"] and report["gitTree"] == pin["gitTree"] and report["repository"] == pin["repository"], "Corpus evidence belongs to another pinned repository")
    expected = {item["id"]: item["path"] for item in pin["tests"]}
    actual = {item["id"]: item["path"] for item in report["tests"]}
    require(len(expected) == 322 and len(actual) == len(report["tests"]) == len(expected) and actual == expected, "Corpus identity conservation failed")
    require(report.get("sourceUnchanged") is True and report["conservation"]["verified"] is True, "Unchanged source evidence missing")
    require(report["environment"]["sha256"] == environment_hash, "Corpus environment differs from release source")
    require(all(report["gate"].get(name) is True for name in ["allTestsPassed", "regressionFree", "negativeControlsVerified", "weaknessBaselineRespected"]), "Corpus release gates are incomplete")
    require(report.get("diff", {}).get("regressions") == [], "Corpus regression evidence contradicts release gates")
    require(all(test["status"] == "passed" for test in report["tests"]), "Release requires every pinned test to execute and pass")
    mutations = report["mutations"]
    require(len(mutations) == len(expected) and {item["id"] for item in mutations} == set(expected), "Negative mutation evidence does not conserve every test")
    require(report.get("mutationEvidenceSchemaVersion") == 2, "Mutation evidence schema 2 is required; legacy receipts remain historical and unqualified")
    audit = verify_negative_controls(mutations, {test["id"]: test for test in report["tests"]})
    require(report["gate"].get("negativeMutationsKilled") is audit["allPrimaryMutationsKilled"], "Negative mutation gate contradicts primary outcomes")
    summary = report["validation"]["summary"]
    require(summary["passed"] == summary["tests"] == len(expected) and all(summary[key] == 0 for key in ["failed", "blocked", "errored"]), "Corpus summary contradicts test results")
    require(report["validation"]["status"] == "preview_passed" and report["validation"]["exitCode"] == 4, "Execution assurance/exit policy changed")
    verify_report_integrity(report)
    require(valid_provenance({**report["validation"], "tests": report["tests"]}, version), "Corpus provenance is missing, malformed or self-promoted")
    for mutation in mutations:
        for observation in [mutation, *([mutation["wiringCheck"]] if "wiringCheck" in mutation else [])]:
            require(valid_provenance(observation["witness"], version), "Mutation provenance is missing, malformed or self-promoted")
    verify_weakness_baseline(report, pin, weakness_baseline, audit)
    return audit


def verify_report_integrity(report):
    """Gate protocol-2 captures and observed assertion counters before redaction."""
    validation = report["validation"]
    require(validation.get("schemaVersion") == 2, "Release requires native report protocol 2")
    files, directories = validation.get("inputs"), validation.get("inputDirectories")
    require(isinstance(files, list) and files and isinstance(directories, list) and directories,
            "Consumed input manifest is missing")
    canonical = {"files": [], "directories": []}
    for values, key in [(files, "files"), (directories, "directories")]:
        paths = [item.get("path") for item in values]
        require(all(isinstance(path, str) and path for path in paths)
                and paths == sorted(set(paths), key=lambda path: path.encode("utf-8")),
                "Input manifest paths must be unique and sorted")
        for item in values:
            digest = item.get("sha256")
            require(isinstance(digest, str) and len(digest) == 64
                    and all(c in "0123456789abcdef" for c in digest), "Invalid input digest")
            value = {"path": item["path"], "sha256": digest}
            if key == "directories":
                require(type(item.get("includeIgnored")) is bool, "Directory inventory policy missing")
                value["includeIgnored"] = item["includeIgnored"]
            canonical[key].append(value)
    fingerprint = sha(json.dumps(canonical, separators=(",", ":"), ensure_ascii=False).encode("utf-8"))
    require(validation.get("inputFingerprint") == fingerprint, "Input fingerprint contradicts manifest")
    for test in report["tests"]:
        for name in ["assertions", "setupAssertions"]:
            counts = test.get(name)
            require(isinstance(counts, dict) and all(type(counts.get(key)) is int and counts[key] >= 0
                    for key in ["executed", "passed", "failed"]), "Assertion counters missing or invalid")
            require(counts["executed"] == counts["passed"] + counts["failed"], "Assertion counters contradict verdicts")
            require(counts["failed"] == 0, "Passing test contains failed assertions")
        if test["assertions"]["executed"] == 0:
            message = test.get("message", "").lower()
            require("no assertions" in message or "no test-body assertions" in message, "Zero-assertion success lacks visible warning")


def valid_provenance(report, version):
    """Mirror protocol provenance constraints so finalization cannot promote it."""
    def exact(value, keys):
        return isinstance(value, dict) and set(value) == set(keys)
    def count(value):
        return type(value) is int and 0 <= value <= 9007199254740991
    def text(value):
        return isinstance(value, str) and 0 < len(value.encode("utf-8")) <= 256 and not re.search(r"[\x00-\x1f\x7f]", value)
    contracts = {"apx.native-method.v1", "apx.native-constructor.v1", "apx.soql.v1", "apx.sosl.v1", "apx.dml.v1"}
    assumptions = {
        "explicit-environment-fixture": ("synthetic", "configured-for-test"),
        "fixture-clock": ("synthetic", "reached-operation"), "host-clock": ("host-derived", "reached-operation"),
        "local-record-store": ("synthetic", "reached-operation"), "local-test-lifecycle": ("synthetic", "reached-operation"),
        "local-limit-model": ("synthetic", "reached-operation"), "local-platform-context": ("synthetic", "reached-operation"),
    }
    try:
        tests, provenance = report.get("tests"), report.get("provenance")
        if not isinstance(tests, list):
            return False
        if provenance is None:
            match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)(?:-[A-Za-z0-9.-]+)?", version)
            return (match is not None and tuple(map(int, match.groups())) < (0, 2, 3)
                    and "provenance" not in report and all(isinstance(test, dict) and "provenance" not in test for test in tests))
        if (not exact(provenance, ("schemaVersion", "catalogueVersion", "conformance", "compiler", "inventory", "contracts"))
                or type(provenance["schemaVersion"]) is not int or provenance["schemaVersion"] != 1
                or provenance["catalogueVersion"] != "apx-capabilities-v1" or provenance["conformance"] != "unqualified"
                or provenance["compiler"] != {"coverage": "bounded-semantic-subset", "conformance": "unqualified"}
                or not exact(provenance["inventory"], ("scope", "complete", "limitations"))
                or provenance["inventory"]["scope"] != "reached-native-dispatch" or provenance["inventory"]["complete"] is not False
                or not isinstance(provenance["inventory"]["limitations"], list) or not 1 <= len(provenance["inventory"]["limitations"]) <= 16
                or not all(text(value) for value in provenance["inventory"]["limitations"])
                or not isinstance(provenance["contracts"], list) or len(provenance["contracts"]) != len(contracts)):
            return False
        seen = set()
        for contract in provenance["contracts"]:
            if (not exact(contract, ("id", "implementation", "basis", "conformance", "independentEvidence"))
                    or contract["id"] not in contracts or contract["id"] in seen
                    or contract["implementation"] != "partial-local-model" or contract["basis"] != "local-implementation"
                    or contract["conformance"] != "unqualified" or contract["independentEvidence"] != []):
                return False
            seen.add(contract["id"])
        for test in tests:
            p = test.get("provenance")
            if (not exact(p, ("schemaVersion", "conformance", "inventoryComplete", "truncated", "operations", "assumptions"))
                    or type(p["schemaVersion"]) is not int or p["schemaVersion"] != 1 or p["conformance"] != "unqualified"
                    or p["inventoryComplete"] is not False or type(p["truncated"]) is not bool
                    or not isinstance(p["operations"], list) or len(p["operations"]) > 256
                    or not isinstance(p["assumptions"], list) or len(p["assumptions"]) > len(assumptions)
                    or test.get("status") == "not_run" and (p["operations"] or p["assumptions"] or p["truncated"])):
                return False
            operations = set()
            for operation in p["operations"]:
                if (not exact(operation, ("contractId", "operation", "phase", "returned", "failed", "blocked"))
                        or operation["contractId"] not in contracts or not text(operation["operation"])
                        or operation["phase"] not in ("setup", "test") or not all(count(operation[key]) for key in ("returned", "failed", "blocked"))):
                    return False
                total = sum(operation[key] for key in ("returned", "failed", "blocked"))
                key = (operation["contractId"], operation["operation"], operation["phase"])
                if not 0 < total <= 9007199254740991 or key in operations:
                    return False
                operations.add(key)
            seen = set()
            for assumption in p["assumptions"]:
                if (not exact(assumption, ("id", "kind", "scope")) or assumption["id"] not in assumptions or assumption["id"] in seen
                        or (assumption["kind"], assumption["scope"]) != assumptions[assumption["id"]]):
                    return False
                seen.add(assumption["id"])
        return True
    except (KeyError, TypeError, AttributeError, UnicodeError):
        return False


def mutation_observation_valid(observed, status):
    """Validate the structured one-test witness, never diagnostic keywords."""
    def count(value):
        return type(value) is int and 0 <= value <= 9007199254740991
    def position(value):
        return count(value) and value > 0
    def counts(value):
        return (isinstance(value, dict) and all(count(value.get(key)) for key in ("executed", "passed", "failed"))
                and value["executed"] == value["passed"] + value["failed"])
    def source_path(value):
        return (isinstance(value, str) and bool(value) and "\\" not in value and ":" not in value
                and not value.startswith("/") and all(part not in ("", ".", "..") for part in value.split("/")))
    def digest(value):
        return isinstance(value, str) and re.fullmatch("[a-f0-9]{64}", value) is not None
    try:
        if not isinstance(observed, dict) or observed.get("evidenceSchemaVersion") != 2 or status not in ("passed", "failed"):
            return False
        target, witness = observed["target"], observed["witness"]
        if (not isinstance(target, dict) or not isinstance(witness, dict) or not isinstance(observed.get("id"), str)
                or target.get("id") != observed["id"] or not source_path(target.get("path"))
                or not position(target.get("line")) or not digest(target.get("sha256"))
                or witness.get("schemaVersion") != 2 or witness.get("command") != "test"
                or not isinstance(witness.get("tests"), list) or len(witness["tests"]) != 1
                or not isinstance(witness.get("diagnostics"), list) or not isinstance(witness.get("inputs"), list)
                or not isinstance(witness.get("summary"), dict)):
            return False
        test = witness["tests"][0]
        failure, exit_code = status == "failed", 1 if status == "failed" else 4
        if (not isinstance(test, dict) or any(test.get(key) != target[key] for key in ("id", "path", "line"))
                or test.get("status") != status or not isinstance(test.get("message"), str)
                or observed.get("status") != status or observed.get("message") != test["message"]
                or type(observed.get("exitCode")) is not int or observed["exitCode"] != exit_code
                or type(witness.get("exitCode")) is not int or witness["exitCode"] != exit_code
                or witness.get("status") != ("failed" if failure else "preview_passed")
                or not counts(test.get("assertions")) or not counts(test.get("setupAssertions"))):
            return False
        if (failure and test["assertions"]["failed"] < 1) or (not failure and (test["assertions"]["failed"] or test["setupAssertions"]["failed"])):
            return False
        summary = {"tests": 1, "passed": 0 if failure else 1, "failed": 1 if failure else 0, "blocked": 0, "errored": 0}
        if any(type(witness["summary"].get(key)) is not int or witness["summary"][key] != value for key, value in summary.items()):
            return False
        inputs = witness["inputs"]
        if (not all(isinstance(item, dict) and source_path(item.get("path")) and digest(item.get("sha256")) for item in inputs)
                or len({item["path"] for item in inputs}) != len(inputs)
                or not any(item["path"] == target["path"] and item["sha256"] == target["sha256"] for item in inputs)):
            return False
        if not all(isinstance(item, dict) and item.get("severity") in ("error", "warning", "information") for item in witness["diagnostics"]):
            return False
        errors = [item for item in witness["diagnostics"] if item["severity"] == "error"]
        if not failure:
            return not errors
        if len(errors) != 1:
            return False
        error = errors[0]
        return (error.get("code") == "APX_ASSERTION" and error.get("message") == test["message"]
                and source_path(error.get("path")) and any(item["path"] == error["path"] for item in inputs)
                and all(position(error.get(key)) for key in ("line", "column", "endLine", "endColumn"))
                and (error["endLine"] > error["line"] or error["endLine"] == error["line"] and error["endColumn"] >= error["column"]))
    except (KeyError, TypeError, AttributeError):
        return False


def verify_weakness_baseline(report, pin, baseline, audit):
    require(isinstance(baseline, dict) and baseline.get("schemaVersion") == 1
            and all(baseline.get(key) == pin.get(key) for key in ("repository", "revision", "gitTree")), "Weakness baseline belongs to a different corpus")
    require(report.get("weaknessBaseline") == baseline, "Report weakness baseline differs from reviewed source")
    identities = {test["id"] for test in pin["tests"]}
    actual = {"survivingAssertionInversions": audit["survivingAssertionInversions"],
              "entrySentinels": audit["primaryEntrySentinelTests"],
              "zeroAssertionTests": [test["id"] for test in report["tests"] if test["assertions"]["executed"] == 0]}
    for key, observed in actual.items():
        allowed = baseline.get(key)
        require(isinstance(allowed, list) and all(isinstance(item, str) for item in allowed)
                and len(set(allowed)) == len(allowed) and set(allowed) <= identities, f"Invalid weakness baseline: {key}")
        require(set(observed) <= set(allowed), f"New corpus weakness requires review: {key}")
    expected = {"verified": True, "added": {key: [] for key in actual}}
    require(report.get("weaknessComparison") == expected, "Weakness comparison contradicts independently verified identities")
    audit["weaknessBaselineRespected"] = True


def verify_negative_controls(mutations, expected):
    """Validate every original control and retain assertion-coverage weaknesses.

    A survivor is never relabeled as killed. A separately observed entry sentinel
    can establish execution wiring, but cannot establish coverage of the original
    assertion path. Blocked, harness-error, and non-assertion failures never count.
    """
    primary_killed = 0
    inversions = 0
    sentinel_only = []
    survivors = []
    for mutation in mutations:
        identity = mutation["id"]
        target = mutation.get("target")
        require(isinstance(target, dict) and target.get("id") == identity
                and target.get("path") == expected[identity]["path"] and target.get("line") == expected[identity].get("line"),
                f"Negative control target differs from selected test: {identity}")
        kind = mutation.get("kind")
        require(kind in {"assertion-inversion", "entry-sentinel"}, f"Unknown primary negative control kind: {identity}")
        if kind == "assertion-inversion":
            inversions += 1
        else:
            sentinel_only.append(identity)

        def assertion_failure(observed):
            return isinstance(observed, dict) and observed.get("killed") is True and mutation_observation_valid(observed, "failed")

        if mutation.get("killed") is True:
            require(assertion_failure(mutation), f"Primary control lacks an observed assertion failure: {identity}")
            require("wiringCheck" not in mutation, f"Additional wiring control contradicts a killed primary: {identity}")
            primary_killed += 1
            continue
        require(mutation.get("killed") is False and kind == "assertion-inversion"
                and mutation_observation_valid(mutation, "passed"),
                f"Unverified primary negative control: {identity}")
        wiring = mutation.get("wiringCheck")
        require(isinstance(wiring, dict) and wiring.get("kind") == "entry-sentinel-after-survivor"
                and wiring.get("id") == identity and isinstance(wiring.get("target"), dict)
                and all(wiring["target"].get(key) == target[key] for key in ("id", "path", "line"))
                and wiring["target"].get("sha256") != target.get("sha256")
                and assertion_failure(wiring), f"Surviving inversion lacks a verified additional wiring control: {identity}")
        survivors.append(identity)
    return {
        "testsWithVerifiedNegativeControls": len(mutations),
        "primaryControls": len(mutations),
        "primaryControlsKilled": primary_killed,
        "assertionInversions": inversions,
        "assertionInversionsKilled": inversions - len(survivors),
        "survivingAssertionInversionCount": len(survivors),
        "survivingAssertionInversions": survivors,
        "primaryEntrySentinelCount": len(sentinel_only),
        "primaryEntrySentinelTests": sentinel_only,
        "additionalEntrySentinelsVerified": len(survivors),
        "negativeControlsVerified": True,
        "allPrimaryMutationsKilled": primary_killed == len(mutations),
        "qualification": "Entry sentinels verify execution wiring only; they do not establish coverage of original assertion paths.",
    }


def qualification_text(audit):
    text = "Experimental local execution; not Salesforce-qualified. Local timing is not a benchmark."
    if audit["survivingAssertionInversionCount"]:
        text += (f" {audit['survivingAssertionInversionCount']} primary assertion inversion(s) survived under the explicit local fixture;"
                 " separate entry sentinels verified execution wiring only. Original assertion-path coverage remains unverified for those tests.")
    return text


def portable(value, replacements):
    if isinstance(value, str):
        for source, target in replacements:
            value = value.replace(str(source), target).replace(str(source).replace("\\", "/"), target)
        return value
    if isinstance(value, list):
        return [portable(item, replacements) for item in value]
    if isinstance(value, dict):
        return {key: portable(item, replacements) for key, item in value.items()}
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=pathlib.Path, default=pathlib.Path(__file__).resolve().parents[1])
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--engine", type=pathlib.Path, required=True)
    parser.add_argument("--corpus-report", type=pathlib.Path, required=True)
    parser.add_argument("--extension-evidence", type=pathlib.Path, required=True)
    parser.add_argument("--sf-evidence", type=pathlib.Path, required=True)
    parser.add_argument("--workspace", type=pathlib.Path, help="Optional absolute prefix to redact from exported evidence")
    args = parser.parse_args()
    source, output, engine = args.source.resolve(), args.output.resolve(), args.engine.resolve()
    require(output.is_dir(), "Run native/VSIX/plugin packaging first")
    version = read_json(source / "package.json")["version"]
    packaging = read_json(output / "packaging-manifest.json")
    require(packaging["version"] == version, "Packaging and source version differ")
    target = packaging["target"]
    engine_hash = sha(engine.read_bytes())
    vsix, plugin = verify_packages(output, version, target, engine_hash)
    corpus = read_json(args.corpus_report)
    pin = read_json(source / "corpus/apex-recipes.pin.json")
    environment_hash = sha((source / "corpus/apex-recipes.environment.json").read_bytes())
    weakness_baseline = read_json(source / "corpus/apex-recipes.weakness-baseline.json")
    mutation_audit = verify_corpus(corpus, pin, engine_hash, version, environment_hash, weakness_baseline)
    host, sf = read_json(args.extension_evidence), read_json(args.sf_evidence)
    require(host.get("engineSha256", "").lower() == engine_hash and host.get("results") and all(item.get("passed") is True for item in host["results"]), "Installed extension evidence failed or belongs to another engine")
    require(f"apx-local.apx-local-apex-{version}" in host.get("extensionRoot", ""), "Extension evidence must identify the installed release, not development source")
    require(sf.get("verified") is True and sf.get("engineSha256", "").lower() == engine_hash and sf.get("archiveSha256", "").lower() == sha(plugin.read_bytes()), "Installed SF plugin evidence failed or belongs to another archive")
    # All preconditions passed. Only now write public deliverables.
    binary = output / ("apx.exe" if target.startswith("win32-") else "apx")
    if engine != binary:
        shutil.copyfile(engine, binary)
    inventory = write_source_zip(source, output / f"apx-{version}-source.zip")
    replacements = [(source, "${APX_SOURCE}"), (output, "${RELEASE}")]
    if args.workspace:
        replacements.append((args.workspace.resolve(), "${WORKSPACE}"))
    evidence = {"schemaVersion": 1, "version": version, "target": target, "engineSha256": engine_hash,
                "inputManifestRepresentation": "Paths redacted for publication; inputFingerprint verified against original absolute paths before redaction.",
                "qualification": qualification_text(mutation_audit), "mutationAudit": mutation_audit,
                "corpus": corpus, "installedExtension": host, "installedSfPlugin": sf,
                "sourceFiles": inventory, "nativeArchiveCopiesVerified": 3}
    (output / "release-evidence.json").write_text(json.dumps(portable(evidence, replacements), indent=2) + "\n", encoding="utf-8")
    checksums = []
    for file in sorted(output.iterdir()):
        if file.is_file() and file.name != "SHA256SUMS.txt":
            checksums.append(f"{sha(file.read_bytes())}  {file.name}")
    (output / "SHA256SUMS.txt").write_text("\n".join(checksums) + "\n", encoding="utf-8")
    print(json.dumps({"version": version, "engineSha256": engine_hash, "sourceFiles": len(inventory), "tests": corpus["validation"]["summary"], "checksummedFiles": len(checksums)}, indent=2))


if __name__ == "__main__":
    main()
