use crate::{
    model::Diagnostic,
    syntax::{Method, Node, Program},
    value::{ExceptionValue, Instance, NumKind, Number, Record, Value},
};
use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{One, ToPrimitive, Zero};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
mod async_jobs;
mod cache;
mod collections;
mod data;
mod dataweave;
mod equality;
mod events;
mod exceptions;
mod http;
mod json;
mod locations;
mod objects;
mod platform_objects;
mod properties;
mod regex_api;
mod schema_adapter;
mod search;
mod security;
mod stubs;
mod triggers;
mod user_context;

#[derive(Debug)]
pub struct EvalError {
    pub exception: Option<Value>,
    pub status: &'static str,
    pub diagnostic: Diagnostic,
}
type R<T> = Result<T, EvalError>;
fn block<T>(n: &Node, msg: impl Into<String>) -> R<T> {
    Err(EvalError {
        exception: None,
        status: "blocked",
        diagnostic: n.diagnostic("APX_UNSUPPORTED", msg),
    })
}
fn fail<T>(n: &Node, msg: impl Into<String>) -> R<T> {
    let msg = msg.into();
    Err(EvalError {
        exception: Some(Value::Exception(Rc::new(RefCell::new(ExceptionValue {
            typ: "System.Exception".into(),
            message: msg.clone(),
            stack: format!("APX local {}:{}", n.path, n.line()),
        })))),
        status: "failed",
        diagnostic: n.diagnostic("APX_RUNTIME", msg),
    })
}
#[derive(Clone, Debug)]
enum Flow {
    Next,
    Return(Value),
    Break,
    Continue,
}

