# Contributing to Axtik

Use [GitHub issues](https://github.com/SamCousinsGB/axtik/issues) for reproducible bugs and scoped proposals, and pull requests for implementation changes. The executable and integration namespace remains `apx`; keep those interfaces stable unless a migration is part of the change.

Start with a minimal Apex fixture that demonstrates the behavior. Include its source API version, needed metadata, expected observation, and whether that expectation came from a Salesforce org, documentation, or a local hypothesis. Synthetic data only. You do not need Rust experience or Salesforce credentials to contribute a regression case.

Run `cargo test --locked`, `npm ci`, `npm run build`, and `npm test` before submitting a change. `npm run check --workspace apx-local-apex` checks extension types. Use the checked-in demo and negative controls when modifying result handling. Source locations and unsupported behavior are part of the contract.

Semantic changes need regression evidence. Performance changes need matched-input timing and unchanged results. A local assertion pass must remain marked experimental until an independent conformance corpus qualifies its complete behavior.

Sign commits with `git commit -s` to certify the Developer Certificate of Origin 1.1, available at https://developercertificate.org/. Identify reused code and preserve its license and notices. Original contributions use Apache-2.0.

Do not add account requirements, automatic uploads, telemetry, or network fallback to local checks. No publication credentials belong in this repository.
