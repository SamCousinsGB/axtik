"""Finalize local release artifacts from frozen, independently checked evidence.

Uses Python's standard library; never builds, downloads, publishes, or modifies
the upstream corpus. Archives source through an explicit allowlist, including
untracked source files. Repeated identical source bytes produce identical ZIPs.
"""
import argparse
import hashlib
import json
import pathlib
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


def verify_corpus(report, pin, engine_hash, version, environment_hash):
    require(report["engineSha256"].lower() == engine_hash and report["engineVersion"] == version, "Corpus evidence belongs to another engine")
    require(report["revision"] == pin["revision"] and report["gitTree"] == pin["gitTree"] and report["repository"] == pin["repository"], "Corpus evidence belongs to another pinned repository")
    expected = {item["id"]: item["path"] for item in pin["tests"]}
    actual = {item["id"]: item["path"] for item in report["tests"]}
    require(len(expected) == 322 and len(actual) == len(report["tests"]) == len(expected) and actual == expected, "Corpus identity conservation failed")
    require(report.get("sourceUnchanged") is True and report["conservation"]["verified"] is True, "Unchanged source evidence missing")
    require(report["environment"]["sha256"] == environment_hash, "Corpus environment differs from release source")
    require(all(report["gate"].get(name) is True for name in ["allTestsPassed", "regressionFree", "negativeControlsVerified"]), "Corpus release gates are incomplete")
    require(report.get("diff", {}).get("regressions") == [], "Corpus regression evidence contradicts release gates")
    require(all(test["status"] == "passed" for test in report["tests"]), "Release requires every pinned test to execute and pass")
    mutations = report["mutations"]
    require(len(mutations) == len(expected) and {item["id"] for item in mutations} == set(expected), "Negative mutation evidence does not conserve every test")
    audit = verify_negative_controls(mutations)
    require(report["gate"].get("negativeMutationsKilled") is audit["allPrimaryMutationsKilled"], "Negative mutation gate contradicts primary outcomes")
    summary = report["validation"]["summary"]
    require(summary["passed"] == summary["tests"] == len(expected) and all(summary[key] == 0 for key in ["failed", "blocked", "errored"]), "Corpus summary contradicts test results")
    require(report["validation"]["status"] == "preview_passed" and report["validation"]["exitCode"] == 4, "Execution assurance/exit policy changed")
    verify_report_integrity(report)
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


def verify_negative_controls(mutations):
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
        kind = mutation.get("kind")
        require(kind in {"assertion-inversion", "entry-sentinel"}, f"Unknown primary negative control kind: {identity}")
        if kind == "assertion-inversion":
            inversions += 1
        else:
            sentinel_only.append(identity)

        def assertion_failure(observed):
            return (isinstance(observed, dict) and observed.get("killed") is True
                    and observed.get("status") == "failed" and observed.get("exitCode") == 1
                    and isinstance(observed.get("message"), str)
                    and "assertion" in observed["message"].lower())

        if mutation.get("killed") is True:
            require(assertion_failure(mutation), f"Primary control lacks an observed assertion failure: {identity}")
            require("wiringCheck" not in mutation, f"Additional wiring control contradicts a killed primary: {identity}")
            primary_killed += 1
            continue
        require(mutation.get("killed") is False and kind == "assertion-inversion"
                and mutation.get("status") == "passed" and mutation.get("exitCode") == 4,
                f"Unverified primary negative control: {identity}")
        wiring = mutation.get("wiringCheck")
        require(isinstance(wiring, dict) and wiring.get("kind") == "entry-sentinel-after-survivor"
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
    mutation_audit = verify_corpus(corpus, pin, engine_hash, version, environment_hash)
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
