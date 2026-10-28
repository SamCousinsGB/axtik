# Axtik

Axtik ("ax-tick") is an open-source local Apex evaluator and test engine, written in Rust, with a VS Code extension and Salesforce CLI plugin. Version 0.2.0 is an installable **developer preview** with an expanded language and platform subset. The full runtime specification remains the roadmap.

The project is [SamCousinsGB/axtik](https://github.com/SamCousinsGB/axtik). This release retains the `apx` executable, `sf apx` commands, `apx.*` settings and APX editor commands so existing integrations continue to work. Axtik is the project name; APX is the current command and protocol namespace.

The extension includes a native executable. Users do not need Rust, a database, an org login or a separate SF CLI installation to run it. The first tested binary targets **Windows x64**. Linux/macOS build jobs are provided but those targets have not been exercised in this local release.

## Install and use

1. Download the Windows x64 `.vsix` from [GitHub Releases](https://github.com/SamCousinsGB/axtik/releases). If no packaged release is listed, use the source build below.
2. In VS Code, run **Extensions: Install from VSIX...** and select the downloaded package.
3. Open and trust your Salesforce project.
4. Select classes or a folder in Explorer and choose **APX: Validate Selected Classes or Folder**.
5. Read **Problems**, **Output > APX Local Apex**, and **APX Local Tests (Preview)** in the Testing panel.

**APX: Check Syntax and Declarations** checks source without running tests. **Validate** runs selected test classes; if you select only production source, it conservatively includes all discovered project tests and reports that expansion. Explicit test filters remain explicit. A syntax check does not replace the Salesforce compiler or deployment validation.

## Salesforce CLI

In VS Code, run **APX: Show Salesforce CLI Setup**. It provides the exact path of the plugin already bundled in the extension:

```text
sf plugins link <the-bundled-sf-plugin-directory>
sf apx check --source-dir force-app/main/default/classes
sf apx validate --source-dir force-app/main/default/classes --project .
sf apx test --source-dir force-app/main/default/classes/CalculatorTest.cls --filter CalculatorTest.exactDecimalArithmetic --json
sf apx validate --project . --environment local-environment.json --json
```

Repeat `--source-dir` for multiple files or directories. Bare class names are accepted by the engine when unambiguous in the loaded project. The extension setup command does not install or modify your CLI automatically. Relink after an extension update changes its installation path; unlink with `sf plugins unlink @apx/sf-plugin`.

The standalone plugin `.tgz` includes production dependencies and the native binary. It can also be extracted and linked as a local plugin. The VSIX is the easiest installation path.

## Current implementation

- Tree-sitter Apex syntax diagnostics with original source locations, project dependency loading and duplicate declaration checks.
- Test discovery, classes and interfaces, inheritance, constructors, properties, static state, typed collections, control flow, exceptions and assertions within the preview subset.
- Exact rational representation for supported arithmetic, with explicit integer and resource bounds. This is not yet a complete Salesforce Decimal policy.
- Project metadata and an owned transaction store, supported DML/triggers, SOQL, fixed-result SOSL tests, savepoints, fresh test contexts and setup data isolation.
- Explicit local user and permission policies, permission-set membership, test stubs, HTTP mocks, cryptographic operations, JSON, UTC dates and a bounded Java-compatible regex subset.
- Source-driven DataWeave, Platform Cache, queued asynchronous test callbacks and supported platform-event delivery. Subsystem documents describe their limits and unresolved semantics.
- Explicit errors for unsupported reached behavior and conservative metadata blockers for data operations.
- Shared bounded child-process runner; safe argument passing, cancellation, timeouts and structured report validation.
- VS Code diagnostics/Test Explorer and real `sf apx` commands consuming the same reports.

## Preview result policy

Successful execution is **experimental**, because it has not been qualified against a Salesforce oracle corpus. The extension reports local assertion passes as advisory/skipped in Test Explorer, with an explanation. It never claims full platform compatibility.

| Exit | Meaning |
| --- | --- |
| 0 | Syntax/declaration checks completed; no claim of full typing or runtime correctness. |
| 1 | A local assertion or runtime test failure. |
| 2 | Invalid input, configuration, syntax or checked declaration. |
| 3 | Engine/host error. |
| 4 | Preview success or blocked unsupported behavior; inspect the distinct report status. |
| 130 | Cancelled adapter request. |

Current evaluation uses an owned syntax-tree walker. A complete binder/type checker, typed IR/bytecode, independently qualified numeric and security behavior, complete Salesforce automation and asynchronous semantics, incremental compilation, parallel workers, and DAP debugging remain engineering work. No cloud fallback occurs when a feature is unsupported.

## Explicit local environment

Org identity, profiles, permissions and service responses are configuration inputs. Supply an environment JSON with the native/SF CLI `--environment` flag or VS Code's `apx.environmentPath` setting. Relative extension and SF CLI profile paths resolve from the Salesforce project. APX never silently selects a profile or logs into an org.

The checked-in `corpus/apex-recipes.environment.json` describes a **synthetic local environment** for the pinned sample project. It records its clock, users, permission policy and fixture provenance; it is not an export of Salesforce's standard profiles. Tests relying on unsupported org behavior remain blocked. Editing the selected profile invalidates VS Code results just as source changes do.

## Build

Requirements: the pinned Rust toolchain, a native C compiler for Tree-sitter, Node.js 22 or newer, and npm. Standard end users do not need these tools.

```text
cargo test --locked
cargo build --release --locked
npm ci --ignore-scripts
npm run build
npm run check --workspace apx-local-apex
npm test
npm run package -- --target win32-x64 --engine target/release/apx.exe --output dist
```

Packaging checks matched package/native versions and the executable format for the requested target. It stages real production dependencies for offline linking, bundles the extension, and produces VSIX/plugin archives. Registry dependencies remain pinned when the owned runner archive changes. Do not publish a platform build before running its native and extension tests.

For frozen corpus evidence, reproducible source archives and final checksums, follow [docs/RELEASING.md](docs/RELEASING.md). The optional artifact finalizer uses Python 3's standard library.

Try the source demo:

```text
target/release/apx check fixtures/demo --json
target/release/apx validate fixtures/demo --json
```

The second command deliberately returns 4 even when its local assertions pass. Negative fixtures live separately so they do not contaminate the valid demo project.

## Development and license

See [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md), [docs/protocol.json](docs/protocol.json), [docs/IMPLEMENTATION.md](docs/IMPLEMENTATION.md), and [local service boundaries](docs/LOCAL-SERVICES.md). Original source is Apache-2.0. Dependency notices and exact lockfiles are included. GitHub is the source and release distribution channel. The VS Code Marketplace publisher and npm package names have not been registered or published; use the repository's release assets or build from source.

## Real project compatibility

The Windows x64 0.2.0 preview executes **all 322 unchanged tests** from Salesforce's official [Apex Recipes](https://github.com/trailheadapps/apex-recipes) at the pinned revision: **322 local advisory passes, zero failures, blockers or engine errors**, with all 142 Apex files and test identities conserved. This result requires the explicitly selected synthetic corpus environment; it is not a claim about an arbitrary Salesforce org.

The negative audit caught 310 of 311 assertion inversions. One test's assertions sit in an unentered catch branch, so its inversion survived; that finding remains in the evidence. Eleven tests without invertible inline assertions, plus the survivor, passed weaker entry-sentinel checks that verify execution and assertion wiring only. See [docs/CORPUS.md](docs/CORPUS.md) for the exact engine/profile hashes, mutation limits and reproduction commands, and [the 0.2.0 release notes](docs/RELEASE-0.2.0.md) for verified integrations. The original 0.1.0 baseline is retained separately. These observations do not establish complete Salesforce parity or a performance advantage.
