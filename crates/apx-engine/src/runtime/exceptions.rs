use super::*;
pub(super) fn raise<T>(at: &Node, typ: &str, message: impl Into<String>) -> R<T> {
    let message = message.into();
    if typ.to_ascii_lowercase().trim_start_matches("system.") == "limitexception" {
        return block(at, format!("Uncatchable Apex governor limit: {message}"));
    }
    Err(EvalError {
        exception: Some(Value::Exception(Rc::new(RefCell::new(ExceptionValue {
            typ: typ.into(),
            message: message.clone(),
            stack: format!("APX local {}:{}", at.path, at.line()),
        })))),
        status: "failed",
        diagnostic: at.diagnostic("APX_EXCEPTION", format!("{typ}: {message}")),
    })
}
impl Context<'_> {
    pub(super) fn is_exception_type(&self, typ: &str) -> bool {
        if typ.eq_ignore_ascii_case("cache.invalidparamexception") {
            return true;
        }
        let t = typ.trim_start_matches("system.");
        if [
            "exception",
            "dmlexception",
            "stringexception",
            "listException",
            "mathexception",
            "nullpointerexception",
            "typeexception",
            "securityexception",
            "illegalargumentexception",
            "queryexception",
            "auraHandledException",
            "calloutexception",
            "jsonexception",
            "finalexception",
            "dataweavescriptexception",
            "unsupportedoperationexception",
            "sobjectexception",
        ]
        .iter()
        .any(|x| x.eq_ignore_ascii_case(t))
        {
            return true;
        }
        let mut candidates = self.resolve_class(typ).into_iter().collect::<Vec<_>>();
        for _ in 0..32 {
            let Some(name) = candidates.pop() else {
                break;
            };
            if let Some(c) = self.program.classes.get(&name) {
                if let Some(parent) = &c.superclass {
                    if parent.eq_ignore_ascii_case("exception")
                        || parent.eq_ignore_ascii_case("system.exception")
                    {
                        return true;
                    }
                    if let Some(parent) = self.resolve_class_from(parent, &name) {
                        candidates.push(parent);
                    }
                }
            }
        }
        false
    }
    pub(super) fn exception_matches(&self, actual: &str, expected: &str) -> bool {
        let a = actual.to_ascii_lowercase();
        let e = expected.to_ascii_lowercase();
        e == "exception"
            || e == "system.exception"
            || a.trim_start_matches("system.") == e.trim_start_matches("system.")
            || self
                .resolve_class(&e)
                .is_some_and(|expected| self.class_assignable(&a, &expected))
    }
    pub(super) fn construct_exception(&self, typ: &str, args: &[Value], at: &Node) -> R<Value> {
        let mut message = match args {
            [] => "Script-thrown exception".into(),
            [Value::Str(s)] => s.clone(),
            [Value::Exception(e)] => e.borrow().message.clone(),
            [Value::Str(s), Value::Exception(_)] => s.clone(),
            _ => return fail(at, "Invalid Exception constructor arguments"),
        };
        if typ.eq_ignore_ascii_case("AuraHandledException")
            || typ.eq_ignore_ascii_case("System.AuraHandledException")
        {
            message = "Script-thrown exception".into();
        }
        let typ = self
            .resolve_class(typ)
            .and_then(|name| self.program.classes.get(&name).map(|c| c.name.clone()))
            .unwrap_or_else(|| {
                let raw = typ.to_ascii_lowercase();
                let key = raw.trim_start_matches("system.");
                let canonical = [
                    "Exception",
                    "DMLException",
                    "StringException",
                    "ListException",
                    "MathException",
                    "NullPointerException",
                    "TypeException",
                    "SecurityException",
                    "IllegalArgumentException",
                    "QueryException",
                    "AuraHandledException",
                    "CalloutException",
                    "JSONException",
                    "FinalException",
                    "DataWeaveScriptException",
                    "UnsupportedOperationException",
                    "SObjectException",
                ]
                .into_iter()
                .find(|s| s.eq_ignore_ascii_case(key));
                canonical
                    .map(|s| format!("System.{s}"))
                    .unwrap_or_else(|| typ.into())
            });
        Ok(Value::Exception(Rc::new(RefCell::new(ExceptionValue {
            typ,
            message,
            stack: format!("APX local {}:{}", at.path, at.line()),
        }))))
    }
    pub(super) fn exception_method(
        &self,
        receiver: &Value,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        let Value::Exception(e) = receiver else {
            return Ok(None);
        };
        Ok(Some(match (name, args) {
            ("getmessage", []) => Value::Str(e.borrow().message.clone()),
            ("gettypename", []) => Value::Str(e.borrow().typ.clone()),
            ("getstacktracestring", []) => Value::Str(e.borrow().stack.clone()),
            ("setmessage", [Value::Str(s)]) => {
                e.borrow_mut().message = s.clone();
                Value::Null
            }
            _ => return block(at, format!("Exception.{name} is not implemented")),
        }))
    }
    pub(super) fn exec_try(&mut self, n: &Node) -> R<Flow> {
        let mut outcome = self.exec(self.need(n, "body")?);
        if let Err(error) = &outcome {
            if error.status == "blocked" || error.exception.is_none() {
                return outcome;
            }
            let exception = error.exception.clone().unwrap();
            let Value::Exception(e) = &exception else {
                return outcome;
            };
            let typ = e.borrow().typ.clone();
            for catch in n.named().filter(|n| n.kind == "catch_clause") {
                let parameter = catch.child("formal_parameter").ok_or_else(|| EvalError {
                    exception: None,
                    status: "blocked",
                    diagnostic: catch.diagnostic("APX_UNSUPPORTED", "Missing catch parameter"),
                })?;
                let wanted = self.need(parameter, "type")?.text();
                if self.exception_matches(&typ, wanted) {
                    let name = self.need(parameter, "name")?.text().to_ascii_lowercase();
                    self.locals
                        .push(BTreeMap::from([(name.clone(), exception.clone())]));
                    self.local_types
                        .push(BTreeMap::from([(name, wanted.into())]));
                    outcome = self.exec(self.need(catch, "body")?);
                    self.locals.pop();
                    self.local_types.pop();
                    break;
                }
            }
        }
        if outcome
            .as_ref()
            .err()
            .is_some_and(|e| e.status == "blocked")
        {
            return outcome;
        }
        if let Some(finally) = n.child("finally_clause") {
            if let Some(body) = finally.child("block") {
                match self.exec(body) {
                    Ok(Flow::Next) => {}
                    other => outcome = other,
                }
            }
        }
        outcome
    }
}
