# Salesforce comparison harness

The harness compares specific native observations with Salesforce Metadata API
validation results. Salesforce parity is **unverified until an authorized live
run produces complete, matching observations**. Local fixtures, synthetic
environments, parser tests, and a successful harness plan do not establish parity.

The first manifest, `corpus/salesforce-comparison/manifest.json`, contains ten
independent API 68.0 projects: invalid code in an unexecuted branch and its positive
control; Account.Name length 256/255; DML rows 10001/10000; query rows 50001/50000;
and PublicReadOnly non-owner/owner updates. Negative governor probes require an
uncaught `System.LimitException` with the relevant limit in its message. An
arbitrary assertion failure cannot satisfy them.

## Offline use

Build the shared report runner first with `npm run build --workspace @apx/runner`.
Use an existing native binary; the harness never downloads or builds one.

```sh
node scripts/salesforce-compare.mjs plan
node scripts/salesforce-compare.mjs local --engine /absolute/path/to/apx
```

Both modes avoid all Salesforce CLI calls and return exit **4** because Salesforce
is unverified. `plan` inventories and hashes the fixtures and records exact planned
arguments. `local` copies each fixture into its evidence directory, runs native
compilation and the exact named test, and records whether the local observation
satisfies its expectation. A local mismatch remains visible as
`localExpectation: "violated"`; it is never presented as a Salesforce match.

Use `--scenario field-negative` to select a case; repeat that option to select
several. Use `--manifest path/to/manifest.json` for a reviewed manifest and
`--output /path/to/evidence` for a different evidence location. Output must be
outside the source fixture projects. Default output is the ignored
`.tmp/salesforce-comparison/` directory.

## Deliberate live comparison

Live-org verification has not been performed for this release. When a disposable
org is available and the operator authorizes the submission, a command has this
form:

```sh
node scripts/salesforce-compare.mjs run \
  --engine /absolute/path/to/apx \
  --sf-entry /absolute/path/to/@salesforce/cli/bin/run.js \
  --target-org disposable-alias \
  --org-id 00D_REPLACE_WITH_ACTUAL_ORG_ID \
  --allow-deploy \
  --scenario field-negative \
  --scenario field-positive
```

On Windows, use an absolute `apx.exe` path and the installed SF CLI's JavaScript
`bin/run.js` entrypoint. The harness invokes it with the current Node executable,
without a command shell. `--sf-executable` can instead name a native executable;
`.cmd`, `.bat`, and `.ps1` shell launchers are rejected. The SF CLI must already be
authenticated using its standard local credential store. Default org settings and
environment access tokens are not used as target selection.

