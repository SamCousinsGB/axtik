# Asynchronous code in the local test runtime

This implementation executes submitted Apex. It does not return successful job
identifiers for work it has skipped. Submissions create system-owned
`AsyncApexJob` rows, retain the actual class/method/arguments, and run at
`Test.stopTest()` with fresh governor counters and the appropriate request
quiddity. Successful data changes commit to the test transaction; failed job
data changes are rolled back and the job failure is reported.

The synchronous **test drain** shares test static values. This is a deliberate
distinction from production asynchronous transactions, which are not implemented
by this engine. Source arguments and queued object instances are copied when
submitted. Each job has its own request identity and execution limits.

## Evidence for the test behavior

The pinned official Apex Recipes revision is
`b364267fc786a8a3ac4c3968467fcc4abd4adac6`. Its release PR skipped CI. The preceding
revision `837a99da089741c3cf40a24d23def9b093b825e1` has a successful
[scratch-org-test job](https://github.com/trailheadapps/apex-recipes/actions/runs/38726516062/job/116393749511),
including a successful `Run Apex tests` step. The workflow runs
`sf apex test run -c -r human -d ./tests/apex -w 20` in a scratch org after source
deployment. The [commit comparison](https://github.com/trailheadapps/apex-recipes/compare/837a99da089741c3cf40a24d23def9b093b825e1...b364267fc786a8a3ac4c3968467fcc4abd4adac6)
changes only `README.md` and `sfdx-project.json`; the Apex implementations and
assertions are unchanged.

Those assertions observe a future method changing a static circuit breaker,
batch `finish` changing a static result, and queueable execution reading a static
switch set before submission. This successful official run supports those tested
interactions. It is not independent qualification of every static/transaction
edge case. Preserved API metadata is in
`corpus/evidence/async-static-test-drain.json`; no test identity is consulted by
the runtime.

Salesforce's [test event delivery documentation](https://developer.salesforce.com/docs/platform/platform-events/guide/platform-events-test-deliver.html)
also establishes actual event delivery at `stopTest` or explicit event-bus
delivery, rather than merely acknowledging publication.

## Boundaries

Queueable, future, scheduled and batch callbacks execute their actual user code.
Batch scope is bounded to one execution within a test; unsupported nested async
chaining blocks. Scheduling uses a bounded numeric Quartz grammar; unknown
modifiers and unsupported timezone semantics block. Forced test execution does
not advance the production cron schedule. The engine does not run a background
worker, connect to an org, or certify production scheduler behavior.

This remains an experimental local model. Extending it requires mutation tests
and source-independent regression cases, not changes to expected test outcomes.
