# Changelog

## 0.2.2

- One VSIX bundles native engines for Windows, macOS and Linux on x64 and ARM64.
- Automatic host selection, static Linux/Windows runtimes, and verified executable-permission recovery for bundled Unix engines.
- Installed-extension verification on all six hosts, with artifact hashes and test receipts.
- Clear recovery instructions for a missing engine or broken custom path; no runtime downloads or SF CLI requirement.

## 0.2.1

- Corrected static checking, field constraints, sharing and terminal governor failures.
- Assertion counts, setup counts and zero-assertion warnings.
- Verified input snapshots and persistent watchers for resources and external projects.

## 0.2.0

- Public project name Axtik, with existing APX commands, settings and package identifiers retained.
- Expanded native language, metadata/data, local permissions, HTTP/crypto/JSON, DataWeave, Cache, asynchronous test and event support with explicit unsupported boundaries.
- Explicit environment profile setting, shared with the native engine and Salesforce CLI; profile/source/metadata changes invalidate results.
- Matched native, extension, runner and CLI package versions, with pinned offline CLI dependencies.
- Advisory execution remains explicit; no Salesforce conformance or deployment certification is implied.

## 0.1.0

- Native local checks and experimental test execution from classes, folders and workspaces.
- Explorer actions, Problems diagnostics, bounded/cancellable execution and Test Explorer integration.
- Explicit advisory preview results and opt-in Salesforce CLI setup instructions.
