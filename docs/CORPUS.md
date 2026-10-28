# Apex Recipes compatibility corpus

Source: Salesforce's official Trailhead sample, [trailheadapps/apex-recipes](https://github.com/trailheadapps/apex-recipes).

- Revision: `b364267fc786a8a3ac4c3968467fcc4abd4adac6`
- Source API version: 68.0 (Winter 27)
- Upstream license: CC0-1.0; preserve its `LICENSE.md` with any copy.
- Source was left unchanged. No upstream install scripts, deployments or org tests were run.

## Reproduce

Clone a separate copy; the Axtik repository does not vendor the sample into its engine:

```text
git clone https://github.com/trailheadapps/apex-recipes.git
git -C apex-recipes checkout b364267fc786a8a3ac4c3968467fcc4abd4adac6
apx check apex-recipes --json
apx validate apex-recipes --json
```

Use the built `apx` executable's full path if it is not on PATH. Alternatively, link the bundled SF CLI plugin and run:

```text
sf apx check --project apex-recipes --source-dir force-app --json
sf apx validate --project apex-recipes --source-dir force-app --json
sf apx test --project apex-recipes --filter AuraEnabledRecipes_Tests.testGetFieldValue --json
sf apx test --project apex-recipes --filter IterationRecipes_Tests.testIterateOnAccountListPositive --json
```

The SF CLI project's path is relative to the shell's working directory; source paths are relative to that project when `--project` is supplied. Without `--project`, source paths are relative to the shell's working directory and APX discovers the enclosing Salesforce project. In VS Code, open the cloned project and use APX's Explorer commands or **Validate Workspace**.

## Recorded preview result

The table below is the immutable **0.1.0 release baseline**, not a claim about the in-progress compatibility expansion. The acceptance target is all 322 unchanged test identities. New evidence is produced by the corpus harness, including assertion mutations and source hashes.

For an expanded run with the explicit synthetic environment, from the Axtik repository:

```text
node scripts/corpus.mjs --repo ../apex-recipes --engine /absolute/path/to/apx --environment corpus/apex-recipes.environment.json --output .corpus-history --require-all
```

The harness records the environment hash alongside the engine and corpus hashes. It never silently selects a profile. Use an absolute profile path with `sf apx validate --environment ...` if the profile is outside the selected Salesforce project; the CLI resolves relative profile paths against that project. In VS Code, set `apx.environmentPath` explicitly. The profile describes synthetic local permissions and service assumptions; it does not qualify them against a real org.

| Measure | Result |
| --- | ---: |
| Apex source files parsed | 142 |
| Discovered tests | 322 |
| Local advisory passes | 2 |
| Blocked by unsupported behavior | 320 |
| Assertion/runtime failures | 0 |
| Host errors | 0 |

The two passing methods are `AuraEnabledRecipes_Tests.testGetFieldValue` and `IterationRecipes_Tests.testIterateOnAccountListPositive`. They exercise static calls, assertions and list/iterable behavior. Each successful run remains experimental and returns exit code 4. The full run has status `blocked` and also returns 4. A parse check returning 0 does not mean the project compiles or deploys on Salesforce.

## What this reveals

The initial release could not execute most of this project. Its frequent first blockers included Maps, Crypto, exception handling, static initializers, nested types, instance construction, and project schema/automation. These now have implementations under expansion. A first-blocker count is not a complete feature inventory: resolving one blocker can reveal another.

Compatibility work should next establish precise object/collection and exception semantics, then metadata-aware storage and transactions. Expand through unchanged upstream tests plus independent small platform fixtures. Do not implement a named API just to make this corpus pass; test its general semantics and negative cases too.

Local timings are smoke-test observations, not benchmark claims. No Salesforce comparison run or matched competitor benchmark was performed.
