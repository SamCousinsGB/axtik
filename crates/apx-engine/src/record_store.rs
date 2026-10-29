//! Owned transaction data, independent of interpreter references and mutable aliases.
use crate::schema::{FieldKind, FieldSchema, MetadataScalar, SchemaCatalog};
use num_rational::BigRational;
use num_traits::ToPrimitive;
use std::collections::{BTreeMap, BTreeSet};
mod content;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT_SAVEPOINT: AtomicU64 = AtomicU64::new(1);
pub(crate) fn transaction_marker() -> u64 {
    NEXT_SAVEPOINT.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DataValue {
    Null,
    Boolean(bool),
    Number(BigRational),
    Text(String),
    Date(String),
    DateTime(String),
    Binary(std::sync::Arc<[u8]>),
    List(Vec<DataValue>),
    Record(Box<RecordData>),
}
impl DataValue {
    pub fn number(s: &str) -> Result<Self, StoreError> {
        crate::value::Number::parse(s)
            .map(|n| Self::Number(n.q))
            .map_err(StoreError::unsupported)
    }
    pub fn integer(v: i64) -> Self {
        Self::Number(BigRational::from_integer(v.into()))
    }
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(s) | Self::Date(s) | Self::DateTime(s) => Some(s),
            _ => None,
        }
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordData {
    pub object: String,
    pub fields: BTreeMap<String, DataValue>,
}
impl RecordData {
    pub fn new(object: impl Into<String>) -> Self {
        Self {
            object: object.into(),
            fields: BTreeMap::new(),
        }
    }
    pub fn with(mut self, field: &str, value: DataValue) -> Self {
        self.set(field, value);
        self
    }
    pub fn set(&mut self, field: &str, value: DataValue) {
        self.fields.insert(field.to_ascii_lowercase(), value);
    }
    pub fn get(&self, field: &str) -> Option<&DataValue> {
        self.fields.get(&field.to_ascii_lowercase())
    }
    pub fn id(&self) -> Option<&str> {
        self.get("Id").and_then(DataValue::text)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GovernorLimit {
    DmlStatements,
    DmlRows,
    QueryRows,
    Savepoints,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreError {
    pub code: String,
    pub message: String,
    pub fields: Vec<String>,
    pub unsupported: bool,
    /// Governor failures terminate the transaction; they are never row errors
    /// or catchable DmlException/QueryException values.
    pub governor: Option<GovernorLimit>,
}
impl StoreError {
    pub fn new(code: &str, message: impl Into<String>, fields: Vec<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            fields,
            unsupported: false,
            governor: None,
        }
    }
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self {
            code: "APX_UNSUPPORTED".into(),
            message: message.into(),
            fields: vec![],
            unsupported: true,
            governor: None,
        }
    }
    pub fn governor(limit: GovernorLimit, actual: usize) -> Self {
        let label = match limit {
            GovernorLimit::DmlStatements => "DML statements",
            GovernorLimit::DmlRows => "DML rows",
            GovernorLimit::QueryRows => "query rows",
            GovernorLimit::Savepoints => "savepoints",
        };
        Self {
            code: "LIMIT_EXCEEDED".into(),
            message: format!("System.LimitException: Too many {label}: {actual}"),
            fields: vec![],
            unsupported: false,
            governor: Some(limit),
        }
    }
    pub fn is_terminal(&self) -> bool {
        self.unsupported || self.governor.is_some()
    }
}
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for StoreError {}
#[derive(Clone, Debug)]
pub struct SaveResult {
    pub success: bool,
    pub id: Option<String>,
    pub created: bool,
    pub errors: Vec<StoreError>,
}
#[derive(Clone, Debug)]
pub enum DmlOperation {
    Insert,
    Update,
    Upsert { external_id: Option<String> },
    Delete,
    Undelete,
}
#[derive(Clone, Debug)]
pub struct BatchError {
    pub results: Vec<SaveResult>,
    pub error: StoreError,
}
#[derive(Clone, Debug)]
struct Snapshot {
    rows: BTreeMap<String, BTreeMap<String, RecordData>>,
    deleted: BTreeMap<String, BTreeMap<String, RecordData>>,
    next_id: u64,
    committed_generation: u64,
    delete_effects: BTreeMap<String, DeleteEffects>,
    deleted_roots: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Default)]
struct DeleteEffects {
    rows: Vec<(String, String)>,
    cleared: Vec<(String, String, String, String)>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Savepoint {
    pub(crate) id: u64,
}
#[derive(Clone, Debug)]
pub struct RecordStore {
    pub schema: SchemaCatalog,
    rows: BTreeMap<String, BTreeMap<String, RecordData>>,
    deleted: BTreeMap<String, BTreeMap<String, RecordData>>,
    next_id: u64,
    savepoints: BTreeMap<u64, Snapshot>,
    committed_generation: u64,
    delete_effects: BTreeMap<String, DeleteEffects>,
    deleted_roots: BTreeMap<String, String>,
    pub dml_statements: usize,
    pub dml_rows: usize,
    pub query_rows: usize,
    pub now: String,
    pub current_user: Option<String>,
    pub content_download_base_url: Option<String>,
}
impl RecordStore {
    pub fn new(schema: SchemaCatalog) -> Result<Self, StoreError> {
        Self::with_environment(schema, "2000-01-01T00:00:00.000Z", None)
    }
    pub fn with_environment(
        schema: SchemaCatalog,
        now: &str,
        current_user: Option<String>,
    ) -> Result<Self, StoreError> {
        if !valid_datetime(now) {
            return Err(StoreError::unsupported(
                "Store clock must be an ISO UTC Datetime",
            ));
        }
        let mut s = Self {
            schema,
            rows: BTreeMap::new(),
            deleted: BTreeMap::new(),
            next_id: 1,
            savepoints: BTreeMap::new(),
            committed_generation: 0,
            delete_effects: BTreeMap::new(),
            deleted_roots: BTreeMap::new(),
            dml_statements: 0,
            dml_rows: 0,
            query_rows: 0,
            now: canonical_datetime(now)?,
            current_user,
            content_download_base_url: None,
        };
        s.seed_metadata()?;
        Ok(s)
    }
    pub fn records(&self, object: &str, all_rows: bool) -> Vec<&RecordData> {
        let key = object.to_ascii_lowercase();
        let mut rows = self
            .rows
            .get(&key)
            .map(|r| r.values().collect())
            .unwrap_or_else(Vec::new);
        if all_rows {
            if let Some(deleted) = self.deleted.get(&key) {
                rows.extend(deleted.values());
            }
        }
        rows
    }
    pub fn find(&self, object: &str, id: &str) -> Option<&RecordData> {
        self.rows.get(&object.to_ascii_lowercase())?.get(id)
    }
    pub fn find_any(&self, id: &str) -> Option<&RecordData> {
        self.rows.values().find_map(|r| r.get(id))
    }
    pub fn savepoint(&mut self) -> Result<Savepoint, StoreError> {
        if self.savepoints.len() >= 5 {
            return Err(StoreError::governor(
                GovernorLimit::Savepoints,
                self.savepoints.len() + 1,
            ));
        }
        let id = NEXT_SAVEPOINT.fetch_add(1, Ordering::Relaxed);
        self.savepoints.insert(id, self.snapshot());
        Ok(Savepoint { id })
    }
    pub fn rollback(&mut self, point: Savepoint) -> Result<(), StoreError> {
        let s = self.savepoints.get(&point.id).cloned().ok_or_else(|| {
            StoreError::new(
                "INVALID_SAVEPOINT",
                "Savepoint is absent or invalidated",
                vec![],
            )
        })?;
        if s.committed_generation != self.committed_generation {
            return Err(StoreError::unsupported("Rollback across a committed independent transaction requires cross-transaction merge semantics"));
        }
        self.restore(s);
        self.savepoints.retain(|id, _| *id <= point.id);
        Ok(())
    }
    pub fn release_savepoint(&mut self, point: Savepoint) -> Result<(), StoreError> {
        if self.savepoints.remove(&point.id).is_none() {
            return Err(StoreError::new(
                "INVALID_SAVEPOINT",
                "Savepoint is absent or invalidated",
                vec![],
            ));
        }
        Ok(())
    }
    pub fn reset_limits(&mut self) {
        self.dml_statements = 0;
        self.dml_rows = 0;
        self.query_rows = 0;
    }
    pub fn fork_transaction(&self) -> Self {
        let mut child = self.clone();
        child.savepoints.clear();
        child.reset_limits();
        child
    }
    pub fn merge_committed(&mut self, child: Self) {
        self.rows = child.rows;
        self.deleted = child.deleted;
        self.delete_effects = child.delete_effects;
        self.deleted_roots = child.deleted_roots;
        self.next_id = self.next_id.max(child.next_id);
        self.committed_generation = self.committed_generation.saturating_add(1);
    }
    /// Build the full before-trigger image without checking required fields yet.
    /// Defaults and previous fields are visible to triggers; formulas remain unsupported.
    pub fn draft_record(&self, input: &RecordData, update: bool) -> Result<RecordData, StoreError> {
        let schema = self
            .schema
            .object(&input.object)
            .ok_or_else(|| StoreError::unsupported("Unknown DML object"))?;
        let mut row = if update {
            self.find(&input.object, required_id(input)?)
                .cloned()
                .ok_or_else(|| {
                    StoreError::new(
                        "INVALID_CROSS_REFERENCE_KEY",
                        "DML target not found",
                        vec!["Id".into()],
                    )
                })?
        } else {
            RecordData::new(&schema.name)
        };
        for (field, value) in &input.fields {
            row.set(field, value.clone());
        }
        if !update {
            for field in schema.fields.values() {
                if field.writable && row.get(&field.name).is_none() {
                    if let Some(raw) = &field.default_value {
                        row.set(&field.name, parse_default(field, raw)?);
                    } else if field.kind == FieldKind::Boolean && field.writable {
                        row.set(&field.name, DataValue::Boolean(false));
                    }
                }
            }
        }
        Ok(row)
    }
    /// Transaction snapshot contains owned values. Failed batches cannot mutate earlier rows.
    /// Callers must execute supported before/after triggers around this operation and roll back
    /// their surrounding savepoint if a trigger fails; this module does not skip trigger execution.
    pub fn apply(
        &mut self,
        operation: DmlOperation,
        records: &[RecordData],
        all_or_none: bool,
    ) -> Result<Vec<SaveResult>, BatchError> {
        self.apply_chunk(operation, records, all_or_none, true)
    }
    pub fn apply_chunk(
        &mut self,
        operation: DmlOperation,
        records: &[RecordData],
        all_or_none: bool,
        count_statement: bool,
    ) -> Result<Vec<SaveResult>, BatchError> {
        let snapshot = self.snapshot();
        if count_statement {
            self.dml_statements += 1;
        }
        self.dml_rows = self.dml_rows.saturating_add(records.len());
        if self.dml_statements > 150 || self.dml_rows > 10_000 {
            return Err(BatchError {
                results: vec![],
                error: if self.dml_statements > 150 {
                    StoreError::governor(GovernorLimit::DmlStatements, self.dml_statements)
                } else {
                    StoreError::governor(GovernorLimit::DmlRows, self.dml_rows)
                },
            });
        }
        let mut seen = BTreeSet::new();
        let mut results = vec![];
        for r in records {
            let before_row = if ["contentversion", "contentdistribution", "emailmessage"]
                .contains(&r.object.to_ascii_lowercase().as_str())
            {
                Some(self.snapshot())
            } else {
                None
            };
            let mut groups = BTreeSet::new();
            if r.object.eq_ignore_ascii_case("PermissionSetGroupComponent") {
                if let Some(id) = r.get("PermissionSetGroupId").and_then(DataValue::text) {
                    groups.insert(id.to_string());
                }
                if let Some(id) = r
                    .id()
                    .and_then(|id| self.find(&r.object, id))
                    .and_then(|r| r.get("PermissionSetGroupId"))
                    .and_then(DataValue::text)
                {
                    groups.insert(id.to_string());
                }
            }
            let outcome = if r.id().is_some_and(|id| !seen.insert(id.to_string())) {
                Err(StoreError::new(
                    "DUPLICATE_ID_IN_LIST",
                    "Duplicate record Id in DML input",
                    vec!["Id".into()],
                ))
            } else {
                self.apply_one(&operation, r)
            };
            if outcome.is_err() {
                if let Some(snapshot) = before_row {
                    self.restore(snapshot);
                }
            }
            if outcome.is_ok() {
                for id in groups {
                    if let Some(group) = self
                        .rows
                        .get_mut("permissionsetgroup")
                        .and_then(|rows| rows.get_mut(&id))
                    {
                        group.set("Status", DataValue::Text("Outdated".into()));
                    }
                }
            }
            match outcome {
                Ok((id, created)) => results.push(SaveResult {
                    success: true,
                    id: Some(id),
                    created,
                    errors: vec![],
                }),
                Err(e) => results.push(SaveResult {
                    success: false,
                    id: r.id().map(str::to_string),
                    created: false,
                    errors: vec![e],
                }),
            };
        }
        // Unsupported operations never turn into ordinary partial failures that Apex can swallow.
        if let Some(error) = results
            .iter()
            .flat_map(|r| &r.errors)
            .find(|e| e.is_terminal())
            .cloned()
        {
            self.restore(snapshot);
            return Err(BatchError { results, error });
        }
        if all_or_none {
            if let Some(error) = results.iter().flat_map(|r| &r.errors).next().cloned() {
                self.restore(snapshot);
                return Err(BatchError { results, error });
            }
        }
        Ok(results)
    }
    fn apply_one(
        &mut self,
        operation: &DmlOperation,
        input: &RecordData,
    ) -> Result<(String, bool), StoreError> {
        let object = self
            .schema
            .object(&input.object)
            .ok_or_else(|| StoreError::unsupported(format!("Unknown object {}", input.object)))?
            .clone();
        if (object.name.eq_ignore_ascii_case("ContentDocument")
            && matches!(
                operation,
                DmlOperation::Insert | DmlOperation::Upsert { .. }
            ))
            || (object.name.eq_ignore_ascii_case("ContentVersion")
                && matches!(operation, DmlOperation::Delete | DmlOperation::Undelete))
        {
            return Err(StoreError::new(
                "INVALID_OPERATION",
                format!("Requested operation is not supported on {}", object.name),
                vec![],
            ));
        }
        if object.read_only {
            return Err(StoreError::new(
                "INVALID_OPERATION",
                format!("{} cannot be changed using DML", object.name),
                vec![],
            ));
        }
        if object.platform_event {
            return Err(StoreError::unsupported(
                "Platform events require event publication and delivery semantics",
            ));
        }
        if let Some(b) = self.schema.blockers_for(&object.name, "dml").first() {
            return Err(StoreError::unsupported(format!(
                "{} ({})",
                b.reason, b.path
            )));
        }
        let key = object.name.to_ascii_lowercase();
        let mut operation = operation.clone();
        let mut input = input.clone();
        if let DmlOperation::Upsert { external_id } = operation {
            if input.id().is_some() {
                operation = DmlOperation::Update;
            } else if let Some(field) = external_id {
                let f = object.field(&field).ok_or_else(|| {
                    StoreError::new(
                        "INVALID_FIELD",
                        format!("Unknown upsert field {field}"),
                        vec![field.clone()],
                    )
                })?;
                if !f.external_id && !f.unique {
                    return Err(StoreError::new(
                        "INVALID_FIELD",
                        "Upsert field must be external Id or unique",
                        vec![field],
                    ));
                }
                let value = input.get(&field).unwrap_or(&DataValue::Null);
                let mut matches = vec![];
                if !value.is_null() {
                    for row in self.records(&object.name, false) {
                        if value_equal(
                            row.get(&field).unwrap_or(&DataValue::Null),
                            value,
                            f.case_sensitive,
                        )? {
                            matches.push(row.id().unwrap().to_string());
                        }
                    }
                }
                if matches.len() > 1 {
                    return Err(StoreError::new(
                        "DUPLICATE_EXTERNAL_ID",
                        "External Id matched multiple records",
                        vec![field],
                    ));
                }
                if let Some(id) = matches.first() {
                    input.set("Id", DataValue::Text(id.clone()));
                    operation = DmlOperation::Update;
                } else {
                    operation = DmlOperation::Insert;
                }
            } else {
                operation = DmlOperation::Insert;
            }
        }
        match operation {
            DmlOperation::Insert => {
                if input.id().is_some() {
                    return Err(StoreError::new(
                        "INVALID_FIELD_FOR_INSERT_UPDATE",
                        "Cannot specify Id in insert",
                        vec!["Id".into()],
                    ));
                }
                let mut row = self.validate_record(&input, None, true)?;
                let id = self.allocate_id(&object.key_prefix);
                row.set("Id", DataValue::Text(id.clone()));
                row.set("IsDeleted", DataValue::Boolean(false));
                row.set("CreatedDate", DataValue::DateTime(self.now.clone()));
                row.set("LastModifiedDate", DataValue::DateTime(self.now.clone()));
                row.set("SystemModstamp", DataValue::DateTime(self.now.clone()));
                if let Some(user) = &self.current_user {
                    row.set("CreatedById", DataValue::Text(user.clone()));
                    row.set("LastModifiedById", DataValue::Text(user.clone()));
                    if row.get("OwnerId").is_none() {
                        row.set("OwnerId", DataValue::Text(user.clone()));
                    }
                }
                if let Some(format) = &object.auto_number_format {
                    row.set(
                        "Name",
                        DataValue::Text(format_auto_number(format, self.next_id - 1)?),
                    );
                }
                self.prepare_content_record(&mut row, None)?;
                self.rows
                    .entry(key)
                    .or_default()
                    .insert(id.clone(), row.clone());
                self.derive_email_relations(&row, None)?;
                Ok((id, true))
            }
            DmlOperation::Update => {
                let id = required_id(&input)?.to_string();
                let previous = self.find(&object.name, &id).cloned().ok_or_else(|| {
                    StoreError::new(
                        "INVALID_CROSS_REFERENCE_KEY",
                        "Update target does not exist",
                        vec!["Id".into()],
                    )
                })?;
                let mut row = self.validate_record(&input, Some(&previous), false)?;
                row.set("LastModifiedDate", DataValue::DateTime(self.now.clone()));
                row.set("SystemModstamp", DataValue::DateTime(self.now.clone()));
                if let Some(user) = &self.current_user {
                    row.set("LastModifiedById", DataValue::Text(user.clone()));
                }
                self.prepare_content_record(&mut row, Some(&previous))?;
                self.rows
                    .entry(key)
                    .or_default()
                    .insert(id.clone(), row.clone());
                self.derive_email_relations(&row, Some(&previous))?;
                Ok((id, false))
            }
            DmlOperation::Delete => {
                let id = required_id(&input)?.to_string();
                if self
                    .records(&object.name, false)
                    .iter()
                    .all(|r| r.id() != Some(&id))
                {
                    return Err(StoreError::new(
                        "ENTITY_IS_DELETED",
                        "Delete target is absent or already deleted",
                        vec!["Id".into()],
                    ));
                }
                self.delete_graph(&key, &id)?;
                Ok((id, false))
            }
            DmlOperation::Undelete => {
                let id = required_id(&input)?.to_string();
                let mut row = self
                    .deleted
                    .get_mut(&key)
                    .and_then(|r| r.remove(&id))
                    .ok_or_else(|| {
                        StoreError::new(
                            "UNDELETE_FAILED",
                            "Record is not in the recycle bin",
                            vec!["Id".into()],
                        )
                    })?;
                row.set("IsDeleted", DataValue::Boolean(false));
                self.rows.entry(key).or_default().insert(id.clone(), row);
                self.deleted_roots.remove(&id);
                if let Some(effects) = self.delete_effects.remove(&id) {
                    for (object, child) in effects.rows {
                        if child == id || self.deleted_roots.get(&child) != Some(&id) {
                            continue;
                        }
                        if let Some(mut row) = self
                            .deleted
                            .get_mut(&object)
                            .and_then(|rows| rows.remove(&child))
                        {
                            row.set("IsDeleted", DataValue::Boolean(false));
                            self.rows
                                .entry(object)
                                .or_default()
                                .insert(child.clone(), row);
                            self.deleted_roots.remove(&child);
                        }
                    }
                    for (object, row, field, parent) in effects.cleared {
                        if let Some(row) = self
                            .rows
                            .get_mut(&object)
                            .and_then(|rows| rows.get_mut(&row))
                        {
                            if row.get(&field).is_none_or(DataValue::is_null) {
                                row.set(&field, DataValue::Text(parent));
                            }
                        }
                    }
                }
                Ok((id, false))
            }
            DmlOperation::Upsert { .. } => unreachable!(),
        }
    }
    fn derive_email_relations(
        &mut self,
        row: &RecordData,
        previous: Option<&RecordData>,
    ) -> Result<(), StoreError> {
        if !row.object.eq_ignore_ascii_case("EmailMessage") {
            return Ok(());
        }
        for (field, role) in [
            ("ToIds", "ToAddress"),
            ("CcIds", "CcAddress"),
            ("BccIds", "BccAddress"),
        ] {
            if let Some(previous) = previous {
                if row.get(field) != previous.get(field) {
                    return Err(StoreError::unsupported("Updating EmailMessage recipient junctions requires relationship reconciliation"));
                }
                continue;
            }
            let Some(DataValue::List(ids)) = row.get(field) else {
                continue;
            };
            let mut seen = BTreeSet::new();
            for value in ids {
                let id = value.text().ok_or_else(|| {
                    StoreError::new(
                        "INVALID_TYPE_ON_FIELD_IN_RECORD",
                        "Recipient Id must be a string",
                        vec![field.into()],
                    )
                })?;
                if !seen.insert(id) {
                    return Err(StoreError::new(
                        "DUPLICATE_VALUE",
                        "Duplicate email recipient",
                        vec![field.into()],
                    ));
                }
                let recipient = self.find_any(id).ok_or_else(|| {
                    StoreError::new(
                        "INVALID_CROSS_REFERENCE_KEY",
                        "Email recipient does not exist",
                        vec![field.into()],
                    )
                })?;
                if !["contact", "lead", "user"]
                    .contains(&recipient.object.to_ascii_lowercase().as_str())
                {
                    return Err(StoreError::new(
                        "FIELD_INTEGRITY_EXCEPTION",
                        "Email recipient must be Contact, Lead, or User",
                        vec![field.into()],
                    ));
                }
                let email = recipient.get("Email").cloned().unwrap_or(DataValue::Null);
                self.upsert_platform_record(
                    &RecordData::new("EmailMessageRelation")
                        .with("EmailMessageId", DataValue::Text(row.id().unwrap().into()))
                        .with("RelationId", DataValue::Text(id.into()))
                        .with("RelationType", DataValue::Text(role.into()))
                        .with("RelationAddress", email),
                )?;
            }
        }
        Ok(())
    }
    fn delete_graph(&mut self, object: &str, id: &str) -> Result<(), StoreError> {
        let mut effects = DeleteEffects::default();
        let mut pending = vec![(object.to_string(), id.to_string())];
        let mut visited = BTreeSet::new();
        while let Some((object, current)) = pending.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            if visited.len() > 10_000 {
                return Err(StoreError::unsupported(
                    "Cascade delete exceeds local10000-record bound",
                ));
            }
            effects.rows.push((object.clone(), current.clone()));
            for rows in self.rows.values() {
                for row in rows.values() {
                    for field in self.schema.object(&row.object).unwrap().fields.values() {
                        if field.kind != FieldKind::Reference
                            || row.get(&field.name).and_then(DataValue::text) != Some(&current)
                        {
                            continue;
                        }
                        match field.delete_behavior.as_deref() {
                            Some("Cascade") => {
                                if object == "account"
                                    && row.object.eq_ignore_ascii_case("Opportunity")
                                {
                                    if let Some(owner) =
                                        row.get("OwnerId").and_then(DataValue::text)
                                    {
                                        if let Some(user) = &self.current_user {
                                            if owner != user {
                                                return Err(StoreError::new("DELETE_FAILED","Account has an opportunity owned by another user",vec![]));
                                            }
                                        } else {
                                            return Err(StoreError::unsupported(
                                                "Cascade ownership context is unresolved",
                                            ));
                                        }
                                    }
                                }
                                if row.object.eq_ignore_ascii_case("Opportunity")
                                    && row
                                        .get("StageName")
                                        .and_then(DataValue::text)
                                        .is_some_and(|s| s.eq_ignore_ascii_case("Closed Won"))
                                {
                                    return Err(StoreError::new(
                                        "DELETE_FAILED",
                                        "Account has a closed-won opportunity",
                                        vec![],
                                    ));
                                }
                                pending.push((
                                    row.object.to_ascii_lowercase(),
                                    row.id().unwrap().into(),
                                ));
                            }
                            Some("SetNull") => effects.cleared.push((
                                row.object.to_ascii_lowercase(),
                                row.id().unwrap().into(),
                                field.name.clone(),
                                current.clone(),
                            )),
                            Some("Restrict") => {
                                return Err(StoreError::new(
                                    "DELETE_FAILED",
                                    format!(
                                        "Cannot delete record referenced by {}.{}",
                                        row.object, field.name
                                    ),
                                    vec![field.name.clone()],
                                ))
                            }
                            _ => {
                                return Err(StoreError::unsupported(format!(
                                    "Delete policy unresolved for {}.{}",
                                    row.object, field.name
                                )))
                            }
                        }
                    }
                }
            }
        }
        for (object, row, field, _) in &effects.cleared {
            if !visited.contains(row) {
                self.rows
                    .get_mut(object)
                    .unwrap()
                    .get_mut(row)
                    .unwrap()
                    .set(field, DataValue::Null);
            }
        }
        for (object, rowid) in &effects.rows {
            let mut row = self.rows.get_mut(object).unwrap().remove(rowid).unwrap();
            row.set("IsDeleted", DataValue::Boolean(true));
            self.deleted
                .entry(object.clone())
                .or_default()
                .insert(rowid.clone(), row);
            self.deleted_roots.insert(rowid.clone(), id.into());
        }
        self.delete_effects.insert(id.into(), effects);
        Ok(())
    }
    fn validate_record(
        &self,
        input: &RecordData,
        previous: Option<&RecordData>,
        insert: bool,
    ) -> Result<RecordData, StoreError> {
        let object = self.schema.object(&input.object).unwrap();
        let mut row = previous
            .cloned()
            .unwrap_or_else(|| RecordData::new(&object.name));
        for (key, value) in &input.fields {
            if key == "id" && !insert {
                continue;
            }
            let field = object.field(key).ok_or_else(|| {
                StoreError::new(
                    "INVALID_FIELD",
                    format!("Unknown field {}.{key}", object.name),
                    vec![key.clone()],
                )
            })?;
            if key == "id" && value.is_null() {
                continue;
            }
            if !field.writable && previous.and_then(|r| r.get(key)) == Some(value) {
                continue;
            }
            if !field.writable {
                return Err(StoreError::new(
                    "INVALID_FIELD_FOR_INSERT_UPDATE",
                    format!("{} is not writable", field.name),
                    vec![field.name.clone()],
                ));
            }
            let value = if matches!(value,DataValue::Text(s) if s.is_empty()) {
                DataValue::Null
            } else {
                value.clone()
            };
            self.validate_value(field, &value)?;
            row.set(key, value);
        }
        for field in object.fields.values() {
            if insert && row.get(&field.name).is_none() {
                if let Some(raw) = &field.default_value {
                    row.set(&field.name, parse_default(field, raw)?);
                } else if field.kind == FieldKind::Boolean {
                    row.set(&field.name, DataValue::Boolean(false));
                }
            }
            let value = row.get(&field.name).unwrap_or(&DataValue::Null);
            if field.required && value.is_null() {
                return Err(StoreError::new(
                    "REQUIRED_FIELD_MISSING",
                    format!("Required field missing: {}", field.name),
                    vec![field.name.clone()],
                ));
            }
            if field.unique && !value.is_null() {
                for existing in self.records(&object.name, false) {
                    if previous.and_then(RecordData::id) == existing.id() {
                        continue;
                    }
                    if value_equal(
                        existing.get(&field.name).unwrap_or(&DataValue::Null),
                        value,
                        field.case_sensitive,
                    )? {
                        return Err(StoreError::new(
                            "DUPLICATE_VALUE",
                            format!("Duplicate unique field {}", field.name),
                            vec![field.name.clone()],
                        ));
                    }
                }
            }
        }
        if object.name.eq_ignore_ascii_case("PermissionSetAssignment") {
            let permission = row.get("PermissionSetId").and_then(DataValue::text);
            let group = row.get("PermissionSetGroupId").and_then(DataValue::text);
            if permission.is_some() == group.is_some() {
                return Err(StoreError::new(
                    "INVALID_FIELD",
                    "Specify exactly one PermissionSetId or PermissionSetGroupId",
                    vec![],
                ));
            }
            if let Some(group) = group {
                if self
                    .find("PermissionSetGroup", group)
                    .and_then(|g| g.get("Status"))
                    .and_then(DataValue::text)
                    != Some("Updated")
                {
                    return Err(StoreError::new(
                        "INVALID_CROSS_REFERENCE_KEY",
                        "Permission set group calculation is not Updated",
                        vec!["PermissionSetGroupId".into()],
                    ));
                }
            }
        }
        if ["contact", "lead"].contains(&object.name.to_ascii_lowercase().as_str()) {
            let first = row.get("FirstName").and_then(DataValue::text).unwrap_or("");
            let last = row.get("LastName").and_then(DataValue::text).unwrap_or("");
            row.set(
                "Name",
                DataValue::Text(format!("{first} {last}").trim().into()),
            );
        }
        Ok(row)
    }
    /// Validate a platform-event payload without publishing or inserting it.
    pub fn validate_event(&self, input: &RecordData) -> Result<RecordData, StoreError> {
        let object = self
            .schema
            .object(&input.object)
            .ok_or_else(|| StoreError::unsupported("Unknown platform-event schema"))?;
        if !object.platform_event {
            return Err(StoreError::new(
                "INVALID_TYPE",
                "EventBus requires a platform event",
                vec![],
            ));
        }
        let mut payload = input.clone();
        let uuid = payload.fields.remove("eventuuid");
        let mut result = self.validate_record(&payload, None, true)?;
        if let Some(uuid) = uuid {
            if !matches!(&uuid,DataValue::Text(s)if s.len()==36&&s.is_ascii()&&s.bytes().enumerate().all(|(i,b)|if [8,13,18,23].contains(&i){b==b'-'}else{b.is_ascii_hexdigit()}))
            {
                return Err(StoreError::new(
                    "INVALID_FIELD",
                    "EventUuid must be a UUID",
                    vec!["EventUuid".into()],
                ));
            }
            result.set("EventUuid", uuid);
        }
        Ok(result)
    }
    fn validate_value(&self, field: &FieldSchema, value: &DataValue) -> Result<(), StoreError> {
        if value.is_null() {
            return Ok(());
        }
        let valid = match (&field.kind, value) {
            (
                FieldKind::Text
                | FieldKind::Picklist
                | FieldKind::MultiPicklist
                | FieldKind::Id
                | FieldKind::Reference,
                DataValue::Text(_),
            ) => true,
            (FieldKind::Boolean, DataValue::Boolean(_)) => true,
            (FieldKind::Integer, DataValue::Number(n)) => {
                n.is_integer() && n.to_integer().to_i32().is_some()
            }
            (FieldKind::Long, DataValue::Number(n)) => {
                n.is_integer() && n.to_integer().to_i64().is_some()
            }
            (FieldKind::Decimal, DataValue::Number(_)) => true,
            (FieldKind::Date, DataValue::Date(s)) => valid_date(s),
            (FieldKind::DateTime, DataValue::DateTime(s)) => valid_datetime(s),
            (FieldKind::Blob, DataValue::Binary(b)) => b.len() <= 4 * 1024 * 1024,
            (FieldKind::StringList, DataValue::List(values)) => {
                values.iter().all(|v| matches!(v, DataValue::Text(_)))
            }
            (FieldKind::Unsupported(t), _) => {
                return Err(StoreError::unsupported(format!(
                    "Field type {t} is unsupported"
                )))
            }
            _ => false,
        };
        if !valid {
            return Err(StoreError::new(
                "INVALID_TYPE_ON_FIELD_IN_RECORD",
                format!("Invalid value for {}", field.name),
                vec![field.name.clone()],
            ));
        }
        if let DataValue::Number(number) = value {
            if let Some(precision) = field.precision {
                let scale = field.scale.unwrap_or(0);
                if scale > precision || precision > 38 {
                    return Err(StoreError::unsupported(
                        "Field numeric precision is unsupported",
                    ));
                }
                let bound =
                    BigRational::from_integer(num_bigint::BigInt::from(10).pow(precision - scale));
                if number >= &bound || number <= &(-bound) {
                    return Err(StoreError::new(
                        "NUMBER_OUTSIDE_VALID_RANGE",
                        format!("{} exceeds field precision", field.name),
                        vec![field.name.clone()],
                    ));
                }
                let scaled =
                    number * BigRational::from_integer(num_bigint::BigInt::from(10).pow(scale));
                if !scaled.is_integer() {
                    // Storage rounding depends on platform/field policy. Until
                    // qualified, never return a successful unrounded value.
                    return Err(StoreError::unsupported(format!(
                        "Numeric storage rounding for {} at scale {scale} is not qualified",
                        field.name
                    )));
                }
            }
        }
        if let DataValue::Text(text) = value {
            if field
                .length
                .is_some_and(|l| text.encode_utf16().count() > l)
            {
                return Err(StoreError::new(
                    "STRING_TOO_LONG",
                    format!("{} exceeds field length", field.name),
                    vec![field.name.clone()],
                ));
            }
            if field.restricted_picklist {
                let values = if field.kind == FieldKind::MultiPicklist {
                    text.split(';').collect::<Vec<_>>()
                } else {
                    vec![text.as_str()]
                };
                if values
                    .iter()
                    .any(|v| !field.picklist_values.iter().any(|x| x == v))
                {
                    return Err(StoreError::new(
                        "INVALID_OR_NULL_FOR_RESTRICTED_PICKLIST",
                        "Value is not in the restricted picklist",
                        vec![field.name.clone()],
                    ));
                }
            }
            if field.kind == FieldKind::Reference {
                let target = self.find_any(text).ok_or_else(|| {
                    StoreError::new(
                        "INVALID_CROSS_REFERENCE_KEY",
                        format!("Reference {} does not exist", text),
                        vec![field.name.clone()],
                    )
                })?;
                if !field
                    .reference_to
                    .iter()
                    .any(|t| t.eq_ignore_ascii_case(&target.object))
                {
                    return Err(StoreError::new(
                        "FIELD_INTEGRITY_EXCEPTION",
                        "Reference has wrong object type",
                        vec![field.name.clone()],
                    ));
                }
            }
        }
        Ok(())
    }
    pub fn set_created_date(&mut self, id: &str, date: &str) -> Result<(), StoreError> {
        if !valid_datetime(date) {
            return Err(StoreError::new(
                "INVALID_DATETIME",
                "Invalid CreatedDate",
                vec!["CreatedDate".into()],
            ));
        }
        for rows in self.rows.values_mut() {
            if let Some(row) = rows.get_mut(id) {
                row.set(
                    "CreatedDate",
                    DataValue::DateTime(canonical_datetime(date)?),
                );
                return Ok(());
            }
        }
        Err(StoreError::new(
            "INVALID_CROSS_REFERENCE_KEY",
            "Record not found",
            vec!["Id".into()],
        ))
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            rows: self.rows.clone(),
            deleted: self.deleted.clone(),
            next_id: self.next_id,
            committed_generation: self.committed_generation,
            delete_effects: self.delete_effects.clone(),
            deleted_roots: self.deleted_roots.clone(),
        }
    }
    fn restore(&mut self, s: Snapshot) {
        self.rows = s.rows;
        self.deleted = s.deleted;
        self.delete_effects = s.delete_effects;
        self.deleted_roots = s.deleted_roots;
        self.next_id = self.next_id.max(s.next_id);
    }
    fn allocate_id(&mut self, prefix: &str) -> String {
        let id = format!("{prefix}{:012}", self.next_id);
        self.next_id += 1;
        id18(&id)
    }
    fn seed(&mut self, mut row: RecordData) -> String {
        let prefix = self
            .schema
            .object(&row.object)
            .map(|o| o.key_prefix.clone())
            .unwrap_or_else(|| "m00".into());
        let id = self.allocate_id(&prefix);
        row.set("Id", DataValue::Text(id.clone()));
        self.rows
            .entry(row.object.to_ascii_lowercase())
            .or_default()
            .insert(id.clone(), row);
        id
    }
    /// Install explicit fixture data atomically without consuming Apex transaction limits.
    pub fn seed_environment(
        &mut self,
        environment: &crate::environment::Environment,
    ) -> Result<(), StoreError> {
        self.seed_environment_for_test(environment, false)
    }
    pub fn seed_environment_for_test(
        &mut self,
        environment: &crate::environment::Environment,
        see_all_data: bool,
    ) -> Result<(), StoreError> {
        self.content_download_base_url = environment
            .capabilities
            .get("contentDownloadBaseUrl")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let previous = self.rows.clone();
        let allowed =
            |row: &&RecordData| see_all_data || !crate::access::business_object(&row.object);
        let result = (|| {
            for row in environment.records.iter().filter(allowed) {
                let schema = self
                    .schema
                    .object(&row.object)
                    .ok_or_else(|| StoreError::unsupported("Environment object schema missing"))?;
                let id = row
                    .id()
                    .ok_or_else(|| StoreError::unsupported("Environment Id missing"))?;
                if !id.starts_with(&schema.key_prefix) || self.find_any(id).is_some() {
                    return Err(StoreError::unsupported(
                        "Environment Id prefix mismatch or collision",
                    ));
                }
                self.rows
                    .entry(row.object.to_ascii_lowercase())
                    .or_default()
                    .insert(id.into(), row.clone());
            }
            for row in environment.records.iter().filter(allowed) {
                let object = self.schema.object(&row.object).unwrap();
                for (name, value) in &row.fields {
                    let field = object.field(name).ok_or_else(|| {
                        StoreError::unsupported("Environment field schema missing")
                    })?;
                    self.validate_value(field, value)?;
                }
            }
            for assignment in &environment.assignments {
                if assignment.unresolved {
                    continue;
                }
                let permission = self
                    .records("PermissionSet", false)
                    .into_iter()
                    .find(|r| {
                        r.get("Name").and_then(DataValue::text).is_some_and(|name| {
                            name.eq_ignore_ascii_case(&assignment.permission_set_name)
                        })
                    })
                    .ok_or_else(|| {
                        StoreError::unsupported(format!(
                            "Assignment references absent permission set {}",
                            assignment.permission_set_name
                        ))
                    })?;
                let id = permission.id().unwrap().to_string();
                if self.find("User", &assignment.assignee_id).is_none() {
                    return Err(StoreError::unsupported("Assignment references absent User"));
                }
                self.seed(
                    RecordData::new("PermissionSetAssignment")
                        .with(
                            "AssigneeId",
                            DataValue::Text(assignment.assignee_id.clone()),
                        )
                        .with("PermissionSetId", DataValue::Text(id)),
                );
            }
            Ok(())
        })();
        if result.is_err() {
            self.rows = previous;
        }
        result
    }
    /// Trusted platform services persist generated job/content state without Apex DML limits.
    /// The call site must implement the actual service; this is never exposed as an Apex bypass.
    pub fn upsert_platform_record(&mut self, input: &RecordData) -> Result<String, StoreError> {
        let schema = self
            .schema
            .object(&input.object)
            .ok_or_else(|| StoreError::unsupported("Platform record object schema missing"))?
            .clone();
        let id = if let Some(id) = input.id() {
            if !id.is_ascii()
                || !id.starts_with(&schema.key_prefix)
                || id.len() != 18
                || id18(&id[..15]) != id
            {
                return Err(StoreError::unsupported("Invalid platform record Id"));
            }
            id.to_string()
        } else {
            self.allocate_id(&schema.key_prefix)
        };
        let mut row = self
            .find(&schema.name, &id)
            .cloned()
            .unwrap_or_else(|| RecordData::new(&schema.name));
        for (name, value) in &input.fields {
            let field = schema.field(name).ok_or_else(|| {
                StoreError::unsupported(format!("Unknown platform field {}.{name}", schema.name))
            })?;
            self.validate_value(field, value)?;
            row.set(name, value.clone());
        }
        row.set("Id", DataValue::Text(id.clone()));
        if row.get("CreatedDate").is_none() {
            row.set("CreatedDate", DataValue::DateTime(self.now.clone()));
        }
        self.rows
            .entry(schema.name.to_ascii_lowercase())
            .or_default()
            .insert(id.clone(), row);
        Ok(id)
    }
    fn seed_metadata(&mut self) -> Result<(), StoreError> {
        for metadata in self.schema.metadata_records.clone() {
            let mut row = RecordData::new(&metadata.object)
                .with(
                    "DeveloperName",
                    DataValue::Text(metadata.developer_name.clone()),
                )
                .with("MasterLabel", DataValue::Text(metadata.label.clone()))
                .with("Label", DataValue::Text(metadata.label))
                .with("QualifiedApiName", DataValue::Text(metadata.developer_name));
            for (field, v) in metadata.values {
                row.set(
                    &field,
                    match v {
                        MetadataScalar::Null => DataValue::Null,
                        MetadataScalar::Binary(bytes) => DataValue::Binary(bytes),
                        MetadataScalar::Boolean(v) => DataValue::Boolean(v),
                        MetadataScalar::Number(n) => DataValue::number(&n)?,
                        MetadataScalar::Text(s) => DataValue::Text(s),
                    },
                );
            }
            self.seed(row);
        }
        for permission in self.schema.permission_sets.clone().values() {
            let parent = self.seed(
                RecordData::new("PermissionSet")
                    .with("IsOwnedByProfile", DataValue::Boolean(false))
                    .with("Name", DataValue::Text(permission.name.clone()))
                    .with("Label", DataValue::Text(permission.label.clone())),
            );
            for (qualified, grant) in &permission.fields {
                let Some((object, field)) = qualified.split_once('.') else {
                    return Err(StoreError::unsupported(
                        "Permission field must be qualified",
                    ));
                };
                let canonical = self
                    .schema
                    .object(object)
                    .and_then(|o| o.field(field).map(|f| (o.name.clone(), f.name.clone())))
                    .or_else(|| {
                        permission
                            .field_api_names
                            .get(qualified)
                            .and_then(|name| name.split_once('.'))
                            .map(|(object, field)| (object.to_string(), field.to_string()))
                    })
                    .ok_or_else(|| {
                        StoreError::unsupported(format!(
                            "Permission field {qualified} has no schema"
                        ))
                    })?;
                self.seed(
                    RecordData::new("FieldPermissions")
                        .with("ParentId", DataValue::Text(parent.clone()))
                        .with("SObjectType", DataValue::Text(canonical.0.clone()))
                        .with(
                            "Field",
                            DataValue::Text(format!("{}.{}", canonical.0, canonical.1)),
                        )
                        .with("PermissionsRead", DataValue::Boolean(grant.read))
                        .with("PermissionsEdit", DataValue::Boolean(grant.edit)),
                );
            }
            for (object, grant) in &permission.objects {
                let canonical = self
                    .schema
                    .object(object)
                    .map(|o| o.name.clone())
                    .or_else(|| permission.object_api_names.get(object).cloned())
                    .ok_or_else(|| {
                        StoreError::unsupported(format!("Permission object {object} has no schema"))
                    })?;
                self.seed(
                    RecordData::new("ObjectPermissions")
                        .with("ParentId", DataValue::Text(parent.clone()))
                        .with("SObjectType", DataValue::Text(canonical))
                        .with("PermissionsRead", DataValue::Boolean(grant.read))
                        .with("PermissionsCreate", DataValue::Boolean(grant.create))
                        .with("PermissionsEdit", DataValue::Boolean(grant.edit))
                        .with("PermissionsDelete", DataValue::Boolean(grant.delete))
                        .with(
                            "PermissionsViewAllRecords",
                            DataValue::Boolean(grant.view_all),
                        )
                        .with(
                            "PermissionsModifyAllRecords",
                            DataValue::Boolean(grant.modify_all),
                        ),
                );
            }
        }
        for object in self.schema.objects.clone().values() {
            let entity = self.seed(
                RecordData::new("EntityDefinition")
                    .with("QualifiedApiName", DataValue::Text(object.name.clone()))
                    .with("DeveloperName", DataValue::Text(object.name.clone()))
                    .with("Label", DataValue::Text(object.name.clone())),
            );
            for field in object.fields.values() {
                self.seed(
                    RecordData::new("FieldDefinition")
                        .with("QualifiedApiName", DataValue::Text(field.name.clone()))
                        .with("DeveloperName", DataValue::Text(field.name.clone()))
                        .with("Label", DataValue::Text(field.name.clone()))
                        .with("DataType", DataValue::Text(field.apex_type().into()))
                        .with("EntityDefinitionId", DataValue::Text(entity.clone()))
                        .with("IsCalculated", DataValue::Boolean(field.formula.is_some()))
                        .with(
                            "RelationshipName",
                            field
                                .relationship_name
                                .as_ref()
                                .map(|s| DataValue::Text(s.clone()))
                                .unwrap_or(DataValue::Null),
                        ),
                );
            }
        }
        let mut links = vec![];
        for object in self
            .schema
            .objects
            .values()
            .filter(|o| o.custom_metadata || o.name == "AppMenuItem")
        {
            for row in self.records(&object.name, false) {
                for field in object
                    .fields
                    .values()
                    .filter(|f| f.kind == FieldKind::Reference)
                {
                    let Some(reference) = row.get(&field.name).and_then(DataValue::text) else {
                        continue;
                    };
                    if self.find_any(reference).is_some() {
                        continue;
                    }
                    let target = field.reference_to.first().ok_or_else(|| {
                        StoreError::unsupported("Metadata reference target missing")
                    })?;
                    let name = reference.rsplit('.').next().unwrap_or(reference);
                    let matches = self
                        .records(target, false)
                        .into_iter()
                        .filter(|r| {
                            r.get(
                                if ["EntityDefinition", "FieldDefinition"]
                                    .contains(&target.as_str())
                                {
                                    "QualifiedApiName"
                                } else {
                                    "DeveloperName"
                                },
                            )
                            .and_then(DataValue::text)
                            .is_some_and(|n| n.eq_ignore_ascii_case(name))
                                && (target != "FieldDefinition"
                                    || !reference.contains('.')
                                    || r.get("EntityDefinitionId")
                                        .and_then(DataValue::text)
                                        .and_then(|id| self.find("EntityDefinition", id))
                                        .and_then(|r| r.get("QualifiedApiName"))
                                        .and_then(DataValue::text)
                                        .is_some_and(|n| {
                                            n.eq_ignore_ascii_case(
                                                reference.split('.').next().unwrap_or(""),
                                            )
                                        }))
                        })
                        .collect::<Vec<_>>();
                    if matches.len() != 1 {
                        return Err(StoreError::unsupported(format!(
                            "Metadata reference {}.{} is unresolved/ambiguous: {reference}",
                            object.name, field.name
                        )));
                    }
                    links.push((
                        object.name.to_ascii_lowercase(),
                        row.id().unwrap().to_string(),
                        field.name.clone(),
                        matches[0].id().unwrap().to_string(),
                    ));
                }
            }
        }
        for (object, row, field, target) in links {
            self.rows
                .get_mut(&object)
                .unwrap()
                .get_mut(&row)
                .unwrap()
                .set(&field, DataValue::Text(target));
        }
        Ok(())
    }
}
fn required_id(row: &RecordData) -> Result<&str, StoreError> {
    row.id()
        .ok_or_else(|| StoreError::new("MISSING_ARGUMENT", "Id is required", vec!["Id".into()]))
}
pub fn value_equal(a: &DataValue, b: &DataValue, case_sensitive: bool) -> Result<bool, StoreError> {
    match (a, b) {
        (DataValue::Text(a), DataValue::Text(b)) if !case_sensitive => {
            if !a.is_ascii() || !b.is_ascii() {
                return Err(StoreError::unsupported(
                    "Non-ASCII collation requires an explicit locale policy",
                ));
            }
            Ok(a.eq_ignore_ascii_case(b))
        }
        (DataValue::List(_) | DataValue::Record(_), _)
        | (_, DataValue::List(_) | DataValue::Record(_)) => Err(StoreError::unsupported(
            "Composite value equality is not a query scalar comparison",
        )),
        _ => Ok(a == b),
    }
}
fn parse_default(field: &FieldSchema, raw: &str) -> Result<DataValue, StoreError> {
    match field.kind {
        FieldKind::Boolean => match raw {
            "true" => Ok(DataValue::Boolean(true)),
            "false" => Ok(DataValue::Boolean(false)),
            _ => Err(StoreError::unsupported(
                "Checkbox default expression requires formula evaluation",
            )),
        },
        FieldKind::Integer | FieldKind::Long | FieldKind::Decimal => DataValue::number(raw),
        FieldKind::Text | FieldKind::Picklist => raw
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .map(|s| DataValue::Text(s.into()))
            .ok_or_else(|| {
                StoreError::unsupported("Text default expression requires formula evaluation")
            }),
        _ => Err(StoreError::unsupported(
            "Field default expression requires formula evaluation",
        )),
    }
}
fn format_auto_number(format: &str, n: u64) -> Result<String, StoreError> {
    let Some(start) = format.find('{') else {
        return Err(StoreError::unsupported(
            "AutoNumber display format is unsupported",
        ));
    };
    let Some(end) = format[start..].find('}').map(|i| i + start) else {
        return Err(StoreError::unsupported(
            "AutoNumber display format is unsupported",
        ));
    };
    let digits = &format[start + 1..end];
    if digits.is_empty()
        || digits.len() > 20
        || !digits.bytes().all(|b| b == b'0')
        || format[end + 1..].contains('{')
    {
        return Err(StoreError::unsupported(
            "AutoNumber date/multiple tokens are unsupported",
        ));
    }
    Ok(format!(
        "{}{:0width$}{}",
        &format[..start],
        n,
        &format[end + 1..],
        width = digits.len()
    ))
}
pub fn id18(id: &str) -> String {
    if id.len() != 15 || !id.is_ascii() {
        return id.into();
    }
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ012345";
    let mut suffix = String::new();
    for group in id.as_bytes().chunks(5) {
        let mut bits = 0;
        for (i, b) in group.iter().enumerate() {
            if b.is_ascii_uppercase() {
                bits |= 1 << i;
            }
        }
        suffix.push(alphabet[bits] as char);
    }
    format!("{id}{suffix}")
}
pub fn valid_date(s: &str) -> bool {
    if !s.is_ascii() || s.len() != 10 || s.as_bytes()[4] != b'-' || s.as_bytes()[7] != b'-' {
        return false;
    }
    let Ok(y) = s[..4].parse::<u32>() else {
        return false;
    };
    let Ok(m) = s[5..7].parse::<u32>() else {
        return false;
    };
    let Ok(d) = s[8..].parse::<u32>() else {
        return false;
    };
    if !(1..=9999).contains(&y) || !(1..=12).contains(&m) {
        return false;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    d >= 1 && d <= days[m as usize - 1]
}
pub fn valid_datetime(s: &str) -> bool {
    if !s.is_ascii()
        || s.len() < 20
        || !valid_date(&s[..10])
        || s.as_bytes()[10] != b'T'
        || s.as_bytes()[13] != b':'
        || s.as_bytes()[16] != b':'
        || !s.ends_with('Z')
    {
        return false;
    }
    let number = |s: &str, limit: u32| s.parse::<u32>().is_ok_and(|n| n < limit);
    if !number(&s[11..13], 24) || !number(&s[14..16], 60) || !number(&s[17..19], 60) {
        return false;
    }
    s.len() == 20
        || (s.as_bytes()[19] == b'.'
            && s.len() <= 29
            && s[20..s.len() - 1].bytes().all(|b| b.is_ascii_digit())
            && s.len() > 21)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parent_delete_cascades_and_restores_children_but_cases_restrict() {
        let mut store = RecordStore::new(SchemaCatalog::standard()).unwrap();
        let id = store
            .apply(DmlOperation::Insert, &[account("A")], true)
            .unwrap()[0]
            .id
            .clone()
            .unwrap();
        store
            .apply(
                DmlOperation::Insert,
                &[RecordData::new("Contact")
                    .with("LastName", DataValue::Text("Child".into()))
                    .with("AccountId", DataValue::Text(id.clone()))],
                true,
            )
            .unwrap();
        let parent = RecordData::new("Account").with("Id", DataValue::Text(id.clone()));
        store
            .apply(DmlOperation::Delete, &[parent.clone()], true)
            .unwrap();
        assert!(store.records("Contact", false).is_empty());
        assert_eq!(store.records("Contact", true).len(), 1);
        store
            .apply(DmlOperation::Undelete, &[parent.clone()], true)
            .unwrap();
        assert_eq!(store.records("Contact", false).len(), 1);
        store
            .apply(
                DmlOperation::Insert,
                &[RecordData::new("Case").with("AccountId", DataValue::Text(id))],
                true,
            )
            .unwrap();
        assert!(store.apply(DmlOperation::Delete, &[parent], true).is_err());
        assert_eq!(store.records("Contact", false).len(), 1);
    }
    fn account(name: &str) -> RecordData {
        RecordData::new("Account").with("Name", DataValue::Text(name.into()))
    }
    #[test]
    fn atomic_and_partial_batches_have_distinct_semantics() {
        let mut s = RecordStore::new(SchemaCatalog::standard()).unwrap();
        assert!(s
            .apply(
                DmlOperation::Insert,
                &[account("A"), RecordData::new("Account")],
                true
            )
            .is_err());
        assert!(s.records("Account", false).is_empty());
        let r = s
            .apply(
                DmlOperation::Insert,
                &[account("A"), RecordData::new("Account")],
                false,
            )
            .unwrap();
        assert!(r[0].success && !r[1].success);
        assert_eq!(s.records("Account", false).len(), 1);
    }
    #[test]
    fn update_preserves_unspecified_fields_and_rollback_is_deep() {
        let mut s = RecordStore::new(SchemaCatalog::standard()).unwrap();
        let id = s
            .apply(
                DmlOperation::Insert,
                &[account("A").with("Description", DataValue::Text("original".into()))],
                true,
            )
            .unwrap()[0]
            .id
            .clone()
            .unwrap();
        let point = s.savepoint().unwrap();
        s.apply(
            DmlOperation::Update,
            &[RecordData::new("Account")
                .with("Id", DataValue::Text(id.clone()))
                .with("Name", DataValue::Text("B".into()))],
            true,
        )
        .unwrap();
        assert_eq!(
            s.find("Account", &id).unwrap().get("Description"),
            Some(&DataValue::Text("original".into()))
        );
        s.rollback(point).unwrap();
        assert_eq!(
            s.find("Account", &id).unwrap().get("Name"),
            Some(&DataValue::Text("A".into()))
        );
        assert_eq!(s.dml_statements, 2);
    }
    #[test]
    fn delete_and_undelete_track_recycle_bin() {
        let mut s = RecordStore::new(SchemaCatalog::standard()).unwrap();
        let id = s
            .apply(DmlOperation::Insert, &[account("A")], true)
            .unwrap()[0]
            .id
            .clone()
            .unwrap();
        let row = RecordData::new("Account").with("Id", DataValue::Text(id));
        s.apply(DmlOperation::Delete, &[row.clone()], true).unwrap();
        assert_eq!(s.records("Account", false).len(), 0);
        assert_eq!(s.records("Account", true).len(), 1);
        s.apply(DmlOperation::Undelete, &[row], true).unwrap();
        assert_eq!(s.records("Account", false).len(), 1);
    }
    #[test]
    fn field_and_reference_types_are_enforced() {
        let mut s = RecordStore::new(SchemaCatalog::standard()).unwrap();
        assert!(s
            .apply(
                DmlOperation::Insert,
                &[account("A").with("NumberOfEmployees", DataValue::Text("wrong".into()))],
                true
            )
            .is_err());
        assert!(s
            .apply(
                DmlOperation::Insert,
                &[RecordData::new("Contact")
                    .with("LastName", DataValue::Text("A".into()))
                    .with("AccountId", DataValue::Text("001000000000000AAA".into()))],
                true
            )
            .is_err());
    }
    #[test]
    fn date_checks_reject_impossible_dates() {
        assert!(valid_date("2024-02-29"));
        assert!(!valid_date("2023-02-29"));
        assert!(!valid_datetime("2024-01-01T25:00:00Z"));
        assert!(valid_datetime("2024-01-01T00:00:00.123Z"));
    }
    #[test]
    fn synthetic_ids_have_salesforce_checksum() {
        assert_eq!(id18("001000000000001"), "001000000000001AAA");
        assert_eq!(id18("a0B000000000001"), "a0B000000000001EAA");
    }
    #[test]
    fn canonical_clock_survives_read_modify_write() {
        let mut s =
            RecordStore::with_environment(SchemaCatalog::standard(), "2026-01-15T12:00:00Z", None)
                .unwrap();
        let id = s
            .apply(DmlOperation::Insert, &[account("A")], true)
            .unwrap()[0]
            .id
            .clone()
            .unwrap();
        let mut row = s.find("Account", &id).unwrap().clone();
        assert_eq!(
            row.get("CreatedDate"),
            Some(&DataValue::DateTime("2026-01-15T12:00:00.000Z".into()))
        );
        row.set("Name", DataValue::Text("B".into()));
        assert!(s.apply(DmlOperation::Update, &[row], true).is_ok());
    }
    #[test]
    fn platform_rows_preserve_ids_without_consuming_dml_limits() {
        let mut s = RecordStore::new(SchemaCatalog::standard()).unwrap();
        let id = s
            .upsert_platform_record(
                &RecordData::new("AsyncApexJob").with("Status", DataValue::Text("Queued".into())),
            )
            .unwrap();
        let again = s
            .upsert_platform_record(
                &RecordData::new("AsyncApexJob")
                    .with("Id", DataValue::Text(id.clone()))
                    .with("Status", DataValue::Text("Completed".into())),
            )
            .unwrap();
        assert_eq!(id, again);
        assert_eq!(s.records("AsyncApexJob", false).len(), 1);
        assert_eq!(s.dml_rows, 0);
        assert_eq!(s.dml_statements, 0);
        assert!(s
            .upsert_platform_record(
                &RecordData::new("AsyncApexJob")
                    .with("NumberOfErrors", DataValue::Text("wrong".into()))
            )
            .is_err());
    }
    #[test]
    fn independent_transaction_cannot_reuse_parent_savepoints() {
        let mut parent = RecordStore::new(SchemaCatalog::standard()).unwrap();
        let original = parent.savepoint().unwrap();
        parent
            .apply(DmlOperation::Insert, &[account("parent")], true)
            .unwrap();
        let mut child = parent.fork_transaction();
        let child_point = child.savepoint().unwrap();
        assert_ne!(original.id, child_point.id);
        assert!(child.rollback(original).is_err());
        child
            .apply(DmlOperation::Insert, &[account("child")], true)
            .unwrap();
        parent.merge_committed(child);
        assert_eq!(parent.dml_statements, 1);
        assert_eq!(parent.records("Account", false).len(), 2);
        assert!(parent.rollback(original).unwrap_err().unsupported);
        parent.release_savepoint(original).unwrap();
    }
}

fn canonical_datetime(source: &str) -> Result<String, StoreError> {
    let value =
        crate::dates::parse_datetime(source).map_err(|e| StoreError::unsupported(e.message))?;
    let crate::value::Value::Datetime(ms) = value else {
        unreachable!()
    };
    crate::dates::datetime_iso(ms).map_err(|e| StoreError::unsupported(e.message))
}
