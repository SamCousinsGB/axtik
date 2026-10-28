# Reproducible Apex Recipes corpus

The pin fixes Salesforce's official `trailheadapps/apex-recipes` repository at `b364267fc786a8a3ac4c3968467fcc4abd4adac6`: 142 Apex files and exactly 322 test identities. The source is not vendored into the evaluator and must remain unchanged. Its license is CC0-1.0.

Build the shared runner, then use a separate clone checked out at the pinned revision:

```text
npm run build --workspace @apx/runner
node scripts/corpus.mjs --repo ../apex-recipes --engine ../apx.exe --output ../corpus-history --baseline corpus/baselines/initial-0.1.0.json
node scripts/corpus-audit.mjs --repo ../apex-recipes
node --test scripts/corpus.test.mjs
```

The engine path can be a release or debug executable. The runner never downloads, builds, deploys, or executes upstream installation scripts. It checks Git origin, commit, tree, clean working state, individual normalized source hashes, and independently scanned test identities before and after execution. Native discovery and execution must preserve every source file and test identity. A substituted or duplicated identity fails even when the total remains 322.

Every locally passing test receives a negative mutation in a temporary copy. The harness inverts its actual assertions while preserving arguments and evaluation order, then records whether that exact test fails an assertion. Tests without an invertible inline assertion receive an explicitly recorded weaker entry sentinel. If an assertion inversion returns passed, a separate entry sentinel checks execution wiring, while the original inversion remains recorded as a survivor. Sentinels prove execution/assertion plumbing only; they do not establish coverage of the original assertion path or domain semantics. The original repository is never edited, and mutation results do not change the original pass count. Blocked, errored and non-assertion failures never count as verified negative controls.

Each run writes an immutable timestamped JSON report and updates `latest.json`. The previous report is the default comparison baseline; `--baseline` selects a specific record. An individual passed-to-nonpassed transition or newly errored test fails the gate, even if the aggregate pass count rises. Missing or added tests fail conservation. Passing the preview audit gate confirms corpus integrity, no detected regression and verified negative controls for every passing test. The separate `negativeMutationsKilled` flag remains false if any primary mutation survived, even when an additional weaker sentinel was caught. Add `--require-all` to fail unless all 322 original tests pass. Local passes still have experimental assurance, not Salesforce qualification.

`baselines/initial-0.1.0.json` preserves the original 2 passed / 320 blocked report. `apex-recipes.environment.json` supplies the documented synthetic local users, permission policies, clock and service fixtures to the current native environment adapter when explicitly selected. Its provenance and unresolved platform assumptions remain visible. It does not modify upstream files or become active implicitly. No test is filtered for needing configuration.

The frozen 0.2.0 run records 322 local passes, 310 of 311 assertion inversions killed, 11 initial entry sentinels caught and one additional entry sentinel caught after a surviving inversion. The survivor and its assertion-coverage limitation remain visible in [docs/CORPUS.md](../docs/CORPUS.md) and the release evidence.

The feature inventory is an independent lexical analysis. Its conservative transitive dependencies include unexecuted methods and branches, so use its counts to plan work, not as coverage measurements. Read `docs/CORPUS-PLAN.md` alongside generated `docs/CORPUS-FEATURES.md` and `docs/corpus-feature-matrix.json`.
