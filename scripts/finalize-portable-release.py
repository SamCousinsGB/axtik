"""Finalize one universal release only after all six real host gates pass.

Inputs are already-built packages, native build receipts, pinned corpus audits,
and installed VS Code host receipts. This script never builds, downloads,
installs, publishes, or contacts Salesforce. It uses only Python's standard library.
"""
import argparse
import importlib.util
import json
import pathlib
import posixpath
import re
import shutil
import stat
import struct
import subprocess
import tarfile
import zipfile

_spec = importlib.util.spec_from_file_location("release_artifacts", pathlib.Path(__file__).with_name("release_artifacts.py"))
base = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(base)
require, sha, read_json = base.require, base.sha, base.read_json

TARGETS = ("win32-x64", "win32-arm64", "darwin-x64", "darwin-arm64", "linux-x64", "linux-arm64")
MAX_FILE = 64 * 1024 * 1024
MAX_ARCHIVE = 512 * 1024 * 1024
HOST_CHECKS = {
    "unknown native API in unexecuted source blocks validation",
    "commands registered in real Extension Host",
    "class command calls native engine and publishes Problems",
    "repaired source clears stale Problems",
    "folder command uses the same native pipeline",
    "editing source invalidates applied diagnostics",
    "bundled engine discovers demo tests through the extension command",
    "production class validation expands selection and preserves advisory exit 4",
    "project-relative environment setting reaches bundled native evaluator",
    "environment edits invalidate applied diagnostics",
    "environment setting changes invalidate applied diagnostics",
    "real native manifest includes binary and expanded resource inputs",
    "completed external result invalidates on binary png payload edit",
    "completed external result invalidates on expanded js payload edit",
    "completed external result invalidates on expanded csv payload edit",
    "completed external result invalidates on resource payload rename",
    "completed external result invalidates on resource payload deletion",
    "expanded resource additions inside normally ignored dist invalidate results",
    "unsaved expanded resource edit invalidates applied diagnostics",
    "completed result retains watcher for environment outside project",
    "actual assertion invocations distinguish helpers, setup and zero assertion execution",
}
WINDOWS_LIBRARIES = set("advapi32 bcrypt bcryptprimitives crypt32 cryptbase dbghelp gdi32 imm32 iphlpapi kernel32 kernelbase msvcrt mswsock ncrypt netapi32 normaliz ntdll ole32 oleaut32 powrprof psapi rpcrt4 secur32 setupapi shell32 shlwapi sspicli ucrtbase user32 userenv version winhttp wininet winmm wintrust wldap32 ws2_32 wsock32 wtsapi32".split())


def read_bounded(path, maximum=MAX_FILE):
    path = pathlib.Path(path)
    require(path.is_file() and not path.is_symlink(), f"Missing or linked input: {path}")
    with path.open("rb") as stream:
        data = stream.read(maximum + 1)
    require(len(data) <= maximum, f"Input exceeds its byte bound: {path.name}")
    return data


def exact_targets(items, label):
    require(isinstance(items, list) and all(isinstance(item, str) for item in items)
            and len(items) == len(TARGETS) and set(items) == set(TARGETS),
            f"{label} must identify all six targets exactly once")