pub struct Context<'a> {
    pub program: &'a Program,
    pub locals: Vec<BTreeMap<String, Value>>,
    pub statics: BTreeMap<String, Value>,
    pub current_class: String,
    pub native_state: BTreeMap<String, Value>,
    pub callouts: i64,
    pub debug_logs: Vec<String>,
    pub data_store: Option<crate::record_store::RecordStore>,
    trigger_readonly: BTreeSet<usize>,
    active_properties: Vec<String>,
    sharing_context: bool,
    async_jobs: Vec<async_jobs::AsyncJob>,
    async_sequence: u64,
    event_state: events::EventState,
    this: Option<Rc<RefCell<Instance>>>,
    local_types: Vec<BTreeMap<String, String>>,
    static_types: BTreeMap<String, String>,
    list_types: RefCell<BTreeMap<usize, String>>,
    set_types: RefCell<BTreeMap<usize, String>>,
    map_types: RefCell<BTreeMap<usize, (String, String)>>,
    list_owners: RefCell<BTreeMap<usize, std::rc::Weak<RefCell<Vec<Value>>>>>,
    set_owners: RefCell<BTreeMap<usize, std::rc::Weak<RefCell<Vec<Value>>>>>,
    map_owners: RefCell<BTreeMap<usize, std::rc::Weak<RefCell<Vec<(Value, Value)>>>>>,
    iteration_locks: BTreeMap<usize, usize>,
    allocated: Cell<usize>,
    initialized: BTreeSet<String>,
    initializing: BTreeSet<String>,
    fuel: u64,
    depth: usize,
    pub queries: i64,
    pub dml: i64,
    test_frame: Option<(i64, i64, i64)>,
    trigger_context: BTreeMap<String, Value>,
    trigger_depth: usize,
}
impl<'a> Context<'a> {
    fn display_value(&self, value: &Value, at: &Node) -> R<String> {
        value.display_checked().map_err(|message| EvalError {
            exception: None,
            status: "blocked",
            diagnostic: at.diagnostic("APX_RESOURCE", message),
        })
    }
    pub fn new(program: &'a Program) -> Self {
        let mut native_state = BTreeMap::from([(
            "clock.iso".into(),
            Value::Str(
                program
                    .environment
                    .as_ref()
                    .map(|e| e.now.clone())
                    .unwrap_or_else(crate::dates::now_iso_utc),
            ),
        )]);
        if let Some(environment) = &program.environment {
            native_state.insert(
                "userinfo.userid".into(),
                Value::Str(environment.current_user_id.clone()),
            );
        }
        Self {
            program,
            locals: vec![BTreeMap::new()],
            statics: BTreeMap::new(),
            current_class: String::new(),
            native_state,
            callouts: 0,
            debug_logs: vec![],
            data_store: None,
            trigger_readonly: BTreeSet::new(),
            active_properties: vec![],
            sharing_context: false,
            async_jobs: vec![],
            async_sequence: 0,
            event_state: Default::default(),
            this: None,
            local_types: vec![BTreeMap::new()],
            static_types: BTreeMap::new(),
            list_types: RefCell::new(BTreeMap::new()),
            set_types: RefCell::new(BTreeMap::new()),
            map_types: RefCell::new(BTreeMap::new()),
            list_owners: RefCell::new(BTreeMap::new()),
            set_owners: RefCell::new(BTreeMap::new()),
            map_owners: RefCell::new(BTreeMap::new()),
            iteration_locks: BTreeMap::new(),
            allocated: Cell::new(0),
            initialized: BTreeSet::new(),
            initializing: BTreeSet::new(),
            fuel: 2_000_000,
            depth: 0,
            queries: 0,
            dml: 0,
            test_frame: None,
            trigger_context: BTreeMap::new(),
            trigger_depth: 0,
        }
    }
    fn tick(&mut self, n: &Node) -> R<()> {
        if self.fuel == 0 {
            return block(
                n,
                "Host evaluation budget exceeded (2000000 steps); this is not an Apex CPU limit",
            );
        };
        self.fuel -= 1;
        Ok(())
    }
    fn charge(&self, value: &Value, at: &Node) -> R<()> {
        let size = match value {
            Value::Str(s) => s.len() + 64,
            Value::Blob(b) => b.len() + 64,
            Value::Num(n) => ((n.q.numer().bits() + n.q.denom().bits()) / 8) as usize + 64,
            _ => 64,
        };
        let used = self.allocated.get().saturating_add(size);
        if used > 16 * 1024 * 1024 {
            return block(at,"Cumulative value-allocation budget exceeded (16 MiB host safeguard, not Apex heap accounting)");
        }
        self.allocated.set(used);
        Ok(())
    }
    fn ensure_list_writable(&self, list: &Rc<RefCell<Vec<Value>>>, at: &Node) -> R<()> {
        if self
            .iteration_locks
            .contains_key(&(Rc::as_ptr(list) as usize))
        {
            return fail(at, "Collection cannot be modified during iteration");
        }
        Ok(())
    }
    fn guard_type_binding(&self, typ: &str, at: &Node) -> R<()> {
        let owner = self.program.classes.values().find(|c| {
            c.declaration.path == at.path
                && c.declaration.start <= at.start
                && c.declaration.end >= at.end
        });
        for token in typ.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
            let key = token.to_ascii_lowercase();
            if owner.is_some_and(|c| c.deferred_names.contains(&key))
                && [
                    "string", "account", "contact", "integer", "long", "decimal", "boolean",
                    "object", "list", "set", "map", "iterable", "blob",
                ]
                .contains(&key.as_str())
            {
                return block(
                    at,
                    format!("Type {typ} reaches an unsupported nested/instance declaration"),
                );
            }
            if [
                "integer", "long", "decimal", "double", "string", "id", "boolean", "object",
                "account", "contact", "sobject", "list", "iterable",
            ]
            .contains(&key.as_str())
                && self.program.classes.contains_key(&key)
            {
                return block(
                    at,
                    format!("User type shadows {token}; qualified type binding is not implemented"),
                );
            }
        }
        Ok(())
    }
    fn need<'n>(&self, n: &'n Node, f: &str) -> R<&'n Node> {
        n.field(f).ok_or_else(|| EvalError {
            exception: None,
            status: "blocked",
            diagnostic: n.diagnostic(
                "APX_UNSUPPORTED",
                format!("Unimplemented shape {} without {f}", n.kind),
            ),
        })
    }
    fn initialize(&mut self, class: &str, at: &Node) -> R<()> {
        self.initialize_class(class, at)
    }
    pub fn call(&mut self, class: &str, name: &str, args: Vec<Value>, at: &Node) -> R<Value> {
        self.tick(at)?;
        self.initialize(class, at)?;
        let (owner, method) = self.find_method(class, name, &args, true, at)?;
        self.check_method_access(&owner, &method, at)?;
        self.invoke_method(&owner, &method, args, None)
    }
    fn coerce(&self, v: Value, typ: &str, n: &Node) -> R<Value> {
        self.guard_type_binding(typ, n)?;
        self.charge(&v, n)?;
        let t = typ.to_ascii_lowercase().replace(' ', "");
        if t == "aggregateresult" {
            return match &v {
                Value::Null => Ok(v),
                Value::Record(r) if r.borrow().typ.eq_ignore_ascii_case(&t) => Ok(v),
                _ => fail(n, "Expected AggregateResult"),
            };
        }
        if events::is_type(&t) {
            return match &v {
                Value::Null => Ok(v),
                Value::Instance(i) if i.borrow().class.eq_ignore_ascii_case(&t) => Ok(v),
                _ => fail(n, format!("Expected {typ}")),
            };
        }
        if async_jobs::async_is_type(&t) {
            return match &v {
                Value::Null => Ok(v),
                Value::Instance(i)
                    if i.borrow()
                        .class
                        .trim_start_matches("system.")
                        .eq_ignore_ascii_case(t.trim_start_matches("system.")) =>
                {
                    Ok(v)
                }
                _ => fail(n, format!("Expected {typ}")),
            };
        }
        if t.trim_start_matches("system.") == "sobjectaccessdecision" {
            return match &v {
                Value::Null => Ok(v),
                Value::Instance(i)
                    if i.borrow()
                        .class
                        .eq_ignore_ascii_case("system.sobjectaccessdecision") =>
                {
                    Ok(v)
                }
                _ => fail(n, format!("Expected {typ}")),
            };
        }
        if regex_api::is_type(&t) {
            return match &v {
                Value::Null => Ok(v),
                Value::Instance(i)
                    if i.borrow()
                        .class
                        .trim_start_matches("system.")
                        .eq_ignore_ascii_case(t.trim_start_matches("system.")) =>
                {
                    Ok(v)
                }
                _ => fail(n, format!("Expected {typ}")),
            };
        }
        if cache::cache_is_type(&t) {
            return match &v {
                Value::Null => Ok(v),
                Value::Instance(i) if cache::cache_assignable(&i.borrow().class, &t) => Ok(v),
                _ => fail(n, format!("Expected {typ}")),
            };
        }
        if platform_objects::is_type(&t) {
            return match &v {
                Value::Null => Ok(v),
                Value::Instance(i) if i.borrow().class.eq_ignore_ascii_case(&t) => Ok(v),
                _ => fail(n, format!("Expected {typ}")),
            };
        }
        if platform_objects::enum_members(&t).is_some() {
            return match &v {
                Value::Null => Ok(v),
                Value::Enum { typ: actual, .. } if actual.eq_ignore_ascii_case(&t) => Ok(v),
                _ => fail(n, format!("Expected enum {typ}")),
            };
        }
        if dataweave::dataweave_is_type(&t) {
            return match &v {
                Value::Null => Ok(v),
                Value::Instance(i) if dataweave::dataweave_assignable(&i.borrow().class, &t) => {
                    Ok(v)
                }
                _ => fail(n, format!("Expected {typ}")),
            };
        }
        if user_context::request_is_type(&t) {
            return match &v {
                Value::Null => Ok(v),
                Value::Instance(i) if user_context::request_is_type(&i.borrow().class) => Ok(v),
                _ => fail(n, format!("Expected {typ}")),
            };
        }
        if ["date", "datetime", "time"].contains(&t.as_str()) {
            return match (&v, t.as_str()) {
                (Value::Null, _)
                | (Value::Date(_), "date")
                | (Value::Datetime(_), "datetime")
                | (Value::Time(_), "time") => Ok(v),
                _ => fail(n, format!("Expected {typ}")),
            };
        }
        if http::http_is_type(&t) {
            return match &v {
                Value::Null => Ok(v),
                Value::Instance(i)
                    if i.borrow()
                        .class
                        .eq_ignore_ascii_case(t.trim_start_matches("system.")) =>
                {
                    Ok(v)
                }
                _ => fail(n, format!("Expected {typ}")),
            };
        }
        if t == "type" || t == "system.type" {
            return match &v {
                Value::Null | Value::Type(_) => Ok(v),
                _ => fail(n, "Expected Type"),
            };
        }
        if [
            "database.saveresult",
            "database.upsertresult",
            "database.deleteresult",
            "database.undeleteresult",
            "database.error",
        ]
        .contains(&t.as_str())
        {
            return match &v {
                Value::Null => Ok(v),
                Value::Instance(i) if i.borrow().class.eq_ignore_ascii_case(&t) => Ok(v),
                _ => fail(n, format!("Expected {typ}")),
            };
        }
        if t == "savepoint" || t == "database.savepoint" {
            return match &v {
                Value::Null => Ok(v),
                Value::Type(s) if s.starts_with("Database.Savepoint:") => Ok(v),
                _ => fail(n, "Expected Savepoint"),
            };
        }
        if let Some(kind) = t.strip_prefix("schema.").or_else(|| {
            [
                "sobjecttype",
                "sobjectfield",
                "describefieldresult",
                "describesobjectresult",
            ]
            .contains(&t.as_str())
            .then_some(t.as_str())
        }) {
            if [
                "sobjecttype",
                "sobjectfield",
                "describefieldresult",
                "describesobjectresult",
            ]
            .contains(&kind)
            {
                return match &v {
                    Value::Null => Ok(v),
                    Value::Type(token)
                        if token
                            .to_ascii_lowercase()
                            .starts_with(&format!("schema.{kind}:")) =>
                    {
                        Ok(v)
                    }
                    _ => fail(n, format!("Expected {typ}")),
                };
            }
        }
        if objects::builtin_enum(&t).is_some() {
            return match &v {
                Value::Null => Ok(v),
                Value::Enum { typ: actual, .. }
                    if actual.to_ascii_lowercase().trim_start_matches("system.")
                        == t.trim_start_matches("system.") =>
                {
                    Ok(v)
                }
                _ => fail(n, format!("Expected enum {typ}")),
            };
        }
        if self.program.schema.object(&t).is_some() {
            return match &v {
                Value::Null => Ok(v),
                Value::Record(r) if r.borrow().typ.eq_ignore_ascii_case(&t) => Ok(v),
                Value::List(xs) if xs.borrow().len() == 1 => {
                    self.coerce(xs.borrow()[0].clone(), &t, n)
                }
                Value::List(xs) => exceptions::raise(
                    n,
                    "System.QueryException",
                    if xs.borrow().is_empty() {
                        "List has no rows for assignment to SObject"
                    } else {
                        "List has more than 1 row for assignment to SObject"
                    },
                ),
                _ => fail(n, format!("Expected {typ}")),
            };
        }
        if t.starts_with("iterator<") || t.starts_with("system.iterator<") {
            return match &v {
                Value::Null => Ok(v),
                Value::Instance(i) if self.class_assignable(&i.borrow().class, &t) => Ok(v),
                _ => fail(n, format!("Expected {typ}")),
            };
        }
        if let Some(result) = self.coerce_collection(v.clone(), &t, n)? {
            return Ok(result);
        }
        if self.is_exception_type(&t) {
            return match &v {
                Value::Null => Ok(v),
                Value::Exception(e) if self.exception_matches(&e.borrow().typ, &t) => Ok(v),
                _ => fail(n, format!("Expected {typ}")),
            };
        }
        if let Some(class) = self.resolve_class(&t) {
            if matches!(v, Value::Null) {
                return Ok(v);
            }
            if let Value::Enum { typ, .. } = &v {
                if typ.eq_ignore_ascii_case(&class) {
                    return Ok(v);
                }
            }
            if let Value::Instance(i) = &v {
                if self.class_assignable(&i.borrow().class, &class) {
                    return Ok(v);
                }
            }
            return fail(n, format!("Value is not assignable to {typ}"));
        }
        if ![
            "integer", "long", "decimal", "double", "string", "id", "boolean", "object", "void",
            "account", "contact", "sobject", "blob",
        ]
        .contains(&t.as_str())
            && !t.starts_with("list<")
            && !t.starts_with("iterable<")
            && !t.ends_with("[]")
        {
            return block(n, format!("Type {typ} is not implemented"));
        }
        if matches!(v, Value::Null) {
            return Ok(v);
        }
        match t.as_str() {
            "blob" => match v {
                Value::Blob(_) => Ok(v),
                _ => fail(n, "Expected Blob"),
            },
            "integer" | "long" | "decimal" | "double" => {
                let Value::Num(mut num) = v else {
                    return fail(n, format!("Expected {typ}"));
                };
                if t == "double" {
                    let value = num.q.to_f64().ok_or_else(|| EvalError {
                        exception: None,
                        status: "blocked",
                        diagnostic: n
                            .diagnostic("APX_NUMERIC", "Double conversion out of finite range"),
                    })?;
                    return Number::from_double(value)
                        .map(Value::Num)
                        .map_err(|message| EvalError {
                            exception: None,
                            status: "blocked",
                            diagnostic: n.diagnostic("APX_NUMERIC", message),
                        });
                }
                if t == "decimal" && num.kind == NumKind::Double && !num.q.is_integer() {
                    return block(n,"Fractional Double to Decimal conversion requires qualified precision policy");
                }
                num.kind = match t.as_str() {
                    "integer" => NumKind::Int,
                    "long" => NumKind::Long,
                    _ => NumKind::Decimal,
                };
                num.checked().map(Value::Num).map_err(|s| EvalError {
                    exception: None,
                    status: "failed",
                    diagnostic: n.diagnostic("APX_NUMERIC", s),
                })
            }
            "id" => match v {
                Value::Str(s) => Ok(Value::Str(self.canonical_id(&s, n)?)),
                _ => fail(n, "Expected Id"),
            },
            "string" => {
                if matches!(v, Value::Str(_)) {
                    Ok(v)
                } else {
                    fail(n, format!("Expected {typ}"))
                }
            }
            "boolean" => {
                if matches!(v, Value::Bool(_)) {
                    Ok(v)
                } else {
                    fail(n, "Expected Boolean")
                }
            }
            "object" => Ok(v),
            "void" => fail(n, "Void method cannot return a value"),
            _ if t.starts_with("iterable<") => {
                let element = t
                    .strip_prefix("iterable<")
                    .unwrap()
                    .strip_suffix('>')
                    .ok_or_else(|| EvalError {
                        exception: None,
                        status: "blocked",
                        diagnostic: n.diagnostic("APX_UNSUPPORTED", "Malformed Iterable type"),
                    })?;
                let list = match v {
                    Value::List(list) | Value::Iterable(list) => list,
                    _ => return block(n, "Iterable binding supports an existing List only"),
                };
                let key = Rc::as_ptr(&list) as usize;
                if self.list_types.borrow().get(&key).map(String::as_str) != Some(element) {
                    return block(
                        n,
                        "Iterable element type must exactly match the underlying List",
                    );
                }
                Ok(Value::Iterable(list))
            }
            _ if t.starts_with("list<") || t.ends_with("[]") => {
                if let Value::List(ls) = &v {
                    let element = if t.ends_with("[]") {
                        t.trim_end_matches("[]")
                    } else {
                        t.trim_start_matches("list<").trim_end_matches('>')
                    };
                    let key = Rc::as_ptr(ls) as usize;
                    if self
                        .list_types
                        .borrow()
                        .get(&key)
                        .is_some_and(|old| old != element)
                    {
                        return block(n, "List covariance/casting is not implemented");
                    };
                    let converted = ls
                        .borrow()
                        .iter()
                        .map(|item| self.coerce(item.clone(), element, n))
                        .collect::<R<Vec<_>>>()?;
                    *ls.borrow_mut() = converted;
                    self.list_types.borrow_mut().insert(key, element.into());
                    Ok(v)
                } else {
                    fail(n, "Expected List")
                }
            }
            _ => {
                if let Value::List(list) = &v {
                    let vs = list.borrow();
                    if vs.len() != 1 {
                        return exceptions::raise(
                            n,
                            "System.QueryException",
                            if vs.is_empty() {
                                "List has no rows for assignment to SObject"
                            } else {
                                "List has more than 1 row for assignment to SObject"
                            },
                        );
                    }
                    return self.coerce(vs[0].clone(), typ, n);
                }
                if let Value::Record(r) = &v {
                    if r.borrow().typ.eq_ignore_ascii_case(&t) || t == "sobject" {
                        return Ok(v);
                    }
                }
                block(n, format!("Type conversion to {typ} is not implemented"))
            }
        }
    }
    fn boolean(&self, v: Value, n: &Node) -> R<bool> {
        if let Value::Bool(b) = v {
            Ok(b)
        } else {
            fail(n, "Expected a non-null Boolean condition")
        }
    }
    fn get(&mut self, name: &str, n: &Node) -> R<Value> {
        let key = name.to_ascii_lowercase();
        for scope in self.locals.iter().rev() {
            if let Some(v) = scope.get(&key) {
                return Ok(v.clone());
            }
        }
        let mut lexical = Some(self.current_class.clone());
        while let Some(scope) = lexical {
            if let Some(v) = self.property_get(&Value::Namespace(scope.clone()), &key, n)? {
                return Ok(v);
            }
            if let Some(v) = self.statics.get(&format!("{scope}.{key}")) {
                self.check_field_access(&scope, &key, true, n)?;
                return Ok(v.clone());
            }
            lexical = scope.rsplit_once('.').map(|(parent, _)| parent.to_string());
        }
        if let Some(instance) = self.this.clone() {
            if let Some(value) = self.property_get(&Value::Instance(instance.clone()), &key, n)? {
                return Ok(value);
            }
            if let Some(value) = instance.borrow().fields.get(&key) {
                self.check_field_access(&instance.borrow().class, &key, false, n)?;
                return Ok(value.clone());
            }
        }
        if let Some(class) = self.resolve_class(&key) {
            if [
                "system",
                "assert",
                "test",
                "limits",
                "string",
                "integer",
                "long",
                "decimal",
                "trigger",
                "database",
                "crypto",
                "blob",
                "encodingutil",
                "json",
                "type",
            ]
            .contains(&class.as_str())
            {
                return block(n,"User type shadows a platform type; qualified name resolution is not implemented");
            }
            return Ok(Value::Namespace(class));
        }
        if self
            .program
            .classes
            .get(&self.current_class)
            .is_some_and(|c| c.deferred_names.contains(&key))
        {
            return block(
                n,
                format!("Instance or nested declaration {name} is not implemented"),
            );
        }
        if self.program.schema.object(&key).is_some() {
            return Ok(Value::Namespace(key));
        }
        if objects::builtin_enum(&key).is_some() {
            return Ok(Value::Namespace(key));
        }
        if self.is_exception_type(&key) {
            return Ok(Value::Namespace(key));
        }
        if self.program.classes.contains_key(&key)
            && [
                "system",
                "assert",
                "test",
                "limits",
                "string",
                "integer",
                "long",
                "decimal",
                "trigger",
                "database",
                "crypto",
                "blob",
                "encodingutil",
            ]
            .contains(&key.as_str())
        {
            return block(
                n,
                "User type shadows a platform type; qualified name resolution is not implemented",
            );
        }
        if self.program.classes.contains_key(&key)
            || [
                "system",
                "assert",
                "test",
                "limits",
                "string",
                "integer",
                "long",
                "decimal",
                "trigger",
                "database",
                "crypto",
                "blob",
                "encodingutil",
                "json",
                "type",
                "restcontext",
                "httpcalloutmock",
                "schema",
                "date",
                "datetime",
                "time",
                "dataweave",
                "dataweavescriptresource",
                "userinfo",
                "request",
                "messaging",
                "connectapi",
                "security",
                "cache",
                "pattern",
                "matcher",
                "eventbus",
                "applauncher",
            ]
            .contains(&key.as_str())
        {
            return Ok(Value::Namespace(key));
        }
        block(n, format!("Unresolved identifier {name}"))
    }
    fn field_value(&mut self, obj: Value, name: &str, n: &Node) -> R<Value> {
        if matches!(obj, Value::Null) && n.child("safe_navigation_operator").is_some() {
            return Ok(Value::Null);
        }
        if let Some(v) = self.property_get(&obj, &name.to_ascii_lowercase(), n)? {
            return Ok(v);
        }
        if let Some(v) = self.schema_get(&obj, name, n)? {
            return Ok(v);
        }
        let key = name.to_ascii_lowercase();
        match obj {
            Value::List(list)
                if n.field("object")
                    .is_some_and(|o| o.kind == "query_expression") =>
            {
                let values = list.borrow();
                if values.len() != 1 {
                    return exceptions::raise(
                        n,
                        "System.QueryException",
                        if values.is_empty() {
                            "List has no rows for assignment to SObject"
                        } else {
                            "List has more than 1 row for assignment to SObject"
                        },
                    );
                }
                self.field_value(values[0].clone(), name, n)
            }
            Value::Instance(instance) => {
                self.check_field_access(&instance.borrow().class, &key, false, n)?;
                let value = instance.borrow().fields.get(&key).cloned();
                value.ok_or_else(|| EvalError {
                    exception: None,
                    status: "blocked",
                    diagnostic: n
                        .diagnostic("APX_UNSUPPORTED", format!("Unknown instance field {name}")),
                })
            }
            Value::Namespace(s)
                if s == "system"
                    && [
                        "assert",
                        "json",
                        "crypto",
                        "blob",
                        "encodingutil",
                        "type",
                        "test",
                        "limits",
                        "request",
                        "userinfo",
                        "pattern",
                        "matcher",
                    ]
                    .contains(&key.as_str()) =>
            {
                Ok(Value::Namespace(key))
            }
            Value::Namespace(s) if s == "system" && objects::builtin_enum(&key).is_some() => {
                Ok(Value::Namespace(key))
            }
            Value::Namespace(s) if s == "dataweave" || s == "dataweavescriptresource" => {
                Ok(Value::Namespace(format!("{s}.{key}")))
            }
            Value::Namespace(s)
                if s == "cache"
                    && [
                        "org",
                        "session",
                        "partition",
                        "orgpartition",
                        "sessionpartition",
                        "cachebuilder",
                        "invalidparamexception",
                    ]
                    .contains(&key.as_str()) =>
            {
                Ok(Value::Namespace(format!("{s}.{key}")))
            }
            Value::Namespace(s) if s == "restcontext" => self.http_context_get(&key, n),
            Value::Record(r) => {
                let r = r.borrow();
                let field = self.record_field_type(&r.typ, &key);
                if field.is_none()
                    && !(r.projected
                        && r.fields
                            .get(&key)
                            .is_some_and(|v| matches!(v, Value::Record(_) | Value::List(_))))
                {
                    return block(n, format!("Unknown SObject field {name}"));
                }
                if let Some(v) = r.fields.get(&key) {
                    if let Some(field) = field {
                        self.coerce(v.clone(), &field, n)
                    } else {
                        Ok(v.clone())
                    }
                } else if r.projected {
                    exceptions::raise(n,"System.SObjectException",format!("SObject row was retrieved via SOQL without querying the requested field: {}.{}",r.typ,name))
                } else {
                    Ok(Value::Null)
                }
            }
            Value::Namespace(s) if s == "trigger" => self
                .trigger_context
                .get(&key)
                .cloned()
                .or_else(|| {
                    if [
                        "isexecuting",
                        "isbefore",
                        "isafter",
                        "isinsert",
                        "isupdate",
                        "isdelete",
                        "isundelete",
                    ]
                    .contains(&key.as_str())
                    {
                        Some(Value::Bool(false))
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    ["new", "old", "newmap", "oldmap"]
                        .contains(&key.as_str())
                        .then_some(Value::Null)
                })
                .ok_or_else(|| EvalError {
                    exception: None,
                    status: "blocked",
                    diagnostic: n
                        .diagnostic("APX_UNSUPPORTED", format!("Trigger.{name} is unavailable")),
                }),
            Value::Namespace(s) => {
                if objects::builtin_enum(&format!("{s}.{key}")).is_some() {
                    return Ok(Value::Namespace(format!("{s}.{key}")));
                }
                if platform_objects::is_namespace(&format!("{s}.{key}")) {
                    return Ok(Value::Namespace(format!("{s}.{key}")));
                }
                if let Some(members) = platform_objects::enum_members(&s) {
                    if let Some(ordinal) = members.iter().position(|x| x.eq_ignore_ascii_case(name))
                    {
                        return Ok(Value::Enum {
                            typ: s,
                            name: members[ordinal].into(),
                            ordinal,
                        });
                    }
                }
                if let Some(members) = objects::builtin_enum(&s) {
                    if let Some(ordinal) = members.iter().position(|x| x.eq_ignore_ascii_case(name))
                    {
                        return Ok(Value::Enum {
                            typ: s,
                            name: members[ordinal].into(),
                            ordinal,
                        });
                    }
                }
                if key == "class" {
                    return Ok(Value::Type(s));
                }
                let nested = format!("{s}.{key}");
                if self.program.classes.contains_key(&nested) {
                    return Ok(Value::Namespace(nested));
                }
                if let Some(c) = self.program.classes.get(&s) {
                    if let Some(ordinal) = c
                        .enum_members
                        .iter()
                        .position(|x| x.eq_ignore_ascii_case(name))
                    {
                        return Ok(Value::Enum {
                            typ: s,
                            name: c.enum_members[ordinal].clone(),
                            ordinal,
                        });
                    }
                }
                if self.program.classes.contains_key(&s) {
                    self.initialize(&s, n)?;
                    self.check_field_access(&s, &key, true, n)?;
                }
                self.statics
                    .get(&format!("{s}.{key}"))
                    .cloned()
                    .ok_or_else(|| EvalError {
                        exception: None,
                        status: "blocked",
                        diagnostic: n.diagnostic(
                            "APX_UNSUPPORTED",
                            format!("Static field {s}.{name} is unavailable"),
                        ),
                    })
            }
            Value::Null => fail(n, "Null object dereference"),
            _ => block(n, "Unsupported field access"),
        }
    }
    pub fn eval(&mut self, n: &Node) -> R<Value> {
        self.tick(n)?;
        match n.kind.as_str() {
            "int" | "decimal_floating_point_literal" => Number::parse(n.text())
                .map(Value::Num)
                .map_err(|m| EvalError {
                    exception: None,
                    status: "blocked",
                    diagnostic: n.diagnostic("APX_NUMBER", m),
                }),
            "boolean" => Ok(Value::Bool(n.text().eq_ignore_ascii_case("true"))),
            "null_literal" => Ok(Value::Null),
            "string_literal" => Ok(Value::Str(unescape(n.text(), n)?)),
            "identifier" => self.get(n.text(), n),
            "this" => self
                .this
                .clone()
                .map(Value::Instance)
                .ok_or_else(|| EvalError {
                    exception: None,
                    status: "failed",
                    diagnostic: n
                        .diagnostic("APX_RUNTIME", "this is unavailable in static context"),
                }),
            "class_literal" => {
                let typ = n.named().next().map(|n| n.text()).unwrap_or("");
                if !self.known_type_literal(typ) {
                    return block(n, format!("Unknown Type literal {typ}"));
                }
                Ok(Value::Type(
                    self.resolve_class(typ).unwrap_or_else(|| typ.into()),
                ))
            }
            "parenthesized_expression" => {
                let v = n.named().next().ok_or_else(|| EvalError {
                    exception: None,
                    status: "blocked",
                    diagnostic: n.diagnostic("APX_UNSUPPORTED", "Empty expression"),
                })?;
                self.eval(v)
            }
            "field_access" => {
                if self.need(n, "field")?.text().eq_ignore_ascii_case("class") {
                    let typ = self.need(n, "object")?.text();
                    if self.known_type_literal(typ) {
                        return Ok(Value::Type(self.qualified_type(typ, &self.current_class)));
                    }
                }
                let v = self.eval(self.need(n, "object")?)?;
                if matches!(v, Value::Null) && n.child("safe_navigation_operator").is_some() {
                    return Ok(Value::Null);
                }
                let name = self.need(n, "field")?.text();
                self.field_value(v, name, n)
            }
            "binary_expression" => {
                let left = self.eval(self.need(n, "left")?)?;
                let op = self.need(n, "operator")?.text();
                if op == "&&" && !self.boolean(left.clone(), n)? {
                    return Ok(Value::Bool(false));
                }
                if op == "||" && self.boolean(left.clone(), n)? {
                    return Ok(Value::Bool(true));
                }
                if op == "??" && !matches!(left, Value::Null) {
                    return Ok(left);
                }
                let right = self.eval(self.need(n, "right")?)?;
                self.binary(left, op, right, n)
            }
            "unary_expression" => {
                let op = n
                    .field("operator")
                    .map(|x| x.text())
                    .unwrap_or_else(|| &n.text()[..1]);
                let v = self.eval(n.field("operand").or_else(|| n.named().last()).unwrap())?;
                match (op, v) {
                    ("!", Value::Bool(b)) => Ok(Value::Bool(!b)),
                    ("-", Value::Num(mut v)) => {
                        if v.kind == NumKind::Double && v.q.is_zero() {
                            return block(n, "Signed-zero Double semantics are not implemented");
                        }
                        v.q = -v.q;
                        Ok(Value::Num(v))
                    }
                    ("+", v @ Value::Num(_)) => Ok(v),
                    _ => block(n, "Unsupported unary operation"),
                }
            }
            "assignment_expression" => {
                let target = self.need(n, "left")?;
                let op = self.need(n, "operator")?.text();
                let rhs = self.need(n, "right")?;
                let location = self.resolve_location(target)?;
                let prior = if op != "=" {
                    Some(self.read_location(&location, n)?)
                } else {
                    None
                };
                let right = self.eval(rhs)?;
                let v = if let Some(left) = prior {
                    self.binary(left, op.trim_end_matches('='), right, n)?
                } else {
                    right
                };
                self.write_location(location, v, n)
            }
            "update_expression" => {
                let target = n.named().find(|x| x.kind != "update_operator").unwrap();
                let location = self.resolve_location(target)?;
                let old = self.read_location(&location, n)?;
                let op = if n.text().contains("++") { "+" } else { "-" };
                let new = self.binary(old.clone(), op, Value::Num(Number::integer(1)), n)?;
                self.write_location(location, new.clone(), n)?;
                Ok(
                    if n.text().starts_with("++") || n.text().starts_with("--") {
                        new
                    } else {
                        old
                    },
                )
            }
            "ternary_expression" => {
                let c = self.eval(self.need(n, "condition")?)?;
                let b = self.boolean(c, n)?;
                self.eval(self.need(n, if b { "consequence" } else { "alternative" })?)
            }
            "cast_expression" => {
                let val = self.eval(self.need(n, "value")?)?;
                let typ = self.need(n, "type")?.text();
                let actual = self.runtime_type(&val);
                match self.coerce(val, typ, n) {
                    Err(e) if e.status == "failed" => exceptions::raise(
                        n,
                        "System.TypeException",
                        format!(
                            "Invalid conversion from runtime type {actual} to {}",
                            self.canonical_type_name(typ)
                        ),
                    ),
                    other => other,
                }
            }
            "instanceof_expression" => {
                let value = self.eval(self.need(n, "left")?)?;
                let typ = self.need(n, "right")?.text();
                let yes = match &value {
                    Value::Null => false,
                    Value::Instance(i) => self.class_assignable(&i.borrow().class, typ),
                    Value::Exception(e) => self.exception_matches(&e.borrow().typ, typ),
                    _ => self.coerce(value.clone(), typ, n).is_ok(),
                };
                Ok(Value::Bool(yes))
            }
            "method_invocation" => self.method(n),
            "map_creation_expression" => self.map_literal(n),
            "object_creation_expression" => self.construct(n),
            "array_creation_expression" => {
                if let Some(v) = n.field("value") {
                    let items = self.eval(v)?;
                    let typ = self.need(n, "type")?.text();
                    let list_type = if typ.to_ascii_lowercase().starts_with("list<")
                        || typ.to_ascii_lowercase().starts_with("set<")
                    {
                        typ.to_string()
                    } else {
                        format!("List<{typ}>")
                    };
                    let items = if list_type.to_ascii_lowercase().starts_with("set<") {
                        if let Value::List(v) = items {
                            Value::Set(v)
                        } else {
                            items
                        }
                    } else {
                        items
                    };
                    self.coerce(items, &list_type, n)
                } else {
                    block(n, "Array allocation without initializer is not implemented")
                }
            }
            "array_initializer" => {
                let mut v = vec![];
                for x in n.named() {
                    let item = self.eval(x)?;
                    self.charge(&item, n)?;
                    v.push(item)
                }
                Ok(Value::list(v))
            }
            "array_access" => {
                let v = self.eval(self.need(n, "array")?)?;
                let idx = self.eval(self.need(n, "index")?)?;
                if let (Value::List(l), Value::Num(i)) = (v, idx) {
                    let i = i.index().ok_or_else(|| EvalError {
                        exception: None,
                        status: "failed",
                        diagnostic: n.diagnostic("APX_INDEX", "Invalid index"),
                    })?;
                    let result = l.borrow().get(i).cloned();
                    result.ok_or_else(|| EvalError {
                        exception: None,
                        status: "failed",
                        diagnostic: n.diagnostic("APX_INDEX", "Index out of bounds"),
                    })
                } else {
                    fail(n, "Invalid indexed value")
                }
            }
            "query_expression" => {
                if n.child("sosl_query_body").is_some() {
                    self.search_query(n)
                } else {
                    self.data_query(n)
                }
            }
            "dml_expression" => self.data_dml_expression(n),
            _ => block(n, format!("Expression {} is not implemented", n.kind)),
        }
    }
    fn binary(&mut self, a: Value, op: &str, b: Value, n: &Node) -> R<Value> {
        if ["==", "===", "!=", "!==", "<>"].contains(&op) {
            let id_operand = n
                .field("left")
                .and_then(|node| self.expression_type(node))
                .into_iter()
                .chain(n.field("right").and_then(|node| self.expression_type(node)))
                .any(|t| t.eq_ignore_ascii_case("Id"));
            if id_operand {
                if let (Value::Str(a), Value::Str(b)) = (&a, &b) {
                    let eq = self.canonical_id(a, n)? == self.canonical_id(b, n)?;
                    return Ok(Value::Bool(if ["!=", "!==", "<>"].contains(&op) {
                        !eq
                    } else {
                        eq
                    }));
                }
            }
            let eq = if op == "===" || op == "!==" {
                self.identity_equal(&a, &b)
            } else {
                self.apex_equal(&a, &b, true, n)?
            };
            return Ok(Value::Bool(if ["!=", "!==", "<>"].contains(&op) {
                !eq
            } else {
                eq
            }));
        }
        if op == "??" {
            return Ok(if matches!(a, Value::Null) { b } else { a });
        }
        if ["<", ">", "<=", ">="].contains(&op)
            && (matches!(a, Value::Null) || matches!(b, Value::Null))
        {
            if matches!(a, Value::Bool(_)) || matches!(b, Value::Bool(_)) {
                return fail(n, "Boolean values cannot be ordered");
            }
            let result = match (&a, &b) {
                (Value::Str(_), Value::Null) => op == ">" || op == ">=",
                (Value::Null, Value::Str(_)) => op == "<" || op == "<=",
                _ => false,
            };
            return Ok(Value::Bool(result));
        }
        if op == "+" && (matches!(a, Value::Str(_)) || matches!(b, Value::Str(_))) {
            let a = self.display_value(&a, n)?;
            let b = self.display_value(&b, n)?;
            if a.len() + b.len() > 1024 * 1024 {
                return block(n, "String exceeds 1 MiB host bound");
            }
            return Ok(Value::Str(a + &b));
        }
        match (a, b) {
            (Value::Bool(a), Value::Bool(b)) => match op {
                "&&" => Ok(Value::Bool(a && b)),
                "||" => Ok(Value::Bool(a || b)),
                _ => block(n, "Unsupported Boolean operator"),
            },
            (Value::Num(a), Value::Num(b)) => {
                if a.kind == NumKind::Double || b.kind == NumKind::Double {
                    if (a.kind == NumKind::Decimal || b.kind == NumKind::Decimal)
                        && (!a.q.is_integer() || !b.q.is_integer())
                    {
                        return block(n,"Mixed fractional Double/Decimal operations require qualified promotion policy");
                    }
                    let af = a.q.to_f64().ok_or_else(|| EvalError {
                        exception: None,
                        status: "blocked",
                        diagnostic: n
                            .diagnostic("APX_NUMERIC", "Double operand out of finite range"),
                    })?;
                    let bf = b.q.to_f64().ok_or_else(|| EvalError {
                        exception: None,
                        status: "blocked",
                        diagnostic: n
                            .diagnostic("APX_NUMERIC", "Double operand out of finite range"),
                    })?;
                    if ["<", ">", "<=", ">="].contains(&op) {
                        return Ok(Value::Bool(match op {
                            "<" => af < bf,
                            ">" => af > bf,
                            "<=" => af <= bf,
                            _ => af >= bf,
                        }));
                    }
                    let result = match op {
                        "+" => af + bf,
                        "-" => af - bf,
                        "*" => af * bf,
                        "/" => af / bf,
                        _ => return block(n, format!("Double operator {op} unsupported")),
                    };
                    return Number::from_double(result)
                        .map(Value::Num)
                        .map_err(|message| EvalError {
                            exception: None,
                            status: "blocked",
                            diagnostic: n.diagnostic("APX_NUMERIC", message),
                        });
                }
                if ["<", ">", "<=", ">="].contains(&op) {
                    return Ok(Value::Bool(match op {
                        "<" => a.q < b.q,
                        ">" => a.q > b.q,
                        "<=" => a.q <= b.q,
                        _ => a.q >= b.q,
                    }));
                }
                let kind = if a.kind == NumKind::Decimal || b.kind == NumKind::Decimal {
                    NumKind::Decimal
                } else if a.kind == NumKind::Long || b.kind == NumKind::Long {
                    NumKind::Long
                } else {
                    NumKind::Int
                };
                if a.q.numer().bits() + a.q.denom().bits() + b.q.numer().bits() + b.q.denom().bits()
                    > 16_384
                {
                    return block(n, "Number exceeds 16384-bit host arithmetic bound");
                }
                let q = match op {
                    "+" => a.q + b.q,
                    "-" => a.q - b.q,
                    "*" => a.q * b.q,
                    "/" => {
                        if b.q.is_zero() {
                            return fail(n, "Division by zero");
                        };
                        let q = a.q / b.q;
                        if kind == NumKind::Decimal {
                            let mut d = q.denom().clone();
                            for x in [2, 5] {
                                let k = BigInt::from(x);
                                while (&d % &k).is_zero() {
                                    d /= &k
                                }
                            }
                            if d != BigInt::one() {
                                return block(n,"Non-terminating Decimal division requires unimplemented precision policy");
                            }
                            q
                        } else {
                            BigRational::from_integer(q.to_integer())
                        }
                    }
                    "%" => {
                        if !a.q.is_integer() || !b.q.is_integer() {
                            return block(n, "Decimal remainder unsupported");
                        };
                        if b.q.is_zero() {
                            return fail(n, "Division by zero");
                        };
                        BigRational::from_integer(a.q.to_integer() % b.q.to_integer())
                    }
                    _ => return block(n, format!("Numeric operator {op} unsupported")),
                };
                Number { q, kind }
                    .checked()
                    .map(Value::Num)
                    .map_err(|m| EvalError {
                        exception: None,
                        status: "failed",
                        diagnostic: n.diagnostic("APX_NUMERIC", m),
                    })
            }
            _ => fail(n, format!("Incompatible operands for {op}")),
        }
    }
    fn method(&mut self, n: &Node) -> R<Value> {
        let name = self.need(n, "name")?.text().to_ascii_lowercase();
        if n.field("object").is_some_and(|o| o.kind == "super") {
            let instance = self.this.clone().ok_or_else(|| EvalError {
                exception: None,
                status: "failed",
                diagnostic: n.diagnostic("APX_RUNTIME", "super is unavailable in static context"),
            })?;
            let parent = self
                .program
                .classes
                .get(&self.current_class)
                .and_then(|c| c.superclass.as_ref())
                .and_then(|p| self.resolve_class_from(p, &self.current_class))
                .ok_or_else(|| EvalError {
                    exception: None,
                    status: "blocked",
                    diagnostic: n
                        .diagnostic("APX_UNSUPPORTED", "super method requires a user superclass"),
                })?;
            let args = self
                .need(n, "arguments")?
                .named()
                .map(|a| self.eval(a))
                .collect::<R<Vec<_>>>()?;
            let (owner, method) = self.find_method(&parent, &name, &args, false, n)?;
            self.check_method_access(&owner, &method, n)?;
            return self.invoke_method(&owner, &method, args, Some(instance));
        }
        let object = if let Some(o) = n.field("object") {
            Some(self.eval(o)?)
        } else {
            None
        };
        if matches!(object, Some(Value::Null)) && n.child("safe_navigation_operator").is_some() {
            return Ok(Value::Null);
        }
        let mut args = vec![];
        for a in self.need(n, "arguments")?.named() {
            args.push(self.eval(a)?)
        }
        if let Some(receiver) = &object {
            if let Value::Namespace(ns) = receiver {
                if ns == "test" {
                    if let Some(v) = self.search_test_method(&name, &args, n)? {
                        return Ok(v);
                    }
                }
                if ns == "limits" {
                    if let Some(v) = self.search_limits_method(&name, &args, n)? {
                        return Ok(v);
                    }
                }
            }
            if let Value::Namespace(ns) = receiver {
                if let Some(v) = self.events_namespace(ns, &name, &args, n)? {
                    return Ok(v);
                }
            }
            if let Some(v) = self.events_method(receiver, &name, &args, n)? {
                return Ok(v);
            }
            if let Value::Namespace(ns) = receiver {
                if let Some(v) = self.async_namespace(ns, &name, &args, n)? {
                    return Ok(v);
                }
            }
            if let Some(v) = self.async_method(receiver, &name, &args, n)? {
                return Ok(v);
            }
            if let Value::Namespace(ns) = receiver {
                if let Some(v) = self.connect_api_namespace(ns, &name, &args, n)? {
                    return Ok(v);
                }
            }
            if let Some(v) = self.security_method(receiver, &name, &args, n)? {
                return Ok(v);
            }
            if let Value::Namespace(ns) = receiver {
                if let Some(v) = self.regex_namespace(ns, &name, &args, n)? {
                    return Ok(v);
                }
            }
            if let Some(v) = self.regex_method(receiver, &name, &args, n)? {
                return Ok(v);
            }
            if let Value::Namespace(ns) = receiver {
                if let Some(v) = self.cache_namespace(ns, &name, &args, n)? {
                    return Ok(v);
                }
            }
            if let Some(v) = self.cache_method(receiver, &name, &args, n)? {
                return Ok(v);
            }
            if let Value::Namespace(ns) = receiver {
                if let Some(v) = self.request_namespace(ns, &name, &args, n)? {
                    return Ok(v);
                }
            }
            if let Some(v) = self.request_method(receiver, &name, &args, n)? {
                return Ok(v);
            }
            if let Value::Namespace(ns) = receiver {
                if let Some(v) = self.userinfo_namespace(ns, &name, &args, n)? {
                    return Ok(v);
                }
            }
            if let Value::Namespace(ns) = receiver {
                if let Some(v) = self.stub_namespace(ns, &name, &args, n)? {
                    return Ok(v);
                }
            }
            if let Value::Namespace(ns) = receiver {
                if let Some(v) = self.dataweave_namespace(ns, &name, &args, n)? {
                    return Ok(v);
                }
            }
            if let Some(v) = self.dataweave_method(receiver, &name, &args, n)? {
                return Ok(v);
            }
            if let Some(v) = self.data_method(receiver, &name, &args, n)? {
                return Ok(v);
            }
            if let Some(v) = self.schema_method(receiver, &name, &args, n)? {
                return Ok(v);
            }
            let date_result = if let Value::Namespace(ns) = receiver {
                let clock = self
                    .native_state
                    .get("clock.iso")
                    .and_then(|v| {
                        if let Value::Str(s) = v {
                            Some(s.as_str())
                        } else {
                            None
                        }
                    })
                    .unwrap_or("");
                crate::dates::invoke(ns, &name, &args, clock)
            } else {
                crate::dates::instance(receiver, &name, &args)
            };
            match date_result {
                Ok(Some(v)) => return Ok(v),
                Ok(None) => {}
                Err(e) if e.unsupported => return block(n, e.message),
                Err(e) => return exceptions::raise(n, e.exception_type, e.message),
            }
            if matches!(receiver,Value::Namespace(ns) if ns=="json"||ns=="system.json") {
                return self.json_call(&name, args, n);
            }
            if matches!(receiver,Value::Namespace(ns) if ns=="test") {
                if let Some(v) = self.http_namespace(&name, &args, n)? {
                    return Ok(v);
                }
            }
            if let Some(v) = self.http_method(receiver, &name, &args, n)? {
                return Ok(v);
            }
            if let Some(v) = self.type_method(receiver, &name, &args, n)? {
                return Ok(v);
            }
            if let Some(result) = self.collection_method(receiver, &name, &args, n)? {
                return Ok(result);
            }
            if let Some(result) = self.exception_method(receiver, &name, &args, n)? {
                return Ok(result);
            }
            if let Some(result) = self.enum_method(receiver, &name, &args, n)? {
                return Ok(result);
            }
            let platform = if let Value::Namespace(ns) = receiver {
                crate::platform::invoke(ns, &name, &args)
            } else {
                crate::platform::instance(receiver, &name, &args)
            };
            match platform {
                Ok(Some(value)) => return Ok(value),
                Ok(None) => {}
                Err(e) if e.unsupported => return block(n, e.message),
                Err(e) => return exceptions::raise(n, e.exception_type, e.message),
            }
        }
        if let Some(Value::Namespace(ns)) = &object {
            if (ns == "system"
                && !(["assert", "assertequals", "assertnotequals", "debug"]
                    .contains(&name.as_str())))
                || (ns == "assert"
                    && !([
                        "istrue",
                        "isfalse",
                        "areequal",
                        "arenotequal",
                        "fail",
                        "isnull",
                        "isnotnull",
                        "isinstanceoftype",
                        "isnotinstanceoftype",
                    ]
                    .contains(&name.as_str())))
            {
                return block(n, format!("{ns}.{name} is not implemented"));
            }
        }
        match object {
            None => {
                if let Some(instance) = self.this.clone() {
                    let class = instance.borrow().class.clone();
                    if self.find_method(&class, &name, &args, false, n).is_ok() {
                        return self.call_instance(instance, &name, args, n);
                    }
                }
                let c = self.current_class.clone();
                self.call(&c, &name, args, n)
            }
            Some(Value::Instance(instance)) => self.call_instance(instance, &name, args, n),
            Some(Value::Namespace(ns)) if ns == "system" || ns == "assert" => match name.as_str() {
                "debug" => {
                    let text = match args.as_slice() {
                        [v] => self.display_value(v, n)?,
                        [Value::Enum { typ, .. }, v] if typ == "logginglevel" => {
                            self.display_value(v, n)?
                        }
                        _ => return fail(n, "Invalid System.debug arguments"),
                    };
                    if self.debug_logs.iter().map(String::len).sum::<usize>() + text.len() > 65536 {
                        return block(n, "Debug log exceeds 64 KiB host bound");
                    }
                    self.debug_logs.push(text);
                    Ok(Value::Null)
                }
                "isnull" | "isnotnull" => {
                    if args.is_empty() || args.len() > 2 {
                        return fail(n, "Null assertion expects value and optional message");
                    }
                    let isnull = matches!(args[0], Value::Null);
                    if isnull != (name == "isnull") {
                        return Err(EvalError {
                            exception: None,
                            status: "failed",
                            diagnostic: n.diagnostic(
                                "APX_ASSERTION",
                                format!(
                                    "Assertion failed: {}{}",
                                    name,
                                    args.get(1)
                                        .map(|v| format!("; {}", v.display()))
                                        .unwrap_or_default()
                                ),
                            ),
                        });
                    }
                    Ok(Value::Null)
                }
                "isinstanceoftype" | "isnotinstanceoftype" => {
                    if args.len() < 2 || args.len() > 3 {
                        return fail(n, "Type assertion requires value and Type");
                    }
                    let Value::Type(typ) = &args[1] else {
                        return fail(n, "Type assertion requires Type");
                    };
                    let yes = match &args[0] {
                        Value::Null => false,
                        Value::Instance(i) => self.class_assignable(&i.borrow().class, typ),
                        Value::Exception(e) => self.exception_matches(&e.borrow().typ, typ),
                        v => self.coerce(v.clone(), typ, n).is_ok(),
                    };
                    if yes != (name == "isinstanceoftype") {
                        return Err(EvalError {
                            exception: None,
                            status: "failed",
                            diagnostic: n.diagnostic(
                                "APX_ASSERTION",
                                "Assertion failed: instance type mismatch",
                            ),
                        });
                    }
                    Ok(Value::Null)
                }
                "assert" | "istrue" | "isfalse" => {
                    if args.is_empty() || args.len() > 2 {
                        return fail(n, "Assertion expects condition and optional message");
                    };
                    let b = self.boolean(args[0].clone(), n)?;
                    let expected = name != "isfalse";
                    if b != expected {
                        return Err(EvalError {
                            exception: None,
                            status: "failed",
                            diagnostic: n.diagnostic(
                                "APX_ASSERTION",
                                format!(
                                    "Assertion failed: expected {expected}, actual {b}{}",
                                    args.get(1)
                                        .map(|m| format!("; {}", m.display()))
                                        .unwrap_or_default()
                                ),
                            ),
                        });
                    }
                    Ok(Value::Null)
                }
                "assertequals" | "areequal" | "assertnotequals" | "arenotequal" => {
                    if args.len() < 2 || args.len() > 3 {
                        return fail(
                            n,
                            "Equality assertion expects expected, actual and optional message",
                        );
                    };
                    let eq = self.apex_equal(&args[0], &args[1], false, n)?;
                    let expecteq = name == "assertequals" || name == "areequal";
                    if eq != expecteq {
                        return Err(EvalError {
                            exception: None,
                            status: "failed",
                            diagnostic: n.diagnostic(
                                "APX_ASSERTION",
                                format!(
                                    "Assertion failed: expected {}{}, actual {}{}",
                                    if expecteq { "" } else { "not " },
                                    args[0].display(),
                                    args[1].display(),
                                    args.get(2)
                                        .map(|m| format!("; {}", m.display()))
                                        .unwrap_or_default()
                                ),
                            ),
                        });
                    }
                    Ok(Value::Null)
                }
                "fail" => Err(EvalError {
                    exception: None,
                    status: "failed",
                    diagnostic: n.diagnostic(
                        "APX_ASSERTION",
                        args.first()
                            .map(Value::display)
                            .unwrap_or_else(|| "Assertion failed".into()),
                    ),
                }),
                _ => block(n, format!("{ns}.{name} is not implemented")),
            },
            Some(Value::Namespace(ns)) if ns == "test" => {
                if !args.is_empty() {
                    return block(n, "Unsupported Test API arguments");
                };
                match name.as_str() {
                    "starttest" => {
                        if self.test_frame.is_some() {
                            return fail(n, "Test.startTest already active");
                        };
                        self.test_frame = Some((self.queries, self.dml, self.callouts));
                        self.search_start_test();
                        self.queries = 0;
                        self.dml = 0;
                        self.callouts = 0;
                        Ok(Value::Null)
                    }
                    "stoptest" => {
                        let Some((q, d, c)) = self.test_frame.take() else {
                            return fail(n, "Test.stopTest without startTest");
                        };
                        self.async_stop_test(n)?;
                        self.events_drain(n, false)?;
                        self.search_stop_test();
                        self.queries = q;
                        self.dml = d;
                        self.callouts = c;
                        Ok(Value::Null)
                    }
                    "isrunningtest" => Ok(Value::Bool(true)),
                    _ => block(n, format!("Test.{name} is not implemented")),
                }
            }
            Some(Value::Namespace(ns)) if ns == "limits" => {
                if !args.is_empty() {
                    return fail(n, "Limits getter takes no arguments");
                };
                let v=match name.as_str(){"getqueries"=>self.queries,"getdmlstatements"=>self.dml,"getlimitqueries"=>100,"getlimitdmlstatements"=>150,_=>return block(n,format!("Limits.{name} is not implemented; CPU/heap counters are not host measurements"))};
                Ok(Value::Num(Number::integer(v)))
            }
            Some(Value::Namespace(ns)) if ns == "string" => match name.as_str() {
                "valueof" if args.len() == 1 => {
                    if matches!(args[0], Value::Null) {
                        Ok(Value::Null)
                    } else {
                        Ok(Value::Str(self.display_value(&args[0], n)?))
                    }
                }
                "isblank" | "isnotblank" | "isempty" | "isnotempty" if args.len() == 1 => {
                    let empty = match &args[0] {
                        Value::Null => true,
                        Value::Str(s) => {
                            if name.contains("blank") {
                                s.trim().is_empty()
                            } else {
                                s.is_empty()
                            }
                        }
                        _ => return fail(n, "String predicate requires String"),
                    };
                    Ok(Value::Bool(if name.starts_with("isnot") {
                        !empty
                    } else {
                        empty
                    }))
                }
                _ => block(n, format!("String.{name} is not implemented")),
            },
            Some(Value::Namespace(ns)) if ["integer", "long", "decimal"].contains(&ns.as_str()) => {
                if name == "valueof" && args.len() == 1 {
                    let text = self.display_value(&args[0], n)?;
                    let v = match Number::parse(&text) {
                        Ok(v) => v,
                        Err(_) => {
                            return exceptions::raise(
                                n,
                                "System.TypeException",
                                format!("Invalid {ns}: {text}"),
                            )
                        }
                    };
                    self.coerce(Value::Num(v), &ns, n)
                } else {
                    block(n, format!("{ns}.{name} is not implemented"))
                }
            }
            Some(Value::Namespace(ns)) => self.call(&ns, &name, args, n),
            Some(Value::Str(s)) => match name.as_str() {
                "length" if args.is_empty() => {
                    Ok(Value::Num(Number::integer(s.encode_utf16().count() as i64)))
                }
                "touppercase" | "tolowercase" if args.is_empty() => {
                    if !s.is_ascii() {
                        return block(
                            n,
                            "Non-ASCII case mapping requires qualified Unicode policy",
                        );
                    }
                    Ok(Value::Str(if name == "touppercase" {
                        s.to_ascii_uppercase()
                    } else {
                        s.to_ascii_lowercase()
                    }))
                }
                "trim" if args.is_empty() => Ok(Value::Str(
                    s.trim_matches(|c: char| c.is_ascii_whitespace()).into(),
                )),
                "equals" | "equalsignorecase" | "contains" | "startswith" | "endswith"
                    if args.len() == 1 =>
                {
                    let Value::Str(b) = &args[0] else {
                        return fail(n, "Expected String argument");
                    };
                    let b = match name.as_str() {
                        "equals" => s == *b,
                        "equalsignorecase" => {
                            if !s.is_ascii() || !b.is_ascii() {
                                return block(
                                    n,
                                    "Non-ASCII equality requires qualified Unicode policy",
                                );
                            }
                            s.eq_ignore_ascii_case(b)
                        }
                        "contains" => s.contains(b),
                        "startswith" => s.starts_with(b),
                        _ => s.ends_with(b),
                    };
                    Ok(Value::Bool(b))
                }
                _ => block(
                    n,
                    format!("String.{name}/{} is not implemented", args.len()),
                ),
            },
            Some(Value::List(list)) => match name.as_str() {
                "size" if args.is_empty() => {
                    Ok(Value::Num(Number::integer(list.borrow().len() as i64)))
                }
                "isempty" if args.is_empty() => Ok(Value::Bool(list.borrow().is_empty())),
                "add" if args.len() == 1 => {
                    self.ensure_list_writable(&list, n)?;
                    if list.borrow().len() > 100_000 {
                        return block(n, "Host list bound exceeded");
                    };
                    self.reject_cycle(&args[0], Rc::as_ptr(&list) as usize, n)?;
                    let typ = self
                        .list_types
                        .borrow()
                        .get(&(Rc::as_ptr(&list) as usize))
                        .cloned()
                        .ok_or_else(|| EvalError {
                            exception: None,
                            status: "blocked",
                            diagnostic: n
                                .diagnostic("APX_UNSUPPORTED", "List element type is unavailable"),
                        })?;
                    let val = self.coerce(args[0].clone(), &typ, n)?;
                    list.borrow_mut().push(val);
                    Ok(Value::Null)
                }
                "clear" if args.is_empty() => {
                    self.ensure_list_writable(&list, n)?;
                    list.borrow_mut().clear();
                    Ok(Value::Null)
                }
                "get" | "set" | "remove" => {
                    if name != "get" {
                        self.ensure_list_writable(&list, n)?;
                    }
                    let Some(Value::Num(idx)) = args.first() else {
                        return fail(n, "List index must be Integer");
                    };
                    let i = idx.index().ok_or_else(|| EvalError {
                        exception: None,
                        status: "failed",
                        diagnostic: n.diagnostic("APX_INDEX", "Invalid index"),
                    })?;
                    let newvalue = if name == "set" && args.len() == 2 {
                        self.reject_cycle(&args[1], Rc::as_ptr(&list) as usize, n)?;
                        let typ = self
                            .list_types
                            .borrow()
                            .get(&(Rc::as_ptr(&list) as usize))
                            .cloned()
                            .ok_or_else(|| EvalError {
                                exception: None,
                                status: "blocked",
                                diagnostic: n
                                    .diagnostic("APX_UNSUPPORTED", "List element type unavailable"),
                            })?;
                        Some(self.coerce(args[1].clone(), &typ, n)?)
                    } else {
                        None
                    };
                    let mut ls = list.borrow_mut();
                    if i >= ls.len() {
                        return fail(n, "List index out of bounds");
                    };
                    match (name.as_str(), args.len()) {
                        ("get", 1) => Ok(ls[i].clone()),
                        ("remove", 1) => Ok(ls.remove(i)),
                        ("set", 2) => {
                            ls[i] = newvalue.unwrap();
                            Ok(Value::Null)
                        }
                        _ => block(n, "Unsupported List arguments"),
                    }
                }
                _ => block(n, format!("List.{name} is not implemented")),
            },
            Some(Value::Record(_)) if name == "adderror" => fail(
                n,
                args.first()
                    .map(Value::display)
                    .unwrap_or_else(|| "Record rejected by addError".into()),
            ),
            Some(Value::Null) => {
                let isblob = n
                    .field("object")
                    .and_then(|o| self.expression_type(o))
                    .is_some_and(|t| t.eq_ignore_ascii_case("blob"));
                exceptions::raise(
                    n,
                    "System.NullPointerException",
                    if isblob && name == "tostring" {
                        "Argument cannot be null."
                    } else {
                        "Attempt to de-reference a null object"
                    },
                )
            }
            _ => block(n, format!("Method {name} on this value is not implemented")),
        }
    }
    fn construct(&mut self, n: &Node) -> R<Value> {
        let typ = self.need(n, "type")?.text().to_ascii_lowercase();
        self.guard_type_binding(&typ, n)?;
        let args = self.need(n, "arguments")?;
        if platform_objects::is_type(&typ) {
            let values = args.named().map(|n| self.eval(n)).collect::<R<Vec<_>>>()?;
            if let Some(v) = self.platform_object_construct(&typ, &values, n)? {
                return Ok(v);
            }
        }
        if dataweave::dataweave_is_type(&typ) {
            let values = args.named().map(|n| self.eval(n)).collect::<R<Vec<_>>>()?;
            if let Some(v) = self.dataweave_construct(&typ, &values, n)? {
                return Ok(v);
            }
        }
        if http::http_is_type(&typ) {
            let values = args.named().map(|n| self.eval(n)).collect::<R<Vec<_>>>()?;
            if let Some(v) = self.http_construct(&typ, &values, n)? {
                return Ok(v);
            }
        }
        if let Some(result) = self.construct_collection(&typ, args, n)? {
            return Ok(result);
        }
        if self.is_exception_type(&typ) {
            let values = args.named().map(|n| self.eval(n)).collect::<R<Vec<_>>>()?;
            return self.construct_exception(&typ, &values, n);
        }
        if let Some(class) = self.resolve_class(&typ) {
            let values = args.named().map(|n| self.eval(n)).collect::<R<Vec<_>>>()?;
            return self.construct_instance(&class, values, n);
        }
        if typ.starts_with("list<") {
            if args.named().next().is_some() {
                return block(n, "List copy constructors are not implemented");
            };
            return self.coerce(Value::list(vec![]), &typ, n);
        }
        if self.program.schema.object(&typ).is_none() {
            return block(n,format!("Construction of {typ} is not implemented; only synthetic Account/Contact records are available"));
        }
        let mut fields = BTreeMap::new();
        for arg in args.named() {
            if arg.kind != "assignment_expression" || self.need(arg, "operator")?.text() != "=" {
                return block(arg, "SObject constructor requires named fields");
            };
            let key = self.need(arg, "left")?.text().to_ascii_lowercase();
            let field_type = self.record_field_type(&typ, &key);
            if field_type.is_none() {
                return block(arg, format!("Unknown {typ} field {key}"));
            };
            let v = self.eval(self.need(arg, "right")?)?;
            if !v.is_scalar() && !matches!(v, Value::Record(_)) {
                return block(arg, "Record-valued fields unsupported");
            };
            let v = self.coerce(v, &field_type.unwrap(), arg)?;
            if fields.insert(key, v).is_some() {
                return fail(arg, "Duplicate constructor field");
            }
        }
        Ok(Value::Record(Rc::new(RefCell::new(Record {
            typ,
            fields,
            projected: false,
        }))))
    }
    fn exec(&mut self, n: &Node) -> R<Flow> {
        self.tick(n)?;
        match n.kind.as_str() {
            "switch_expression" => self.exec_switch(n),
            "try_statement" => self.exec_try(n),
            "throw_statement" => {
                let value = self.eval(n.named().next().ok_or_else(|| EvalError {
                    exception: None,
                    status: "blocked",
                    diagnostic: n.diagnostic("APX_UNSUPPORTED", "Missing throw value"),
                })?)?;
                if let Value::Exception(e) = &value {
                    return Err(EvalError {
                        exception: Some(value.clone()),
                        status: "failed",
                        diagnostic: n.diagnostic(
                            "APX_EXCEPTION",
                            format!("{}: {}", e.borrow().typ, e.borrow().message),
                        ),
                    });
                }
                fail(n, "Thrown value is not an Exception")
            }
            "block" | "trigger_body" | "constructor_body" => {
                self.locals.push(BTreeMap::new());
                self.local_types.push(BTreeMap::new());
                let res = (|| {
                    for c in n.named() {
                        let f = self.exec(c)?;
                        if !matches!(f, Flow::Next) {
                            return Ok(f);
                        }
                    }
                    Ok(Flow::Next)
                })();
                self.locals.pop();
                self.local_types.pop();
                res
            }
            "local_variable_declaration" => {
                let typ = self.need(n, "type")?.text();
                for d in n.fields("declarator") {
                    let key = self.need(d, "name")?.text().to_ascii_lowercase();
                    let val = if let Some(v) = d.field("value") {
                        self.eval(v)?
                    } else {
                        Value::Null
                    };
                    let val = self.coerce(val, typ, d)?;
                    self.local_types
                        .last_mut()
                        .unwrap()
                        .insert(key.clone(), typ.into());
                    if self.locals.last_mut().unwrap().insert(key, val).is_some() {
                        return fail(d, "Duplicate local variable");
                    }
                }
                Ok(Flow::Next)
            }
            "expression_statement" => {
                if let Some(c) = n.named().next() {
                    self.eval(c)?;
                }
                Ok(Flow::Next)
            }
            "return_statement" => {
                let v = if let Some(c) = n.named().next() {
                    self.eval(c)?
                } else {
                    Value::Null
                };
                Ok(Flow::Return(v))
            }
            "if_statement" => {
                let c = self.eval(self.need(n, "condition")?)?;
                if self.boolean(c, n)? {
                    self.exec(self.need(n, "consequence")?)
                } else if let Some(alt) = n.field("alternative") {
                    self.exec(alt)
                } else {
                    Ok(Flow::Next)
                }
            }
            "while_statement" => {
                loop {
                    let c = self.eval(self.need(n, "condition")?)?;
                    if !self.boolean(c, n)? {
                        break;
                    };
                    match self.exec(self.need(n, "body")?)? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        _ => {}
                    }
                }
                Ok(Flow::Next)
            }
            "for_statement" => {
                self.locals.push(BTreeMap::new());
                self.local_types.push(BTreeMap::new());
                let result = (|| {
                    for x in n.fields("init") {
                        if x.kind == "local_variable_declaration" {
                            self.exec(x)?;
                        } else {
                            self.eval(x)?;
                        }
                    }
                    loop {
                        if let Some(c) = n.field("condition") {
                            let v = self.eval(c)?;
                            if !self.boolean(v, c)? {
                                break;
                            }
                        }
                        match self.exec(self.need(n, "body")?)? {
                            Flow::Break => break,
                            Flow::Return(v) => return Ok(Flow::Return(v)),
                            _ => {}
                        }
                        for u in n.fields("update") {
                            self.eval(u)?;
                        }
                    }
                    Ok(Flow::Next)
                })();
                self.locals.pop();
                self.local_types.pop();
                result
            }
            "enhanced_for_statement" => {
                let value = self.eval(self.need(n, "value")?)?;
                if let Value::Instance(instance) = value {
                    return self.exec_custom_iterable(instance, n);
                }
                let list = match value {
                    Value::List(list) | Value::Iterable(list) | Value::Set(list) => list,
                    _ => return block(n, "For-each supports List and List-backed Iterable only"),
                };
                let identity = Rc::as_ptr(&list) as usize;
                *self.iteration_locks.entry(identity).or_default() += 1;
                let key = self.need(n, "name")?.text().to_ascii_lowercase();
                let typ = self.need(n, "type")?.text().to_string();
                let items = if self.need(n, "value")?.kind == "query_expression"
                    && (typ.to_ascii_lowercase().starts_with("list<") || typ.ends_with("[]"))
                {
                    list.borrow()
                        .chunks(200)
                        .map(|chunk| Value::list(chunk.to_vec()))
                        .collect()
                } else {
                    list.borrow().clone()
                };
                self.locals.push(BTreeMap::new());
                self.local_types
                    .push(BTreeMap::from([(key.clone(), typ.clone())]));
                let result = (|| {
                    for item in items {
                        let item = self.coerce(item, &typ, n)?;
                        self.locals.last_mut().unwrap().insert(key.clone(), item);
                        match self.exec(self.need(n, "body")?)? {
                            Flow::Break => break,
                            Flow::Return(v) => return Ok(Flow::Return(v)),
                            _ => {}
                        }
                    }
                    Ok(Flow::Next)
                })();
                self.locals.pop();
                self.local_types.pop();
                let locks = self.iteration_locks.get_mut(&identity).unwrap();
                *locks -= 1;
                if *locks == 0 {
                    self.iteration_locks.remove(&identity);
                }
                result
            }
            "break_statement" => Ok(Flow::Break),
            "run_as_statement" => self.exec_run_as(n),
            "continue_statement" => Ok(Flow::Continue),
            "line_comment" | "block_comment" | ";" => Ok(Flow::Next),
            _ => block(n, format!("Statement {} is not implemented", n.kind)),
        }
    }
}
fn unescape(s: &str, n: &Node) -> R<String> {
    let Some(inner) = s.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')) else {
        return block(n, "Multiline/unusual String literal unsupported");
    };
    let mut out = String::new();
    let mut ch = inner.chars();
    while let Some(c) = ch.next() {
        if c == '\'' {
            return block(
                n,
                "Unescaped quote or compound query predicate is unsupported",
            );
        };
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(e) = ch.next() else {
            return fail(n, "Unterminated escape");
        };
        out.push(match e {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'b' => '\x08',
            'f' => '\x0c',
            '\\' => '\\',
            '\'' => '\'',
            '"' => '"',
            _ => {
                return block(
                    n,
                    format!("String escape \\{e} requires unimplemented handling"),
                )
            }
        })
    }
    Ok(out)
}
