# Implementation status

Axtik version 0.2.0 expands the first vertical implementation of the greenfield design. It preserves the `apx` native/CLI/editor interfaces and provides a bounded evaluator that can be extended with conformance evidence. It does not meet the P1 or v1 gates in the specification.

## Implemented

Rust source loader/parser, declaration inventory, bounded experimental evaluator, assertions, selected language/data behavior, fresh test contexts, JSON CLI, safe shared process runner, Salesforce CLI commands, VS Code commands/Problems/Test Explorer, and native package staging.

The compatibility expansion adds typed collections and object instances, constructors/inheritance, properties, exceptions, exact JSON numbers, cryptographic primitives, HTTP mocks, UTC dates, source-driven DataWeave, metadata schema loading, a transactional record store, SOQL and fixed-result test SOSL, explicit user/permission fixtures, test stubs, Platform Cache, asynchronous test callbacks and supported event delivery. Each subsystem has independent positive and negative regressions. Reached APIs and overloads outside its implemented contract still block. Final release evidence must identify the frozen engine and environment hashes; source changes are not retroactively credited to older packages.

The native engine and SF CLI accept `--environment` for an explicit JSON profile. The extension accepts `apx.environmentPath`, resolved relative to the Salesforce project. The extension invalidates results when that file, source, metadata, DataWeave, or relevant configuration changes. Without an explicit profile, org/user identity and unsupported access policies are not guessed. The checked-in corpus profile is synthetic and is not an export of Salesforce's standard profile permissions.

Local service responses must also be explicit. The corpus profile currently supplies an opaque `ConnectApi.FollowingPage` for one declared Chatter request to exercise SeeAllData isolation; page contents and remote behavior remain unimplemented. Such fixture-assisted execution is not evidence that the Chatter service has been recreated. Cache capacity uses a named local accounting model; regex supports an explicitly bounded ASCII subset. See [local service boundaries](LOCAL-SERVICES.md), [test search](TEST-SEARCH.md), the other subsystem documents and source contracts before relying on behavior beyond the recorded corpus.

## Qualification status

All runtime execution is unqualified preview behavior. No user org or credentials are used. Local regression tests establish implementation behavior, not complete Salesforce parity. Public upstream Salesforce CI evidence is used to resolve specific test-harness interactions, with its exact source revision recorded separately; this is not a complete independent oracle comparison of APX. Syntax/declaration check scope is deliberately narrower than compilation.

## Recorded 0.2.0 verification

The frozen Windows x64 engine executes all 322 unchanged Apex Recipes tests under the explicit synthetic corpus profile, with 322 advisory passes and zero failures, blockers or engine errors. All 142 source files and 322 identities were conserved, and the original sample stayed unchanged. The negative audit caught 310 of 311 assertion inversions. One survived because its original catch-branch assertions were not reached; an additional weaker entry sentinel verified execution wiring. Eleven tests without invertible inline assertions also use entry sentinels. The report preserves `negativeMutationsKilled: false` alongside `negativeControlsVerified: true`; these are different claims. See [CORPUS.md](CORPUS.md) for hashes and the specific coverage gap.

Verification also passed 237 ordinary native tests, the separately invoked external DataWeave integration test, 40 Node tests and 7 Python release-finalizer tests. The packaged Windows extension passed 10 checks in a real isolated VS Code Extension Host. The extracted packaged plugin passed 5 checks in a real isolated Salesforce CLI, including the full 322-test corpus. Both adapters used the same frozen engine hash. Native and adapter CI completed on Windows, Linux and macOS for implementation revision `04fcbf7`; the downloadable native packages in this release are Windows x64.

## Next engineering gates

1. Freeze an independently observed platform fixture corpus and the exact first profile. Resolve numeric, type, access and rollback rules against that corpus.
2. Introduce canonical typed IR and a complete checker for the supported slice; replace unchecked interpretation decisions with resolved contracts.
3. Introduce guest handles and explicit collection/object semantics, then the bytecode evaluator with reference parity.
4. Expand schema, transactions and automation through per-feature fixtures. Keep unsupported active metadata blocking affected operations.
5. Implement immutable artifacts, test-world overlays, sound incremental selection and bounded workers, one optimization at a time.
6. Add DAP, replay and versioned capability catalogues; qualify cross-platform packages and publish matched benchmark evidence.

The installed extension already uses the same engine boundary the later implementation will retain. Engine replacement must preserve protocol validation and visibly update its assurance level; it cannot quietly promote experimental outcomes to qualified ones.

## Publishing

Source and packaged releases use [SamCousinsGB/axtik](https://github.com/SamCousinsGB/axtik). The Marketplace publisher and npm scope are not registered distribution channels for this preview; existing `@apx/*` package names identify bundled local packages. See [SECURITY.md](../SECURITY.md) for private reporting and [RELEASING.md](RELEASING.md) for evidence gates. Hosted CI results are recorded by GitHub Actions for the published commit; local checks alone do not establish a hosted run or qualify an untested platform.
