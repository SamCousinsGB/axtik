//! Explicit local fixture inputs. Never inferred from project/test names.
use crate::{
    record_store::{DataValue, RecordData, StoreError},
    schema::{FieldKind, SchemaCatalog},
};
use serde_json::Value;
use std::{collections::BTreeSet, fs, io::Read, path::Path};
#[derive(Clone, Debug)]
pub struct Environment {
    pub name: String,
    pub now: String,
    pub current_user_id: String,
    pub records: Vec<RecordData>,
    pub capabilities: Value,
    pub security: Value,
    pub assignments: Vec<Assignment>,
    pub source: String,
}
#[derive(Clone, Debug)]
pub struct Assignment {
    pub assignee_id: String,
    pub permission_set_name: String,
    pub unresolved: bool,
}
impl Environment {
    pub fn load(path: &Path, schema: &SchemaCatalog) -> Result<Self, StoreError> {
        crate::inputs::ensure_regular_path(path)
            .map_err(|e| StoreError::unsupported(format!("Environment input path: {e}")))?;
        let mut bytes = vec![];
        fs::File::open(path)
            .and_then(|f| f.take(1024 * 1024 + 1).read_to_end(&mut bytes))
            .map_err(|e| StoreError::unsupported(format!("Environment read: {e}")))?;
        if bytes.len() > 1024 * 1024 {
            return Err(StoreError::unsupported("Environment exceeds1MiB"));
        }
        crate::inputs::record_read(path, &bytes)
            .map_err(|e| StoreError::unsupported(format!("Environment input identity: {e}")))?;
        let json: Value = serde_json::from_slice(&bytes)
            .map_err(|e| StoreError::unsupported(format!("Environment JSON: {e}")))?;
        Self::parse(&json, schema, path.to_string_lossy().as_ref())
    }
    pub fn parse(json: &Value, schema: &SchemaCatalog, source: &str) -> Result<Self, StoreError> {
        let root = json
            .as_object()
            .ok_or_else(|| StoreError::unsupported("Environment must be a JSON object"))?;
        let allowed = [
            "schemaVersion",
            "name",
            "status",
            "repository",
            "revision",
            "sourceApiVersion",
            "clock",
            "currentUserId",
            "records",
            "capabilities",
            "metadataImports",
            "assignments",
            "isolation",
            "security",
            "unresolved",
            "provenance",
        ];
        if let Some(key) = root.keys().find(|k| !allowed.contains(&k.as_str())) {
            return Err(StoreError::unsupported(format!(
                "Unknown environment key {key}"
            )));
        }
        if json["schemaVersion"] != 1 {
            return Err(StoreError::unsupported(
                "Unsupported environment schema version",
            ));
        }
        let now = json
            .pointer("/clock/now")
            .and_then(Value::as_str)
            .ok_or_else(|| StoreError::unsupported("Environment clock.now missing"))?
            .to_string();
        if !crate::record_store::valid_datetime(&now) {
            return Err(StoreError::unsupported(
                "Environment clock.now must be ISO UTC",
            ));
        }
        let user = json["currentUserId"]
            .as_str()
            .ok_or_else(|| StoreError::unsupported("Environment currentUserId missing"))?
            .to_string();
        let rows = json["records"]
            .as_array()
            .ok_or_else(|| StoreError::unsupported("Environment records missing"))?;
        if rows.len() > 10_000 {
            return Err(StoreError::unsupported("Environment exceeds10000 records"));
        }
        let mut records = vec![];
        let mut ids = BTreeSet::new();
        for row in rows {
            let object = row["object"]
                .as_str()
                .ok_or_else(|| StoreError::unsupported("Environment record object missing"))?;
            let object_schema = schema.object(object).ok_or_else(|| {
                StoreError::unsupported(format!("Environment schema unavailable for {object}"))
            })?;
            let fields = row["fields"]
                .as_object()
                .ok_or_else(|| StoreError::unsupported("Environment record fields missing"))?;
            if fields.len() > 500 {
                return Err(StoreError::unsupported(
                    "Environment record exceeds500 fields",
                ));
            }
            let mut record = RecordData::new(&object_schema.name);
            for (name, value) in fields {
                let field = object_schema.field(name).ok_or_else(|| {
                    StoreError::unsupported(format!("Unknown environment field {object}.{name}"))
                })?;
                let value = if value.is_null() {
                    DataValue::Null
                } else {
                    match (&field.kind, value) {
                        (FieldKind::Boolean, Value::Bool(v)) => DataValue::Boolean(*v),
                        (
                            FieldKind::Integer | FieldKind::Long | FieldKind::Decimal,
                            Value::Number(v),
                        ) => DataValue::number(&v.to_string())?,
                        (
                            FieldKind::Integer | FieldKind::Long | FieldKind::Decimal,
                            Value::String(v),
                        ) => DataValue::number(v)?,
                        (FieldKind::Date, Value::String(v))
                            if crate::record_store::valid_date(v) =>
                        {
                            DataValue::Date(v.clone())
                        }
                        (FieldKind::DateTime, Value::String(v))
                            if crate::record_store::valid_datetime(v) =>
                        {
                            DataValue::DateTime(v.clone())
                        }
                        (
                            FieldKind::Text
                            | FieldKind::Id
                            | FieldKind::Reference
                            | FieldKind::AutoNumber
                            | FieldKind::Picklist,
                            Value::String(v),
                        ) => DataValue::Text(v.clone()),
                        _ => {
                            return Err(StoreError::unsupported(format!(
                                "Invalid environment field type {object}.{name}"
                            )))
                        }
                    }
                };
                record.set(name, value);
            }
            let id = record.id().ok_or_else(|| {
                StoreError::unsupported("Environment records require explicit Ids")
            })?;
            if id.len() != 18 || !id.is_ascii() || crate::record_store::id18(&id[..15]) != id {
                return Err(StoreError::unsupported(
                    "Environment Id must have a valid18-character checksum",
                ));
            }
            if !ids.insert(id.to_string()) {
                return Err(StoreError::unsupported("Duplicate environment record Id"));
            }
            records.push(record);
        }
        if !records
            .iter()
            .any(|r| r.object.eq_ignore_ascii_case("User") && r.id() == Some(user.as_str()))
        {
            return Err(StoreError::unsupported(
                "Environment currentUserId must reference a supplied User",
            ));
        }
        let mut assignments = vec![];
        if let Some(items) = json["assignments"].as_array() {
            for a in items {
                assignments.push(Assignment {
                    assignee_id: a["assigneeId"]
                        .as_str()
                        .ok_or_else(|| StoreError::unsupported("Assignment assigneeId missing"))?
                        .into(),
                    permission_set_name: a["permissionSetName"]
                        .as_str()
                        .ok_or_else(|| {
                            StoreError::unsupported("Assignment permissionSetName missing")
                        })?
                        .into(),
                    unresolved: a.get("resolution").is_some(),
                });
            }
        }
        let security = json.get("security").cloned().unwrap_or(Value::Null);
        if !security.is_null() && security["mode"] != "synthetic-local" {
            return Err(StoreError::unsupported(
                "Unknown environment security model",
            ));
        }
        Ok(Self {
            name: json["name"]
                .as_str()
                .unwrap_or("Explicit local environment")
                .into(),
            now,
            current_user_id: user,
            records,
            capabilities: json.get("capabilities").cloned().unwrap_or(Value::Null),
            security,
            assignments,
            source: source.into(),
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn environment_is_explicit_and_rejects_unknown_keys() {
        let json = serde_json::json!({"schemaVersion":1,"clock":{"now":"2026-01-15T12:00:00Z"},"currentUserId":"005000000000001AAA","records":[{"object":"User","fields":{"Id":"005000000000001AAA","LastName":"Local"}}]});
        let schema = SchemaCatalog::standard();
        assert!(Environment::parse(&json, &schema, "fixture").is_ok());
        let mut bad = json;
        bad["testPassOverrides"] = serde_json::json!({});
        assert!(Environment::parse(&bad, &schema, "fixture").is_err());
    }
}
