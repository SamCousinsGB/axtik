//! Independent Files transaction and unsupported-operation counterexamples.
use apx_engine::{
    record_store::{DataValue, DmlOperation, RecordData, RecordStore},
    schema::SchemaCatalog,
};

fn text(value: &str) -> DataValue {
    DataValue::Text(value.into())
}
fn setup() -> (RecordStore, String, String) {
    let mut store = RecordStore::new(SchemaCatalog::standard()).unwrap();
    let owner = store
        .upsert_platform_record(&RecordData::new("User").with("LastName", text("Owner")))
        .unwrap();
    store.current_user = Some(owner);
    let first = store
        .apply(
            DmlOperation::Insert,
            &[version("first.txt", "First", None)],
            true,
        )
        .unwrap()[0]
        .id
        .clone()
        .unwrap();
    let document = store
        .find("ContentVersion", &first)
        .unwrap()
        .get("ContentDocumentId")
        .unwrap()
        .text()
        .unwrap()
        .to_string();
    (store, first, document)
}
fn version(path: &str, title: &str, document: Option<&str>) -> RecordData {
    let mut row = RecordData::new("ContentVersion")
        .with("Title", text(title))
        .with("PathOnClient", text(path))
        .with(
            "VersionData",
            DataValue::Binary(title.as_bytes().to_vec().into()),
        );
    if let Some(id) = document {
        row.set("ContentDocumentId", text(id));
    }
    row
}

#[test]
fn unsupported_files_operations_cannot_create_orphans_or_invent_documents() {
    let (mut store, first, document) = setup();
    assert!(
        store
            .apply(
                DmlOperation::Insert,
                &[RecordData::new("ContentDocument").with("Title", text("Invented"))],
                true
            )
            .is_err(),
        "ContentDocument create is not a supported platform API"
    );
    assert!(store.apply(DmlOperation::Delete,&[RecordData::new("ContentVersion").with("Id",text(&first))],true).is_err(),"ContentVersion delete must not succeed and leave the document pointing at a deleted version");
    assert!(store.find("ContentVersion", &first).is_some());
    assert_eq!(
        store
            .find("ContentDocument", &document)
            .unwrap()
            .get("LatestPublishedVersionId")
            .and_then(DataValue::text),
        Some(first.as_str())
    );
}

#[test]
fn supplied_derived_file_metadata_is_rejected_instead_of_silently_overwritten() {
    let (mut store, _, document) = setup();
    let forged = version("second.txt", "Changed", Some(&document)).with("FileType", text("PDF"));
    assert!(
        store.apply(DmlOperation::Insert, &[forged], true).is_err(),
        "FileType is derived, not writable input"
    );
    store.content_download_base_url = Some("https://files.example.invalid".into());
    let version = store.records("ContentVersion", false)[0]
        .id()
        .unwrap()
        .to_string();
    let distribution = RecordData::new("ContentDistribution")
        .with("Name", text("Forged URL"))
        .with("ContentVersionId", text(&version))
        .with(
            "ContentDownloadUrl",
            text("https://wrong.example.invalid/download"),
        );
    assert!(
        store
            .apply(DmlOperation::Insert, &[distribution], true)
            .is_err(),
        "Generated distribution URLs must not accept caller-supplied values"
    );
}

#[test]
fn version_graph_follows_batch_atomicity_partial_success_and_explicit_rollback() {
    let (mut store, first, document) = setup();
    let point = store.savepoint().unwrap();
    let changed = version("second.png", "Second", Some(&document));
    let bad = RecordData::new("ContentVersion")
        .with("PathOnClient", text("missing-title.txt"))
        .with("VersionData", DataValue::Binary(vec![1].into()));
    assert!(store
        .apply(DmlOperation::Insert, &[changed.clone(), bad.clone()], true)
        .is_err());
    assert_eq!(store.records("ContentVersion", false).len(), 1);
    assert_eq!(
        store
            .find("ContentVersion", &first)
            .unwrap()
            .get("IsLatest"),
        Some(&DataValue::Boolean(true))
    );
    assert_eq!(
        store
            .find("ContentDocument", &document)
            .unwrap()
            .get("LatestPublishedVersionId")
            .and_then(DataValue::text),
        Some(first.as_str())
    );
    let partial = store
        .apply(DmlOperation::Insert, &[changed, bad], false)
        .unwrap();
    assert!(partial[0].success && !partial[1].success);
    let second = partial[0].id.as_ref().unwrap();
    assert_eq!(store.records("ContentDocument", false).len(), 1);
    assert_eq!(store.records("ContentDocumentLink", false).len(), 1);
    assert_eq!(store.records("ContentVersion", false).len(), 2);
    assert_eq!(
        store
            .find("ContentDocument", &document)
            .unwrap()
            .get("LatestPublishedVersionId")
            .and_then(DataValue::text),
        Some(second.as_str())
    );
    assert_eq!(
        store
            .find("ContentVersion", second)
            .unwrap()
            .get("VersionNumber")
            .and_then(DataValue::text),
        Some("2")
    );
    assert_eq!(
        store
            .find("ContentVersion", second)
            .unwrap()
            .get("VersionData"),
        Some(&DataValue::Binary(b"Second".to_vec().into()))
    );
    store.rollback(point).unwrap();
    assert_eq!(store.records("ContentVersion", false).len(), 1);
    assert_eq!(
        store
            .find("ContentVersion", &first)
            .unwrap()
            .get("IsLatest"),
        Some(&DataValue::Boolean(true))
    );
    assert_eq!(
        store
            .find("ContentDocument", &document)
            .unwrap()
            .get("LatestPublishedVersionId")
            .and_then(DataValue::text),
        Some(first.as_str())
    );
}
