use super::*;
use crate::schema::FieldKind;

fn token(kind: &str, name: &str) -> Value {
    Value::Type(format!("Schema.{kind}:{name}"))
}
fn parts(value: &Value) -> Option<(&str, &str)> {
    let Value::Type(s) = value else {
        return None;
    };
    s.strip_prefix("Schema.")?.split_once(':')
}
fn field_name(value: &Value, object: &str, at: &Node) -> R<String> {
    match value {
        Value::Str(s) => Ok(s.clone()),
        Value::Type(s) => {
            let Some((owner, name)) = s
                .strip_prefix("Schema.SObjectField:")
                .and_then(|s| s.split_once('.'))
            else {
                return fail(at, "Expected a field name or SObjectField token");
            };
            if !owner.eq_ignore_ascii_case(object) {
                return fail(at, "Field token belongs to a different object");
            }
            Ok(name.into())
        }
        _ => fail(at, "Expected a field name or SObjectField token"),
    }
}
impl Context<'_> {
    pub(super) fn schema_get(
        &mut self,
        receiver: &Value,
        field: &str,
        at: &Node,
    ) -> R<Option<Value>> {
        if let Value::Namespace(ns) = receiver {
            if ns.eq_ignore_ascii_case("schema") {
                if field.eq_ignore_ascii_case("SObjectType") {
                    return Ok(Some(token("SObjectTypeRoot", "")));
                }
                if let Some(object) = self.program.schema.object(field) {
                    return Ok(Some(Value::Namespace(object.name.clone())));
                }
                return Ok(None);
            }
            if let Some(object) = self.program.schema.object(ns) {
                if field.eq_ignore_ascii_case("SObjectType") {
                    return Ok(Some(token("SObjectType", &object.name)));
                }
                if field.eq_ignore_ascii_case("fields") {
                    return Ok(Some(token("FieldMap", &object.name)));
                }
                if let Some(f) = object.field(field) {
                    return Ok(Some(token(
                        "SObjectField",
                        &format!("{}.{}", object.name, f.name),
                    )));
                }
            }
        }
        if let Some((kind, name)) = parts(receiver) {
            match kind {
                "SObjectTypeRoot" => {
                    let object = self.program.schema.object(field).ok_or_else(|| EvalError {
                        exception: None,
                        status: "blocked",
                        diagnostic: at
                            .diagnostic("APX_UNSUPPORTED", format!("Unknown object {field}")),
                    })?;
                    return Ok(Some(token("SObjectType", &object.name)));
                }
                "SObjectType" | "DescribeSObjectResult" if field.eq_ignore_ascii_case("fields") => {
                    return Ok(Some(token("FieldMap", name)))
                }
                "FieldMap" => {
                    let f = self
                        .program
                        .schema
                        .field(name, field)
                        .ok_or_else(|| EvalError {
                            exception: None,
                            status: "failed",
                            diagnostic: at
                                .diagnostic("APX_RUNTIME", format!("Unknown field {name}.{field}")),
                        })?;
                    return Ok(Some(token("SObjectField", &format!("{name}.{}", f.name))));
                }
                _ => {}
            }
        }
        Ok(None)
    }
    pub(super) fn schema_method(
        &mut self,
        receiver: &Value,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        let method = name.to_ascii_lowercase();
        if let Value::Namespace(ns) = receiver {
            if method == "getsobjecttype" && args.is_empty() {
                if let Some(object) = self.program.schema.object(ns) {
                    return Ok(Some(token("SObjectType", &object.name)));
                }
            }
        }
        if let Value::Namespace(ns) = receiver {
            if ns.eq_ignore_ascii_case("string") && method == "valueof" && args.len() == 1 {
                if let Some((kind, name)) = parts(&args[0]) {
                    let display = match kind {
                        "SObjectType" => Some(name),
                        "SObjectField" => name.split_once('.').map(|(_, f)| f),
                        _ => None,
                    };
                    if let Some(display) = display {
                        return Ok(Some(Value::Str(display.into())));
                    }
                }
            }
            if ns.eq_ignore_ascii_case("schema") {
                match method.as_str() {
                    "getglobaldescribe" if args.is_empty() => {
                        let entries = self
                            .program
                            .schema
                            .objects
                            .values()
                            .map(|o| (Value::Str(o.name.clone()), token("SObjectType", &o.name)))
                            .collect();
                        return Ok(Some(self.coerce(
                            Value::Map(Rc::new(RefCell::new(entries))),
                            "Map<String,Schema.SObjectType>",
                            at,
                        )?));
                    }
                    "describesobjects"
                        if args.len() == 1
                            || matches!(args,[_,Value::Enum{typ,name,..}]if typ.eq_ignore_ascii_case("Schema.SObjectDescribeOptions")&&["DEFERRED","FULL"].contains(&name.as_str())) =>
                    {
                        let Value::List(values) = &args[0] else {
                            return fail(at, "describeSObjects requires List<String>");
                        };
                        let mut out = vec![];
                        for v in values.borrow().iter() {
                            let Value::Str(name) = v else {
                                return fail(at, "describeSObjects object name must be String");
                            };
                            let object =
                                self.program.schema.object(name).ok_or_else(|| EvalError {
                                    exception: None,
                                    status: "failed",
                                    diagnostic: at.diagnostic(
                                        "APX_RUNTIME",
                                        format!("Unknown object {name}"),
                                    ),
                                })?;
                            out.push(token("DescribeSObjectResult", &object.name));
                        }
                        return Ok(Some(Value::list(out)));
                    }
                    _ => {}
                }
            }
        }
        if let Value::Str(id) = receiver {
            if method == "getsobjecttype" && args.is_empty() {
                if !id.is_ascii() || (id.len() != 15 && id.len() != 18) {
                    return fail(at, "Invalid Id length");
                }
                let prefix = &id[..3];
                let matches = self
                    .program
                    .schema
                    .objects
                    .values()
                    .filter(|o| o.key_prefix == prefix)
                    .collect::<Vec<_>>();
                if matches.len() != 1 {
                    return block(at, "Id key prefix has no unique known object type");
                }
                return Ok(Some(token("SObjectType", &matches[0].name)));
            }
        }
        if let Value::Record(record) = receiver {
            match method.as_str() {
                "getsobjecttype" if args.is_empty() => {
                    let typ = record.borrow().typ.clone();
                    let object = self.program.schema.object(&typ).ok_or_else(|| EvalError {
                        exception: None,
                        status: "blocked",
                        diagnostic: at
                            .diagnostic("APX_UNSUPPORTED", format!("Unknown object {typ}")),
                    })?;
                    return Ok(Some(token("SObjectType", &object.name)));
                }
                "get" | "getsobject" | "getsobjects" if args.len() == 1 => {
                    let record = record.borrow();
                    let field = field_name(&args[0], &record.typ, at)?;
                    let key = field.to_ascii_lowercase();
                    if method == "get"
                        && self.program.schema.field(&record.typ, &field).is_none()
                        && record.typ != "AggregateResult"
                    {
                        return fail(at, format!("Unknown field {}.{field}", record.typ));
                    }
                    if let Some(v) = record.fields.get(&key) {
                        return Ok(Some(v.clone()));
                    }
                    if record.projected {
                        return exceptions::raise(at,"System.SObjectException",format!("SObject row was retrieved via SOQL without querying the requested field: {}.{field}",record.typ));
                    }
                    return Ok(Some(Value::Null));
                }
                "put" if args.len() == 2 => {
                    self.data_record_writable(record, at)?;
                    let typ = record.borrow().typ.clone();
                    let field = field_name(&args[0], &typ, at)?;
                    let schema = self
                        .program
                        .schema
                        .field(&typ, &field)
                        .cloned()
                        .ok_or_else(|| EvalError {
                            exception: None,
                            status: "failed",
                            diagnostic: at
                                .diagnostic("APX_RUNTIME", format!("Unknown field {typ}.{field}")),
                        })?;
                    if !schema.writable && schema.name != "Id" {
                        return fail(at, format!("Field {typ}.{field} is not writable"));
                    }
                    let value = self.coerce(args[1].clone(), schema.apex_type(), at)?;
                    let old = record
                        .borrow_mut()
                        .fields
                        .insert(field.to_ascii_lowercase(), value)
                        .unwrap_or(Value::Null);
                    return Ok(Some(old));
                }
                "getpopulatedfieldsasmap" if args.is_empty() => {
                    let record = record.borrow();
                    let schema = self.program.schema.object(&record.typ);
                    let entries = record
                        .fields
                        .iter()
                        .map(|(k, v)| {
                            let name = schema
                                .and_then(|s| s.field(k))
                                .map(|f| f.name.clone())
                                .unwrap_or_else(|| k.clone());
                            (Value::Str(name), v.clone())
                        })
                        .collect();
                    return Ok(Some(self.coerce(
                        Value::Map(Rc::new(RefCell::new(entries))),
                        "Map<String,Object>",
                        at,
                    )?));
                }
                "clone" if args.len() <= 4 => {
                    let mut flags = [false; 4];
                    for (i, a) in args.iter().enumerate() {
                        let Value::Bool(v) = a else {
                            return fail(at, "SObject.clone options must be Boolean");
                        };
                        flags[i] = *v;
                    }
                    let original = record.borrow();
                    let mut fields = BTreeMap::new();
                    for (key, value) in &original.fields {
                        if key == "isdeleted" {
                            continue;
                        }
                        if key == "id" && !flags[0] {
                            continue;
                        }
                        if [
                            "createddate",
                            "createdbyid",
                            "lastmodifieddate",
                            "lastmodifiedbyid",
                            "systemmodstamp",
                        ]
                        .contains(&key.as_str())
                            && !flags[2]
                        {
                            continue;
                        }
                        if !flags[3]
                            && self
                                .program
                                .schema
                                .field(&original.typ, key)
                                .is_some_and(|f| f.kind == FieldKind::AutoNumber)
                        {
                            continue;
                        }
                        fields.insert(
                            key.clone(),
                            if flags[1] {
                                deep_value(value, 0, at)?
                            } else {
                                value.clone()
                            },
                        );
                    }
                    return Ok(Some(Value::Record(Rc::new(RefCell::new(Record {
                        typ: original.typ.clone(),
                        fields,
                        projected: original.projected,
                    })))));
                }
                _ => {}
            }
        }
        if let Some((kind, qualified)) = parts(receiver) {
            if [
                "isaccessible",
                "iscreateable",
                "isupdateable",
                "isdeletable",
                "isundeletable",
            ]
            .contains(&method.as_str())
                && args.is_empty()
            {
                let access = self.data_access(at)?;
                let permitted = if ["SObjectField", "DescribeFieldResult"].contains(&kind) {
                    let permission = access
                        .fields
                        .get(&qualified.to_ascii_lowercase())
                        .cloned()
                        .unwrap_or_default();
                    match method.as_str() {
                        "isaccessible" => permission.read,
                        "iscreateable" | "isupdateable" => permission.edit,
                        _ => return fail(at, "Unsupported field permission method"),
                    }
                } else {
                    let permission = access
                        .objects
                        .get(&qualified.to_ascii_lowercase())
                        .cloned()
                        .unwrap_or_default();
                    match method.as_str() {
                        "isaccessible" => permission.read,
                        "iscreateable" | "isundeletable" => permission.create,
                        "isupdateable" => permission.edit,
                        "isdeletable" => permission.delete,
                        _ => unreachable!(),
                    }
                };
                return Ok(Some(Value::Bool(permitted)));
            }
            match kind {
                "SObjectType" | "DescribeSObjectResult" => {
                    let object =
                        self.program
                            .schema
                            .object(qualified)
                            .cloned()
                            .ok_or_else(|| EvalError {
                                exception: None,
                                status: "blocked",
                                diagnostic: at
                                    .diagnostic("APX_UNSUPPORTED", "Unknown schema object token"),
                            })?;
                    match method.as_str() {
                        "getdescribe"
                            if args.is_empty()
                                || matches!(args,[Value::Enum{typ,name,..}]if typ.eq_ignore_ascii_case("Schema.SObjectDescribeOptions")&&["DEFERRED","FULL"].contains(&name.as_str())) =>
                        {
                            return Ok(Some(token("DescribeSObjectResult", &object.name)))
                        }
                        "getsobjecttype" if args.is_empty() => {
                            return Ok(Some(token("SObjectType", &object.name)))
                        }
                        "getname" if args.is_empty() => {
                            return Ok(Some(Value::Str(object.name.clone())))
                        }
                        "getkeyprefix" if args.is_empty() => {
                            return Ok(Some(Value::Str(object.key_prefix.clone())))
                        }
                        "iscustom" if args.is_empty() => {
                            return Ok(Some(Value::Bool(
                                object.name.ends_with("__c")
                                    || object.custom_metadata
                                    || object.platform_event,
                            )))
                        }
                        "newsobject" if args.len() <= 2 => {
                            if args.first().is_some_and(|v| !matches!(v, Value::Null)) {
                                return block(at,"Record-type-specific newSObject defaults require record type metadata");
                            }
                            let defaults = match args.get(1) {
                                None => false,
                                Some(Value::Bool(v)) => *v,
                                _ => return fail(at, "newSObject loadDefaults must be Boolean"),
                            };
                            let mut fields = BTreeMap::new();
                            if defaults {
                                for field in object.fields.values() {
                                    if let Some(raw) = &field.default_value {
                                        let v = match field.kind {
                                            FieldKind::Boolean => match raw.as_str() {
                                                "true" => Value::Bool(true),
                                                "false" => Value::Bool(false),
                                                _ => {
                                                    return block(
                                                        at,
                                                        "Formula default not implemented",
                                                    )
                                                }
                                            },
                                            FieldKind::Integer
                                            | FieldKind::Long
                                            | FieldKind::Decimal => {
                                                Value::Num(Number::parse(raw).map_err(|e| {
                                                    EvalError {
                                                        exception: None,
                                                        status: "failed",
                                                        diagnostic: at.diagnostic("APX_NUMBER", e),
                                                    }
                                                })?)
                                            }
                                            FieldKind::Text | FieldKind::Picklist => Value::Str(
                                                raw.strip_prefix('"')
                                                    .and_then(|s| s.strip_suffix('"'))
                                                    .ok_or_else(|| EvalError {
                                                        exception: None,
                                                        status: "blocked",
                                                        diagnostic: at.diagnostic(
                                                            "APX_UNSUPPORTED",
                                                            "Formula default not implemented",
                                                        ),
                                                    })?
                                                    .into(),
                                            ),
                                            _ => {
                                                return block(at, "Field default type unsupported")
                                            }
                                        };
                                        fields.insert(field.name.to_ascii_lowercase(), v);
                                    }
                                }
                            }
                            let record = Rc::new(RefCell::new(Record {
                                typ: object.name.clone(),
                                fields,
                                projected: false,
                            }));
                            if object.platform_event && defaults {
                                self.initialize_event_uuid(&record, at)?;
                            }
                            return Ok(Some(Value::Record(record)));
                        }
                        _ => {}
                    }
                }
                "FieldMap" if method == "getmap" && args.is_empty() => {
                    let object =
                        self.program
                            .schema
                            .object(qualified)
                            .ok_or_else(|| EvalError {
                                exception: None,
                                status: "blocked",
                                diagnostic: at
                                    .diagnostic("APX_UNSUPPORTED", "Unknown schema fields token"),
                            })?;
                    let entries = object
                        .fields
                        .values()
                        .map(|f| {
                            (
                                Value::Str(f.name.clone()),
                                token("SObjectField", &format!("{}.{}", object.name, f.name)),
                            )
                        })
                        .collect();
                    return Ok(Some(self.coerce(
                        Value::Map(Rc::new(RefCell::new(entries))),
                        "Map<String,Schema.SObjectField>",
                        at,
                    )?));
                }
                "SObjectField" | "DescribeFieldResult" => {
                    let (object, name) = qualified.split_once('.').ok_or_else(|| EvalError {
                        exception: None,
                        status: "blocked",
                        diagnostic: at.diagnostic("APX_UNSUPPORTED", "Invalid schema field token"),
                    })?;
                    let field =
                        self.program
                            .schema
                            .field(object, name)
                            .ok_or_else(|| EvalError {
                                exception: None,
                                status: "failed",
                                diagnostic: at
                                    .diagnostic("APX_RUNTIME", "Unknown schema field token"),
                            })?;
                    match method.as_str() {
                        "getdescribe" if args.is_empty() => {
                            return Ok(Some(token("DescribeFieldResult", qualified)))
                        }
                        "getsobjectfield" if args.is_empty() => {
                            return Ok(Some(token("SObjectField", qualified)))
                        }
                        "getname" if args.is_empty() => {
                            return Ok(Some(Value::Str(field.name.clone())))
                        }
                        "isautonumber" if args.is_empty() => {
                            return Ok(Some(Value::Bool(field.kind == FieldKind::AutoNumber)))
                        }
                        "isnillable" if args.is_empty() => {
                            return Ok(Some(Value::Bool(
                                !field.required
                                    && !matches!(field.kind, FieldKind::Id | FieldKind::AutoNumber)
                                    && ![
                                        "createddate",
                                        "createdbyid",
                                        "lastmodifieddate",
                                        "lastmodifiedbyid",
                                        "systemmodstamp",
                                        "isdeleted",
                                    ]
                                    .contains(&field.name.to_ascii_lowercase().as_str()),
                            )))
                        }
                        "isunique" if args.is_empty() => {
                            return Ok(Some(Value::Bool(field.unique)))
                        }
                        "isexternalid" if args.is_empty() => {
                            return Ok(Some(Value::Bool(field.external_id)))
                        }
                        "iscalculated" if args.is_empty() => {
                            return Ok(Some(Value::Bool(field.formula.is_some())))
                        }
                        "getreferenceTo" | "getreferenceto" if args.is_empty() => {
                            return Ok(Some(Value::list(
                                field
                                    .reference_to
                                    .iter()
                                    .map(|n| token("SObjectType", n))
                                    .collect(),
                            )))
                        }
                        "getrelationshipname" if args.is_empty() => {
                            return Ok(Some(
                                field
                                    .relationship_name
                                    .as_ref()
                                    .map(|s| Value::Str(s.clone()))
                                    .unwrap_or(Value::Null),
                            ))
                        }
                        "getlength" if args.is_empty() => {
                            return field
                                .length
                                .map(|n| Some(Value::Num(Number::integer(n as i64))))
                                .ok_or_else(|| EvalError {
                                    exception: None,
                                    status: "blocked",
                                    diagnostic: at.diagnostic(
                                        "APX_UNSUPPORTED",
                                        "Field length is not available in the local schema",
                                    ),
                                })
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        Ok(None)
    }
}
fn deep_value(value: &Value, depth: usize, at: &Node) -> R<Value> {
    if depth > 32 {
        return block(at, "SObject deep clone exceeds reference depth bound");
    }
    match value {
        Value::Record(record) => {
            let r = record.borrow();
            let fields = r
                .fields
                .iter()
                .map(|(k, v)| Ok((k.clone(), deep_value(v, depth + 1, at)?)))
                .collect::<R<BTreeMap<_, _>>>()?;
            Ok(Value::Record(Rc::new(RefCell::new(Record {
                typ: r.typ.clone(),
                fields,
                projected: r.projected,
            }))))
        }
        Value::List(values) => Ok(Value::list(
            values
                .borrow()
                .iter()
                .map(|v| deep_value(v, depth + 1, at))
                .collect::<R<Vec<_>>>()?,
        )),
        v if v.is_scalar() || matches!(v, Value::Blob(_) | Value::Type(_)) => Ok(v.clone()),
        _ => block(at, "SObject clone contains an unsupported reference value"),
    }
}
