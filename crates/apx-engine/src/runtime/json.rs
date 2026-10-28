use super::*;
use crate::value::Instance;
use base64::{engine::general_purpose::STANDARD, Engine};
use num_traits::ToPrimitive;
use serde_json::Value as Json;
use std::str::FromStr;

impl Context<'_> {
    pub(super) fn json_call(&mut self, name: &str, args: Vec<Value>, at: &Node) -> R<Value> {
        match (name, args.as_slice()) {
            ("serialize" | "serializepretty", [value])
            | ("serialize" | "serializepretty", [value, Value::Bool(false)]) => {
                let json = self.encode_json(value, at, 0, false, &Cell::new(0))?;
                let text = if name == "serializepretty" {
                    serde_json::to_string_pretty(&json)
                } else {
                    serde_json::to_string(&json)
                }
                .map_err(|e| EvalError {
                    exception: None,
                    status: "blocked",
                    diagnostic: at.diagnostic("APX_JSON", e.to_string()),
                })?;
                if text.len() > 1_048_576 {
                    return block(at, "JSON output exceeds bounded 1 MiB host allocation");
                }
                Ok(Value::Str(text))
            }
            ("serialize" | "serializepretty", [value, Value::Bool(true)]) => {
                let json = self.encode_json(value, at, 0, true, &Cell::new(0))?;
                let text = if name == "serializepretty" {
                    serde_json::to_string_pretty(&json)
                } else {
                    serde_json::to_string(&json)
                }
                .map_err(|e| EvalError {
                    exception: None,
                    status: "blocked",
                    diagnostic: at.diagnostic("APX_JSON", e.to_string()),
                })?;
                if text.len() > 1_048_576 {
                    return block(at, "JSON output exceeds bounded 1 MiB host allocation");
                }
                Ok(Value::Str(text))
            }
            ("deserializeuntyped", [Value::Str(text)]) => {
                let json = self.parse_json(text, at)?;
                self.decode_json(&json, None, false, at, 0)
            }
            ("deserialize" | "deserializestrict", [Value::Str(text), Value::Type(typ)]) => {
                self.json_known_type(typ, at, 0)?;
                let json = self.parse_json(text, at)?;
                self.decode_json(&json, Some(typ), name == "deserializestrict", at, 0)
            }
            ("deserialize" | "deserializestrict" | "deserializeuntyped", [Value::Null, ..]) => {
                exceptions::raise(at, "NullPointerException", "JSON input cannot be null")
            }
            _ => block(at, format!("JSON.{name} overload is not implemented")),
        }
    }
    fn parse_json(&self, text: &str, at: &Node) -> R<Json> {
        if text.len() > 1_048_576 {
            return block(at, "JSON input exceeds bounded 1 MiB host allocation");
        }
        serde_json::from_str(text).map_err(|e| {
            let message = if e.is_eof() {
                format!("Unexpected end-of-input: {e}")
            } else {
                format!("Malformed JSON: {e}")
            };
            self.json_error(at, message)
        })
    }
    fn json_class_name(&self, typ: &str, owner: Option<&str>) -> Option<String> {
        let key = typ.to_ascii_lowercase();
        if self.program.classes.contains_key(&key) {
            return Some(key);
        }
        let mut outer = owner.unwrap_or(&self.current_class).to_ascii_lowercase();
        loop {
            let nested = format!("{outer}.{key}");
            if self.program.classes.contains_key(&nested) {
                return Some(nested);
            }
            let Some((parent, _)) = outer.rsplit_once('.') else {
                break;
            };
            outer = parent.to_string();
        }
        None
    }
    fn json_known_type(&self, typ: &str, at: &Node, depth: usize) -> R<()> {
        if depth > 64 {
            return block(at, "JSON type contract exceeds depth bound");
        }
        let typ = typ.replace(' ', "").to_ascii_lowercase();
        if let Some(t) = typ.strip_suffix("[]") {
            return self.json_known_type(t, at, depth + 1);
        }
        if let Some((kind, parts)) = collections::generic(&typ) {
            if !["list", "set", "map"].contains(&kind) {
                return block(at, "Unknown JSON collection type");
            }
            for p in parts {
                self.json_known_type(&p, at, depth + 1)?;
            }
            return Ok(());
        }
        if [
            "string", "integer", "long", "decimal", "boolean", "blob", "date", "datetime",
            "object", "id", "sobject",
        ]
        .contains(&typ.as_str())
            || self.program.schema.object(&typ).is_some()
            || self.json_class_name(&typ, None).is_some()
        {
            Ok(())
        } else {
            block(at, format!("Unknown JSON type {typ}"))
        }
    }
    fn json_field_types(
        &self,
        class: &str,
        at: &Node,
    ) -> R<BTreeMap<String, (String, String, bool)>> {
        let mut result = BTreeMap::new();
        let mut next = Some(class.to_string());
        let mut seen = BTreeSet::new();
        while let Some(key) = next {
            if !seen.insert(key.clone()) {
                return block(at, "Cyclic class ancestry in JSON type");
            }
            let Some(class) = self.program.classes.get(&key) else {
                return block(at, "JSON superclass type is unavailable");
            };
            if class.kind != "class_declaration" {
                return block(at, "JSON object target must be a class");
            }
            for field in &class.instance_fields {
                let Some(typ) = field.field("type") else {
                    return block(field, "JSON property accessor is not implemented");
                };
                let transient = field.child("modifiers").is_some_and(|m| {
                    m.text()
                        .split_whitespace()
                        .any(|s| s.eq_ignore_ascii_case("transient"))
                });
                for declarator in field.named().filter(|n| n.kind == "variable_declarator") {
                    let Some(name) = declarator.field("name") else {
                        continue;
                    };
                    let typ = self.json_qualified_type(typ.text(), &key);
                    self.json_known_type(&typ, at, 0)?;
                    result.entry(name.text().to_ascii_lowercase()).or_insert((
                        name.text().into(),
                        typ,
                        transient,
                    ));
                }
            }
            next = if let Some(parent) = class.superclass.as_deref() {
                Some(
                    self.json_class_name(parent, class.parent.as_deref())
                        .ok_or_else(|| EvalError {
                            exception: None,
                            status: "blocked",
                            diagnostic: at.diagnostic(
                                "APX_UNSUPPORTED",
                                format!("Unknown JSON superclass {parent}"),
                            ),
                        })?,
                )
            } else {
                None
            };
        }
        Ok(result)
    }
    fn json_qualified_type(&self, typ: &str, owner: &str) -> String {
        let typ = typ.replace(' ', "");
        if let Some((kind, parts)) = collections::generic(&typ) {
            return format!(
                "{}<{}>",
                kind,
                parts
                    .iter()
                    .map(|p| self.json_qualified_type(p, owner))
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
        if let Some(element) = typ.strip_suffix("[]") {
            return format!("List<{}>", self.json_qualified_type(element, owner));
        }
        self.json_class_name(&typ, Some(owner)).unwrap_or(typ)
    }
    fn decode_json(
        &self,
        json: &Json,
        typ: Option<&str>,
        strict: bool,
        at: &Node,
        depth: usize,
    ) -> R<Value> {
        if depth > 64 {
            return block(at, "JSON value depth exceeds 64-level host bound");
        }
        if matches!(json, Json::Null) {
            return Ok(Value::Null);
        }
        let requested = typ.map(|t| t.replace(' ', "").to_ascii_lowercase());
        let generic = requested.as_deref().and_then(collections::generic);
        let value = match json {
            Json::Null => Value::Null,
            Json::Bool(v) => Value::Bool(*v),
            Json::String(s) => {
                if requested.as_deref() == Some("date") {
                    crate::dates::parse_date(s).map_err(|e| self.json_error(at, e.message))?
                } else if requested.as_deref() == Some("datetime") {
                    crate::dates::parse_datetime(s).map_err(|e| self.json_error(at, e.message))?
                } else if requested.as_deref() == Some("blob") {
                    Value::Blob(
                        STANDARD.decode(s).map_err(|_| {
                            self.json_error(at, "Invalid base64 in Blob JSON value")
                        })?,
                    )
                } else if let Some(class) = requested
                    .as_deref()
                    .and_then(|t| self.json_class_name(t, None))
                    .and_then(|n| self.program.classes.get(&n))
                {
                    if class.kind == "enum_declaration" {
                        let Some(index) = class
                            .enum_members
                            .iter()
                            .position(|v| v.eq_ignore_ascii_case(s))
                        else {
                            return Err(self.json_error(at, "Invalid enum value"));
                        };
                        Value::Enum {
                            typ: class.name.to_ascii_lowercase(),
                            name: class.enum_members[index].clone(),
                            ordinal: index,
                        }
                    } else {
                        Value::Str(s.clone())
                    }
                } else {
                    Value::Str(s.clone())
                }
            }
            Json::Number(n) => {
                let text = n.to_string();
                let mut n = Number::parse(&text).map_err(|e| self.json_error(at, e))?;
                if n.kind == NumKind::Int && n.q.to_integer().to_i32().is_none() {
                    n.kind = if n.q.to_integer().to_i64().is_some() {
                        NumKind::Long
                    } else {
                        NumKind::Decimal
                    };
                }
                Value::Num(n)
            }
            Json::Array(values) => {
                let element = match &generic {
                    Some(("list" | "set", params)) if params.len() == 1 => Some(params[0].as_str()),
                    _ => None,
                };
                if requested.is_some()
                    && element.is_none()
                    && requested.as_deref() != Some("object")
                {
                    return Err(
                        self.json_error(at, "Expected object or scalar JSON for requested type")
                    );
                }
                let items = values
                    .iter()
                    .map(|v| self.decode_json(v, element, strict, at, depth + 1))
                    .collect::<R<Vec<_>>>()?;
                if matches!(generic, Some(("set", _))) {
                    let mut unique = vec![];
                    for value in items {
                        if !unique.iter().any(|v: &Value| v.equal(&value)) {
                            unique.push(value);
                        }
                    }
                    let items = Rc::new(RefCell::new(unique));
                    self.register_set_contract(&items, element.unwrap_or("object"));
                    Value::Set(items)
                } else {
                    let items = Rc::new(RefCell::new(items));
                    self.register_list_contract(&items, element.unwrap_or("object"));
                    Value::List(items)
                }
            }
            Json::Object(object) => {
                if let Some(class_name) = requested
                    .as_deref()
                    .and_then(|t| self.json_class_name(t, None))
                {
                    let fields = self.json_field_types(&class_name, at)?;
                    let mut instance = Instance {
                        class: class_name,
                        fields: BTreeMap::new(),
                        field_types: BTreeMap::new(),
                    };
                    for (key, (_, typ, _)) in &fields {
                        instance.fields.insert(key.clone(), Value::Null);
                        instance.field_types.insert(key.clone(), typ.clone());
                    }
                    for (name, value) in object {
                        let key = name.to_ascii_lowercase();
                        if let Some((_, typ, _)) = fields.get(&key) {
                            instance.fields.insert(
                                key,
                                self.decode_json(value, Some(typ), strict, at, depth + 1)?,
                            );
                        } else if strict {
                            return Err(self.json_error(at, format!("Unknown field {name}")));
                        }
                    }
                    Value::Instance(Rc::new(RefCell::new(instance)))
                } else if let Some(record_type) = requested
                    .as_deref()
                    .filter(|t| self.program.schema.object(t).is_some() || *t == "sobject")
                {
                    let named = if record_type == "sobject" {
                        object
                            .get("attributes")
                            .and_then(|x| x.get("type"))
                            .and_then(Json::as_str)
                            .ok_or_else(|| {
                                self.json_error(at, "SObject JSON requires attributes.type")
                            })?
                    } else {
                        record_type
                    };
                    let schema = self
                        .program
                        .schema
                        .object(named)
                        .ok_or_else(|| self.json_error(at, "Unknown SObject JSON type"))?;
                    let mut record = Record {
                        typ: schema.name.to_ascii_lowercase(),
                        fields: BTreeMap::new(),
                        projected: false,
                    };
                    for (name, value) in object {
                        if name == "attributes" {
                            continue;
                        }
                        let Some(field) = schema.field(name) else {
                            if !strict {
                                let Some(version) = self.program.api_versions.get(at.path.as_ref())
                                else {
                                    return block(at,"SObject JSON extra-field behavior requires the caller API version");
                                };
                                if *version >= 35 {
                                    // JSON.deserialize ignores scalar extras from API 35.
                                    // Relationship/object extras have a documented platform
                                    // caveat (W-6827544) and remain explicitly unqualified.
                                    if value.is_object() || value.is_array() {
                                        return block(at,"SObject JSON unknown relationship/object fields are not qualified");
                                    }
                                    continue;
                                }
                            }
                            return Err(self.json_error(at, format!("Unknown field {name}")));
                        };
                        record.fields.insert(
                            name.to_ascii_lowercase(),
                            self.decode_json(
                                value,
                                Some(field.apex_type()),
                                strict,
                                at,
                                depth + 1,
                            )?,
                        );
                    }
                    Value::Record(Rc::new(RefCell::new(record)))
                } else {
                    let (key_type, value_type) = match &generic {
                        Some(("map", p)) if p.len() == 2 => (p[0].as_str(), p[1].as_str()),
                        _ if requested.is_none() || requested.as_deref() == Some("object") => {
                            ("string", "object")
                        }
                        _ => {
                            return Err(self
                                .json_error(at, "JSON object is incompatible with requested type"))
                        }
                    };
                    if !["string", "id", "object"].contains(&key_type) {
                        return block(at, "JSON Map keys require String or Id");
                    }
                    let mut entries = vec![];
                    for (key, value) in object {
                        entries.push((
                            Value::Str(key.clone()),
                            self.decode_json(value, Some(value_type), strict, at, depth + 1)?,
                        ));
                    }
                    let entries = Rc::new(RefCell::new(entries));
                    self.register_map_contract(&entries, key_type, value_type);
                    Value::Map(entries)
                }
            }
        };
        self.charge(&value, at)?;
        if let Some(typ) = requested.filter(|t| t != "object") {
            self.coerce(value, &typ, at)
        } else {
            Ok(value)
        }
    }
    fn json_error(&self, at: &Node, message: impl Into<String>) -> EvalError {
        let message = message.into();
        EvalError {
            exception: Some(Value::Exception(Rc::new(RefCell::new(ExceptionValue {
                typ: "System.JSONException".into(),
                message: message.clone(),
                stack: format!("APX local {}:{}", at.path, at.line()),
            })))),
            status: "failed",
            diagnostic: at.diagnostic("APX_JSON", message),
        }
    }
    fn encode_json(
        &self,
        value: &Value,
        at: &Node,
        depth: usize,
        suppress_nulls: bool,
        budget: &Cell<usize>,
    ) -> R<Json> {
        if depth > 64 {
            return block(
                at,
                "Cyclic or deeply nested JSON graph exceeds host depth bound",
            );
        }
        let cost = 64
            + match value {
                Value::Str(s) => s.len(),
                Value::Blob(b) => b.len().saturating_mul(4) / 3 + 4,
                _ => 0,
            };
        budget.set(budget.get().saturating_add(cost));
        if budget.get() > 1_048_576 {
            return block(
                at,
                "Expanded JSON graph exceeds bounded 1 MiB host allocation",
            );
        }
        Ok(match value {
            Value::Null => Json::Null,
            Value::Bool(b) => Json::Bool(*b),
            Value::Str(s) => Json::String(s.clone()),
            Value::Blob(b) => Json::String(STANDARD.encode(b)),
            Value::Date(d) => Json::String(
                crate::dates::date_iso(*d).map_err(|e| self.json_error(at, e.message))?,
            ),
            Value::Datetime(d) => Json::String(
                crate::dates::datetime_iso(*d).map_err(|e| self.json_error(at, e.message))?,
            ),
            Value::Num(n) => {
                Json::Number(serde_json::Number::from_str(&n.display()).map_err(|_| {
                    self.json_error(
                        at,
                        "Number cannot be represented exactly by this JSON implementation",
                    )
                })?)
            }
            Value::Enum { name, .. } => Json::String(name.clone()),
            Value::List(items) | Value::Set(items) | Value::Iterable(items) => Json::Array(
                items
                    .borrow()
                    .iter()
                    .map(|v| self.encode_json(v, at, depth + 1, suppress_nulls, budget))
                    .collect::<R<Vec<_>>>()?,
            ),
            Value::Map(items) => {
                let mut object = serde_json::Map::new();
                for (key, value) in items.borrow().iter() {
                    let key = match key {
                        Value::Str(s) => s.clone(),
                        Value::Num(n) => n.display(),
                        _ => return block(at, "JSON Map key type is not implemented"),
                    };
                    budget.set(budget.get().saturating_add(key.len()));
                    object.insert(
                        key,
                        self.encode_json(value, at, depth + 1, suppress_nulls, budget)?,
                    );
                }
                Json::Object(object)
            }
            Value::Instance(instance) => {
                let instance = instance.borrow();
                let fields = self.json_field_types(&instance.class, at)?;
                let mut object = serde_json::Map::new();
                for (key, value) in &instance.fields {
                    let Some((name, _, transient)) = fields.get(key) else {
                        return block(at, "Instance JSON field contract unavailable");
                    };
                    if *transient || (suppress_nulls && matches!(value, Value::Null)) {
                        continue;
                    }
                    object.insert(
                        name.clone(),
                        self.encode_json(value, at, depth + 1, suppress_nulls, budget)?,
                    );
                }
                Json::Object(object)
            }
            Value::Record(record) => {
                let record = record.borrow();
                let schema = self
                    .program
                    .schema
                    .object(&record.typ)
                    .ok_or_else(|| self.json_error(at, "Unknown record schema in JSON"))?;
                let mut object = serde_json::Map::new();
                object.insert("attributes".into(), serde_json::json!({"type":schema.name}));
                for (key, value) in &record.fields {
                    let field = schema.field(key).ok_or_else(|| {
                        self.json_error(at, format!("Unknown record field {key}"))
                    })?;
                    object.insert(
                        field.name.clone(),
                        self.encode_json(value, at, depth + 1, suppress_nulls, budget)?,
                    );
                }
                Json::Object(object)
            }
            _ => {
                return block(
                    at,
                    "JSON serialization of this platform value is not implemented",
                )
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (Program, Node) {
        let source="class DTO { public String camelCase; public Integer count; public DTO() { throw new IllegalArgumentException('constructor must not run'); } }";
        let node = crate::syntax::parse(source, "dto.cls").unwrap();
        let mut p = Program::default();
        crate::syntax::declarations(&node, &mut p, &mut vec![]);
        (p, node)
    }
    #[test]
    fn typed_deserialize_bypasses_constructor_and_preserves_names() {
        let (p, n) = setup();
        let mut c = Context::new(&p);
        let v = c
            .json_call(
                "deserialize",
                vec![
                    Value::Str("{\"camelCase\":\"ok\",\"count\":7}".into()),
                    Value::Type("DTO".into()),
                ],
                &n,
            )
            .unwrap();
        let out = c.json_call("serialize", vec![v], &n).unwrap();
        assert_eq!(out.display(), "{\"camelCase\":\"ok\",\"count\":7}");
    }
    #[test]
    fn strict_json_rejects_unknown_fields_and_malformed_input() {
        let (p, n) = setup();
        let mut c = Context::new(&p);
        assert!(c
            .json_call(
                "deserializestrict",
                vec![
                    Value::Str("{\"extra\":true}".into()),
                    Value::Type("DTO".into())
                ],
                &n
            )
            .is_err());
        assert!(c
            .json_call("deserializeuntyped", vec![Value::Str("[broken".into())], &n)
            .unwrap_err()
            .exception
            .is_some());
    }
    #[test]
    fn sobject_extra_fields_follow_caller_version_and_strict_mode() {
        let (mut p, n) = setup();
        let args = vec![
            Value::Str("{\"Name\":\"preserved\",\"unknownScalar\":true}".into()),
            Value::Type("Account".into()),
        ];
        p.api_versions.insert(n.path.to_string(), 35);
        let mut c = Context::new(&p);
        let Value::Record(record) = c.json_call("deserialize", args.clone(), &n).unwrap() else {
            panic!("record expected")
        };
        assert_eq!(record.borrow().fields["name"].display(), "preserved");
        assert_eq!(record.borrow().fields.len(), 1);
        assert!(c
            .json_call("deserializestrict", args.clone(), &n)
            .unwrap_err()
            .exception
            .is_some());
        p.api_versions.insert(n.path.to_string(), 34);
        assert!(Context::new(&p)
            .json_call("deserialize", args, &n)
            .unwrap_err()
            .exception
            .is_some());
    }
    #[test]
    fn untyped_large_numbers_are_not_rounded_through_float() {
        let (p, n) = setup();
        let mut c = Context::new(&p);
        let v = c
            .json_call(
                "deserializeuntyped",
                vec![Value::Str(
                    "{\"n\":9007199254740993,\"decimal\":0.100000000000000001}".into(),
                )],
                &n,
            )
            .unwrap();
        let out = c.json_call("serialize", vec![v], &n).unwrap();
        assert!(out.display().contains("9007199254740993"));
        assert!(out.display().contains("0.100000000000000001"));
    }
    #[test]
    fn incomplete_json_dates_and_unknown_type_are_distinct() {
        let (p, n) = setup();
        let mut c = Context::new(&p);
        let error = c
            .json_call("deserializeuntyped", vec![Value::Str("[1,".into())], &n)
            .unwrap_err();
        assert!(error.exception.is_some());
        assert!(error.diagnostic.message.contains("Unexpected end-of-input"));
        let date = c
            .json_call(
                "deserialize",
                vec![
                    Value::Str("\"2024-02-29\"".into()),
                    Value::Type("Date".into()),
                ],
                &n,
            )
            .unwrap();
        assert!(matches!(date, Value::Date(_)));
        assert_eq!(
            c.json_call("serialize", vec![date], &n).unwrap().display(),
            "\"2024-02-29\""
        );
        assert!(c
            .json_call(
                "deserialize",
                vec![Value::Str("null".into()), Value::Type("Invented".into())],
                &n
            )
            .is_err());
    }
}
