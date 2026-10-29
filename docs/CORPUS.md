# Apex Recipes compatibility corpus

Source: Salesforce's official Trailhead sample, [trailheadapps/apex-recipes](https://github.com/trailheadapps/apex-recipes).

- Revision: `b364267fc786a8a3ac4c3968467fcc4abd4adac6`
- Source API version: 68.0 (Winter 27)
- Upstream license: CC0-1.0; preserve its `LICENSE.md` with any copy.
- Source was left unchanged. No upstream install scripts, deployments or org tests were run.

## Reproduce

From the Axtik repository, clone a separate sibling copy. Axtik does not vendor the sample into its engine:

```text
git clone https://github.com/trailheadapps/apex-recipes.git ../apex-recipes
git -C ../apex-recipes checkout b364267fc786a8a3ac4c3968467fcc4abd4adac6
apx check ../apex-recipes --json
apx validate ../apex-recipes --environment /absolute/path/to/axtik/corpus/apex-recipes.environment.json --json
```

Use the built `apx` executable's full path if it is not on PATH. Alternatively, link the bundled SF CLI plugin and run:

```text
sf apx check --project ../apex-recipes --source-dir force-app --json
sf apx validate --project ../apex-recipes --source-dir force-app --environment /absolute/path/to/axtik/corpus/apex-recipes.environment.json --json
sf apx test --project ../apex-recipes --filter AuraEnabledRecipes_Tests.testGetFieldValue --environment /absolute/path/to/axtik/corpus/apex-recipes.environment.json --json
```

Replace the profile placeholder with the actual absolute file path on your machine. The SF CLI project's path is relative to the shell's working directory; source paths are relative to that project when `--project` is supplied. Without `--project`, source paths are relative to the shell's working directory and APX discovers the enclosing Salesforce project. In VS Code, open the cloned project, explicitly set `apx.environmentPath` to this profile, and use APX's Explorer commands or **Validate Workspace**.

## Recorded 0.2.1 result

The frozen Windows x64 0.2.1 report is `2026-10-28T23-58-21.700Z-06770609cbc7.json`. All 322 unchanged tests pass locally, with zero failures, blockers, engine errors or regressions from 0.2.0. Source and identity conservation remain verified. The negative-control results below are unchanged: 310/311 assertion inversions killed, one survivor retained, 11 original entry sentinels caught and one separate survivor wiring check caught.

- Engine SHA-256: `06770609cbc746f08cfc5453519c731a2a8ecdb42e46efe65c1814f8ae1eaa66`
- Environment SHA-256: `3658d2a0fc89176375fd02f5a5a1ea280b8d84a087085c90a0e7808563901895`
- Native protocol: 2; 492 consumed files and 117 directory inventories captured and verified.
- Observed test-body/helper/async assertion verdicts: 3,882, all passing. Setup assertion verdicts: 0.

Three locally passing tests record zero assertions and receive execution-only warnings:

1. `AtFutureRecipes_Tests.callAtFutureCalloutPositive`
2. `DMLRecipes_Tests.testDatabaseMethodInsertInUserModeNegative`
3. `PSGAdvancedTestingRecipes.testNegativePSGTestsWithMetadata`

This adds evidence about executed assertions without changing upstream source or converting a weak test into a stronger one. The native validation took 12.395 seconds in this individual local run; this is not a benchmark. The complete report is preserved in the 0.2.1 release evidence. [Release checks](RELEASE-0.2.1.md) cover the packaged clients and comparison harness.

## Recorded 0.2.0 result

The frozen **0.2.0 Windows x64** run on 28 October 2026 executes all 322 unchanged tests under the explicitly selected synthetic corpus profile. The report is `2026-10-28T22-46-31.532Z-6ab75af8f366.json`; its complete contents are included in the release's `release-evidence.json`.

To reproduce the complete identity and negative-control audit from the Axtik repository:

