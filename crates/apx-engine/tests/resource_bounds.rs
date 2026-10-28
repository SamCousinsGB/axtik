use apx_engine::{
    model::Diagnostic,
    record_store::{DataValue, RecordStore},
    schema::{MetadataRecord, MetadataScalar, SchemaCatalog},
};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn binary_payloads_share_immutable_storage_across_transaction_snapshots() {
    let payload: Arc<[u8]> = vec![37; 1024 * 1024].into();
    let mut schema = SchemaCatalog::standard();
    schema.metadata_records.push(MetadataRecord {
        object: "StaticResource".into(),
        developer_name: "LargePayload".into(),
        label: "Large payload".into(),
        protected: false,
        values: BTreeMap::from([("Body".into(), MetadataScalar::Binary(payload.clone()))]),
    });
    let mut parent = RecordStore::new(schema).unwrap();
    let checkpoint = parent.savepoint().unwrap();
    let child = parent.fork_transaction();
    for store in [&parent, &child] {
        let row = store.records("StaticResource", false)[0];
        let Some(DataValue::Binary(actual)) = row.get("Body") else {
            panic!("missing payload")
        };
        assert!(Arc::ptr_eq(&payload, actual));
        assert_eq!(actual.len(), 1024 * 1024);
    }
    parent.rollback(checkpoint).unwrap();
    let rows = parent.records("StaticResource", false);
    let Some(DataValue::Binary(restored)) = rows[0].get("Body") else {
        panic!("missing payload")
    };
    assert!(Arc::ptr_eq(&payload, restored));
}

#[test]
fn report_messages_are_bounded_at_valid_unicode_boundaries() {
    let diagnostic = Diagnostic::plain("APX_TEST", "é".repeat(10_000));
    assert!(diagnostic.message.len() < 8300);
    assert!(diagnostic.message.ends_with("[diagnostic truncated]"));
    assert!(diagnostic.message.starts_with("éé"));
}
