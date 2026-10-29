# Axtik Local Apex Preview

Run local Apex syntax/declaration checks and experimental tests from a class or folder. The platform-specific VSIX includes its native engine, so you do not need Rust, Salesforce CLI, an org, or an account for local execution.

Axtik is the project name. This preview retains the APX command labels, `apx.*` settings and `apx-local.apx-local-apex` extension identifier. Source, documentation and release downloads are available from [SamCousinsGB/axtik](https://github.com/SamCousinsGB/axtik).

This is an early runtime implementation. It does **not** certify deployment compatibility or implement every Salesforce feature. Unknown execution paths are blocked. Successful local assertions are explicitly advisory until the supported behavior has been qualified against Salesforce.

Version 0.2.1 includes local metadata/data operations, explicit user and permission fixtures, supported asynchronous callbacks and events, HTTP mocks, JSON/crypto, DataWeave and Platform Cache in the bounded language evaluator. Each family has a supported subset; finding an API in this list does not imply every overload or org feature is implemented.

This version adds semantic checks of unexecuted source, corrected field/sharing/governor rules, actual assertion counts and separate setup counts. Zero-assertion successes are marked execution-only. Consumed file bytes and directory entries are verified before displaying results; resource changes and external folders invalidate old results. Input symlinks/junctions are unsupported. The bundled engine and adapters use report protocol 2 and must be upgraded together.

## Install

Download the package for your extension host from [GitHub Releases](https://github.com/SamCousinsGB/axtik/releases), then use **Extensions: Install from VSIX...** in VS Code. The first packaged release targets Windows x64. On Remote SSH, WSL or Containers the extension runs on the remote host, so it needs an engine built for that host. It does not run in vscode.dev. The preview is distributed through GitHub; it is not listed in the VS Code Marketplace.

## Validate a class or folder

1. Open and trust the Salesforce project in VS Code.
2. Right-click one or more `.cls` files, `.trigger` files or folders in Explorer.
3. Choose **APX: Validate Selected Classes or Folder**.
4. Read diagnostics in **Problems** and the detailed report in **Output > APX Local Apex**.

The Command Palette also provides **Validate Active Apex File**, **Choose Folder to Validate**, **Validate Workspace**, and **Check Syntax and Declarations**. The engine loads project dependencies. When selected source contains test methods, it runs that selection. Selecting production classes without tests conservatively runs the project's discovered tests and reports that expansion; it does not claim to know which tests are affected. The Test Explorer uses an exact test filter.

If relevant project files have unsaved changes, APX asks before saving them. Cancelling that prompt stops validation. Changes during a run cancel its result rather than applying diagnostics from stale source. Long runs can be cancelled from the progress notification.

## Test Explorer

Choose **APX: Discover Local Tests**, or expand **APX Local Tests (Preview)** in Testing. Run a test with **Run locally (experimental)**. A failed assertion is a failure. Unsupported execution is an error with a reason. Successful preview assertions are shown as **skipped/advisory**, with an explanation in test output, because VS Code has no advisory success state. They are never displayed as qualified green passes.

Syntax/declaration checks do not execute tests and do not validate all semantic behavior. Local execution reports its coverage limitations explicitly and returns exit code 4 for preview results.

## Salesforce CLI

Choose **APX: Show Salesforce CLI Setup** for the exact command to link the plugin bundled in the extension to an existing `sf` installation. Linking is a deliberate user action. APX never modifies your CLI installation automatically.

Once linked:

```text
sf apx check --source-dir force-app/main/default/classes
sf apx validate --source-dir force-app/main/default/classes
sf apx validate --project . --environment local-environment.json --json
```

The extension and plugin call the same native engine and consume the same versioned report. This is local validation, not `sf project deploy validate` and not an org deployment.

## Settings

- `apx.enginePath`: optional absolute user/machine path to a compatible APX executable. Empty uses the bundled engine. Project settings cannot supply a program to execute.
- `apx.environmentPath`: optional JSON profile for explicit local users, permissions, clock, and service fixtures. An absolute path or a path relative to the Salesforce project is accepted. No profile is inferred or applied automatically. Profile edits invalidate previous results.
- `apx.timeoutSeconds`: request deadline, 1–600 seconds, default 60.

For a project that needs local org fixtures, set `apx.environmentPath` in VS Code Settings before validating. Environment files contain explicit synthetic inputs, not credentials or a connection to Salesforce. The source repository includes a documented Apex Recipes example; it is never activated automatically. Changing a profile cancels an in-flight request and invalidates prior results.

The extension runs only in trusted workspaces, never sends source to a service, and does not collect telemetry. Output is bounded to prevent a malfunctioning engine from exhausting the extension host. It uses child-process arguments rather than interpolated shell commands.

## Build from source

From the repository root, install the locked npm dependencies, build the native engine, and stage it under `packages/vscode/bin/<platform>-<arch>/`. Then typecheck, test and bundle the extension. The repository packaging script stages the SF CLI plugin and produces the VSIX. Original source is Apache-2.0; dependency licenses remain with their components.