`--allow-deploy` authorizes uploading the reviewed fixtures for **check-only**
validation. Every submission uses `sf project deploy start --dry-run`, an explicit
target, `--source-dir force-app`, and the manifest's pinned `--api-version`.
Runtime cases use `RunSpecifiedTests` and one exact test class; compile-only
cases use `NoTestRun`. No normal deployment, quick deployment, org creation,
permission assignment, or cleanup mutation is implemented. Successful validation
does not persist these metadata components. This is the sandbox validation flow
recommended in the [Salesforce CLI deployment documentation](https://github.com/salesforcecli/plugin-deploy-retrieve#sf-project-deploy-start).

Before submitting anything, the harness queries `Organization` and requires its
live ID to match `--org-id`, with `IsSandbox=true`. A null `TrialExpirationDate`
identifies a sandbox; a valid future expiration identifies a scratch org. This
follows the [Salesforce Core org classification](https://github.com/forcedotcom/sfdx-core/blob/main/src/org/org.ts).
The selected alias is resolved once to a canonical username, which is used for
all identity queries and submissions. Only its hash is retained in the receipt;
the raw `org display` response is discarded. Identity is queried again before each scenario. Production, ambiguous, expired,
inaccessible, or mismatched identities stop the run with unverified evidence.
The operator must still choose an org that is disposable: the sandbox flag alone
does not mean an org is unused.

`--timeout-ms` bounds each native or SF process (default 120000; maximum 3600000).
`--wait-minutes` controls the SF polling interval limit (default 10; maximum 60).
A local timeout does not establish a Salesforce test outcome and does not
guarantee that Salesforce has stopped a remote check-only job. The harness does
not retry or turn an incomplete response into success.

## Controlled sharing cases

The two sharing projects carry
`requires: ["controlled-sharing-environment"]`. Their local environments are
synthetic. The live harness deliberately skips submitting these cases until
executable checks can establish their org prerequisites. A hand-written receipt
or assertion of correct configuration cannot promote them to verified.

The required setup is Account OWD PublicReadOnly; two distinct active users with
aliases `axtikact` and `axtikown`; Account read/create/edit/delete permission for
the actor; no View All, Modify All, View All Data, or Modify All Data bypass; and
no role hierarchy, team, or share granting the actor edit access to the owner's
record. The fixtures use explicit user/system operation modes. Implementing and
testing a verifier for that complete setup is remaining work.

## Evidence and verdicts

Each invocation creates a fresh timestamp/UUID directory. File writes use
exclusive creation, so the harness never overwrites an earlier receipt. It saves:

- `receipt.json` and the SHA-256 of its exact bytes in `receipt.sha256`;
- the selected manifest hash, pinned API, fixture file/directory identities;
- isolated source snapshots for executed cases;
- the native binary hash, and for live runs the SF CLI entrypoint hash/version,
  Node version, and verified org ID/type;
- normalized compile diagnostics, exact test identities, terminal outcomes,
  observed native assertion counts, and comparison verdicts.

The receipts are append-only by harness policy and tamper-evident through hashes.
They are not signed or stored on write-once infrastructure. Preserve them in
controlled CI artifact storage when they are used as release evidence.

Raw CLI output, authentication records, access tokens, refresh tokens, and org
display responses are not written to receipts. The harness records an allowlist
of outcome fields, redacts common credential patterns in diagnostic text, strips
unrelated credential environment variables from subprocesses, and bounds combined
stdout/stderr to 16 MiB. Keep evidence private if future fixtures contain private
source or diagnostic content.

A `match` requires both implementations to satisfy the manifest's compile result,
exact test identity, expected pass/fail result, and expected negative diagnostic
fragments and category. Governor results require the actual leading
`System.LimitException` type; native results additionally require the matching
`APX_GOVERNOR` diagnostic. A quoted exception name inside an assertion or DML error
does not qualify. Native blocked/errored/missing results, positive tests with zero observed
assertions, Salesforce auth/transport/configuration errors, pending jobs, missing
tests, extra tests, contradictory counts, and non-Apex deployment failures are
`unverified`. Expected compilation failures require actual Apex compiler
diagnostics bound to the expected captured source and line. Native check and test
manifests must match each other and the original captured bytes and directory
inventories; a transient source change cannot silently mix the observations.
Snapshot reads are streamed within a 32 MiB/10000-entry bound, with cancellation
and a depth limit. A completed fixture match remains scoped
evidence for those inputs, API version, binary, and org; it does not certify the
runtime generally.

Exit codes are **0** for all selected cases matched, **1** for a verified
mismatch, **4** when verification is incomplete, and **2** for invalid arguments
or failure before an evidence run can start. Mixed runs retain each case's verdict.
No result from `plan` or `local` is a parity pass.

## Tests

```sh
node --test scripts/salesforce-compare.test.mjs
```

The offline contract tests cover explicit authorization, sandbox/scratch identity,
pinned API arguments, terminal-result accounting, negative failure matching,
credential omission, immutable receipt writes, timeout/output/cancellation bounds,
and a real no-process fixture plan. With `APX_ENGINE_PATH` set to the current
binary, an additional integration test executes all ten real native fixtures and
requires the intended local classifications while preserving unverified Salesforce
status. Synthetic SF JSON in contract tests is labeled as test data and is not
live Salesforce evidence.
