# Local Platform Cache model

The evaluator reads `PlatformCachePartition` XML through the metadata catalogue.
Partition names and org/session allocations come from those records. It never
promotes a partition to the default: unqualified static keys require exactly one
declared default. Fully qualified static keys and partition handles identify the
same entries. Zero allocated capacity yields misses.

Entries are isolated per test Context. Org entries are shared by users within
that Context. Session entries additionally require an explicit
`capabilities.platformCacheSessionId` and current user, and are scoped by both.
This models a declared local session; it does not create or authenticate a real
Salesforce session. Unknown cross-namespace visibility remains unsupported.

Implemented operations are `getPartition`, `getName`, `put`, `get`, `contains`,
`remove`, `getKeys`, `getNumKeys`, and `getCapacity`, including builder get/remove
and partition get/contains for sets of keys. Builder classes must implement
`Cache.CacheBuilder`. A miss instantiates the actual Apex class and calls its
`doLoad` method; errors propagate and no result is substituted. Builder entries
use the class-qualified `_B_` key. Removal invalidates that entry, so the next get
executes the builder again. Null builder results are returned without caching.

Stored and retrieved values are copied recursively, preserving record projection
and collection type contracts. Mutating an input or fetched object does not
mutate the cached value. Cyclic graphs, unsupported native instances, transient
field serialization, and unsupported overloads block evaluation.

Expiration uses the runtime clock. Accepted TTLs are 300–172800 seconds for org
cache and 300–28800 for session cache; defaults are 86400 and 28800 respectively.
Boundary behavior outside those ranges remains blocked pending qualification.
Clock advancement is controlled by the runtime fixture; no wall-clock sleep is
needed to test expiry.

Capacity-sensitive operations require the explicit synthetic policy
`capabilities.platformCacheCapacityModel: "apx-value-v1"`. This local model counts
UTF-8 strings/blob bytes with fixed container/scalar overhead and uses least
recently used eviction. It is **not Salesforce serialization byte accounting**.
Without the policy, non-empty capacity reporting and capacity-driven eviction
block. The host additionally limits individual values to 1 MiB, graph depth to 64,
and stored entries to 10000. These safeguards are not Salesforce governor limits.

The regression suite covers metadata/default selection, no-allocation misses,
deep-copy independence, exact expiry, user/session/test isolation, actual builder
execution and invalidation, malformed and cyclic values, and local LRU behavior.
It does not qualify distributed persistence, cross-transaction sharing, platform
eviction timing, Salesforce cache metrics, or exact platform byte sizing.

Reference semantics: [Salesforce Platform Cache tutorial](https://trailhead.salesforce.com/content/learn/modules/platform_cache/platform_cache_use)
and the [Apex reference guide](https://resources.docs.salesforce.com/latest/latest/en-us/sfdc/pdf/salesforce_apex_reference_guide.pdf).
