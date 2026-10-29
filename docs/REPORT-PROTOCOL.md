# Native report protocol 2

Axtik 0.2.1 emits `schemaVersion: 2`. The bundled runner rejects reports from other protocol versions, incomplete reports, inconsistent counters and contradictory success labels. Engine and adapters must be upgraded together. The existing assurance identifier `syntax-and-declarations` is retained for the bounded `check` operation; it does not certify complete Salesforce compilation.

## Assertions

Every selected test has `assertions` and `setupAssertions`, each containing nonnegative integer `executed`, `passed` and `failed` fields. `executed = passed + failed`. Counts represent completed assertion verdicts; a failure while evaluating an assertion argument does not invent a verdict. Test helpers, future/queueable work and delivered event callbacks share the test's ledger. Setup has a separate ledger and is reported for each test's isolated setup execution.

Discovery reports zero counters. A successful test with zero test-body assertion verdicts retains an advisory result with `APX_NO_ASSERTIONS` and an explicit execution-only message. Setup assertions do not disguise a test that has no assertions. Counts do not establish branch coverage or the quality of an assertion.

## Input identity

`inputs` is an ordered array of `{path, sha256}` for bytes actually read by the engine, including source, metadata, project configuration, selected environment profiles and arbitrary resource payloads. `inputDirectories` contains `{path, sha256, includeIgnored}` for inventories captured before scanning. Paths are absolute and normalized; arrays use native UTF-8 byte order. Consumed links and Windows reparse points are rejected, including path ancestors.

The fingerprint is SHA-256 of UTF-8 JSON, without whitespace, in this exact property order:

```json
{"files":[{"path":"...","sha256":"..."}],"directories":[{"path":"...","sha256":"...","includeIgnored":false}]}
```

A directory digest covers a JSON array of `{name,kind}`, sorted by UTF-8 name bytes. Kinds are `file`, `directory`, `symlink` and `other`. Ordinary scanning excludes exact names `.git`, `node_modules`, `target`, `.sf`, `.sfdx`, `.apx` and `dist`; `includeIgnored: true` includes them for expanded resources. Inventory hashes detect additions, removals and entry-kind changes; consumed file hashes detect content changes.

Readers record the exact parsed bytes. Repeated reads that disagree invalidate a run even if the original content is later restored. Final native verification rejects changed or missing input contents and directory entries. The runner independently rehashes inputs after process completion; the editor verifies again immediately before displaying results and retains watchers for external roots. Dirty documents and request generations also invalidate editor results. This is bounded filesystem verification, not an atomic filesystem snapshot against an adversarial writer.

Consumed files are bounded to 256 MiB in aggregate, inventories to 100,000 entries. Host verification is cancellable and shares the request deadline. A changed input produces an explicit rejection; clients must not display its prior advisory passes as current. Input paths in published release evidence are redacted after fingerprint verification; the original fingerprint identifies the original absolute-path manifest.

## Assurance

Local passing tests remain `preview_passed`, `experimental-local-execution`, exit code **4**. A compiler check returning 0 is limited to the implemented checks. Protocol integrity, assertions, negative controls and corpus passes improve evidence; none independently qualifies Salesforce parity.