def native_dependencies(data):
    if data[:2] == b"MZ":
        pe = struct.unpack_from("<I", data, 0x3c)[0]
        optional = pe + 24
        optional_size = struct.unpack_from("<H", data, pe + 20)[0]
        section_count = struct.unpack_from("<H", data, pe + 6)[0]
        require(optional_size >= 112 and 0 < section_count <= 96
                and optional + optional_size + section_count * 40 <= len(data), "Invalid PE section table")
        directory_count = struct.unpack_from("<I", data, optional + 108)[0]
        header_size = struct.unpack_from("<I", data, optional + 60)[0]
        require(directory_count <= 16 and optional_size >= 112 + directory_count * 8 and header_size <= len(data), "Invalid PE directories")
        sections = []
        for index in range(section_count):
            rva, size, start = struct.unpack_from("<III", data, optional + optional_size + index * 40 + 12)
            require(start + size <= len(data), "Invalid PE section bounds")
            sections.append((rva, size, start))
        def address(rva, size):
            if rva + size <= header_size:
                return rva
            matches = [start + rva - base for base, length, start in sections if rva >= base and rva + size <= base + length]
            require(len(matches) == 1, "Ambiguous or unavailable PE import address")
            return matches[0]
        def name(rva):
            result = bytearray()
            for offset in range(256):
                char = data[address(rva + offset, 1)]
                if char == 0:
                    require(result, "Empty PE import name")
                    return result.decode("ascii").lower()
                require(32 <= char <= 126, "Invalid PE import name")
                result.append(char)
            raise ValueError("Unterminated PE import name")
        libraries = []
        for index, size, name_offset in ((1, 20, 12), (13, 32, 4)):
            if directory_count <= index:
                continue
            rva, length = struct.unpack_from("<II", data, optional + 112 + index * 8)
            if rva == length == 0:
                continue
            require(rva and size <= length <= size * 1024, "Invalid PE import directory")
            terminated = False
            for offset in range(0, length - size + 1, size):
                at = address(rva + offset, size)
                if not any(data[at:at + size]):
                    terminated = True
                    break
                require(index != 13 or struct.unpack_from("<I", data, at)[0] == 1, "Unsupported PE delay import addressing")
                libraries.append(name(struct.unpack_from("<I", data, at + name_offset)[0]))
            require(terminated, "Unterminated PE import table")
        libraries = sorted(set(libraries))
        require(all((library.endswith(".dll") and library[:-4] in WINDOWS_LIBRARIES)
                    or re.fullmatch(r"(?:api|ext)-ms-win-[a-z0-9-]+\.dll", library) for library in libraries),
                "Windows engine requires a non-system DLL")
        return libraries
    if data[:4] == b"\xcf\xfa\xed\xfe":
        count, total = struct.unpack_from("<II", data, 16)
        require(count <= total // 8 and 32 + total <= len(data), "Invalid Mach-O command table")
        at, libraries = 32, []
        for _ in range(count):
            require(at + 8 <= len(data), "Invalid Mach-O command header")
            command, size = struct.unpack_from("<II", data, at)
            require(size >= 8 and size % 8 == 0 and at + size <= 32 + total, "Invalid Mach-O command bounds")
            if command in (0xc, 0x80000018, 0x8000001f, 0x20, 0x80000023):
                require(size >= 24, "Invalid Mach-O dylib command")
                offset = struct.unpack_from("<I", data, at + 8)[0]
                end = data.find(b"\0", at + offset, at + size)
                require(24 <= offset < size and end >= 0, "Invalid Mach-O dylib name")
                library = data[at + offset:end].decode("utf-8")
                require(posixpath.normpath(library) == library and library.startswith(("/usr/lib/", "/System/Library/Frameworks/")),
                        "macOS engine requires a non-system dylib")
                libraries.append(library)
            at += size
        require(at == 32 + total, "Mach-O command accounting differs")
        return sorted(set(libraries))
    return []


def native_identity(data):
    """Read machine type independently; Linux must have no dynamic loader/deps."""
    if len(data) >= 64 and data[:2] == b"MZ":
        pe = struct.unpack_from("<I", data, 0x3c)[0]
        if pe + 26 <= len(data) and data[pe:pe + 4] == b"PE\0\0" and struct.unpack_from("<H", data, pe + 24)[0] == 0x20b:
            machine = struct.unpack_from("<H", data, pe + 4)[0]
            flags = struct.unpack_from("<H", data, pe + 22)[0]
            require(flags & 2 and not flags & 0x2000, "PE native input is not an executable")
            if machine in (0x8664, 0xaa64):
                return ("win32-x64" if machine == 0x8664 else "win32-arm64", "PE32+", "windows")
    if len(data) >= 64 and data[:6] == b"\x7fELF\x02\x01":
        require(data[6] == 1 and struct.unpack_from("<H", data, 16)[0] in (2, 3), "ELF native input is not an executable")
        machine = struct.unpack_from("<H", data, 18)[0]
        require(machine in (62, 183), "Unsupported ELF architecture")
        offset = struct.unpack_from("<Q", data, 32)[0]
        size, count = struct.unpack_from("<HH", data, 54)
        require(count > 0 and size >= 56 and offset >= 64 and offset + size * count <= len(data), "Invalid ELF program header table")
        for index in range(count):
            header = offset + size * index
            kind = struct.unpack_from("<I", data, header)[0]
            start = struct.unpack_from("<Q", data, header + 8)[0]
            length = struct.unpack_from("<Q", data, header + 32)[0]
            require(start + length <= len(data), "Invalid ELF segment bounds")
            require(kind != 3, "Linux engine has a dynamic interpreter")
            if kind == 2:
                require(length % 16 == 0 and start + length <= len(data), "Invalid ELF dynamic table")
                terminated = False
                for entry in range(start, start + length, 16):
                    tag = struct.unpack_from("<Q", data, entry)[0]
                    if tag == 0:
                        terminated = True
                        break
                    require(tag != 1, "Linux engine requires a shared library")
                require(terminated, "Unterminated ELF dynamic table")
        return ("linux-x64" if machine == 62 else "linux-arm64", "ELF64", "static")
    if len(data) >= 32 and struct.unpack_from("<I", data)[0] == 0xfeedfacf:
        require(struct.unpack_from("<I", data, 12)[0] == 2, "Mach-O native input is not an executable")
        machine = struct.unpack_from("<I", data, 4)[0]
        if machine in (0x01000007, 0x0100000c):
            return ("darwin-x64" if machine == 0x01000007 else "darwin-arm64", "Mach-O64", "macos")
    raise ValueError("Unrecognized native executable format")


def verify_engine(root, target, version):
    executable = "apx.exe" if target.startswith("win32-") else "apx"
    data = read_bounded(root / target / executable)
    receipt = json.loads(read_bounded(root / target / "engine.json", 1024 * 1024))
    actual, file_format, linkage = native_identity(data)
    dependencies = native_dependencies(data)
    platform, arch = target.split("-")
    require(actual == target, f"Native executable architecture differs: {target}")
    require(receipt.get("schemaVersion") == 1 and receipt.get("target") == target
            and receipt.get("version") == version and receipt.get("observedVersion") == f"apx {version}",
            f"Native version receipt differs: {target}")
    require(receipt.get("host") == {"platform": platform, "arch": arch}, f"Native receipt was not observed on its target host: {target}")
    require(receipt.get("sha256") == sha(data) and receipt.get("bytes") == len(data), f"Native receipt hash or size differs: {target}")
    require(receipt.get("format") == file_format and receipt.get("linkage") == linkage, f"Native receipt format/linkage differs: {target}")
    require(receipt.get("dependencies") == dependencies, f"Native dependency receipt differs: {target}")
    return {"target": target, "file": f"bin/{target}/{executable}", "sha256": sha(data),
            "format": file_format, "linkage": linkage, "receipt": receipt}


def safe_archive_names(names):
    require(len(names) <= 50000, "Archive exceeds 50000 entries")
    require(len(names) == len(set(names)), "Archive contains duplicate paths")
    for name in names:
        parts = name.rstrip("/").split("/")
        require(name and not name.startswith("/") and "\\" not in name and ":" not in name
                and all(part not in ("", ".", "..") for part in parts),
                f"Unsafe archive path: {name}")


def verify_package_manifest(manifest, version, engines):
    require(manifest.get("schemaVersion") == 2 and manifest.get("target") == "universal"
            and manifest.get("version") == version, "Universal packaging manifest version differs")
    exact_targets(manifest.get("targets"), "Packaging manifest")
    entries = manifest.get("engines")
    require(isinstance(entries, list) and all(isinstance(item, dict) for item in entries), "Packaging engine receipts missing")
    exact_targets([item.get("target") for item in entries], "Packaging engine receipts")
    for entry in entries:
        expected = engines[entry["target"]]
        require(all(entry.get(key) == expected[key] for key in ("file", "sha256", "format", "linkage", "receipt")),
                f"Packaging engine receipt differs: {entry['target']}")
    require(manifest.get("vsix") == f"apx-local-apex-{version}-universal.vsix"
            and manifest.get("plugin") == f"apx-sf-plugin-{version}.tgz", "Unexpected portable package filenames")


def verify_archive_contents(names, read, mode, version, engines, extension):
    prefix = "extension/" if extension else "package/"
    package_paths = [prefix + "package.json"]
    engine_prefixes = [prefix + "bin/"]
    if extension:
        package_paths += ["extension/sf-plugin/package.json", "extension/sf-plugin/node_modules/@apx/runner/package.json"]
        engine_prefixes += ["extension/sf-plugin/bin/"]
        require("extension/dist/extension.js" in names, "VSIX extension code missing")
        require("extension/sf-plugin/node_modules/@apx/runner/lib/index.js" in names, "VSIX bundled runner missing")
        from xml.etree import ElementTree
        xml = ElementTree.fromstring(read("extension.vsixmanifest"))
        identity = next((node for node in xml.iter() if node.tag.rsplit("}", 1)[-1] == "Identity"), None)
        require(identity is not None and identity.attrib.get("TargetPlatform") in (None, "universal"), "VSIX is restricted to one platform")
    else:
        package_paths += ["package/node_modules/@apx/runner/package.json"]
        require("package/node_modules/@apx/runner/lib/index.js" in names, "Plugin runner missing")
    for name in package_paths:
        require(json.loads(read(name)).get("version") == version, f"Packaged version differs: {name}")
    extension_manifest = json.loads(read(prefix + "package.json"))
    if extension:
        require(extension_manifest.get("extensionKind") == ["workspace"], "Extension must execute on the workspace host")
    for engine_prefix in engine_prefixes:
        found = {name[len(engine_prefix):].split("/")[0] for name in names if name.startswith(engine_prefix) and name[len(engine_prefix):]}
        require(found == set(TARGETS), f"Archive native target set differs: {engine_prefix}")
        for target, expected in engines.items():
            filename = "apx.exe" if target.startswith("win32-") else "apx"
            native = engine_prefix + target + "/" + filename
            require(native in names and engine_prefix + target + "/engine.json" in names, f"Packaged native file or receipt missing: {target}")
            require(sha(read(native)) == expected["sha256"], f"Packaged engine hash differs: {native}")
            require(json.loads(read(engine_prefix + target + "/engine.json")) == expected["receipt"], f"Packaged engine receipt differs: {target}")
            if not target.startswith("win32-"):
                require(mode(native) & 0o111, f"Packaged Unix engine is not executable: {native}")
    return extension_manifest


def verify_packages(package_dir, manifest, version, engines):
    vsix, plugin = package_dir / manifest["vsix"], package_dir / manifest["plugin"]
    require(vsix.stat().st_size <= MAX_ARCHIVE and plugin.stat().st_size <= MAX_ARCHIVE, "Package exceeds 512 MiB")
    with zipfile.ZipFile(vsix) as archive:
        infos = archive.infolist()
        safe_archive_names([item.filename for item in infos])
        require(sum(item.file_size for item in infos) <= MAX_ARCHIVE and all(item.file_size <= MAX_FILE for item in infos), "VSIX expanded size exceeds bound")
        require(not any(stat.S_ISLNK(item.external_attr >> 16) for item in infos), "VSIX contains a symlink")
        require(not any(set(pathlib.PurePosixPath(item.filename).parts) & {".git", ".sf", ".sfdx", ".tmp", ".env"} for item in infos), "VSIX contains workspace state")
        require(not any("/test/driver/" in item.filename for item in infos), "VSIX contains development test driver")
        require(archive.testzip() is None, "VSIX CRC validation failed")
        extension = verify_archive_contents(set(archive.namelist()), archive.read,
            lambda name: archive.getinfo(name).external_attr >> 16, version, engines, True)
    with tarfile.open(plugin, "r:gz") as archive:
        members = []
        expanded = 0
        for member in archive:
            require(len(members) < 50000, "Plugin archive exceeds entry bound")
            expanded += member.size
            require(member.size <= MAX_FILE and expanded <= MAX_ARCHIVE, "Plugin expanded size exceeds bound")
            require(member.isfile() or member.isdir(), "Plugin archive contains a link or special entry")
            members.append(member)
        safe_archive_names([item.name for item in members])
        def read_member(name):
            member = archive.getmember(name)
            require(member.isfile(), f"Expected a packaged file: {name}")
            return archive.extractfile(member).read(MAX_FILE + 1)
        verify_archive_contents(set(archive.getnames()), read_member,
            lambda name: archive.getmember(name).mode, version, engines, False)
    return vsix, plugin, f"{extension['publisher']}.{extension['name']}"


def verify_host(receipt, target, version, extension_id, engine_hash, vsix_hash):
    platform, arch = target.split("-")
    require(receipt.get("schemaVersion") == 1 and receipt.get("kind") == "installed-vsix"
            and receipt.get("verified") is True and receipt.get("installExitCode") == 0
            and receipt.get("hostExitCode") == 0, f"Installed host did not complete successfully: {target}")
    require(receipt.get("vsixSha256") == vsix_hash, f"Host receipt belongs to another VSIX: {target}")
    host = receipt.get("host")
    require(isinstance(host, dict) and host.get("schemaVersion") == 1, f"Actual extension-host evidence missing: {target}")
    for record in (receipt, host):
        require(record.get("platform") == platform and record.get("arch") == arch
                and record.get("version") == version and record.get("extensionId") == extension_id,
                f"Installed host platform or version differs: {target}")
        require(record.get("engineSha256") == engine_hash, f"Installed host engine hash differs: {target}")
        install_root = record.get("extensionRoot", "").replace("\\", "/").rstrip("/").split("/")[-1].lower()
        expected = f"{extension_id}-{version}".lower()
        require(install_root == expected or install_root.startswith(expected + "-"), f"Evidence did not use the installed extension: {target}")
        checks = record.get("results")
        require(isinstance(checks, list) and all(isinstance(item, dict) and item.get("passed") is True for item in checks), f"Installed host checks failed: {target}")
        names = [item.get("name") for item in checks]
        require(len(names) == len(set(names)) and HOST_CHECKS.issubset(names), f"Installed host checks are missing or duplicated: {target}")
    require(receipt.get("target") == target and receipt["results"] == host["results"]
            and receipt["extensionRoot"] == host["extensionRoot"]
            and receipt.get("vscodeVersion") == host.get("vscodeVersion")
            and isinstance(host.get("vscodeVersion"), str) and isinstance(host.get("extensionHost"), str),
            f"Installed host summary contradicts actual host: {target}")


def verify_evidence(evidence_dir, source, version, engines, extension_id, vsix_hash):
    pin = read_json(source / "corpus/apex-recipes.pin.json")
    weakness_baseline = read_json(source / "corpus/apex-recipes.weakness-baseline.json")
    environment_hash = sha(read_bounded(source / "corpus/apex-recipes.environment.json"))
    result = {}
    for target in TARGETS:
        corpus = read_json(evidence_dir / target / "corpus.json")
        host = read_json(evidence_dir / target / "installed-vsix.json")
        audit = base.verify_corpus(corpus, pin, engines[target]["sha256"], version, environment_hash, weakness_baseline)
        verify_host(host, target, version, extension_id, engines[target]["sha256"], vsix_hash)
        result[target] = {"corpus": corpus, "installedExtension": host, "mutationAudit": audit}
    return result


def verify_committed_source(source):
    files = base.source_files(source)
    def git(*args):
        return subprocess.check_output(["git", "-C", str(source), *args], timeout=60)
    commit = git("rev-parse", "HEAD").decode().strip()
    require(re.fullmatch(r"[0-9a-f]{40}", commit), "Source commit is invalid")
    tree = {}
    for record in git("ls-tree", "-rz", "HEAD").split(b"\0"):
        if record:
            metadata, name = record.split(b"\t", 1)
            tree[name.decode()] = metadata.split()[2].decode()
    def included(name):
        path = pathlib.PurePosixPath(name)
        return name in base.ROOT_FILES or (path.parts[0] in base.ROOT_DIRS
            and not any(part in base.EXCLUDED or part == ".env" or part.startswith(".env.") for part in path.parts)
            and path.suffix.lower() not in base.BINARY_SUFFIXES)
    require(set(files) == {name for name in tree if included(name)},
            "Source archive includes uncommitted or untracked source, or omits committed files")
    for offset in range(0, len(files), 64):
        batch = files[offset:offset + 64]
        hashes = git("hash-object", "--", *batch).decode().splitlines()
        require(len(batch) == len(hashes) and all(tree.get(name) == digest for name, digest in zip(batch, hashes)),
                "Source archive includes uncommitted or untracked source; commit before finalizing")
    return commit


def public_paths(value):
    """Preserve unique manifest identities while omitting machine-specific roots."""
    if isinstance(value, str) and (pathlib.PurePosixPath(value).is_absolute() or pathlib.PureWindowsPath(value).is_absolute()):
        name = value.replace("\\", "/").rstrip("/").split("/")[-1]
        return "${HOST_PATH}/" + sha(value.encode())[:12] + "/" + name
    if isinstance(value, list):
        return [public_paths(item) for item in value]
    if isinstance(value, dict):
        return {key: public_paths(item) for key, item in value.items()}
    return value


def finalize(source, package_dir, engines_dir, evidence_dir, output):
    version = read_json(source / "package.json")["version"]
    engines = {target: verify_engine(engines_dir, target, version) for target in TARGETS}
    manifest = read_json(package_dir / "packaging-manifest.json")
    verify_package_manifest(manifest, version, engines)
    vsix, plugin, extension_id = verify_packages(package_dir, manifest, version, engines)
    evidence = verify_evidence(evidence_dir, source, version, engines, extension_id, sha(read_bounded(vsix, MAX_ARCHIVE)))
    commit = verify_committed_source(source)
    allowed = {vsix.name, plugin.name, "packaging-manifest.json", f"apx-{version}-source.zip", "release-evidence.json", "SHA256SUMS.txt"}
    if output.exists():
        require(all(file.is_file() and file.name in allowed for file in output.iterdir()),
                "Output contains unexpected files; use a clean release directory")
    # All gates passed before changing public output files.
    output.mkdir(parents=True, exist_ok=True)
    for file in (vsix, plugin, package_dir / "packaging-manifest.json"):
        if file.resolve() != (output / file.name).resolve():
            shutil.copyfile(file, output / file.name)
    inventory = base.write_source_zip(source, output / f"apx-{version}-source.zip")
    require(verify_committed_source(source) == commit, "Source changed while archiving")
    result = {"schemaVersion": 2, "version": version, "target": "universal", "targets": list(TARGETS),
              "sourceCommit": commit, "vsixSha256": sha(read_bounded(vsix, MAX_ARCHIVE)),
              "pluginSha256": sha(read_bounded(plugin, MAX_ARCHIVE)), "engines": list(engines.values()),
              "qualification": "All six native hosts passed installed extension and pinned offline corpus gates. Runtime execution remains experimental; no Salesforce org comparison or deployment certification.",
              "inputManifestRepresentation": "Absolute host paths redacted after original fingerprint verification. Fingerprints identify original absolute-path manifests.",
              "nativeArchiveCopiesVerified": 18, "hosts": evidence, "sourceFiles": inventory}
    (output / "release-evidence.json").write_text(json.dumps(public_paths(result), indent=2) + "\n", encoding="utf-8")
    checksums = [f"{sha(read_bounded(file, MAX_ARCHIVE))}  {file.name}"
                 for file in sorted(output.iterdir()) if file.is_file() and file.name != "SHA256SUMS.txt"]
    (output / "SHA256SUMS.txt").write_text("\n".join(checksums) + "\n", encoding="utf-8")
    return {"version": version, "sourceCommit": commit, "targetsVerified": len(TARGETS),
            "nativeArchiveCopiesVerified": 18, "sourceFiles": len(inventory), "checksummedFiles": len(checksums)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=pathlib.Path, default=pathlib.Path(__file__).resolve().parents[1])
    parser.add_argument("--package-dir", type=pathlib.Path, required=True)
    parser.add_argument("--engines", type=pathlib.Path, required=True)
    parser.add_argument("--evidence", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    print(json.dumps(finalize(args.source.resolve(), args.package_dir.resolve(), args.engines.resolve(),
                              args.evidence.resolve(), args.output.resolve()), indent=2))


if __name__ == "__main__":
    main()
