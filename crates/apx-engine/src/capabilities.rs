//! Observations of reached local dispatch, never independent conformance evidence.
use serde::Serialize;
use std::{cell::RefCell, collections::BTreeMap};

pub const CATALOGUE_VERSION: &str = "apx-capabilities-v1";
pub const NATIVE_METHOD: &str = "apx.native-method.v1";
pub const NATIVE_CONSTRUCTOR: &str = "apx.native-constructor.v1";
pub const SOQL: &str = "apx.soql.v1";
pub const SOSL: &str = "apx.sosl.v1";
pub const DML: &str = "apx.dml.v1";
const MAX_OPERATIONS: usize = 256;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Contract {
    pub id: &'static str,
    pub implementation: &'static str,
    pub basis: &'static str,
    pub conformance: &'static str,
    pub independent_evidence: Vec<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct CompilerCoverage {
    pub coverage: &'static str,
    pub conformance: &'static str,
}
#[derive(Clone, Debug, Serialize)]
pub struct InventoryCoverage {
    pub scope: &'static str,
    pub complete: bool,
    pub limitations: Vec<&'static str>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Provenance {
    pub schema_version: u32,
    pub catalogue_version: &'static str,
    pub conformance: &'static str,
    pub compiler: CompilerCoverage,
    pub inventory: InventoryCoverage,
    pub contracts: Vec<Contract>,
}
impl Default for Provenance {
    fn default() -> Self {
        Self {
            schema_version: 1,
            catalogue_version: CATALOGUE_VERSION,
            conformance: "unqualified",
            compiler: CompilerCoverage { coverage: "bounded-semantic-subset", conformance: "unqualified" },
            inventory: InventoryCoverage {
                scope: "reached-native-dispatch", complete: false,
                limitations: vec![
                    "Observed method dispatch, native construction attempts and query/DML expression routes only; not all language operators, fields, properties, coercions or internal platform helpers.",
                    "Operations are recorded only when reached on the evaluator thread; unexecuted branches and compiler-rejected calls are not runtime observations.",
                    "Returned or failed local operations do not establish Salesforce equivalence. No independent reference evidence is registered.",
                    "A constructor attempt includes argument evaluation. Its failure does not prove the constructor implementation was reached.",
                    "APX_COMPILE_UNSUPPORTED identifies a compiler coverage gap, not proof that Salesforce rejects the source.",
                    "Synthetic tags identify selected known model assumptions; their absence does not imply platform-backed behavior or a complete assumption inventory.",
                    "At most 256 distinct operation/phase pairs are retained; truncated inventories remain explicitly incomplete.",
                ],
            },
            contracts: [NATIVE_METHOD, NATIVE_CONSTRUCTOR, SOQL, SOSL, DML].into_iter().map(|id| Contract {
                id, implementation: "partial-local-model", basis: "local-implementation",
                conformance: "unqualified", independent_evidence: vec![],
            }).collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub contract_id: &'static str,
    pub operation: String,
    pub phase: &'static str,
    pub returned: u64,
    pub failed: u64,
    pub blocked: u64,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Assumption {
    pub id: &'static str,
    pub kind: &'static str,
    pub scope: &'static str,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestProvenance {
    pub schema_version: u32,
    pub conformance: &'static str,
    pub inventory_complete: bool,
    pub truncated: bool,
    pub operations: Vec<Operation>,
    pub assumptions: Vec<Assumption>,
}
impl Default for TestProvenance {
    fn default() -> Self {
        Self {
            schema_version: 1,
            conformance: "unqualified",
            inventory_complete: false,
            truncated: false,
            operations: vec![],
            assumptions: vec![],
        }
    }
}
struct Trace {
    phase: &'static str,
    explicit_environment: bool,
    operations: BTreeMap<(&'static str, String, &'static str), Operation>,
    report: TestProvenance,
}
thread_local! { static ACTIVE: RefCell<Option<Trace>> = const { RefCell::new(None) }; }

/// A test owns its session, including setup and same-thread async/event frames.
/// No environment JSON or runtime success can add independent evidence.
pub(crate) struct Session {
    previous: Option<Trace>,
    finished: bool,
}
impl Session {
    pub(crate) fn begin(explicit_environment: bool) -> Self {
        let mut report = TestProvenance::default();
        if explicit_environment {
            report.assumptions.push(Assumption {
                id: "explicit-environment-fixture",
                kind: "synthetic",
                scope: "configured-for-test",
            });
        }
        let previous = ACTIVE.with(|active| {
            active.replace(Some(Trace {
                phase: "setup",
                explicit_environment,
                operations: BTreeMap::new(),
                report,
            }))
        });
        Self {
            previous,
            finished: false,
        }
    }
    pub(crate) fn test_phase(&self) {
        ACTIVE.with(|active| {
            if let Some(trace) = active.borrow_mut().as_mut() {
                trace.phase = "test";
            }
        });
    }
    pub(crate) fn finish(mut self) -> TestProvenance {
        let trace = ACTIVE
            .with(|active| active.replace(self.previous.take()))
            .expect("active capability session");
        self.finished = true;
        let mut report = trace.report;
        report.operations = trace.operations.into_values().collect();
        report.assumptions.sort();
        report.assumptions.dedup();
        report
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if !self.finished {
            ACTIVE.with(|active| {
                active.replace(self.previous.take());
            });
        }
    }
}

pub(crate) fn observe(contract: &'static str, operation: &str, outcome: &str) {
    ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        let Some(trace) = active.as_mut() else {
            return;
        };
        // Type syntax may span lines (for example List<\nInteger>). Formatting
        // is not part of the operation identity or safe display text.
        let operation: String = operation.chars().filter(|c| !c.is_whitespace()).collect();
        if operation.is_empty() || operation.len() > 256 || operation.chars().any(char::is_control)
        {
            trace.report.truncated = true;
            return;
        }
        let operation = operation.to_ascii_lowercase();
        let key = (contract, operation.clone(), trace.phase);
        if !trace.operations.contains_key(&key) && trace.operations.len() >= MAX_OPERATIONS {
            trace.report.truncated = true;
            return;
        }
        let entry = trace.operations.entry(key).or_insert(Operation {
            contract_id: contract,
            operation: operation.clone(),
            phase: trace.phase,
            returned: 0,
            failed: 0,
            blocked: 0,
        });
        let counter = match outcome {
            "returned" => &mut entry.returned,
            "blocked" => &mut entry.blocked,
            _ => &mut entry.failed,
        };
        if *counter == MAX_SAFE_INTEGER {
            trace.report.truncated = true;
        } else {
            *counter += 1;
        }
        let mut assumption = |id, kind| {
            let value = Assumption {
                id,
                kind,
                scope: "reached-operation",
            };
            if !trace.report.assumptions.contains(&value) {
                trace.report.assumptions.push(value);
            }
        };
        if matches!(contract, SOQL | SOSL | DML) || operation.starts_with("database.") {
            assumption("local-record-store", "synthetic");
        }
        if operation == "date.today" || operation == "datetime.now" || operation == "system.now" {
            assumption(
                if trace.explicit_environment {
                    "fixture-clock"
                } else {
                    "host-clock"
                },
                if trace.explicit_environment {
                    "synthetic"
                } else {
                    "host-derived"
                },
            );
        }
        if operation.starts_with("test.") {
            assumption("local-test-lifecycle", "synthetic");
        }
        if operation.starts_with("limits.") {
            assumption("local-limit-model", "synthetic");
        }
        if operation.starts_with("userinfo.")
            || operation.starts_with("cache.")
            || operation.starts_with("connectapi.")
        {
            assumption("local-platform-context", "synthetic");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_observations_never_promote_conformance() {
        assert!(Provenance::default()
            .inventory
            .limitations
            .iter()
            .all(|text| text.len() <= 256));
        let session = Session::begin(false);
        session.test_phase();
        for index in 0..300 {
            observe(NATIVE_METHOD, &format!("string.method{index}"), "returned");
        }
        let report = session.finish();
        assert!(report.truncated);
        assert!(!report.inventory_complete);
        assert_eq!(report.operations.len(), 256);
        assert_eq!(report.conformance, "unqualified");
        assert!(Provenance::default()
            .contracts
            .iter()
            .all(|c| c.independent_evidence.is_empty() && c.conformance == "unqualified"));
        let empty = Session::begin(false).finish();
        assert!(empty.operations.is_empty());
    }

    #[test]
    fn labels_canonicalize_formatting_before_bounding_and_reject_other_controls() {
        let session = Session::begin(false);
        observe(NATIVE_CONSTRUCTOR, "List<\n\t Integer >.new", "returned");
        observe(NATIVE_CONSTRUCTOR, "list<integer>.new", "returned");
        observe(
            NATIVE_METHOD,
            &format!("{}String.length", " ".repeat(300)),
            "returned",
        );
        observe(NATIVE_METHOD, "String.\u{0000}length", "returned");
        observe(NATIVE_METHOD, "String.\u{007f}length", "returned");
        let report = session.finish();
        assert!(report.truncated);
        assert_eq!(report.operations.len(), 2);
        assert!(report
            .operations
            .iter()
            .any(|o| o.operation == "list<integer>.new" && o.returned == 2));
        assert!(report
            .operations
            .iter()
            .any(|o| o.operation == "string.length" && o.returned == 1));
        assert!(report
            .operations
            .iter()
            .all(|o| !o.operation.chars().any(char::is_control) && o.operation.len() <= 256));
    }
}
