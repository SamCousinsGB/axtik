//! Regression contracts for field constraints, row access, and terminal governors.
use apx_engine::{
    environment::Environment,
    record_store::{DataValue, DmlOperation, GovernorLimit, RecordData, RecordStore},
    runtime::Context,
    schema::{FieldKind, FieldSchema, SchemaCatalog},
    syntax::{declarations, parse, Program},
};

fn program(body: &str) -> Program {
    let source = format!("@IsTest class Probe {{ @IsTest static void sample() {{ {body} }} }}");
    let tree = parse(&source, "data_correctness.cls").unwrap();
    let mut p = Program::default();
    let mut diagnostics = vec![];
    declarations(&tree, &mut p, &mut diagnostics);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    p
}
fn account(name: &str) -> RecordData {
    RecordData::new("Account").with("Name", DataValue::Text(name.into()))
}

#[test]
fn standard_text_constraints_enforce_boundary_and_utf16_on_insert_update_and_partial_batches() {
    let mut store = RecordStore::new(SchemaCatalog::standard()).unwrap();
    let at_limit = account(&"a".repeat(255));
    let too_long = account(&"b".repeat(256));
    let valid = store
        .apply(DmlOperation::Insert, &[at_limit.clone()], true)
        .unwrap();
    let id = valid[0].id.clone().unwrap();
    let error = store
        .apply(
            DmlOperation::Update,
            &[too_long.clone().with("Id", DataValue::Text(id.clone()))],
            true,
        )
        .unwrap_err();
    assert_eq!(error.error.code, "STRING_TOO_LONG");
    assert_eq!(
        store.find("Account", &id).unwrap().get("Name"),
        at_limit.get("Name")
    );
    let error = store
        .apply(
            DmlOperation::Insert,
            &[account("valid"), too_long.clone()],
            true,
        )
        .unwrap_err();
    assert_eq!(error.error.code, "STRING_TOO_LONG");
    assert_eq!(store.records("Account", false).len(), 1);
    let results = store
        .apply(DmlOperation::Insert, &[account("valid"), too_long], false)
        .unwrap();
    assert!(results[0].success);
    assert!(!results[1].success);
    assert_eq!(results[1].errors[0].fields, ["Name"]);
    assert_eq!(store.records("Account", false).len(), 2);
    assert!(store
        .apply(
            DmlOperation::Insert,
            &[account(&format!("{}a", "😀".repeat(127)))],
            true
        )
        .is_ok());
    assert_eq!(
        store
            .apply(DmlOperation::Insert, &[account(&"😀".repeat(128))], true)
            .unwrap_err()
            .error
            .code,
        "STRING_TOO_LONG"
    );
    for (object, field, limit) in [
        ("Contact", "LastName", 80),
        ("Lead", "Company", 255),
        ("Task", "Subject", 255),
    ] {
        assert_eq!(
            store.schema.field(object, field).unwrap().length,
            Some(limit)
        );
        let mut row = RecordData::new(object).with(field, DataValue::Text("x".repeat(limit)));
        if object == "Lead" {
            row.set("LastName", DataValue::Text("Lead".into()));
        }
        store
            .apply(DmlOperation::Insert, &[row.clone()], true)
            .unwrap();
        row.set(field, DataValue::Text("x".repeat(limit + 1)));
        assert_eq!(
            store
                .apply(DmlOperation::Insert, &[row], true)
                .unwrap_err()
                .error
                .code,
            "STRING_TOO_LONG"
        );
    }
}

#[test]
fn numeric_precision_is_enforced_and_unqualified_rounding_cannot_pass() {
    let mut store = RecordStore::new(SchemaCatalog::standard()).unwrap();
    store
        .apply(
            DmlOperation::Insert,
            &[account("boundary").with("NumberOfEmployees", DataValue::integer(99_999_999))],
            true,
        )
        .unwrap();
    for value in [100_000_000, -100_000_000] {
        assert_eq!(
            store
                .apply(
                    DmlOperation::Insert,
                    &[account("overflow").with("NumberOfEmployees", DataValue::integer(value))],
                    true
                )
                .unwrap_err()
                .error
                .code,
            "NUMBER_OUTSIDE_VALID_RANGE"
        );
    }
    let amount = store.schema.field("Opportunity", "Amount").unwrap();
    assert_eq!((amount.precision, amount.scale), (Some(18), Some(2)));
    let mut field = FieldSchema::new("Exact__c", FieldKind::Decimal);
    field.precision = Some(5);
    field.scale = Some(2);
    store
        .schema
        .objects
        .get_mut("account")
        .unwrap()
        .fields
        .insert("exact__c".into(), field);
    store
        .apply(
            DmlOperation::Insert,
            &[account("exact").with("Exact__c", DataValue::number("999.99").unwrap())],
            true,
        )
        .unwrap();
    assert_eq!(
        store
            .apply(
                DmlOperation::Insert,
                &[account("overflow").with("Exact__c", DataValue::number("1000").unwrap())],
                false
            )
            .unwrap()[0]
            .errors[0]
            .code,
        "NUMBER_OUTSIDE_VALID_RANGE"
    );
    let before = store.records("Account", false).len();
    let error = store
        .apply(
            DmlOperation::Insert,
            &[
                account("rollback"),
                account("rounding").with("Exact__c", DataValue::number("1.001").unwrap()),
            ],
            false,
        )
        .unwrap_err();
    assert!(error.error.unsupported);
    assert_eq!(store.records("Account", false).len(), before);
}

