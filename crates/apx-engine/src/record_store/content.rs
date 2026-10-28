//! Local Salesforce Files graph. Generated rows stay in the enclosing DML
//! transaction; a failing insert cannot leave documents, links, or versions.
use super::*;

fn file_type(path: &str) -> Result<&'static str, StoreError> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let extension = name.rsplit_once('.').map(|(_, s)| s.to_ascii_lowercase());
    Ok(match extension.as_deref() {
        None | Some("") => "UNKNOWN",
        Some("docx") => "WORD_X",
        Some("doc") => "WORD",
        Some("xlsx") => "EXCEL_X",
        Some("xls") => "EXCEL",
        Some("pptx") => "POWER_POINT_X",
        Some("ppt") => "POWER_POINT",
        Some("pdf") => "PDF",
        Some("png") => "PNG",
        Some("gif") => "GIF",
        Some("jpg" | "jpeg") => "JPG",
        Some("m4a") => "M4A",
        Some("mp3") => "MP3",
        Some("txt") => "TEXT",
        Some("csv") => "CSV",
        Some("zip") => "ZIP",
        Some("html" | "htm") => "HTML",
        Some(other) => {
            return Err(StoreError::unsupported(format!(
                "Content file-type mapping for .{other} is not qualified"
            )))
        }
    })
}

