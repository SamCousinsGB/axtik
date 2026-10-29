use apx_engine::{run, Options};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "apx-capabilities-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn write(&self, name: &str, source: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, source).unwrap();
        path
    }
    fn run(&self, command: &str, environment: Option<PathBuf>) -> apx_engine::model::Report {
        run(&Options {
            command: command.into(),
            project: Some(self.0.to_string_lossy().into()),
            environment: environment.map(|p| p.to_string_lossy().into()),
            ..Default::default()
        })
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn each_test_records_reached_dispatch_and_separates_setup_without_dead_branches() {
    let f = Fixture::new();
    f.write("Probe.cls", "@IsTest class Probe { @TestSetup static void setup(){ Date d=Date.newInstance(2020,1,1); } @IsTest static void strings(){ System.assertEquals(3,'abc'.length()); if(false){Date.today();} } @IsTest static void json(){ System.assertEquals('3',JSON.serialize(3)); } }");
    let report = f.run("validate", None);
    assert_eq!(report.status, "preview_passed", "{:?}", report.diagnostics);
    assert_eq!(report.exit_code, 4);
    let strings = &report
        .tests
        .iter()
        .find(|t| t.id.ends_with(".strings"))
        .unwrap()
        .provenance;
    let json = &report
        .tests
        .iter()
        .find(|t| t.id.ends_with(".json"))
        .unwrap()
        .provenance;
    assert!(strings
        .operations
        .iter()
        .any(|o| o.operation == "string.length" && o.phase == "test" && o.returned == 1));
    assert!(!json
        .operations
        .iter()
        .any(|o| o.operation == "string.length"));
    assert!(json
        .operations
        .iter()
        .any(|o| o.operation == "json.serialize" && o.returned == 1));
    for trace in [strings, json] {
        assert!(trace
            .operations
            .iter()
            .any(|o| o.operation == "date.newinstance" && o.phase == "setup"));
        assert!(!trace.operations.iter().any(|o| o.operation == "date.today"));
        assert!(!trace.inventory_complete);
        assert_eq!(trace.conformance, "unqualified");
    }
    let discovery = f.run("check", None);
    assert!(discovery
        .tests
        .iter()
        .all(|t| t.provenance.operations.is_empty() && t.provenance.assumptions.is_empty()));
}

#[test]
fn fresh_async_contexts_contribute_to_the_calling_test_inventory() {
    let f = Fixture::new();
    f.write("Worker.cls", "public class Worker { @future public static void later(){ System.assertEquals(3,'abc'.length()); } }");
    f.write("Probe.cls", "@IsTest class Probe { @IsTest static void asynchronous(){ Test.startTest(); Worker.later(); Test.stopTest(); } }");
    let report = f.run("validate", None);
    assert_eq!(report.status, "preview_passed", "{:?}", report.diagnostics);
    let trace = &report.tests[0].provenance;
    assert!(trace
        .operations
        .iter()
        .any(|o| o.operation == "string.length" && o.phase == "test" && o.returned == 1));
    assert!(trace
        .assumptions
        .iter()
        .any(|a| a.id == "local-test-lifecycle" && a.kind == "synthetic"));
}

#[test]
fn blocked_unknown_dispatch_is_an_observation_not_implemented_conformance() {
    let f = Fixture::new();
    f.write(
        "Probe.cls",
        "@IsTest class Probe { @IsTest static void unknown(){ JSON.noSuchOperation(); } }",
    );
    let report = f.run("validate", None);
    assert_eq!(report.status, "blocked", "{:?}", report.diagnostics);
    assert_eq!(report.exit_code, 4);
    let trace = &report.tests[0].provenance;
    assert!(trace
        .operations
        .iter()
        .any(|o| o.operation == "json.nosuchoperation" && o.blocked == 1 && o.returned == 0));
    assert_eq!(trace.conformance, "unqualified");
    assert!(report
        .provenance
        .contracts
        .iter()
        .all(|c| c.independent_evidence.is_empty()));
}

#[test]
fn native_construction_and_data_routes_record_synthetic_store_usage() {
    let f = Fixture::new();
    f.write("Probe.cls", "@IsTest class Probe { @IsTest static void data(){ Account a=new Account(Name='Local'); insert a; List<Account> rows=[SELECT Id FROM Account]; System.assertEquals(1,rows.size()); } }");
    let report = f.run("validate", None);
    assert_eq!(report.status, "preview_passed", "{:?}", report.diagnostics);
    let trace = &report.tests[0].provenance;
    for (contract, operation) in [
        ("apx.native-constructor.v1", "account.new"),
        ("apx.dml.v1", "statement.insert"),
        ("apx.soql.v1", "query.soql"),
    ] {
        assert!(
            trace
                .operations
                .iter()
                .any(|o| o.contract_id == contract && o.operation == operation && o.returned == 1),
            "missing {operation}: {:?}",
            trace.operations
        );
    }
    assert!(trace
        .assumptions
        .iter()
        .any(|a| a.id == "local-record-store" && a.kind == "synthetic"));
}

#[test]
fn multiline_generic_constructor_has_a_canonical_safe_operation_label() {
    let f = Fixture::new();
    f.write("Probe.cls", "@IsTest class Probe { @IsTest static void multiline(){ Object values=new List<\n\tInteger>(); System.assert(true); } }");
    let report = f.run("validate", None);
    assert_eq!(report.status, "preview_passed", "{:?}", report.diagnostics);
    assert_eq!(report.exit_code, 4);
    let trace = &report.tests[0].provenance;
    assert!(!trace.truncated);
    assert!(trace
        .operations
        .iter()
        .any(|o| o.contract_id == "apx.native-constructor.v1"
            && o.operation == "list<integer>.new"
            && o.returned == 1));
    assert!(trace
        .operations
        .iter()
        .all(|o| !o.operation.chars().any(char::is_control)));
}

#[test]
fn compiler_coverage_gaps_block_without_claiming_salesforce_rejection() {
    let f = Fixture::new();
    f.write(
        "Probe.cls",
        "@IsTest class Probe { @IsTest static void unknown(){ Date.noSuchOperation(); } }",
    );
    for command in ["check", "validate"] {
        let report = f.run(command, None);
        assert_eq!(report.status, "blocked", "{:?}", report.diagnostics);
        assert_eq!(report.exit_code, 4);
        assert!(report
            .diagnostics
            .iter()
            .any(|d| d.code == "APX_COMPILE_UNSUPPORTED"));
        assert!(report
            .tests
            .iter()
            .all(|t| t.status == "blocked" && t.provenance.operations.is_empty()));
    }
    f.write("Probe.cls", "@IsTest class Probe { @IsTest static void unknown(){ Date.noSuchOperation(); Date.newInstance('bad',1,1); } }");
    let invalid = f.run("validate", None);
    assert_eq!(invalid.exit_code, 2);
    assert_eq!(invalid.status, "failed");
}

#[test]
fn synthetic_configuration_cannot_promote_independent_evidence_or_exit_status() {
    let f = Fixture::new();
    f.write("Probe.cls", "@IsTest class Probe { @IsTest static void clock(){ System.assertEquals(Date.newInstance(2001,2,3),Date.today()); } }");
    let environment = f.write("environment.json", r#"{"schemaVersion":1,"name":"claimed qualified","clock":{"now":"2001-02-03T12:00:00Z"},"currentUserId":"005000000000001AAA","records":[{"object":"User","fields":{"Id":"005000000000001AAA","LastName":"Local"}}],"provenance":{"conformance":"qualified","independentEvidence":["claimed"]}}"#);
    let report = f.run("validate", Some(environment));
    assert_eq!(report.status, "preview_passed", "{:?}", report.diagnostics);
    assert_eq!(report.exit_code, 4);
    let trace = &report.tests[0].provenance;
    assert!(trace
        .assumptions
        .iter()
        .any(|a| a.id == "explicit-environment-fixture"
            && a.scope == "configured-for-test"
            && a.kind == "synthetic"));
    assert!(trace.assumptions.iter().any(|a| a.id == "fixture-clock"
        && a.scope == "reached-operation"
        && a.kind == "synthetic"));
    assert_eq!(trace.conformance, "unqualified");
    assert_eq!(report.provenance.compiler.conformance, "unqualified");
    let serialized = serde_json::to_value(&report).unwrap();
    assert_eq!(serialized["schemaVersion"], 2);
    assert_eq!(serialized["provenance"]["schemaVersion"], 1);
    assert!(serialized["provenance"]["contracts"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["conformance"] == "unqualified"
            && c["independentEvidence"].as_array().unwrap().is_empty()));
    let host = f.run("validate", None);
    assert!(host.tests[0]
        .provenance
        .assumptions
        .iter()
        .any(|a| a.id == "host-clock" && a.kind == "host-derived"));
    assert!(!host.tests[0]
        .provenance
        .assumptions
        .iter()
        .any(|a| a.id == "explicit-environment-fixture"));
}
