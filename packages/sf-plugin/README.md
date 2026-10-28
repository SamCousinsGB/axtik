# Axtik for Salesforce CLI

Local Apex syntax/declaration checks and experimental test execution. This plugin does not connect to an org, deploy metadata, or provide complete Salesforce validation.

## Install

The command namespace remains `sf apx`, and the bundled local package remains `@apx/sf-plugin`. Download the plugin archive from [GitHub Releases](https://github.com/SamCousinsGB/axtik/releases), extract it, and link its `package` directory. Alternatively, the VS Code extension's **APX: Show Salesforce CLI Setup** command supplies the exact path of its bundled plugin.

Use the extracted or bundled plugin directory:

```sh
sf plugins link /absolute/path/to/extracted/package
sf apx check --source-dir force-app/main/default/classes --project .
sf apx validate --source-dir force-app/main/default/classes --project . --json
sf apx test --source-dir classes/ExampleTest.cls --filter ExampleTest.example
sf apx validate --project . --environment local-environment.json --json
```

Repeat `--source-dir` for multiple class files or folders. By default, the engine finds the enclosing `sfdx-project.json` from the selected path and loads its package directories, including when invoked from a nested source folder. `--project` overrides that discovery with an explicit dependency root. When `validate` selects only production classes, the engine conservatively selects project tests and reports `APX_SELECTION_EXPANDED`; it does not pretend to know which tests cover the selected class. An explicit test filter selects the requested test identities. Inspect the report's selected files, tests and diagnostics to see the actual scope. All paths pass as process arguments without a shell. `--engine` accepts an explicit absolute native executable path when the package does not include your platform.

With `--project`, source paths are relative to that project; omitting `--source-dir` selects all its Apex sources. For example, `sf apx test --project /path/to/project --filter ExampleTest.example` works from another folder. Without `--project` or `--source-dir`, the current folder supplies the source selection while ancestor discovery supplies dependencies.

`--environment` supplies explicit local users, permissions, clock, and service fixtures. Relative profile paths resolve from the selected project. Profiles are never loaded automatically; their synthetic values are inputs to local execution, not evidence of an org's configuration.

The native engine is bundled at `bin/<platform>-<arch>/apx` (or `apx.exe`). No compiler, database server or org authentication is required. The current built release targets Windows x64; other targets must be built and qualified separately.

## Result policy

`check` exit 0 means the reported syntax/declaration checks completed. It does not imply type-system completeness, semantic parity or deployment readiness. `validate` and `test` execute the available preview subset; successful preview execution returns exit 4 deliberately. Unsupported operations also return 4, with separate report statuses and diagnostics. Assertion failures return 1, input errors 2, engine errors 3, cancellation 130.

With `--json`, Salesforce CLI returns its normal `{status,result,warnings}` envelope. `status`, native process exit, and `result.exitCode` agree. Treat the report's assurance and limitations as part of every result.

## Build from source

From the workspace root, run `npm ci`, `npm run build --workspace @apx/runner`, and `npm run build --workspace @apx/sf-plugin`. Then stage the native binary and production dependencies using the workspace packaging script. Linking the un-staged source package requires workspace dependencies to remain installed.

Commands use Salesforce's public `SfCommand` / oclif plugin interfaces. No third-party implementation is required by the native engine.
