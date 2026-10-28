# Axtik 0.2.0 developer preview

Released as a Windows x64 local Apex evaluator, VS Code extension and Salesforce CLI plugin. The public project name is Axtik; executable names, `sf apx` commands, `apx.*` settings and bundled package identifiers remain compatible with the APX preview.

## What is included

- A native Rust engine with the expanded language, metadata, data, query, security, mock, asynchronous and local-service subsets described in [IMPLEMENTATION.md](IMPLEMENTATION.md).
- A VSIX containing the native engine and SF CLI plugin. Select classes or a folder in VS Code and run **APX: Validate Selected Classes or Folder**.
- A standalone plugin archive for extraction and `sf plugins link`, a native executable, source ZIP, release evidence and SHA-256 checksums.
- Explicit environment profiles shared by the engine, CLI and editor. Local validation needs no org login, database server or network service.

Download from [GitHub Releases](https://github.com/SamCousinsGB/axtik/releases/tag/v0.2.0). Installation and commands are in the [README](../README.md). Marketplace/npm publication is not part of this release.

## Verified behavior

The frozen engine executes all **322 unchanged Apex Recipes tests** across 142 Apex files with the checked-in synthetic environment explicitly selected: 322 local advisory passes, zero failures, blockers or engine errors, and no baseline regressions. Source hashes and every test identity were conserved.

The negative audit caught **310 of 311 assertion inversions**, all 11 initial entry sentinels, and one additional entry sentinel after the surviving inversion. The survivor remains recorded: its assertions are inside an exception branch not reached under the local fixture. Entry sentinels establish execution/assertion wiring only. This is not complete assertion-path coverage; [CORPUS.md](CORPUS.md) names the test and records the exact hashes.

| Verification | Result |
| --- | ---: |
| Ordinary native tests | 237 passed |
| Explicit external DataWeave integration | 1 passed |
| Node tests | 40 passed |
| Python release-finalizer tests | 7 passed |
| Installed VSIX in a real isolated VS Code Extension Host | 10 checks passed |
| Extracted plugin in a real isolated Salesforce CLI | 5 checks passed, including all 322 corpus tests |

The installed extension and CLI plugin used the same frozen native engine, SHA-256 `6ab75af8f3666294f60e8bcbcb5613354b07a501f00651b1a64c9b71088f10c4`. Native and adapter CI also completed on Windows, Linux and macOS for implementation revision `04fcbf7`; this release packages the verified Windows x64 target.

## Preview boundary

Successful native execution remains `preview_passed` with exit code **4**, and VS Code displays local successes as advisory. The explicit corpus profile is synthetic, not a Salesforce org export. No independent Salesforce oracle qualification or matched competitor benchmark was performed. The bounded runtime does not certify arbitrary projects for deployment, and [local service assumptions](LOCAL-SERVICES.md) remain visible.

`release-evidence.json` preserves the full corpus result, primary mutation survivor, weaker controls and installed-adapter evidence. `SHA256SUMS.txt` covers the release files. Original source is Apache-2.0, with dependency licenses and notices included.