fn environment(
    schema: &SchemaCatalog,
    sharing: &str,
    view_all: bool,
    modify_all: bool,
) -> Environment {
    let json = serde_json::json!({"schemaVersion":1,"clock":{"now":"2026-01-15T12:00:00Z"},"currentUserId":"005000000000001AAA","records":[
        {"object":"Profile","fields":{"Id":"00e000000000001AAA","Name":"Explicit local policy"}},
        {"object":"User","fields":{"Id":"005000000000001AAA","ProfileId":"00e000000000001AAA"}},
        {"object":"User","fields":{"Id":"005000000000002AAA","ProfileId":"00e000000000001AAA"}}
    ],"security":{"mode":"synthetic-local","defaultSharingModel":sharing,"objectSharingModels":{"Contact":"ControlledByParent"},"profilePolicies":[{
        "profileId":"00e000000000001AAA","objects":{"selector":"all-registered-objects","read":true,"create":true,"edit":true,"delete":true,"viewAll":view_all,"modifyAll":modify_all},
        "fields":{"selector":"all-registered-fields","read":true,"edit":true}
    }]}});
    Environment::parse(&json, schema, "explicit test access policy").unwrap()
}
fn execute_access(body: &str, sharing: &str, view_all: bool, modify_all: bool) {
    let mut p = program(body);
    let mut external = FieldSchema::new("External__c", FieldKind::Text);
    external.external_id = true;
    external.length = Some(80);
    p.schema
        .objects
        .get_mut("account")
        .unwrap()
        .fields
        .insert("external__c".into(), external);
    p.environment = Some(environment(&p.schema, sharing, view_all, modify_all));
    let declaration = &p.classes["probe"].methods[0].declaration;
    let result = Context::new(&p).call("Probe", "sample", vec![], declaration);
    assert!(result.is_ok(), "{result:?}");
}
const FOREIGN_ACCOUNT: &str = "Account a=new Account(Name='Original',OwnerId='005000000000002AAA',External__c='foreign'); Database.insert(a,AccessLevel.SYSTEM_MODE);";

#[test]
fn public_read_only_and_view_all_do_not_grant_record_edit_or_delete() {
    for (sharing, view_all) in [("PublicReadOnly", false), ("Private", true)] {
        execute_access(&format!("{FOREIGN_ACCOUNT} System.assertEquals(1,[SELECT count() FROM Account WITH USER_MODE]); Boolean denied=false; try {{ Database.update(new Account(Id=a.Id,Name='Changed'),AccessLevel.USER_MODE); }} catch(DmlException e) {{ denied=true; }} System.assert(denied); denied=false; try {{ Database.delete(a,AccessLevel.USER_MODE); }} catch(DmlException e) {{ denied=true; }} System.assert(denied); System.assertEquals('Original',[SELECT Name FROM Account WITH SYSTEM_MODE].Name);"), sharing, view_all, false);
    }
}

#[test]
fn statement_user_mode_is_enforced_and_explicit_system_mode_is_preserved() {
    for operation in ["update", "upsert", "delete"] {
        execute_access(&format!("{FOREIGN_ACCOUNT} Boolean denied=false; a.Name='User write'; try {{ {operation} as user a; }} catch(DmlException e) {{ denied=true; }} System.assert(denied); System.assertEquals('Original',[SELECT Name FROM Account WITH SYSTEM_MODE].Name); {operation} as system a;"), "PublicReadOnly", false, false);
    }
    execute_access(&format!("{FOREIGN_ACCOUNT} delete as system a; Boolean denied=false; try {{ undelete as user a; }} catch(DmlException e) {{ denied=true; }} System.assert(denied); undelete as system a; System.assertEquals('Original',[SELECT Name FROM Account WITH SYSTEM_MODE].Name);"), "PublicReadOnly", false, false);
    execute_access("Account a=new Account(Name='Owner'); insert as user a; a.Name='Owner update'; update as user a; upsert as user a; delete as user a; undelete as user a; System.assertEquals('Owner update',[SELECT Name FROM Account WITH SYSTEM_MODE].Name);", "Private", false, false);
}