```text
node scripts/corpus.mjs --repo ../apex-recipes --engine /absolute/path/to/apx --environment corpus/apex-recipes.environment.json --output .corpus-history --require-all
```

The harness records the environment hash alongside the engine and corpus hashes. It never silently selects a profile. Use an absolute profile path with `sf apx validate --environment ...` if the profile is outside the selected Salesforce project; the CLI resolves relative profile paths against that project. In VS Code, set `apx.environmentPath` explicitly. The profile describes synthetic local permissions and service assumptions; it does not qualify them against a real org.

| Measure | Result |
| --- | ---: |
| Apex source files parsed | 142 |
| Discovered tests | 322 |
| Local advisory passes | 322 |
| Blocked by unsupported behavior | 0 |
| Assertion/runtime failures | 0 |
| Host errors | 0 |
| Regressions from the 0.1.0 baseline | 0 |
| Source/file/test identity conservation | Verified |

The native result is `preview_passed`, assurance `experimental-local-execution`, exit code **4**. The audit command completes with exit 0 when its preview gates pass; that does not promote the native runtime's assurance. A syntax/declaration check returning 0 also does not imply Salesforce compilation or deployment readiness.

The frozen identities are:

- Engine SHA-256: `6ab75af8f3666294f60e8bcbcb5613354b07a501f00651b1a64c9b71088f10c4`
- Explicit environment SHA-256: `3658d2a0fc89176375fd02f5a5a1ea280b8d84a087085c90a0e7808563901895`
- Upstream Git tree: `eef4a823fe2b25d28e153f824648bd89e8a0d8ac`

The validation took 11.373 seconds in this individual local release run. This is a smoke-test observation, not a benchmark: no matched Salesforce or competitor performance run was performed.

## Negative controls and coverage gap

| Negative audit | Observed result |
| --- | ---: |
| Primary assertion inversions | 311 |
| Assertion inversions killed by assertion failures | 310 |
| Surviving assertion inversions | 1 |
| Primary entry sentinels for tests without invertible inline assertions | 11 caught |
| Additional entry sentinel after the survivor | 1 caught |
| Tests with a verified negative control | 322 |

The survivor is `DMLRecipes_Tests.testDatabaseMethodInsertInUserModeNegative`. At the pinned source, it inserts a valid `Account` named `Texas` as the configured privileged user. Its assertions are exclusively inside a `catch (CustomDmlException)` branch, and there is no failure assertion if insertion succeeds. In this explicit environment, insertion succeeds and those assertions are not reached, so inverting them cannot fail the test. The original source and runtime were not changed to force an exception.

The audit retains this primary result as `killed: false`. A separate entry-sentinel run fails an assertion, verifying that the test executes and assertion failures propagate. That weaker result does **not** verify the original catch branch. The 11 primary sentinel tests have the same execution-only limitation; their identities are listed in the full evidence. Accordingly, the report records `negativeControlsVerified: true` and `negativeMutationsKilled: false`. It does not claim that all original assertions were exercised.

The first completed audit, `2026-10-28T22-40-21.015Z-6ab75af8f366.json`, preserved the same surviving inversion. The final audit uses the unchanged engine and profile and adds the separately recorded control; it does not relabel or suppress the finding.

## Baseline and remaining qualification

The immutable [0.1.0 baseline](../corpus/baselines/initial-0.1.0.json) records 2 passes and 320 blockers. Version 0.2.0 expands the actual evaluator and platform subset while preserving every upstream test identity. The complete baseline remains available for comparison.

These results establish local execution of this sample under one declared environment. Synthetic permissions, fixed service fixtures, the unexercised assertion branch, and the bounded language/platform implementation remain explicit limitations. Independent Salesforce conformance fixtures, complete type checking, broader org/metadata cases and matched performance measurements remain engineering work. Add general positive and negative regressions when expanding behavior; do not introduce test-name-specific outcomes.
