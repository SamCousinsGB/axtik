# Install Axtik

1. Download `apx-local-apex-0.2.3-universal.vsix` from [GitHub Releases](https://github.com/SamCousinsGB/axtik/releases/tag/v0.2.3).
2. In VS Code, run **Extensions: Install from VSIX...** and select it.
3. Open and trust your Salesforce project. Right-click a class or folder and choose **APX: Validate Selected Classes or Folder**.

The same file supports Windows, macOS and Linux on x64 and ARM64. It bundles every native engine and chooses the extension host's architecture automatically. No Rust compiler, Node installation, Salesforce CLI, org login or first-run download is required. Use a current VS Code desktop release on an operating system it supports.

For WSL, Remote SSH or Dev Containers, install the VSIX in the remote VS Code window. The engine runs beside the project. Linux engines are statically linked, avoiding a separate libc/runtime install. Browser-only VS Code and 32-bit extension hosts are unsupported.

Results appear in **Problems**, **Output > APX Local Apex**, and **APX Local Tests (Preview)**. Successful local tests are advisory; they do not certify Salesforce compatibility.

## Troubleshooting

- **Custom engine path:** clear `apx.enginePath` in Settings to return to the bundled engine.
- **Missing or damaged engine:** reinstall the complete VSIX. Do not copy individual files out of the extension.
- **Execution denied:** install on a filesystem that permits executables. Axtik can repair a lost owner-execute bit only after checking the bundled binary's hash.
- **Restricted Mode:** trust the workspace before running local validation.

Optional SF CLI integration is available through **APX: Show Salesforce CLI Setup**. It is not needed for the extension.

## Verify a download

Compare its SHA-256 with the release's `SHA256SUMS.txt`. CI-built releases also provide GitHub provenance, verifiable with:

```sh
gh attestation verify apx-local-apex-0.2.3-universal.vsix --repo SamCousinsGB/axtik
```

`release-evidence.json` records each target's executable hash, unchanged corpus results and real installed-extension checks. Build provenance identifies the source and workflow; it does not establish Salesforce semantic parity.
