# Local platform services

Axtik's `apx` engine executes the application's Apex against local state. Service behavior below is an implementation boundary, not a claim of complete Salesforce compatibility. The explicit environment profile supplies identities, permission policy, clock and any external-service fixtures. No source file or test name selects a canned result.

## Source metadata and static resources

`StaticResource` bodies come from the actual source payload paired with its metadata XML. A payload may be one file or an expanded resource folder. Expanded folders become a deterministic ZIP with stored entries, UTF-8 names and a fixed ZIP timestamp. This canonical local encoding preserves entry names and bytes; it does not claim byte-for-byte equality with an archive produced by Salesforce deployment tooling. Missing, ambiguous, symbolic-link or over-budget resources fail loading. The current limits are 5 MiB per resource, 50 MiB total and 1,000 entries per expanded folder.

Local `ApexClass.LengthWithoutComments` uses the parsed source with comment nodes removed and counts UTF-16 code units. Comment-like text inside string literals is retained. This is an explicit source-size model and still needs independent Salesforce qualification.

## Platform events

`EventBus.publish` validates the event schema and required fields, snapshots the payload and callback, and produces per-event publish results. `Test.getEventBus().deliver()` runs subscriber triggers and success callbacks; `fail()` runs failure callbacks without subscriber delivery. Callback UUIDs identify the actual queued events. `newSObject(null, true)` creates an event UUID; caller-invented UUIDs do not acquire platform provenance.

Subscriber batches and callbacks have separate transaction contexts and limits, using the explicitly configured Automated Process user. Completed data changes merge into the test's local store. An after-commit publication follows savepoint rollback; immediate publications remain queued. Rollback across already delivered work is blocked. Downstream publications require a subsequent delivery and cannot silently disappear at the completion gate.

Local tests cover changed payloads, both callback paths, actual subscriber DML, failed required-field validation, UUID forgery, savepoints, and publication from a future method. Production event infrastructure, Change Data Capture, full retry/resume semantics and cross-context search-fixture inheritance are outside this boundary.

## Salesforce Files

Inserting a local `ContentVersion` creates its `ContentDocument` and initial `ContentDocumentLink`. An additional version of an existing document updates the latest-version relationship and clears the earlier version's `IsLatest`. The runtime stores the supplied bytes and derives size, extension, version number and the supported file-type mappings from the input. Generated rows participate in transaction rollback, including an individual failure in partial-success DML.

`ContentDistribution` records refer to an existing local version. They require an explicit `contentDownloadBaseUrl`. The corpus profile uses `https://apx-files.invalid`: generated URLs are **opaque local references, not hosted download endpoints**. Creating one does not publish data or send a network request. PDF conversion and file delivery are not implemented.

The local first-publication link policy is `V` / `AllUsers`, recorded in the corpus environment's provenance. This does not infer the settings of another org. Additional-version first-publish locations, unknown extension mappings, and version/distribution updates block until their behavior is qualified.

## App Launcher and Chatter

App Menu changes require explicit local permission: `appMenuCanCustomize` applies to the configured current user, while `appMenuCustomizers` can list other permitted user IDs. `setOrgSortOrder` validates a complete permutation of actual local application IDs and changes stored `SortOrder` values atomically. Visibility changes update the corresponding local menu record. Partial permutations and transactions mixing this API with prior DML block. Local metadata IDs derive from source metadata; they are not org identifiers.

The implemented Chatter call accepts only an explicit fixture matching its arguments and a data-visible test context. Its result is currently an opaque `FollowingPage`; page contents and live Chatter behavior are not modeled. An absent fixture blocks instead of returning an invented empty service response.

## Regex compatibility

Pattern/Matcher and String regex operations share a bounded Java-compatible ASCII subset. Full matches anchor the expression during matching, so alternatives are reconsidered correctly. Matcher iteration tracks the previous match boundaries, including failed and zero-width matches. Default dots exclude both ASCII line terminators; dot-all mode is scoped by groups.

Line anchors with line-terminated input, unqualified Unicode input/classes, lookaround, backreferences and advanced flag syntax block. String split/replacement also block zero-width cases whose Java iteration semantics have not been implemented. Patterns, input, capture expansion and compiled automata have explicit resource bounds. Independent probe source and observed results from an official JDK are saved under `corpus/evidence/`; no Java implementation source is incorporated.

## Evidence and remaining qualification

The immutable upstream corpus, mutation checks, runtime regressions and actual adapter-host checks test different layers. Passing them does not establish general org parity. In particular, synthetic permission profiles, local cache accounting, opaque service responses and local delivery references remain explicit assumptions. Preview execution retains its advisory status and exit code until independent Salesforce qualification supports a stronger claim.
