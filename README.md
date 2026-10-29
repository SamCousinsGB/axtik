# Axtik

Run Apex tests offline from VS Code or the Salesforce CLI.

## VS Code

1. [Download the universal VSIX](https://github.com/SamCousinsGB/axtik/releases/tag/v0.2.3) for Windows, macOS or Linux (x64 and ARM64).
2. Install it using **Extensions: Install from VSIX...**.
3. Open your Salesforce project, right-click a class or folder, and choose **APX: Validate Selected Classes or Folder**.

## CLI

Run **APX: Show Salesforce CLI Setup** in VS Code and follow the plugin-linking instructions. Then:

```sh
sf apx validate --project . --source-dir force-app
```

**Developer preview:** supports a subset of Apex. Local passes are advisory; successful test runs return exit code **4**.

[CLI guide](packages/sf-plugin/README.md) | [Supported features](docs/IMPLEMENTATION.md) | [Test evidence](docs/CORPUS.md) | [Contributing](CONTRIBUTING.md) | [Apache-2.0](LICENSE)
