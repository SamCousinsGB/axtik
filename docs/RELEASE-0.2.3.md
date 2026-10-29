# Axtik 0.2.3

This correctness release closes an observed false-pass path: an invented Date method in an unexecuted branch now blocks validation before any test runs. Date, Datetime, Time, EncodingUtil, Limits, Math and Describe methods have explicit receiver, argument and return contracts. Known invalid calls fail checks; unimplemented or ambiguous contracts block.

Reports expose per-test local model provenance: reached operations, setup/test phases, outcomes and selected synthetic assumptions. The inventory is bounded and incomplete. All contracts remain unqualified; no local fixture can declare itself independently verified. VS Code and SF CLI show this information and validate its structure.

Mutation evidence schema 2 binds the selected test and mutant source bytes to an actual failed assertion counter and matching APX_ASSERTION diagnostic. An unrelated error containing the word “assertion” cannot pass. The reviewed baseline permits only the existing known survivor, sentinel-only tests and three assertion-free tests. New weaknesses block release. Original upstream source and historical evidence remain unchanged. Mutations still invert assertions per method; this does not claim individual assertion-site or branch coverage.

The universal VSIX continues to bundle Windows, macOS and Linux engines for x64 and ARM64. The installed-host suite now includes the invalid dead-code regression and checks actual provenance from a configured clock fixture. Release artifacts require all 21 checks on each native host, plus the pinned corpus and strengthened negative-control evidence.

No live Salesforce comparison was performed. Compiler coverage outside the documented native families remains bounded, and local execution still returns advisory exit 4. Complete type checking, broader independent fixtures, and versioned Salesforce conformance evidence remain future qualification work.