#[test]
fn statement_user_insert_requires_create_even_when_system_insert_is_allowed() {
    let mut p = program("Account a=new Account(Name='System insert'); insert as system a; Boolean denied=false; try { insert as user new Account(Name='Denied'); } catch(SecurityException e) { denied=true; } System.assert(denied); System.assertEquals(1,[SELECT count() FROM Account WITH SYSTEM_MODE]);");
    let mut env = environment(&p.schema, "PublicReadOnly", false, false);
    env.security["profilePolicies"][0]["objects"]["create"] = serde_json::json!(false);
    p.environment = Some(env);
    let result = Context::new(&p).call(
        "Probe",
        "sample",
        vec![],
        &p.classes["probe"].methods[0].declaration,
    );
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn public_read_write_allows_edit_but_delete_remains_an_owner_right() {
    execute_access(&format!("{FOREIGN_ACCOUNT} Database.update(new Account(Id=a.Id,Name='Changed'),AccessLevel.USER_MODE); Boolean denied=false; try {{ Database.delete(a,AccessLevel.USER_MODE); }} catch(DmlException e) {{ denied=true; }} System.assert(denied); System.assertEquals('Changed',[SELECT Name FROM Account WITH SYSTEM_MODE].Name);"), "PublicReadWrite", false, false);
}

#[test]
fn owner_and_modify_all_can_update_delete_and_restore() {
    for modify_all in [false, true] {
        let setup = if modify_all {
            FOREIGN_ACCOUNT
        } else {
            "Account a=new Account(Name='Original'); Database.insert(a,AccessLevel.SYSTEM_MODE);"
        };
        execute_access(&format!("{setup} Database.update(new Account(Id=a.Id,Name='Changed'),AccessLevel.USER_MODE); Database.delete(a,AccessLevel.USER_MODE); System.assertEquals(0,[SELECT count() FROM Account WITH SYSTEM_MODE]); Database.undelete(a,AccessLevel.USER_MODE); System.assertEquals('Changed',[SELECT Name FROM Account WITH SYSTEM_MODE].Name);"), "Private", false, modify_all);
    }
}

#[test]
fn controlled_by_parent_preserves_read_only_rights_and_external_upsert_cannot_bypass_access() {
    execute_access(&format!("{FOREIGN_ACCOUNT} Contact c=new Contact(LastName='Child',AccountId=a.Id,OwnerId='005000000000002AAA'); Database.insert(c,AccessLevel.SYSTEM_MODE); System.assertEquals(1,[SELECT count() FROM Contact WITH USER_MODE]); Boolean denied=false; try {{ Database.update(new Contact(Id=c.Id,LastName='Changed'),AccessLevel.USER_MODE); }} catch(DmlException e) {{ denied=true; }} System.assert(denied); denied=false; try {{ Database.upsert(new Account(Name='Changed',External__c='foreign'),Account.External__c,AccessLevel.USER_MODE); }} catch(DmlException e) {{ denied=true; }} System.assert(denied); System.assertEquals('Original',[SELECT Name FROM Account WITH SYSTEM_MODE].Name);"), "PublicReadOnly", false, false);
}

#[test]
fn dml_row_governor_is_terminal_even_with_partial_results_catch_and_finally_return() {
    for partial in [false, true] {
        let call = if partial {
            "Database.insert(rows,false);"
        } else {
            "insert rows;"
        };
        let p = program(&format!("List<Account> rows=new List<Account>(); for(Integer i=0;i<10001;i++) rows.add(new Account(Name='Account')); try {{ {call} }} catch(Exception e) {{ System.assert(false,'must not catch'); }} finally {{ return; }}"));
        let declaration = &p.classes["probe"].methods[0].declaration;
        let mut context = Context::new(&p);
        let error = context
            .call("Probe", "sample", vec![], declaration)
            .unwrap_err();
        assert_eq!(error.status, "failed");
        assert_eq!(error.diagnostic.code, "APX_GOVERNOR");
        assert!(error.exception.is_none());
        let store = context.data_store.as_ref().unwrap();
        assert_eq!(
            store.records("Account", false).len(),
            0,
            "earlier chunks must roll back"
        );
        assert_eq!(store.dml_rows, 10_001);
    }
}

#[test]
fn exact_runtime_dml_boundary_and_native_query_statement_limits_remain_terminal() {
    let p = program("List<Account> rows=new List<Account>(); for(Integer i=0;i<10000;i++) rows.add(new Account(Name='Boundary')); insert rows; System.assertEquals(10000,[SELECT count() FROM Account]);");
    let result = Context::new(&p).call(
        "Probe",
        "sample",
        vec![],
        &p.classes["probe"].methods[0].declaration,
    );
    assert!(result.is_ok(), "{result:?}");
    for (body, label) in [
        (
            "for(Integer i=0;i<151;i++) insert new Account(Name='row');",
            "DML",
        ),
        (
            "for(Integer i=0;i<101;i++) { List<Account> rows=[SELECT Id FROM Account]; }",
            "SOQL",
        ),
    ] {
        let p = program(&format!(
            "try {{ {body} }} catch(Exception e) {{ return; }} finally {{ return; }}"
        ));
        let error = Context::new(&p)
            .call(
                "Probe",
                "sample",
                vec![],
                &p.classes["probe"].methods[0].declaration,
            )
            .unwrap_err();
        assert!(error.exception.is_none());
        assert_eq!(error.status, "failed");
        assert_eq!(error.diagnostic.code, "APX_GOVERNOR");
        assert!(error.diagnostic.message.contains(label));
    }
}

#[test]
fn terminal_governor_raised_inside_catch_cannot_be_masked_by_finally() {
    let p = program("try { insert new Account(); } catch(DmlException e) { for(Integer i=0;i<6;i++) Database.setSavepoint(); } finally { return; }");
    let error = Context::new(&p)
        .call(
            "Probe",
            "sample",
            vec![],
            &p.classes["probe"].methods[0].declaration,
        )
        .unwrap_err();
    assert!(error.exception.is_none());
    assert_eq!(error.diagnostic.code, "APX_GOVERNOR");
    assert!(error.diagnostic.message.contains("savepoints: 6"));
}

#[test]
fn exact_dml_statement_and_row_boundaries_succeed_and_next_charge_is_typed_terminal() {
    let mut store = RecordStore::new(SchemaCatalog::standard()).unwrap();
    store
        .apply(
            DmlOperation::Insert,
            &vec![account("boundary"); 10_000],
            true,
        )
        .unwrap();
    assert_eq!(store.records("Account", false).len(), 10_000);
    let error = store
        .apply(DmlOperation::Insert, &[account("too many")], false)
        .unwrap_err();
    assert_eq!(error.error.governor, Some(GovernorLimit::DmlRows));
    assert_eq!(store.records("Account", false).len(), 10_000);
    let mut store = RecordStore::new(SchemaCatalog::standard()).unwrap();
    for _ in 0..150 {
        store
            .apply(DmlOperation::Insert, &[account("boundary")], true)
            .unwrap();
    }
    let error = store
        .apply(DmlOperation::Insert, &[account("too many")], true)
        .unwrap_err();
    assert_eq!(error.error.governor, Some(GovernorLimit::DmlStatements));
    assert_eq!(store.records("Account", false).len(), 150);
}

#[test]
fn query_row_limit_including_child_rows_is_terminal_at_runtime() {
    for query in [
        "SELECT Id FROM Account",
        "SELECT Id,(SELECT Id FROM Contacts) FROM Account",
    ] {
        let p = program(&format!("try {{ List<Account> rows=[{query}]; }} catch(QueryException e) {{ System.assert(false,'must not catch'); }} finally {{ return; }}"));
        let mut store = RecordStore::new(p.schema.clone()).unwrap();
        let account_id = store
            .apply(DmlOperation::Insert, &[account("parent")], true)
            .unwrap()[0]
            .id
            .clone()
            .unwrap();
        store
            .apply(
                DmlOperation::Insert,
                &[RecordData::new("Contact")
                    .with("LastName", DataValue::Text("child".into()))
                    .with("AccountId", DataValue::Text(account_id))],
                true,
            )
            .unwrap();
        let mut context = Context::new(&p);
        context.install_data_store(Some(store));
        let consumed = if query.contains("Contacts") {
            49_999
        } else {
            50_000
        };
        context.data_store.as_mut().unwrap().query_rows = consumed;
        let error = context
            .call(
                "Probe",
                "sample",
                vec![],
                &p.classes["probe"].methods[0].declaration,
            )
            .unwrap_err();
        assert!(error.exception.is_none());
        assert_eq!(error.diagnostic.code, "APX_GOVERNOR");
        assert!(error.diagnostic.message.contains("query rows: 50001"));
    }
    let mut store = RecordStore::new(SchemaCatalog::standard()).unwrap();
    store
        .apply(DmlOperation::Insert, &[account("last allowed row")], true)
        .unwrap();
    store.query_rows = 49_999;
    let query = apx_engine::soql::parse("SELECT Id FROM Account").unwrap();
    store
        .query(
            &query,
            &Default::default(),
            apx_engine::soql::QueryAccess::System,
        )
        .unwrap();
    assert_eq!(store.query_rows, 50_000);
    let error = store
        .query(
            &query,
            &Default::default(),
            apx_engine::soql::QueryAccess::System,
        )
        .unwrap_err();
    assert_eq!(error.governor, Some(GovernorLimit::QueryRows));
}
