# Apex Recipes feature and dependency matrix

Pinned revision: `b364267fc786a8a3ac4c3968467fcc4abd4adac6`. 142 Apex files, 322 independently inventoried test methods.

Independent lexical inventory; comments and strings excluded. Class references and their transitive closure are conservative file-level dependencies, not a runtime call graph or a claim that every feature is reached. Dynamic query strings are intentionally not parsed. Native corpus reports remain authoritative for observed first blockers.

## Test families

| Family | Tests | Test classes | Features directly present in test classes |
| --- | ---: | ---: | --- |
| Async Apex Recipes | 11 | 7 | async-jobs, http-rest-mocks, lists-iteration, maps-sets, soql, soql-aggregates |
| Collection Recipes | 7 | 2 | exceptions, http-rest-mocks, lists-iteration, maps-sets, sorting-comparators |
| Custom Metadata Recipes | 2 | 1 | lists-iteration, metadata-data, relationship-queries, soql |
| Data Recipes | 72 | 5 | async-jobs, dates-times, dml, enum-request-context, exceptions, lists-iteration, maps-sets, relationship-queries, security-user-mode, soql, soql-aggregates |
| DataWeaveInApex Recipes | 16 | 7 | dataweave, dml, exceptions, json-xml, lists-iteration, maps-sets, soql |
| Email Recipes | 3 | 1 | crypto-blob-encoding, email, files-content, lists-iteration, soql, soql-aggregates |
| Encryption Recipes | 10 | 1 | crypto-blob-encoding, exceptions |
| Files Recipes | 3 | 1 | files-content, lists-iteration, soql |
| Integration Recipes | 47 | 6 | crypto-blob-encoding, dml, enum-request-context, exceptions, http-rest-mocks, json-xml, lists-iteration, maps-sets, schema-reflection, security-user-mode, soql, stub-provider-connectapi |
| Invocable Recipes | 3 | 1 | lists-iteration |
| LDV Recipes | 1 | 1 | async-jobs, lists-iteration, soql |
| Platform Cache Recipes | 13 | 2 | cache-org-context, exceptions |
| Platform Event Recipes | 7 | 2 | events-triggers, exceptions, lists-iteration, schema-reflection, security-user-mode, soql |
| Quiddity Recipes | 3 | 2 | async-jobs, enum-request-context |
| Security Recipes | 41 | 3 | cache-org-context, dml, enum-request-context, exceptions, json-xml, lists-iteration, maps-sets, schema-reflection, security-user-mode, soql |
| Shared Code | 61 | 13 | async-jobs, cache-org-context, dml, enum-request-context, events-triggers, exceptions, http-rest-mocks, inheritance-interfaces, lists-iteration, maps-sets, metadata-data, schema-reflection, security-user-mode, soql, static-initializers |
| Testing Recipes | 11 | 4 | dml, exceptions, lists-iteration, maps-sets, metadata-data, relationship-queries, security-user-mode, soql |
| Trigger Recipes | 11 | 2 | dml, exceptions, lists-iteration, metadata-data, schema-reflection, soql |

## Feature reach beyond current first blockers

The final column is a conservative upper bound from class references. It deliberately includes helper methods and branches that a particular test may not execute. The method column excludes setup/static initialization and called helpers. Neither column replaces runtime coverage.

| Feature | Source files | Direct test methods | Tests whose dependency closure contains feature |
| --- | ---: | ---: | ---: |
| async-jobs | 17 | 10 | 23 |
| cache-org-context | 9 | 4 | 94 |
| crypto-blob-encoding | 8 | 24 | 209 |
| dataweave | 9 | 3 | 16 |
| dates-times | 4 | 1 | 37 |
| dml | 33 | 63 | 188 |
| email | 2 | 3 | 3 |
| enum-request-context | 30 | 20 | 271 |
| events-triggers | 17 | 9 | 98 |
| exceptions | 50 | 67 | 284 |
| files-content | 4 | 4 | 7 |
| http-rest-mocks | 15 | 54 | 59 |
| inheritance-interfaces | 42 | 0 | 275 |
| json-xml | 12 | 24 | 103 |
| lists-iteration | 95 | 152 | 279 |
| maps-sets | 44 | 56 | 225 |
| metadata-data | 14 | 9 | 23 |
| properties | 6 | 0 | 117 |
| relationship-queries | 8 | 2 | 45 |
| safe-navigation | 9 | 0 | 90 |
| schema-reflection | 14 | 11 | 159 |
| security-user-mode | 26 | 42 | 182 |
| soql | 53 | 83 | 235 |
| soql-aggregates | 7 | 9 | 89 |
| sorting-comparators | 5 | 1 | 5 |
| static-initializers | 3 | 0 | 95 |
| stub-provider-connectapi | 6 | 1 | 85 |
| type-reflection | 4 | 0 | 157 |

## Supplied metadata

| Metadata directory | Files |
| --- | ---: |
| applications | 1 |
| cachePartitions | 1 |
| cspTrustedSites | 1 |
| customMetadata | 11 |
| dw | 28 |
| flexipages | 1 |
| flows | 1 |
| labels | 1 |
| layouts | 9 |
| lwc | 34 |
| namedCredentials | 1 |
| objects | 45 |
| permissionsetgroups | 1 |
| permissionsets | 9 |
| prompts | 1 |
| staticresources | 86 |
| tabs | 2 |

The companion `corpus-feature-matrix.json` contains every source file, feature evidence line, dotted API call, class dependency, test identity, and metadata path. Regenerate using `node scripts/corpus-audit.mjs --repo <unchanged pinned clone>`. See `CORPUS-PLAN.md` for dependency sequencing and configuration requirements.
