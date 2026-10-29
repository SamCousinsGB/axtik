use super::*;
use crate::{
    record_store::{
        DataValue, DmlOperation, RecordData, RecordStore, SaveResult, Savepoint, StoreError,
    },
    soql::{self, Bindings, Predicate, Query, QueryAccess, QueryResult, Selection, ValueExpr},
};
use num_traits::ToPrimitive;

fn store_error(at: &Node, error: StoreError, exception_type: &str) -> EvalError {
    EvalError {
        exception: if error.is_terminal() {
            None
        } else {
            Some(Value::Exception(Rc::new(RefCell::new(ExceptionValue {
                typ: exception_type.into(),
                message: format!("{}: {}", error.code, error.message),
                stack: format!("APX local {}:{}", at.path, at.line()),
            }))))
        },
        status: if error.unsupported {
            "blocked"
        } else {
            "failed"
        },
        diagnostic: at.diagnostic(
            if error.governor.is_some() {
                "APX_GOVERNOR"
            } else if error.unsupported {
                "APX_UNSUPPORTED"
            } else {
                "APX_DATABASE"
            },
            format!("{}: {}", error.code, error.message),
        ),
    }
}
fn native(class: &str, fields: BTreeMap<String, Value>) -> Value {
    Value::Instance(Rc::new(RefCell::new(Instance {
        class: class.to_ascii_lowercase(),
        field_types: BTreeMap::new(),
        fields,
    })))
}
pub(super) fn to_data(value: &Value, at: &Node, depth: usize) -> R<DataValue> {
    if depth > 32 {
        return block(at, "Data value nesting exceeds32");
    }
    Ok(match value {
        Value::Null => DataValue::Null,
        Value::Bool(b) => DataValue::Boolean(*b),
        Value::Num(n) => DataValue::Number(n.q.clone()),
        Value::Str(s) => DataValue::Text(s.clone()),
        Value::Blob(b) => DataValue::Binary(b.clone().into()),
        Value::Date(d) => DataValue::Date(crate::dates::date_iso(*d).map_err(|e| EvalError {
            exception: None,
            status: "blocked",
            diagnostic: at.diagnostic("APX_DATE", e.message),
        })?),
        Value::Datetime(d) => {
            DataValue::DateTime(crate::dates::datetime_iso(*d).map_err(|e| EvalError {
                exception: None,
                status: "blocked",
                diagnostic: at.diagnostic("APX_DATE", e.message),
            })?)
        }
        Value::List(items) | Value::Set(items) | Value::Iterable(items) => DataValue::List(
            items
                .borrow()
                .iter()
                .map(|v| to_data(v, at, depth + 1))
                .collect::<R<Vec<_>>>()?,
        ),
        Value::Record(record) => {
            let r = record.borrow();
            DataValue::Record(Box::new(RecordData {
                object: r.typ.clone(),
                fields: r
                    .fields
                    .iter()
                    .map(|(k, v)| Ok((k.clone(), to_data(v, at, depth + 1)?)))
                    .collect::<R<_>>()?,
            }))
        }
        _ => return block(at, "Unsupported non-data value passed to database"),
    })
}
pub(super) fn from_data(value: &DataValue, at: &Node, depth: usize) -> R<Value> {
    if depth > 32 {
        return block(at, "Database projection nesting exceeds32");
    }
    Ok(match value {
        DataValue::Null => Value::Null,
        DataValue::Boolean(b) => Value::Bool(*b),
        DataValue::Number(n) => Value::Num(Number {
            q: n.clone(),
            kind: if n.is_integer() && n.to_integer().to_i32().is_some() {
                NumKind::Int
            } else if n.is_integer() && n.to_integer().to_i64().is_some() {
                NumKind::Long
            } else {
                NumKind::Decimal
            },
        }),
        DataValue::Text(s) => Value::Str(s.clone()),
        DataValue::Date(s) => crate::dates::parse_date(s).map_err(|e| EvalError {
            exception: None,
            status: "blocked",
            diagnostic: at.diagnostic("APX_DATE", e.message),
        })?,
        DataValue::DateTime(s) => crate::dates::parse_datetime(s).map_err(|e| EvalError {
            exception: None,
            status: "blocked",
            diagnostic: at.diagnostic("APX_DATE", e.message),
        })?,
        DataValue::Binary(b) => Value::Blob(b.to_vec()),
        DataValue::List(items) => Value::list(
            items
                .iter()
                .map(|v| from_data(v, at, depth + 1))
                .collect::<R<Vec<_>>>()?,
        ),
        DataValue::Record(record) => Value::Record(Rc::new(RefCell::new(Record {
            typ: record.object.clone(),
            fields: record
                .fields
                .iter()
                .map(|(k, v)| Ok((k.clone(), from_data(v, at, depth + 1)?)))
                .collect::<R<_>>()?,
            projected: true,
        }))),
    })
}
fn record_data(record: &Rc<RefCell<Record>>, at: &Node) -> R<RecordData> {
    let DataValue::Record(record) = to_data(&Value::Record(record.clone()), at, 0)? else {
        unreachable!()
    };
    Ok(*record)
}
fn runtime_record(record: &RecordData, projected: bool, at: &Node) -> R<Rc<RefCell<Record>>> {
    let Value::Record(r) = from_data(&DataValue::Record(Box::new(record.clone())), at, 0)? else {
        unreachable!()
    };
    r.borrow_mut().projected = projected;
    Ok(r)
}
impl Context<'_> {
    pub(super) fn resolve_data_bind(&mut self, name: &str, at: &Node) -> R<Value> {
        if name.ends_with("()") {
            let source = format!("class __ApxBind {{ Object read() {{ return {name}; }} }}");
            let tree = crate::syntax::parse(&source, at.path.as_ref()).map_err(|_| EvalError {
                exception: None,
                status: "blocked",
                diagnostic: at.diagnostic("APX_BIND", "Unsupported bind call"),
            })?;
            fn find(n: &Node) -> Option<&Node> {
                if n.kind == "method_invocation" {
                    return Some(n);
                }
                for (_, child) in &n.children {
                    if let Some(found) = find(child) {
                        return Some(found);
                    }
                }
                None
            }
            let call = find(&tree).ok_or_else(|| EvalError {
                exception: None,
                status: "blocked",
                diagnostic: at.diagnostic("APX_BIND", "Bind call did not parse"),
            })?;
            self.eval(call)
        } else {
            let mut parts = name.split('.');
            let first = parts.next().unwrap();
            let mut value = if first.eq_ignore_ascii_case("this") {
                self.this
                    .as_ref()
                    .map(|i| Value::Instance(i.clone()))
                    .ok_or_else(|| EvalError {
                        exception: None,
                        status: "blocked",
                        diagnostic: at.diagnostic("APX_BIND", "this binding requires an instance"),
                    })?
            } else {
                self.get(first, at)?
            };
            for field in parts {
                value = self.field_value(value, field, at)?;
            }
            Ok(value)
        }
    }
    fn api_user_default(&self, at: &Node) -> bool {
        self.program
            .api_versions
            .get(at.path.as_ref())
            .is_some_and(|version| *version >= 67)
    }
    pub(super) fn data_access(&mut self, at: &Node) -> R<crate::access::ResolvedAccess> {
        self.ensure_data_store(at)?;
        let environment = self.program.environment.as_ref().ok_or_else(|| EvalError {
            exception: None,
            status: "blocked",
            diagnostic: at.diagnostic(
                "APX_ACCESS",
                "CRUD/FLS requires an explicit environment policy",
            ),
        })?;
        let user = match self.native_state.get("userinfo.userid") {
            Some(Value::Str(s)) => s.as_str(),
            _ => environment.current_user_id.as_str(),
        };
        crate::access::resolve(self.data_store.as_ref().unwrap(), environment, user)
            .map_err(|e| store_error(at, e, "System.SecurityException"))
    }
    fn check_user_dml(&mut self, operation: &DmlOperation, value: &Value, at: &Node) -> R<()> {
        let access = self.data_access(at)?;
        let rows = match value {
            Value::Record(r) => vec![r.clone()],
            Value::List(v) => v
                .borrow()
                .iter()
                .filter_map(|v| {
                    if let Value::Record(r) = v {
                        Some(r.clone())
                    } else {
                        None
                    }
                })
                .collect(),
            _ => vec![],
        };
        for record in rows {
            let r = record.borrow();
            let object = r.typ.to_ascii_lowercase();
            let grant = access.objects.get(&object).cloned().unwrap_or_default();
            let mut target_id = match r.fields.get("id") {
                Some(Value::Str(id)) => Some(id.clone()),
                _ => None,
            };
            // External-ID upsert authorizes the resolved existing record, just
            // like Id upsert; an absent Id is not proof that this is an insert.
            if target_id.is_none() {
                if let DmlOperation::Upsert {
                    external_id: Some(field),
                } = operation
                {
                    if let Some(schema) = self.program.schema.field(&object, field) {
                        if schema.external_id || schema.unique {
                            if let Some(value) = r.fields.get(&field.to_ascii_lowercase()) {
                                let value = to_data(value, at, 0)?;
                                if !value.is_null() {
                                    for candidate in
                                        self.data_store.as_ref().unwrap().records(&object, false)
                                    {
                                        if crate::record_store::value_equal(
                                            candidate.get(field).unwrap_or(&DataValue::Null),
                                            &value,
                                            schema.case_sensitive,
                                        )
                                        .map_err(|e| store_error(at, e, "System.DmlException"))?
                                        {
                                            if target_id.is_some() {
                                                return exceptions::raise(at, "System.DmlException", "DUPLICATE_EXTERNAL_ID: External Id matched multiple records");
                                            }
                                            target_id = candidate.id().map(str::to_owned);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let allowed = match operation {
                DmlOperation::Insert => grant.create,
                DmlOperation::Update => grant.edit,
                DmlOperation::Delete => grant.delete,
                DmlOperation::Undelete => grant.create,
                DmlOperation::Upsert { .. } => {
                    if target_id.is_some() {
                        grant.edit
                    } else {
                        grant.create
                    }
                }
            };
            if !allowed {
                return exceptions::raise(
                    at,
                    "System.SecurityException",
                    format!(
                        "Access to entity '{}' denied",
                        self.program
                            .schema
                            .object(&r.typ)
                            .map(|o| o.name.as_str())
                            .unwrap_or(&r.typ)
                    ),
                );
            }
            if !matches!(operation, DmlOperation::Delete | DmlOperation::Undelete) {
                for (field, value) in &r.fields {
                    if field == "id" {
                        continue;
                    }
                    let actual = self
                        .program
                        .schema
                        .object(&r.typ)
                        .and_then(|o| {
                            o.fields.values().find(|f| {
                                f.relationship_name
                                    .as_deref()
                                    .is_some_and(|name| name.eq_ignore_ascii_case(field))
                            })
                        })
                        .map(|f| f.name.to_ascii_lowercase())
                        .unwrap_or_else(|| field.clone());
                    let qualified = format!("{object}.{actual}");
                    if !access.fields.get(&qualified).is_some_and(|f| f.edit) {
                        if matches!(value, Value::Null) {
                            continue;
                        }
                        return exceptions::raise(at,"System.DmlException",format!("INVALID_FIELD_FOR_INSERT_UPDATE: {qualified} not writable for current user"));
                    }
                }
            }
            if let Some(id) = target_id {
                let rights = access.records.get(&id).copied().unwrap_or_default();
                let writable = match operation {
                    DmlOperation::Insert => true,
                    DmlOperation::Update | DmlOperation::Upsert { .. } => rights.edit,
                    DmlOperation::Delete | DmlOperation::Undelete => rights.delete,
                };
                if !writable && !grant.modify_all {
                    return exceptions::raise(
                        at,
                        "System.DmlException",
                        "INSUFFICIENT_ACCESS_OR_READONLY: Record access does not permit this operation",
                    );
                }
            }
        }
        Ok(())
    }
    pub fn install_data_store(&mut self, mut store: Option<RecordStore>) {
        if let Some(s) = &mut store {
            s.reset_limits();
        }
        self.data_store = store;
    }
    fn dml_record_data(&self, record: &Rc<RefCell<Record>>, at: &Node) -> R<RecordData> {
        let mut row = record_data(record, at)?;
        let schema = self.program.schema.object(&row.object).ok_or_else(|| {
            store_error(
                at,
                StoreError::unsupported("DML schema unavailable"),
                "System.DmlException",
            )
        })?;
        for key in row.fields.keys().cloned().collect::<Vec<_>>() {
            if schema.field(&key).is_some() {
                continue;
            }
            let Some(field) = schema.fields.values().find(|f| {
                f.relationship_name
                    .as_deref()
                    .is_some_and(|name| name.eq_ignore_ascii_case(&key))
            }) else {
                continue;
            };
            let relation = row.fields.remove(&key).unwrap();
            let id = match relation {
                DataValue::Null => DataValue::Null,
                DataValue::Record(parent) => {
                    if !field
                        .reference_to
                        .iter()
                        .any(|o| o.eq_ignore_ascii_case(&parent.object))
                    {
                        return exceptions::raise(
                            at,
                            "System.DmlException",
                            "FIELD_INTEGRITY_EXCEPTION: Relationship has wrong object type",
                        );
                    }
                    let supplied = parent
                        .fields
                        .iter()
                        .filter(|(_, v)| !v.is_null())
                        .collect::<Vec<_>>();
                    if supplied.len() != 1 {
                        return exceptions::raise(at,"System.DmlException","INVALID_FIELD: Relationship reference requires one Id or external ID field");
                    }
                    let (name, value) = supplied[0];
                    if name == "id" {
                        value.clone()
                    } else {
                        let parent_schema = self.program.schema.object(&parent.object).unwrap();
                        let lookup = parent_schema.field(name).ok_or_else(|| {
                            store_error(
                                at,
                                StoreError::new(
                                    "INVALID_FIELD",
                                    "Unknown external reference field",
                                    vec![name.clone()],
                                ),
                                "System.DmlException",
                            )
                        })?;
                        if !lookup.external_id {
                            return exceptions::raise(
                                at,
                                "System.DmlException",
                                "INVALID_FIELD: Relationship reference field is not an external ID",
                            );
                        }
                        let mut found = vec![];
                        for candidate in self
                            .data_store
                            .as_ref()
                            .unwrap()
                            .records(&parent.object, false)
                        {
                            if crate::record_store::value_equal(
                                candidate.get(name).unwrap_or(&DataValue::Null),
                                value,
                                lookup.case_sensitive,
                            )
                            .map_err(|e| store_error(at, e, "System.DmlException"))?
                            {
                                found.push(candidate.id().unwrap().to_string());
                            }
                        }
                        if found.len() != 1 {
                            return exceptions::raise(at,"System.DmlException",format!("INVALID_FIELD: External ID relationship resolves to {} records",found.len()));
                        }
                        DataValue::Text(found.remove(0))
                    }
                }
                _ => {
                    return exceptions::raise(
                        at,
                        "System.DmlException",
                        "INVALID_FIELD: Relationship reference requires an SObject",
                    )
                }
            };
            let fk = field.name.to_ascii_lowercase();
            if row.fields.get(&fk).is_some_and(|v| !v.is_null()) {
                return exceptions::raise(
                    at,
                    "System.DmlException",
                    "INVALID_FIELD: Both a relationship reference and foreign key were specified",
                );
            }
            row.set(&fk, id);
        }
        Ok(row)
    }
    fn restore_dml_store(&mut self, store: Option<RecordStore>) {
        let counts = self
            .data_store
            .as_ref()
            .map(|s| (s.dml_statements, s.dml_rows, s.query_rows));
        self.data_store = store;
        if let (Some(s), Some((statements, rows, queries))) = (&mut self.data_store, counts) {
            s.dml_statements = statements;
            s.dml_rows = rows;
            s.query_rows = queries;
        }
    }
    pub(super) fn ensure_data_store(&mut self, at: &Node) -> R<()> {
        if self.data_store.is_none() {
            let now = match self.native_state.get("clock.iso") {
                Some(Value::Str(s)) => s.clone(),
                _ => crate::dates::now_iso_utc(),
            };
            let user = match self.native_state.get("userinfo.userid") {
                Some(Value::Str(s)) => Some(s.clone()),
                _ => self
                    .program
                    .environment
                    .as_ref()
                    .map(|e| e.current_user_id.clone()),
            };
            let mut store = RecordStore::with_environment(self.program.schema.clone(), &now, user)
                .map_err(|e| store_error(at, e, "System.DmlException"))?;
            if let Some(environment) = &self.program.environment {
                store
                    .seed_environment_for_test(
                        environment,
                        matches!(
                            self.native_state.get("test.seealldata"),
                            Some(Value::Bool(true))
                        ),
                    )
                    .map_err(|e| store_error(at, e, "System.DmlException"))?;
            }
            self.data_store = Some(store);
        }
        Ok(())
    }
    pub(super) fn data_dml_expression(&mut self, n: &Node) -> R<Value> {
        let user_mode = n
            // The grammar gives both the `as` token and mode child the same
            // field name. Selecting the first field reads only `as`.
            .child("dml_security_mode")
            .map(|m| m.text().eq_ignore_ascii_case("user"))
            .unwrap_or_else(|| self.api_user_default(n));
        let operation = n
            .child("dml_type")
            .map(|n| n.text().to_ascii_lowercase())
            .unwrap_or_default();
        let value = self.eval(self.need(n, "target")?)?;
        let op = match operation.as_str() {
            "insert" => DmlOperation::Insert,
            "update" => DmlOperation::Update,
            "delete" => DmlOperation::Delete,
            "undelete" => DmlOperation::Undelete,
            "upsert" => {
                if n.named().any(|n| n.kind == "field_access") {
                    return block(
                        n,
                        "Statement external-ID upsert field parsing not implemented",
                    );
                }
                DmlOperation::Upsert { external_id: None }
            }
            _ => return block(n, format!("DML {operation} unsupported")),
        };
        if user_mode {
            self.check_user_dml(&op, &value, n)?;
        }
        self.data_dml(op, &value, true, n)?;
        Ok(Value::Null)
    }
    pub(super) fn data_dml(
        &mut self,
        operation: DmlOperation,
        value: &Value,
        all_or_none: bool,
        at: &Node,
    ) -> R<Vec<SaveResult>> {
        self.ensure_data_store(at)?;
        let inputs = match value {
            Value::Null => {
                return exceptions::raise(
                    at,
                    "System.NullPointerException",
                    "Attempt to de-reference a null object",
                )
            }
            Value::Record(r) => vec![r.clone()],
            Value::List(rows) => rows
                .borrow()
                .iter()
                .map(|v| match v {
                    Value::Record(r) => Ok(r.clone()),
                    _ => fail(at, "DML list contains a non-record"),
                })
                .collect::<R<Vec<_>>>()?,
            _ => return fail(at, "DML requires an SObject or List<SObject>"),
        };
        self.dml += 1;
        if self.dml > 150 {
            return exceptions::raise(at, "System.LimitException", "Too many DML statements:151");
        }
        if inputs.is_empty() {
            return Ok(vec![]);
        }
        if matches!(
            self.native_state.get("appmenu.changed"),
            Some(Value::Bool(true))
        ) {
            return block(
                at,
                "DML after AppMenu changes requires mixed-setup transaction qualification",
            );
        }
        let mut identities = BTreeSet::new();
        for input in &inputs {
            if !identities.insert(Rc::as_ptr(input) as usize) {
                return exceptions::raise(
                    at,
                    "System.ListException",
                    "Duplicate record identity in DML input",
                );
            }
        }
        let mut segments: Vec<Vec<Rc<RefCell<Record>>>> = vec![];
        for input in &inputs {
            if segments
                .last()
                .is_none_or(|s| !s[0].borrow().typ.eq_ignore_ascii_case(&input.borrow().typ))
            {
                segments.push(vec![]);
            }
            segments.last_mut().unwrap().push(input.clone());
        }
        if segments.len() > 10 {
            return exceptions::raise(at,"System.DmlException","Too many chunks in a single operation. Please rearrange the data to reduce chunking.");
        }
        let original = inputs
            .iter()
            .map(|r| r.borrow().clone())
            .collect::<Vec<_>>();
        let before_store = self.data_store.clone();
        let event_point = crate::record_store::transaction_marker();
        self.events_savepoint(event_point);
        let async_count = self.async_jobs.len();
        let mut output = vec![];
        let mut checkpoints = vec![];
        let outcome = (|| {
            let mut first_chunk = true;
            for segment in &segments {
                let object = segment[0].borrow().typ.clone();
                let has_triggers = self
                    .program
                    .triggers
                    .iter()
                    .any(|t| t.object.eq_ignore_ascii_case(&object));
                for chunk in segment.chunks(200) {
                    let chunk_store = self.data_store.clone();
                    let chunk_event = crate::record_store::transaction_marker();
                    self.events_savepoint(chunk_event);
                    checkpoints.push(chunk_event);
                    let chunk_async = self.async_jobs.len();
                    let mut incoming = chunk
                        .iter()
                        .map(|r| self.dml_record_data(r, at))
                        .collect::<R<Vec<_>>>()?;
                    let mut kind = match operation {
                        DmlOperation::Insert => "insert",
                        DmlOperation::Update => "update",
                        DmlOperation::Delete => "delete",
                        DmlOperation::Undelete => "undelete",
                        DmlOperation::Upsert { .. } => "",
                    };
                    if let DmlOperation::Upsert { external_id } = &operation {
                        let mut operations = BTreeSet::new();
                        for r in &mut incoming {
                            if r.id().is_none() {
                                if let Some(field) = external_id {
                                    let value = r.get(field).unwrap_or(&DataValue::Null);
                                    if !value.is_null() {
                                        let mut found = vec![];
                                        for row in self
                                            .data_store
                                            .as_ref()
                                            .unwrap()
                                            .records(&object, false)
                                        {
                                            if crate::record_store::value_equal(
                                                row.get(field).unwrap_or(&DataValue::Null),
                                                value,
                                                false,
                                            )
                                            .map_err(|e| {
                                                store_error(at, e, "System.DmlException")
                                            })? {
                                                found.push(row.id().unwrap().to_string());
                                            }
                                        }
                                        if found.len() > 1 {
                                            return exceptions::raise(
                                                at,
                                                "System.DmlException",
                                                "DUPLICATE_EXTERNAL_ID: Multiple upsert matches",
                                            );
                                        }
                                        if let Some(id) = found.first() {
                                            r.set("Id", DataValue::Text(id.clone()));
                                        }
                                    }
                                }
                            }
                            operations.insert(if r.id().is_some() { "update" } else { "insert" });
                        }
                        if operations.len() > 1 {
                            self.events_release(chunk_event);
                            // Preserve source ordering while executing homogeneous trigger groups.
                            let mut results = Vec::with_capacity(chunk.len());
                            let mut start = 0;
                            while start < chunk.len() {
                                let updating = incoming[start].id().is_some();
                                let mut end = start + 1;
                                while end < chunk.len() && incoming[end].id().is_some() == updating
                                {
                                    end += 1;
                                }
                                let rows = chunk[start..end]
                                    .iter()
                                    .cloned()
                                    .map(Value::Record)
                                    .collect();
                                for (index, row) in chunk[start..end].iter().enumerate() {
                                    if let Some(id) = incoming[start + index].id() {
                                        row.borrow_mut()
                                            .fields
                                            .insert("id".into(), Value::Str(id.into()));
                                    }
                                }
                                self.dml -= 1;
                                if !first_chunk {
                                    self.data_store.as_mut().unwrap().dml_statements = self
                                        .data_store
                                        .as_ref()
                                        .unwrap()
                                        .dml_statements
                                        .saturating_sub(1);
                                }
                                let next = self.data_dml(
                                    operation.clone(),
                                    &Value::list(rows),
                                    all_or_none,
                                    at,
                                )?;

                                first_chunk = false;
                                results.extend(next);
                                start = end;
                            }
                            output.extend(results);
                            continue;
                        }
                        kind = *operations.first().unwrap_or(&"insert");
                    }
                    let mut old = vec![];
                    let mut working = vec![];
                    for r in &incoming {
                        if kind == "update" || kind == "delete" {
                            if r.id().is_none() {
                                return exceptions::raise(
                                    at,
                                    "System.DmlException",
                                    format!(
                                        "MISSING_ARGUMENT: Id not specified in an {kind} call:"
                                    ),
                                );
                            }
                            let prior = r
                                .id()
                                .and_then(|id| self.data_store.as_ref().unwrap().find(&object, id))
                                .ok_or_else(|| {
                                    store_error(
                                        at,
                                        StoreError::new(
                                            "INVALID_CROSS_REFERENCE_KEY",
                                            "invalid cross reference id",
                                            vec!["Id".into()],
                                        ),
                                        "System.DmlException",
                                    )
                                })?;
                            old.push(runtime_record(prior, false, at)?);
                        }
                        if kind == "insert" || kind == "update" {
                            let draft = self
                                .data_store
                                .as_ref()
                                .unwrap()
                                .draft_record(r, kind == "update")
                                .map_err(|e| store_error(at, e, "System.DmlException"))?;
                            working.push(runtime_record(&draft, false, at)?);
                        }
                    }
                    if kind != "undelete" {
                        self.fire_data_triggers(
                            &object,
                            &format!("before {kind}"),
                            &working,
                            &old,
                            at,
                        )?;
                    }
                    let prepared = if kind == "insert" || kind == "update" {
                        working
                            .iter()
                            .map(|r| self.dml_record_data(r, at))
                            .collect::<R<Vec<_>>>()?
                    } else {
                        incoming
                    };
                    let result = self
                        .data_store
                        .as_mut()
                        .unwrap()
                        .apply_chunk(operation.clone(), &prepared, all_or_none, first_chunk)
                        .map_err(|e| store_error(at, e.error, "System.DmlException"))?;
                    first_chunk = false;
                    if has_triggers && result.iter().any(|r| !r.success) {
                        if result.iter().all(|r| !r.success) {
                            self.events_rollback(chunk_event, at)?;
                            self.events_release(chunk_event);
                            self.async_jobs.truncate(chunk_async);
                            self.restore_dml_store(chunk_store);
                            output.extend(result);
                            continue;
                        }
                        return block(
                            at,
                            "Partial DML failure with triggers requires retry isolation semantics",
                        );
                    }
                    let mut after = vec![];
                    for (i, result) in result.iter().enumerate() {
                        if result.success {
                            if let Some(id) = &result.id {
                                if kind == "insert" {
                                    chunk[i]
                                        .borrow_mut()
                                        .fields
                                        .insert("id".into(), Value::Str(id.clone()));
                                }
                                if kind != "delete" {
                                    if let Some(row) =
                                        self.data_store.as_ref().unwrap().find(&object, id)
                                    {
                                        after.push(runtime_record(row, false, at)?);
                                    }
                                }
                            }
                        }
                    }
                    self.fire_data_triggers(&object, &format!("after {kind}"), &after, &old, at)?;
                    self.events_release(chunk_event);
                    output.extend(result);
                }
            }
            Ok(())
        })();
        for point in checkpoints {
            self.events_release(point);
        }
        if let Err(error) = outcome {
            let rollback = self.events_rollback(event_point, at);
            self.events_release(event_point);
            self.async_jobs.truncate(async_count);
            let counts = self
                .data_store
                .as_ref()
                .map(|s| (s.dml_statements, s.dml_rows, s.query_rows));
            self.data_store = before_store;
            if let (Some(store), Some((statements, rows, queried))) = (&mut self.data_store, counts)
            {
                store.dml_statements = statements;
                store.dml_rows = rows;
                store.query_rows = queried;
            }
            for (r, original) in inputs.iter().zip(original) {
                *r.borrow_mut() = original;
            }
            rollback?;
            if error.status == "blocked" || error.exception.is_none() {
                return Err(error);
            }
            let action = match operation {
                DmlOperation::Insert => "Insert",
                DmlOperation::Update => "Update",
                DmlOperation::Delete => "Delete",
                DmlOperation::Undelete => "Undelete",
                DmlOperation::Upsert { .. } => "Upsert",
            };
            return exceptions::raise(
                at,
                "System.DmlException",
                format!("{action} failed. {}", error.diagnostic.message),
            );
        }
        self.events_release(event_point);
        Ok(output)
    }
    pub(super) fn data_query(&mut self, n: &Node) -> R<Value> {
        let query =
            soql::parse(n.text()).map_err(|e| store_error(n, e, "System.QueryException"))?;
        let mut names = BTreeSet::new();
        query_binds(&query, &mut names);
        let mut bindings = Bindings::new();
        for name in names {
            let value = self.resolve_data_bind(&name, n)?;
            bindings.insert(name, to_data(&value, n, 0)?);
        }
        self.execute_data_query(&query, &bindings, n)
    }
    fn execute_data_query(&mut self, query: &Query, bindings: &Bindings, at: &Node) -> R<Value> {
        if query.object.eq_ignore_ascii_case("DatedConversionRate")
            && self
                .program
                .environment
                .as_ref()
                .and_then(|e| e.capabilities.get("advancedCurrencyManagement"))
                .and_then(serde_json::Value::as_bool)
                == Some(false)
        {
            return exceptions::raise(at,"System.QueryException","sObject type 'DatedConversionRate' is not supported because Advanced Currency Management is disabled");
        }
        self.ensure_data_store(at)?;
        self.queries += 1;
        if self.queries > 100 {
            return exceptions::raise(at, "System.LimitException", "Too many SOQL queries:101");
        }
        let mut query = query.clone();
        if self.api_user_default(at) {
            if query.mode == soql::QueryMode::SecurityEnforced {
                return block(at, "WITH SECURITY_ENFORCED is retired in API67 and later");
            }
            if !query.explicit_mode {
                query.mode = soql::QueryMode::User;
            }
        }
        inherit_query_mode(&mut query);
        let policy = if query.mode != soql::QueryMode::System || self.effective_sharing() {
            let mut policy = self.data_access(at)?;
            if query.mode == soql::QueryMode::System {
                policy.query.readable_fields = self
                    .program
                    .schema
                    .objects
                    .values()
                    .map(|o| {
                        (
                            o.name.to_ascii_lowercase(),
                            o.fields.keys().cloned().collect(),
                        )
                    })
                    .collect();
                query.mode = soql::QueryMode::User;
                inherit_query_mode(&mut query);
            }
            Some(policy)
        } else {
            None
        };
        let access = policy
            .as_ref()
            .map(|p| QueryAccess::User(&p.query))
            .unwrap_or(QueryAccess::System);
        let result = self
            .data_store
            .as_mut()
            .unwrap()
            .query(&query, bindings, access)
            .map_err(|e| store_error(at, e, "System.QueryException"))?;
        match result {
            QueryResult::Count(n) => Ok(Value::Num(Number::integer(n as i64))),
            QueryResult::Rows(rows) => {
                let values = rows
                    .iter()
                    .map(|r| runtime_record(r, true, at).map(Value::Record))
                    .collect::<R<Vec<_>>>()?;
                let v = Value::list(values);
                if let Value::List(list) = &v {
                    let typ = rows
                        .first()
                        .map(|r| r.object.to_ascii_lowercase())
                        .unwrap_or_else(|| query.object.to_ascii_lowercase());
                    self.register_list_contract(list, &typ);
                }
                Ok(v)
            }
        }
    }
    pub(super) fn data_method(
        &mut self,
        receiver: &Value,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        if let Value::Namespace(ns) = receiver {
            if ns.eq_ignore_ascii_case("database") {
                match name {
                    "insert" | "update" | "upsert" | "delete" | "undelete" => {
                        let Some(target) = args.first() else {
                            return fail(at, "Database DML requires records");
                        };
                        let mut all = true;
                        let mut external = None;
                        let mut user_mode = self.api_user_default(at);
                        for arg in &args[1..] {
                            match arg {
                                Value::Bool(v) => all = *v,
                                Value::Type(t) if t.starts_with("Schema.SObjectField:") => {
                                    external = t
                                        .split_once(':')
                                        .and_then(|(_, f)| f.split_once('.'))
                                        .map(|(_, f)| f.to_string());
                                }
                                Value::Enum { typ, name, .. }
                                    if typ.to_ascii_lowercase().ends_with("accesslevel") =>
                                {
                                    user_mode = name.eq_ignore_ascii_case("USER_MODE");
                                    if !user_mode && !name.eq_ignore_ascii_case("SYSTEM_MODE") {
                                        return fail(at, "Invalid AccessLevel");
                                    }
                                }
                                _ => return block(at, "Unsupported Database DML option"),
                            }
                        }
                        let operation = match name {
                            "insert" => DmlOperation::Insert,
                            "update" => DmlOperation::Update,
                            "upsert" => DmlOperation::Upsert {
                                external_id: external,
                            },
                            "delete" => DmlOperation::Delete,
                            _ => DmlOperation::Undelete,
                        };
                        if user_mode {
                            self.check_user_dml(&operation, target, at)?;
                        }
                        let results = self.data_dml(operation, target, all, at)?;
                        let class = match name {
                            "upsert" => "Database.UpsertResult",
                            "delete" => "Database.DeleteResult",
                            "undelete" => "Database.UndeleteResult",
                            _ => "Database.SaveResult",
                        };
                        let values = results
                            .iter()
                            .map(|r| self.save_result_value(r, class, at))
                            .collect::<R<Vec<_>>>()?;
                        return Ok(Some(if matches!(target, Value::Record(_)) {
                            values.into_iter().next().unwrap_or(Value::Null)
                        } else {
                            let value = Value::list(values);
                            if let Value::List(list) = &value {
                                self.register_list_contract(list, class);
                            }
                            value
                        }));
                    }
                    "query" | "countquery" | "querywithbinds" => {
                        let Some(Value::Str(source)) = args.first() else {
                            return fail(at, "Database query requires a String");
                        };
                        let mut query = soql::parse(source)
                            .map_err(|e| store_error(at, e, "System.QueryException"))?;
                        let mut bindings = Bindings::new();
                        if name == "querywithbinds" {
                            let Some(Value::Map(map)) = args.get(1) else {
                                return fail(at, "queryWithBinds requires Map<String,Object>");
                            };
                            for (k, v) in map.borrow().iter() {
                                let Value::Str(k) = k else {
                                    return fail(at, "Bind keys must be Strings");
                                };
                                bindings.insert(k.to_ascii_lowercase(), to_data(v, at, 0)?);
                            }
                            if let Some(Value::Enum { name, .. }) = args.get(2) {
                                query.explicit_mode = true;
                                query.mode = if name.eq_ignore_ascii_case("USER_MODE") {
                                    soql::QueryMode::User
                                } else if name.eq_ignore_ascii_case("SYSTEM_MODE") {
                                    soql::QueryMode::System
                                } else {
                                    return fail(at, "Invalid AccessLevel");
                                };
                            }
                        } else {
                            if args.len() == 2 {
                                let Value::Enum {
                                    typ, name: mode, ..
                                } = &args[1]
                                else {
                                    return fail(
                                        at,
                                        "Database.query requires AccessLevel as second argument",
                                    );
                                };
                                if !typ.to_ascii_lowercase().ends_with("accesslevel") {
                                    return fail(at, "Expected AccessLevel");
                                }
                                query.explicit_mode = true;
                                query.mode = if mode.eq_ignore_ascii_case("USER_MODE") {
                                    soql::QueryMode::User
                                } else if mode.eq_ignore_ascii_case("SYSTEM_MODE") {
                                    soql::QueryMode::System
                                } else {
                                    return fail(at, "Invalid AccessLevel");
                                };
                            } else if args.len() != 1 {
                                return block(at, "Unsupported Database.query overload");
                            }
                            let mut names = BTreeSet::new();
                            query_binds(&query, &mut names);
                            for name in names {
                                let value = self.resolve_data_bind(&name, at)?;
                                bindings.insert(name, to_data(&value, at, 0)?);
                            }
                        }
                        let value = self.execute_data_query(&query, &bindings, at)?;
                        if name == "countquery" && !matches!(value, Value::Num(_)) {
                            return exceptions::raise(
                                at,
                                "System.QueryException",
                                "countQuery requires COUNT()",
                            );
                        }
                        return Ok(Some(value));
                    }
                    "setsavepoint" if args.is_empty() => {
                        self.ensure_data_store(at)?;
                        let p = self
                            .data_store
                            .as_mut()
                            .unwrap()
                            .savepoint()
                            .map_err(|e| store_error(at, e, "System.DmlException"))?;
                        self.events_savepoint(p.id);
                        return Ok(Some(Value::Type(format!("Database.Savepoint:{}", p.id))));
                    }
                    "rollback" | "releasesavepoint" if args.len() == 1 => {
                        self.ensure_data_store(at)?;
                        let Value::Type(token) = &args[0] else {
                            return fail(at, "Expected Savepoint");
                        };
                        let id = token
                            .strip_prefix("Database.Savepoint:")
                            .and_then(|s| s.parse::<u64>().ok())
                            .ok_or_else(|| {
                                store_error(
                                    at,
                                    StoreError::new(
                                        "INVALID_SAVEPOINT",
                                        "Invalid Savepoint",
                                        vec![],
                                    ),
                                    "System.DmlException",
                                )
                            })?;
                        let p = Savepoint { id };
                        if name == "rollback" {
                            self.events_rollback(id, at)?;
                        }
                        if name == "rollback" {
                            self.data_store.as_mut().unwrap().rollback(p)
                        } else {
                            self.data_store.as_mut().unwrap().release_savepoint(p)
                        }
                        .map_err(|e| store_error(at, e, "System.DmlException"))?;
                        if name == "releasesavepoint" {
                            self.events_release(id);
                        }
                        return Ok(Some(Value::Null));
                    }
                    _ => {}
                }
            }
            if ns.eq_ignore_ascii_case("test") && name == "setcreateddate" && args.len() == 2 {
                self.ensure_data_store(at)?;
                let Value::Str(id) = &args[0] else {
                    return fail(at, "setCreatedDate requires Id");
                };
                let DataValue::DateTime(date) = to_data(&args[1], at, 0)? else {
                    return fail(at, "setCreatedDate requires Datetime");
                };
                self.data_store
                    .as_mut()
                    .unwrap()
                    .set_created_date(id, &date)
                    .map_err(|e| store_error(at, e, "System.DmlException"))?;
                return Ok(Some(Value::Null));
            }
        }
        if let Value::Instance(instance) = receiver {
            let instance = instance.borrow();
            if instance.class.starts_with("database.") {
                let key = match name {
                    "issuccess" => "success",
                    "getid" => "id",
                    "iscreated" | "getcreated" => "created",
                    "geterrors" => "errors",
                    "getmessage" => "message",
                    "getstatuscode" => "statuscode",
                    "getfields" => "fields",
                    _ => return Ok(None),
                };
                if !args.is_empty() {
                    return fail(at, "Database result getter takes no arguments");
                }
                return Ok(Some(
                    instance.fields.get(key).cloned().unwrap_or(Value::Null),
                ));
            }
        }
        Ok(None)
    }
    pub(super) fn save_result_value(
        &self,
        result: &SaveResult,
        class: &str,
        at: &Node,
    ) -> R<Value> {
        let errors = result
            .errors
            .iter()
            .map(|e| {
                let fields = Value::list(e.fields.iter().map(|s| Value::Str(s.clone())).collect());
                if let Value::List(list) = &fields {
                    self.register_list_contract(list, "string");
                }
                native(
                    "Database.Error",
                    BTreeMap::from([
                        ("message".into(), Value::Str(e.message.clone())),
                        (
                            "statuscode".into(),
                            Value::Enum {
                                typ: "System.StatusCode".into(),
                                name: e.code.clone(),
                                ordinal: 0,
                            },
                        ),
                        ("fields".into(), fields),
                    ]),
                )
            })
            .collect();
        let errors = Value::list(errors);
        if let Value::List(list) = &errors {
            self.register_list_contract(list, "database.error");
        }
        let _ = at;
        Ok(native(
            class,
            BTreeMap::from([
                ("success".into(), Value::Bool(result.success)),
                (
                    "id".into(),
                    result
                        .id
                        .as_ref()
                        .map(|s| Value::Str(s.clone()))
                        .unwrap_or(Value::Null),
                ),
                ("created".into(), Value::Bool(result.created)),
                ("errors".into(), errors),
            ]),
        ))
    }
}
fn value_binds(value: &ValueExpr, out: &mut BTreeSet<String>) {
    match value {
        ValueExpr::Bind(name) => {
            out.insert(name.clone());
        }
        ValueExpr::List(values) => {
            for v in values {
                value_binds(v, out);
            }
        }
        ValueExpr::Subquery(query) => query_binds(query, out),
        _ => {}
    }
}
fn predicate_binds(p: &Predicate, out: &mut BTreeSet<String>) {
    match p {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            predicate_binds(a, out);
            predicate_binds(b, out);
        }
        Predicate::Not(p) => predicate_binds(p, out),
        Predicate::Compare { value, .. } => value_binds(value, out),
    }
}
pub(super) fn query_binds(query: &Query, out: &mut BTreeSet<String>) {
    if let Some(p) = &query.predicate {
        predicate_binds(p, out);
    }
    if let Some(v) = &query.limit {
        value_binds(v, out);
    }
    if let Some(v) = &query.offset {
        value_binds(v, out);
    }
    for s in &query.selections {
        if let Selection::Subquery(q) = s {
            query_binds(q, out);
        }
    }
}

fn inherit_query_mode(query: &mut Query) {
    for selection in &mut query.selections {
        if let Selection::Subquery(child) = selection {
            if !child.explicit_mode {
                child.mode = query.mode.clone();
            }
            inherit_query_mode(child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn execute(source: &str, extra: &str) -> (Program, Node) {
        let tree = crate::syntax::parse(source, "data-regression.cls").unwrap();
        let mut program = Program::default();
        let mut diagnostics = vec![];
        crate::syntax::declarations(&tree, &mut program, &mut diagnostics);
        if !extra.is_empty() {
            let trigger = crate::syntax::parse(extra, "trigger-regression.trigger").unwrap();
            crate::syntax::declarations(&trigger, &mut program, &mut diagnostics);
        }
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        (program, tree)
    }
    #[test]
    fn mixed_dml_resolves_prior_external_relationships_and_rolls_back_later_failure() {
        let(program,tree)=execute("class DataProbe { static void run(){ Account a=new Account(Name='Parent',AccountNumber='external'); Contact c=new Contact(LastName='Child',Account=new Account(AccountNumber='external')); insert new List<SObject>{a,c}; System.assertEquals(a.Id,[SELECT AccountId FROM Contact].AccountId); System.assertEquals(1,Limits.getDmlStatements()); try{insert new List<SObject>{new Account(Name='discard'),new Contact()};System.assert(false);}catch(DmlException e){} System.assertEquals(1,[SELECT COUNT() FROM Account]);System.assertEquals(1,[SELECT COUNT() FROM Contact]); }}","");
        let mut program = program;
        program
            .schema
            .objects
            .get_mut("account")
            .unwrap()
            .fields
            .get_mut("accountnumber")
            .unwrap()
            .external_id = true;
        let mut context = Context::new(&program);
        context.call("DataProbe", "run", vec![], &tree).unwrap();
    }
    #[test]
    fn mixed_upsert_keeps_results_and_single_statement() {
        let(program,tree)=execute("class DataProbe { static void run(){ Account a=new Account(Name='Old');insert a;a.Name='Changed';Account b=new Account(Name='New');List<Database.UpsertResult> r=Database.upsert(new List<Account>{a,b},false);System.assertEquals(2,r.size());System.assertEquals(false,r[0].isCreated());System.assertEquals(true,r[1].isCreated());System.assertEquals(2,Limits.getDmlStatements());System.assertEquals('Changed',[SELECT Name FROM Account WHERE Id=:a.Id].Name);System.assertEquals(2,[SELECT COUNT() FROM Account]); }}","");
        let mut context = Context::new(&program);
        context.call("DataProbe", "run", vec![], &tree).unwrap();
    }
    #[test]
    fn all_failed_partial_trigger_batch_rolls_back_nested_dml() {
        let(program,tree)=execute("class DataProbe { static void run(){List<Database.SaveResult> r=Database.insert(new List<Account>{new Account(),new Account()},false);System.assertEquals(2,r.size());System.assertEquals(false,r[0].isSuccess());System.assertEquals(false,r[1].isSuccess());System.assertEquals(0,[SELECT COUNT() FROM Contact]);System.assertEquals(0,[SELECT COUNT() FROM Account]);}}","trigger Audit on Account(before insert){insert new Contact(LastName='trigger side effect');}");
        let mut context = Context::new(&program);
        context.call("DataProbe", "run", vec![], &tree).unwrap();
    }
    #[test]
    fn qualified_child_projection_uses_relationship_name() {
        let(program,tree)=execute("class DataProbe { static void run(){Account a=new Account(Name='Parent');insert a;insert new Contact(LastName='Child',AccountId=a.Id);Account queried=[SELECT Id,(SELECT LastName FROM Account.Contacts) FROM Account];System.assertEquals(1,queried.getSObjects('Contacts').size());}}","");
        let mut context = Context::new(&program);
        context.call("DataProbe", "run", vec![], &tree).unwrap();
    }
    #[test]
    fn generated_readonly_defaults_are_not_writes() {
        let(program,tree)=execute("class DataProbe { static void run(){PermissionSet p=new PermissionSet(Name='P',Label='P');insert p;System.assertEquals(false,[SELECT IsOwnedByProfile FROM PermissionSet WHERE Id=:p.Id].IsOwnedByProfile);try{insert new PermissionSet(Name='Bad',IsOwnedByProfile=true);System.assert(false);}catch(DmlException e){}}}","");
        let mut context = Context::new(&program);
        context.call("DataProbe", "run", vec![], &tree).unwrap();
    }
    #[test]
    fn app_menu_blocks_mixed_dml_in_both_orders_and_scopes_permission_to_current_user() {
        let (mut program, tree) = execute(
            "class DataProbe {
                static void menuOnly(){AppLauncher.AppMenu.setAppVisibility('02u000000000001AAA',false);}
                static void menuThenDml(){menuOnly();insert new Account(Name='Blocked');}
                static void dmlThenMenu(){insert new Account(Name='Existing');menuOnly();}
                static void menuThenEmpty(){menuOnly();insert new List<Account>();}
            }",
            "",
        );
        let fixture = serde_json::json!({
            "schemaVersion": 1,
            "clock": {"now": "2026-01-15T12:00:00Z"},
            "currentUserId": "005000000000001AAA",
            "records": [{"object": "User", "fields": {
                "Id": "005000000000001AAA", "LastName": "Local"
            }}],
            "capabilities": {"appMenuCanCustomize": true}
        });
        program.environment = Some(
            crate::environment::Environment::parse(
                &fixture,
                &program.schema,
                "app-menu-regression",
            )
            .unwrap(),
        );
        let make_context = || {
            let mut context = Context::new(&program);
            context.ensure_data_store(&tree).unwrap();
            let store = context.data_store.as_mut().unwrap();
            store
                .upsert_platform_record(
                    &RecordData::new("AppDefinition")
                        .with("Id", DataValue::Text("02u000000000001AAA".into()))
                        .with("DeveloperName", DataValue::Text("First".into())),
                )
                .unwrap();
            store
                .upsert_platform_record(
                    &RecordData::new("AppMenuItem")
                        .with("Name", DataValue::Text("First".into()))
                        .with(
                            "ApplicationId",
                            DataValue::Text("02u000000000001AAA".into()),
                        )
                        .with("IsVisible", DataValue::Boolean(true)),
                )
                .unwrap();
            context
        };
        for (method, expected_accounts, expected_visibility) in
            [("menuThenDml", 0, false), ("dmlThenMenu", 1, true)]
        {
            let mut context = make_context();
            let error = context
                .call("DataProbe", method, vec![], &tree)
                .unwrap_err();
            assert_eq!(error.status, "blocked");
            assert!(error.diagnostic.message.contains("mixed-setup"));
            let store = context.data_store.as_ref().unwrap();
            assert_eq!(store.records("Account", false).len(), expected_accounts);
            assert_eq!(
                store.records("AppMenuItem", false)[0].get("IsVisible"),
                Some(&DataValue::Boolean(expected_visibility))
            );
        }
        let mut context = make_context();
        context
            .call("DataProbe", "menuThenEmpty", vec![], &tree)
            .unwrap();
        let mut context = make_context();
        context.native_state.insert(
            "userinfo.userid".into(),
            Value::Str("005000000000002AAA".into()),
        );
        let error = context
            .call("DataProbe", "menuOnly", vec![], &tree)
            .unwrap_err();
        assert_eq!(error.status, "blocked");
        assert!(error
            .diagnostic
            .message
            .contains("permission for the current user"));
        assert_eq!(
            context
                .data_store
                .as_ref()
                .unwrap()
                .records("AppMenuItem", false)[0]
                .get("IsVisible"),
            Some(&DataValue::Boolean(true))
        );
    }
}