impl RecordStore {
    pub(super) fn prepare_content_record(
        &mut self,
        row: &mut RecordData,
        previous: Option<&RecordData>,
    ) -> Result<(), StoreError> {
        if !row.object.eq_ignore_ascii_case("ContentVersion")
            && !row.object.eq_ignore_ascii_case("ContentDistribution")
        {
            return Ok(());
        }
        // Also isolate each record in partial-success DML, where the caller
        // keeps successful siblings instead of rolling back the whole batch.
        let snapshot = self.snapshot();
        let original = row.clone();
        let result = self.prepare_content_inner(row, previous);
        if result.is_err() {
            self.restore(snapshot);
            *row = original;
        }
        result
    }
    fn prepare_content_inner(
        &mut self,
        row: &mut RecordData,
        previous: Option<&RecordData>,
    ) -> Result<(), StoreError> {
        if row.object.eq_ignore_ascii_case("ContentVersion") {
            if previous.is_some() {
                return Err(StoreError::unsupported(
                    "ContentVersion updates require version-field update qualification",
                ));
            }
            if row
                .get("ContentLocation")
                .and_then(DataValue::text)
                .is_some_and(|s| s != "S")
            {
                return Err(StoreError::unsupported(
                    "Only locally stored Salesforce Files are modeled",
                ));
            }
            let version_id = row
                .id()
                .ok_or_else(|| StoreError::unsupported("ContentVersion requires allocated Id"))?
                .to_string();
            let path = row
                .get("PathOnClient")
                .and_then(DataValue::text)
                .ok_or_else(|| {
                    StoreError::new(
                        "REQUIRED_FIELD_MISSING",
                        "PathOnClient is required",
                        vec!["PathOnClient".into()],
                    )
                })?;
            let kind = file_type(path)?;
            let extension = path
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(path)
                .rsplit_once('.')
                .map(|(_, ext)| ext.to_ascii_lowercase())
                .unwrap_or_default();
            let size = match row.get("VersionData") {
                Some(DataValue::Binary(bytes)) => bytes.len(),
                _ => {
                    return Err(StoreError::new(
                        "REQUIRED_FIELD_MISSING",
                        "VersionData is required",
                        vec!["VersionData".into()],
                    ))
                }
            };
            let title = row.get("Title").cloned().unwrap_or(DataValue::Null);
            let existing_document = row
                .get("ContentDocumentId")
                .and_then(DataValue::text)
                .map(str::to_string);
            let document_id = if let Some(id) = existing_document.as_ref() {
                if self.find("ContentDocument", id).is_none() {
                    return Err(StoreError::new(
                        "INVALID_CROSS_REFERENCE_KEY",
                        "ContentDocument does not exist",
                        vec!["ContentDocumentId".into()],
                    ));
                }
                if row
                    .get("FirstPublishLocationId")
                    .is_some_and(|v| !v.is_null())
                {
                    return Err(StoreError::unsupported(
                        "FirstPublishLocationId on an additional version is not qualified",
                    ));
                }
                id.clone()
            } else {
                let mut document = RecordData::new("ContentDocument")
                    .with("Title", title.clone())
                    .with("FileType", DataValue::Text(kind.into()));
                if let Some(owner) = row.get("OwnerId") {
                    document.set("OwnerId", owner.clone());
                }
                self.upsert_platform_record(&document)?
            };
            let older = self
                .records("ContentVersion", false)
                .into_iter()
                .filter(|r| {
                    r.get("ContentDocumentId").and_then(DataValue::text)
                        == Some(document_id.as_str())
                })
                .cloned()
                .collect::<Vec<_>>();
            for old in &older {
                self.upsert_platform_record(
                    &RecordData::new("ContentVersion")
                        .with("Id", DataValue::Text(old.id().unwrap().into()))
                        .with("IsLatest", DataValue::Boolean(false)),
                )?;
            }
            row.set("ContentDocumentId", DataValue::Text(document_id.clone()));
            row.set("FileType", DataValue::Text(kind.into()));
            row.set("IsLatest", DataValue::Boolean(true));
            row.set(
                "VersionNumber",
                DataValue::Text((older.len() + 1).to_string()),
            );
            row.set("FileExtension", DataValue::Text(extension));
            row.set("ContentSize", DataValue::integer(size as i64));
            // Persist the valid version before the reciprocal latest-version link.
            self.upsert_platform_record(row)?;
            self.upsert_platform_record(
                &RecordData::new("ContentDocument")
                    .with("Id", DataValue::Text(document_id.clone()))
                    .with("Title", title)
                    .with("FileType", DataValue::Text(kind.into()))
                    .with("ContentSize", DataValue::integer(size as i64))
                    .with("LatestPublishedVersionId", DataValue::Text(version_id)),
            )?;
            if existing_document.is_none() {
                let location = row
                    .get("FirstPublishLocationId")
                    .and_then(DataValue::text)
                    .map(str::to_string)
                    .or_else(|| self.current_user.clone());
                let Some(location) = location else {
                    return Err(StoreError::unsupported(
                        "A first publication requires an explicit local location/user",
                    ));
                };
                if self.find_any(&location).is_none() {
                    return Err(StoreError::new(
                        "INVALID_CROSS_REFERENCE_KEY",
                        "First publication location does not exist",
                        vec!["FirstPublishLocationId".into()],
                    ));
                }
                self.upsert_platform_record(
                    &RecordData::new("ContentDocumentLink")
                        .with("ContentDocumentId", DataValue::Text(document_id))
                        .with("LinkedEntityId", DataValue::Text(location))
                        .with("ShareType", DataValue::Text("V".into()))
                        .with("Visibility", DataValue::Text("AllUsers".into())),
                )?;
            }
        } else if row.object.eq_ignore_ascii_case("ContentDistribution") {
            if previous.is_some() {
                return Err(StoreError::unsupported(
                    "ContentDistribution updates are not qualified",
                ));
            }
            let version = row
                .get("ContentVersionId")
                .and_then(DataValue::text)
                .ok_or_else(|| {
                    StoreError::new(
                        "REQUIRED_FIELD_MISSING",
                        "ContentVersionId is required",
                        vec!["ContentVersionId".into()],
                    )
                })?;
            if self.find("ContentVersion", version).is_none() {
                return Err(StoreError::new(
                    "INVALID_CROSS_REFERENCE_KEY",
                    "Content version does not exist",
                    vec!["ContentVersionId".into()],
                ));
            }
            let base=self.content_download_base_url.as_deref().ok_or_else(||StoreError::unsupported("ContentDistribution needs an explicit local contentDownloadBaseUrl; no network publishing occurs"))?;
            if !base.starts_with("https://") || base.contains(['?', '#', '\r', '\n']) {
                return Err(StoreError::unsupported(
                    "contentDownloadBaseUrl must be a plain HTTPS origin/path",
                ));
            }
            let id = row.id().ok_or_else(|| {
                StoreError::unsupported("ContentDistribution requires allocated Id")
            })?;
            row.set(
                "ContentDownloadUrl",
                DataValue::Text(format!(
                    "{}/apx/content/{version}/distribution/{id}",
                    base.trim_end_matches('/')
                )),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn files_derive_graph_from_payload_and_new_versions_replace_latest_atomically() {
        let mut store = RecordStore::new(crate::schema::SchemaCatalog::standard()).unwrap();
        let account = store
            .apply(
                DmlOperation::Insert,
                &[RecordData::new("Account").with("Name", DataValue::Text("Owner".into()))],
                true,
            )
            .unwrap()[0]
            .id
            .clone()
            .unwrap();
        let v = RecordData::new("ContentVersion")
            .with("Title", DataValue::Text("Original".into()))
            .with("PathOnClient", DataValue::Text("original.docx".into()))
            .with("VersionData", DataValue::Binary(vec![1, 2, 3].into()))
            .with("FirstPublishLocationId", DataValue::Text(account.clone()));
        let first = store.apply(DmlOperation::Insert, &[v], true).unwrap()[0]
            .id
            .clone()
            .unwrap();
        let first_row = store.find("ContentVersion", &first).unwrap();
        let document = first_row
            .get("ContentDocumentId")
            .unwrap()
            .text()
            .unwrap()
            .to_string();
        assert_eq!(first_row.get("FileType").unwrap().text(), Some("WORD_X"));
        assert_eq!(store.records("ContentDocumentLink", false).len(), 1);
        assert_eq!(
            store.records("ContentDocumentLink", false)[0]
                .get("LinkedEntityId")
                .unwrap()
                .text(),
            Some(account.as_str())
        );
        let second = RecordData::new("ContentVersion")
            .with("Title", DataValue::Text("Changed".into()))
            .with("PathOnClient", DataValue::Text("changed.png".into()))
            .with("VersionData", DataValue::Binary(vec![4, 5].into()))
            .with("ContentDocumentId", DataValue::Text(document.clone()));
        let second = store.apply(DmlOperation::Insert, &[second], true).unwrap()[0]
            .id
            .clone()
            .unwrap();
        assert_eq!(store.records("ContentDocument", false).len(), 1);
        assert_eq!(
            store
                .find("ContentDocument", &document)
                .unwrap()
                .get("LatestPublishedVersionId")
                .unwrap()
                .text(),
            Some(second.as_str())
        );
        assert_eq!(
            store
                .find("ContentVersion", &first)
                .unwrap()
                .get("IsLatest"),
            Some(&DataValue::Boolean(false))
        );
        assert_eq!(
            store
                .find("ContentVersion", &second)
                .unwrap()
                .get("IsLatest"),
            Some(&DataValue::Boolean(true))
        );
        let bad = RecordData::new("ContentVersion")
            .with("Title", DataValue::Text("No graph".into()))
            .with("PathOnClient", DataValue::Text("bad.unqualified".into()))
            .with("VersionData", DataValue::Binary(vec![6].into()));
        assert!(store.apply(DmlOperation::Insert, &[bad], true).is_err());
        assert_eq!(store.records("ContentDocument", false).len(), 1);
        assert_eq!(store.records("ContentVersion", false).len(), 2);
    }
    #[test]
    fn generated_distribution_url_requires_explicit_local_delivery_contract() {
        let mut store = RecordStore::new(crate::schema::SchemaCatalog::standard()).unwrap();
        let user = store
            .upsert_platform_record(
                &RecordData::new("User").with("LastName", DataValue::Text("Local".into())),
            )
            .unwrap();
        store.current_user = Some(user);
        let v = RecordData::new("ContentVersion")
            .with("Title", DataValue::Text("Local".into()))
            .with("PathOnClient", DataValue::Text("local.txt".into()))
            .with(
                "VersionData",
                DataValue::Binary(b"actual bytes".to_vec().into()),
            );
        let version = store.apply(DmlOperation::Insert, &[v], true).unwrap()[0]
            .id
            .clone()
            .unwrap();
        let distribution = RecordData::new("ContentDistribution")
            .with("Name", DataValue::Text("Explicit".into()))
            .with("ContentVersionId", DataValue::Text(version.clone()));
        assert!(store
            .apply(DmlOperation::Insert, &[distribution.clone()], true)
            .is_err());
        assert!(store.records("ContentDistribution", false).is_empty());
        store.content_download_base_url = Some("https://files.example.invalid".into());
        let id = store
            .apply(DmlOperation::Insert, &[distribution], true)
            .unwrap()[0]
            .id
            .clone()
            .unwrap();
        let url = store
            .find("ContentDistribution", &id)
            .unwrap()
            .get("ContentDownloadUrl")
            .unwrap()
            .text()
            .unwrap();
        assert!(url.starts_with("https://files.example.invalid/apx/content/"));
        assert!(url.contains(&version));
        assert!(url.ends_with(&id));
    }
}
