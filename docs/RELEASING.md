# Preparing an Axtik release

Keep the workspace, native crate, runner, SF plugin and extension versions matched. Run native tests with the locked Cargo dependencies, build the native release executable, run the locked npm build/typecheck/tests, and explicitly run any optional external-corpus integration tests with their documented environment variables. Record ignored tests accurately.

Freeze the executable and explicit environment profile before collecting release evidence. Run the complete corpus harness with `--require-all`; it checks the pinned source and all 322 test identities and performs a negative mutation for each passing test. Do not replace this report with filtered smoke tests.

Use the root packaging script to produce a VSIX and plugin archive in a new release directory. It checks platform and version consistency, stages production dependencies, and retains registry integrity pins when refreshing the owned runner archive. `--refresh-package-lock` is only for a deliberate reviewed dependency refresh.

Install the produced VSIX into an isolated real VS Code Extension Host and link the extracted plugin in an isolated real SF CLI state directory. Preserve both test reports. The extension report must identify the installed release root, engine SHA-256 and passing checks. The SF report must record the engine/archive SHA-256 and set `verified: true` only after its assertions pass.

After completing release documentation, use Python 3's standard library finalizer:

```text
python scripts/release_artifacts.py --output /absolute/path/to/release-directory --engine /absolute/path/to/apx --corpus-report /absolute/path/to/frozen-corpus-report.json --extension-evidence /absolute/path/to/installed-extension-result.json --sf-evidence /absolute/path/to/installed-sf-result.json --workspace /absolute/path/to/build-workspace
```

The finalizer verifies native copies in both packages, package versions, the environment hash, corpus identity conservation and all negative mutation gates. It writes the standalone engine, deterministic source ZIP, portable release evidence and `SHA256SUMS.txt`. The source allowlist includes untracked implementation files, fixtures, licenses, documentation and tests; generated binaries, build directories, dependency installations and workspace state are excluded. The optional workspace prefix redacts local paths from exported evidence.

Run `python scripts/test_release_artifacts.py` to check deterministic source packaging and release-gate rejection. If source documentation or any deliverable changes after finalization, rerun the finalizer so the source archive and checksums agree. Finalization does not publish anything or modify the upstream repository.

Publish reviewed source to [SamCousinsGB/axtik](https://github.com/SamCousinsGB/axtik) and attach only the finalized assets to a matching GitHub release. Label the release a developer preview and identify its tested host platform and explicit corpus environment. Existing executable, package, setting and protocol names remain `apx`/`@apx`; the public project name is Axtik. Marketplace/npm publication is a separate release step requiring registered identities and its own verification.
