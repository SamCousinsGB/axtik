# Axtik 0.2.1 developer preview

This release addresses incorrect local passes and stale editor results, and adds a Salesforce comparison harness. Download the Windows x64 extension, CLI plugin and executable from [GitHub Releases](https://github.com/SamCousinsGB/axtik/releases/tag/v0.2.1). The executable and commands retain the APX namespace.

## Correctness changes

- A semantic pass checks all source bodies, including dead branches, for definite invalid types, assignments, members, access and supported overloads. Native API signatures and full Salesforce compilation remain incomplete.
- Reviewed standard-field constraints reject overlong values, including `Account.Name` beyond 255 UTF-16 units. Unsupported rounding semantics block instead of silently accepting a value.
- Record reads, edits and deletes use distinct sharing permissions. Public Read Only and View All do not grant writes. Explicit `as user` statements use the same security mode rules as Database USER_MODE calls.
- Modeled DML and query governor failures terminate execution. Catch blocks, partial-DML options and finally returns cannot convert them into passes.
- Assertion verdict counts include helpers and asynchronous callbacks, with setup counted separately. A successful test that reaches no assertion is visibly marked execution-only.
- Native and client checks cover consumed bytes and directory inventories. Resource edits, additions, renames, deletions, dirty buffers and external-folder changes invalidate results. Consumed links and junctions are rejected.

Protocol 2 is required by the bundled adapters. Upgrade the engine, extension and CLI plugin together; older reports are rejected explicitly. [Protocol details](REPORT-PROTOCOL.md) describe fingerprints, counters and limits.

## Verified release checks

The frozen Windows x64 engine is SHA-256 `06770609cbc746f08cfc5453519c731a2a8ecdb42e46efe65c1814f8ae1eaa66`. It runs all **322 unchanged Apex Recipes tests**, with no failures, blockers, engine errors or regressions from 0.2.0. The negative audit caught 310 of 311 assertion inversions, 11 initial entry sentinels and the additional entry sentinel for the preserved survivor.

| Check | Result |
| --- | ---: |
| Native tests | 268 ordinary + 2 explicitly enabled pinned-source integrations passed |
| Node tests | 63 passed, including 17 comparison-harness checks |
| Python release-finalizer tests | 8 passed |
| Installed VSIX in a real isolated VS Code host | 20 checks passed |
| Packaged plugin in a real isolated SF CLI | 5 checks passed, including all 322 corpus tests |
| Comparison fixtures against the frozen native binary | 10/10 local expectations satisfied; Salesforce unverified |

The corpus records 3,882 test-body/helper/async assertion verdicts. Three passing tests execute no assertion and are explicitly flagged; their identities are in [CORPUS.md](CORPUS.md). Assertion counts are observations, not a coverage percentage. The new CI workflow also exercises the unchanged corpus and its negative controls on Linux, alongside native and adapter tests on Windows, Linux and macOS.

## Assurance boundary

The release remains experimental. Local advisory passes return **4**; they do not certify deployment readiness. The Apex Recipes environment is explicitly selected and synthetic. One original assertion inversion survives because its assertions are in an unentered exception branch; the audit retains this result and separately verifies execution wiring with a weaker entry sentinel.

The Salesforce comparison harness supports offline plans, real local runs, and explicit sandbox dry-run submissions with org-identity checks. Its tests and local receipts do not constitute live Salesforce evidence. Live-org verification is deferred, and controlled sharing scenarios remain unverified until their org prerequisites can be checked automatically. See [SALESFORCE-COMPARISON.md](SALESFORCE-COMPARISON.md).

Release evidence preserves the corpus observations, mutation coverage gap and installed adapter checks. Checksums cover every distributed asset. Marketplace/npm publication, signed binaries, complete compiler parity and general enterprise qualification remain future gates.
