# Axtik Local Apex

Run Apex tests offline from a class or folder. The universal extension includes native engines for Windows, macOS and Linux, on x64 and ARM64.

No Rust, Node.js installation, Salesforce CLI or org login is required.

## Install and run

1. [Download the universal VSIX](https://github.com/SamCousinsGB/axtik/releases).
2. In VS Code, choose **Extensions: Install from VSIX...**.
3. Open and trust your Salesforce project.
4. Right-click a class or folder and choose **APX: Validate Selected Classes or Folder**.

Read results in **Problems**, **Output > APX Local Apex**, or **APX Local Tests (Preview)** in Testing. The engine is selected automatically; leave `apx.enginePath` empty.

For WSL, Remote SSH or Dev Containers, connect first and install the VSIX in that remote window. Axtik runs on the machine containing the workspace. Browser-only workspaces without a native extension host are unsupported.

## Preview

Axtik supports a subset of Apex. Local passes are advisory and appear as skipped in Test Explorer; failures remain failures. Execution does not certify Salesforce compatibility. Tests that reach no assertions are marked execution-only.

Selecting production source runs the discovered project tests. Selecting test classes runs those classes. Unsupported behaviour blocks with an explanation. Edited files invalidate old results.

## Optional settings

- `apx.environmentPath`: explicit local fixture data and permissions, relative to the project or absolute. No profile is applied automatically.
- `apx.timeoutSeconds`: request deadline, default 60 seconds.
- `apx.enginePath`: advanced engine override. Empty uses the included engine.

For an existing Salesforce CLI installation, run **APX: Show Salesforce CLI Setup** and follow its linking instructions.

[Source and contribution guide](https://github.com/SamCousinsGB/axtik) | [Supported features](https://github.com/SamCousinsGB/axtik/blob/main/docs/IMPLEMENTATION.md) | [Test evidence](https://github.com/SamCousinsGB/axtik/blob/main/docs/CORPUS.md)
