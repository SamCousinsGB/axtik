# Axtik 0.2.2

One VSIX for Windows, macOS and Linux, on x64 and ARM64. Download it from [GitHub Releases](https://github.com/SamCousinsGB/axtik/releases/tag/v0.2.2) and use **Extensions: Install from VSIX...**. [Installation help](INSTALLATION.md).

The extension bundles all six native engines and selects the actual extension host automatically. Linux engines use static musl builds; Windows engines use a static C runtime. Packaging rejects unsupported external native dependencies. No engine download or developer toolchain is required on the user's machine.

The portable release workflow builds and tests on six native runners, runs the unchanged Apex Recipes corpus and its negative controls on each, packages one universal VSIX, and installs that exact archive into a real VS Code host on every target. The release finalizer requires matching binary hashes, all six corpus receipts, and all twenty installed-host checks per target. It rejects incomplete coverage and publishes source, checksums, evidence and build provenance.

The Apex runtime retains the 0.2.1 preview boundary. Passing results are advisory, unsupported features remain explicit, and no live Salesforce verification is performed. Three corpus tests execute no assertions; the known assertion-inversion survivor remains visible in each corpus audit.

Distribution is through GitHub, with no Marketplace publisher account required. Existing APX command and package identifiers remain compatible.
