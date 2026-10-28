//! Bridge from Apex values to the bounded, source-driven DataWeave interpreter.
use super::*;
use crate::dataweave::{DwError, DwValue};
use num_traits::ToPrimitive;

fn canonical(typ: &str) -> String {
    typ.to_ascii_lowercase()
        .trim_start_matches("system.")
        .into()
}
pub(super) fn dataweave_is_type(typ: &str) -> bool {
    let t = canonical(typ);
    t == "dataweave.script" || t == "dataweave.result" || t.starts_with("dataweavescriptresource.")
}
pub(super) fn dataweave_assignable(actual: &str, expected: &str) -> bool {
    let a = canonical(actual);
    let e = canonical(expected);
    a == e || (e == "dataweave.script" && a.starts_with("dataweavescriptresource."))
}
fn dw_error<T>(at: &Node, error: DwError) -> R<T> {
    if error.unsupported {
        block(at, error.message)
    } else {
        exceptions::raise(at, "System.DataWeaveScriptException", error.message)
    }
}
impl Context<'_> {
    pub(super) fn dataweave_construct(
        &mut self,
        typ: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        let t = canonical(typ);
        let Some(name) = t.strip_prefix("dataweavescriptresource.") else {
            return Ok(None);
        };
        if !args.is_empty() {
            return block(
                at,
                "Generated DataWeave script constructor takes no arguments",
            );
        }
        self.dw_script(name, at).map(Some)
    }
    fn dw_script(&self, name: &str, at: &Node) -> R<Value> {
        let key = name.to_ascii_lowercase();
        if !self.program.dataweave_scripts.contains_key(&key) {
            return exceptions::raise(
                at,
                "System.DataWeaveScriptException",
                format!("DataWeave script {name} was not found"),
            );
        }
        Ok(Value::Instance(Rc::new(RefCell::new(Instance {
            class: format!("dataweavescriptresource.{key}"),
            fields: BTreeMap::from([("\0script".into(), Value::Str(key))]),
            field_types: BTreeMap::new(),
        }))))
    }
    pub(super) fn dataweave_namespace(
        &mut self,
        ns: &str,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        if canonical(ns) != "dataweave.script" {
            return Ok(None);
        }
        match (name, args) {
            ("createscript", [Value::Str(script)]) => self.dw_script(script, at).map(Some),
            _ => block(at, "DataWeave.Script overload is not implemented"),
        }
    }
    pub(super) fn dataweave_method(
        &mut self,
        receiver: &Value,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        let Value::Instance(instance) = receiver else {
            return Ok(None);
        };
        let typ = instance.borrow().class.clone();
        if !dataweave_is_type(&typ) {
            return Ok(None);
        }
        if typ == "dataweave.result" {
            let instance = instance.borrow();
            if !args.is_empty() {
                return block(at, "DataWeave.Result getters take no arguments");
            }
            return Ok(Some(match name {
                "getvalueasstring" => instance.fields["\0text"].clone(),
                "getvalue" => {
                    if !matches!(&instance.fields["\0media"],Value::Str(s)if s=="application/apex")
                    {
                        return block(
                            at,
                            "DataWeave.getValue is supported for application/apex output",
                        );
                    }
                    instance.fields["\0value"].clone()
                }
                _ => return block(at, format!("DataWeave.Result.{name} is not implemented")),
            }));
        }
        let [Value::Map(parameters)] = args else {
            return block(at, "DataWeave.execute requires Map<String,Object>");
        };
        if name != "execute" {
            return block(at, "Unknown DataWeave.Script method");
        }
        let Value::Str(script) = instance.borrow().fields["\0script"].clone() else {
            return block(at, "Missing script identity");
        };
        let source = self.program.dataweave_scripts[&script].clone();
        let mut inputs = BTreeMap::new();
        let budget = Cell::new(0);
        for (key, value) in parameters.borrow().iter() {
            let Value::Str(key) = key else {
                return block(at, "DataWeave input names must be Strings");
            };
            inputs.insert(key.clone(), self.dw_input(value, at, 0, &budget)?);
        }
        let result = match crate::dataweave::execute(&source, &inputs) {
            Ok(v) => v,
            Err(e) => return dw_error(at, e),
        };
        let value = if result.media_type == "application/apex" {
            self.dw_output(result.value, at, 0)?
        } else {
            Value::Null
        };
        let logs = result.logs.into_iter().map(Value::Str).collect::<Vec<_>>();
        self.native_state
            .insert("dataweave.logs".into(), Value::list(logs));
        let fields = BTreeMap::from([
            ("\0value".into(), value),
            ("\0text".into(), Value::Str(result.text)),
            ("\0media".into(), Value::Str(result.media_type)),
        ]);
        Ok(Some(Value::Instance(Rc::new(RefCell::new(Instance {
            class: "dataweave.result".into(),
            fields,
            field_types: BTreeMap::new(),
        })))))
    }
    fn dw_input(
        &mut self,
        value: &Value,
        at: &Node,
        depth: usize,
        budget: &Cell<usize>,
    ) -> R<DwValue> {
        if depth > 64 {
            return block(
                at,
                "DataWeave input depth exceeds bounded host representation",
            );
        }
        budget.set(budget.get().saturating_add(
            64 + if let Value::Str(s) = value {
                s.len()
            } else {
                0
            },
        ));
        if budget.get() > 1_048_576 {
            return block(
                at,
                "DataWeave input expansion exceeds bounded 1 MiB host representation",
            );
        }
        Ok(match value {
            Value::Null => DwValue::Null,
            Value::Bool(b) => DwValue::Bool(*b),
            Value::Num(n) => DwValue::Number(n.q.clone()),
            Value::Str(s) => DwValue::String(s.clone()),
            Value::Date(d) => {
                DwValue::String(crate::dates::date_iso(*d).map_err(|e| EvalError {
                    exception: None,
                    status: "blocked",
                    diagnostic: at.diagnostic("APX_DATE", e.message),
                })?)
            }
            Value::Datetime(d) => {
                DwValue::String(crate::dates::datetime_iso(*d).map_err(|e| EvalError {
                    exception: None,
                    status: "blocked",
                    diagnostic: at.diagnostic("APX_DATE", e.message),
                })?)
            }
            Value::List(items) | Value::Set(items) | Value::Iterable(items) => DwValue::Array(
                items
                    .borrow()
                    .iter()
                    .map(|v| self.dw_input(v, at, depth + 1, budget))
                    .collect::<R<Vec<_>>>()?,
            ),
            Value::Map(entries) => {
                let mut fields = vec![];
                for (k, v) in entries.borrow().iter() {
                    let Value::Str(key) = k else {
                        return block(at, "DataWeave input object keys require Strings");
                    };
                    fields.push(crate::dataweave::DwField::new(
                        key,
                        self.dw_input(v, at, depth + 1, budget)?,
                    ));
                }
                DwValue::Object(fields)
            }
            Value::Instance(_) | Value::Record(_) => {
                let serialized = self.json_call("serialize", vec![value.clone()], at)?;
                let Value::Str(text) = serialized else {
                    return block(at, "Unable to convert Apex object to DataWeave input");
                };
                let json = serde_json::from_str(&text).map_err(|e| EvalError {
                    exception: None,
                    status: "blocked",
                    diagnostic: at.diagnostic("APX_JSON", e.to_string()),
                })?;
                match DwValue::from_json(&json) {
                    Ok(v) => v,
                    Err(e) => return dw_error(at, e),
                }
            }
            _ => return block(at, "DataWeave input value type is not implemented"),
        })
    }
    fn dw_output(&mut self, value: DwValue, at: &Node, depth: usize) -> R<Value> {
        if depth > 64 {
            return block(at, "DataWeave output depth exceeds host bound");
        }
        let value = match value {
            DwValue::Null => Value::Null,
            DwValue::Bool(b) => Value::Bool(b),
            DwValue::String(s) => Value::Str(s),
            DwValue::Number(q) => {
                let kind = if q.is_integer() {
                    if q.to_integer().to_i32().is_some() {
                        NumKind::Int
                    } else if q.to_integer().to_i64().is_some() {
                        NumKind::Long
                    } else {
                        NumKind::Decimal
                    }
                } else {
                    NumKind::Decimal
                };
                Value::Num(Number { q, kind })
            }
            DwValue::Array(values) => Value::list(
                values
                    .into_iter()
                    .map(|v| self.dw_output(v, at, depth + 1))
                    .collect::<R<Vec<_>>>()?,
            ),
            DwValue::Object(fields) => {
                let mut items = vec![];
                for field in fields {
                    if !field.attributes.is_empty() {
                        return block(
                            at,
                            "XML attributes cannot be converted implicitly to Apex Map",
                        );
                    }
                    items.push((
                        Value::Str(field.key),
                        self.dw_output(field.value, at, depth + 1)?,
                    ));
                }
                self.coerce(
                    Value::Map(Rc::new(RefCell::new(items))),
                    "map<string,object>",
                    at,
                )?
            }
            DwValue::TypedObject { class, fields } => {
                let value = self.dw_output(DwValue::Object(fields), at, depth + 1)?;
                let json = self.json_call("serialize", vec![value], at)?;
                self.json_call("deserialize", vec![json, Value::Type(class)], at)?
            }
        };
        self.charge(&value, at)?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_script_source_drives_static_and_dynamic_invocation() {
        let n = crate::syntax::parse("class Host{}", "host.cls").unwrap();
        let mut p = Program::default();
        p.dataweave_scripts.insert(
            "greeting".into(),
            "%dw 2.0\noutput application/json\n---\n\"Different greeting\"".into(),
        );
        let mut c = Context::new(&p);
        let script = c
            .dataweave_construct("DataWeaveScriptResource.greeting", &[], &n)
            .unwrap()
            .unwrap();
        let inputs = Value::Map(Rc::new(RefCell::new(vec![])));
        let result = c
            .dataweave_method(&script, "execute", &[inputs], &n)
            .unwrap()
            .unwrap();
        assert_eq!(
            c.dataweave_method(&result, "getvalueasstring", &[], &n)
                .unwrap()
                .unwrap()
                .display(),
            "\"Different greeting\""
        );
    }
    #[test]
    fn runtime_script_errors_remain_catchable_but_unknown_syntax_does_not() {
        let n = crate::syntax::parse("class Host{}", "host.cls").unwrap();
        let mut p = Program::default();
        p.dataweave_scripts
            .insert("divide".into(), "%dw 2.0\n---\n2/0".into());
        let mut c = Context::new(&p);
        let script = c.dw_script("divide", &n).unwrap();
        let error = c
            .dataweave_method(
                &script,
                "execute",
                &[Value::Map(Rc::new(RefCell::new(vec![])))],
                &n,
            )
            .unwrap_err();
        assert!(error.exception.is_some());
        assert!(error.diagnostic.message.contains("Division by zero"));
    }
}
