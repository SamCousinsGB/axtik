use super::*;
pub(super) fn builtin_enum(name: &str) -> Option<&'static [&'static str]> {
    match name.trim_start_matches("system.") {
        "logginglevel" => Some(&[
            "NONE", "ERROR", "WARN", "INFO", "DEBUG", "FINE", "FINER", "FINEST",
        ]),
        "accesslevel" => Some(&["SYSTEM_MODE", "USER_MODE"]),
        "accesstype" => Some(&["READABLE", "CREATABLE", "UPDATABLE", "UPSERTABLE"]),
        "schema.sobjectdescribeoptions" => Some(&["DEFERRED", "FULL"]),
        "triggeroperation" => Some(&[
            "BEFORE_INSERT",
            "BEFORE_UPDATE",
            "BEFORE_DELETE",
            "AFTER_INSERT",
            "AFTER_UPDATE",
            "AFTER_DELETE",
            "AFTER_UNDELETE",
        ]),
        "quiddity" => Some(&[
            "RUNTEST_SYNC",
            "RUNTEST_ASYNC",
            "RUNTEST_DEPLOY",
            "FUTURE",
            "QUEUEABLE",
            "BATCH_APEX",
            "SCHEDULED",
            "ANONYMOUS",
            "AURA",
            "REST",
            "SOAP",
            "TRIGGER",
            "VISUALFORCE",
            "SYNCHRONOUS",
            "VF",
            "DISCOVERABLE_LOGIN",
            "INBOUND_EMAIL_SERVICE",
            "INVOCABLE_ACTION",
            "IOT",
            "QUICK_ACTION",
            "REMOTE_ACTION",
        ]),
        _ => None,
    }
}
impl Context<'_> {
    pub(super) fn effective_sharing(&self) -> bool {
        self.sharing_context
    }
    fn class_sharing(&self, class: &str) -> bool {
        let modifiers = self
            .program
            .classes
            .get(&class.to_ascii_lowercase())
            .and_then(|c| c.declaration.child("modifiers"))
            .map(|n| {
                n.text()
                    .to_ascii_lowercase()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        if modifiers.contains("without sharing") {
            false
        } else if modifiers.contains("with sharing") {
            true
        } else {
            self.sharing_context
        }
    }
    pub(super) fn canonical_id(&self, text: &str, at: &Node) -> R<String> {
        if ![15, 18].contains(&text.len()) || !text.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return exceptions::raise(at, "System.StringException", format!("Invalid id: {text}"));
        }
        if text.len() == 15 {
            return Ok(crate::record_store::id18(text));
        }
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ012345";
        let mut body = text.as_bytes()[..15].to_vec();
        for (group, suffix) in text.as_bytes()[15..].iter().enumerate() {
            let Some(mask) = alphabet
                .iter()
                .position(|b| *b == suffix.to_ascii_uppercase())
            else {
                return exceptions::raise(
                    at,
                    "System.StringException",
                    format!("Invalid id: {text}"),
                );
            };
            for i in 0..5 {
                let b = &mut body[group * 5 + i];
                if mask & (1 << i) != 0 {
                    if !b.is_ascii_alphabetic() {
                        return exceptions::raise(
                            at,
                            "System.StringException",
                            format!("Invalid id: {text}"),
                        );
                    }
                    *b = b.to_ascii_uppercase();
                } else {
                    *b = b.to_ascii_lowercase();
                }
            }
        }
        Ok(crate::record_store::id18(
            std::str::from_utf8(&body).unwrap(),
        ))
    }
    pub(super) fn known_type_literal(&self, typ: &str) -> bool {
        if typ.len() > 4096 || typ.bytes().filter(|c| *c == b'<').count() > 64 {
            return false;
        }
        let t = typ.to_ascii_lowercase().replace(' ', "");
        let t = t.trim_start_matches("system.");
        if let Some(element) = t.strip_suffix("[]") {
            return self.known_type_literal(element);
        }
        if let Some((base, params)) = collections::generic(t) {
            return ["list", "set", "map", "iterable", "iterator"].contains(&base)
                && params.iter().all(|p| self.known_type_literal(p));
        }
        if let Some(script) = t.strip_prefix("dataweavescriptresource.") {
            return self
                .program
                .dataweave_scripts
                .keys()
                .any(|s| s.eq_ignore_ascii_case(script));
        }
        self.resolve_class(t).is_some()
            || self.program.schema.object(t).is_some()
            || self.is_exception_type(t)
            || http::http_is_type(t)
            || cache::cache_is_type(t)
            || events::is_type(t)
            || async_jobs::async_is_type(t)
            || regex_api::is_type(t)
            || platform_objects::is_type(t)
            || platform_objects::enum_members(t).is_some()
            || builtin_enum(t).is_some()
            || dataweave::dataweave_is_type(t)
            || [
                "integer",
                "long",
                "decimal",
                "double",
                "boolean",
                "string",
                "id",
                "object",
                "sobject",
                "date",
                "datetime",
                "time",
                "blob",
                "type",
                "aggregateresult",
                "request",
                "httpcalloutmock",
                "stubprovider",
                "queueable",
                "schedulable",
                "schema.sobjecttype",
                "schema.sobjectfield",
                "schema.describesobjectresult",
                "schema.describefieldresult",
                "database.saveresult",
                "database.upsertresult",
                "database.deleteresult",
                "database.undeleteresult",
                "database.error",
                "sobjectaccessdecision",
                "eventbus.eventpublishsuccesscallback",
                "eventbus.eventpublishfailurecallback",
            ]
            .contains(&t)
    }
    pub(super) fn record_field_type(&self, object: &str, field: &str) -> Option<String> {
        if let Some(f) = self.program.schema.field(object, field) {
            return Some(f.apex_type().into());
        }
        let object = self.program.schema.object(object)?;
        let related = object
            .fields
            .values()
            .filter(|f| {
                f.relationship_name
                    .as_ref()
                    .is_some_and(|r| r.eq_ignore_ascii_case(field))
            })
            .collect::<Vec<_>>();
        if related.len() == 1 && related[0].reference_to.len() == 1 {
            Some(related[0].reference_to[0].clone())
        } else {
            None
        }
    }
    pub(super) fn canonical_type_name(&self, typ: &str) -> String {
        let key = typ.to_ascii_lowercase();
        if let Some(c) = self
            .resolve_class(&key)
            .and_then(|k| self.program.classes.get(&k))
        {
            return c.name.clone();
        }
        if let Some(o) = self.program.schema.object(&key) {
            return o.name.clone();
        }
        match key.trim_start_matches("system.") {
            "string" => "String",
            "integer" => "Integer",
            "long" => "Long",
            "decimal" => "Decimal",
            "boolean" => "Boolean",
            "date" => "Date",
            "datetime" => "Datetime",
            "time" => "Time",
            "blob" => "Blob",
            "object" => "Object",
            _ => return typ.into(),
        }
        .into()
    }
    pub(super) fn runtime_type(&self, v: &Value) -> String {
        match v {
            Value::Null => "null".into(),
            Value::Str(_) => "String".into(),
            Value::Num(n) => match n.kind {
                NumKind::Int => "Integer",
                NumKind::Long => "Long",
                NumKind::Decimal => "Decimal",
                NumKind::Double => "Double",
            }
            .into(),
            Value::Bool(_) => "Boolean".into(),
            Value::Date(_) => "Date".into(),
            Value::Datetime(_) => "Datetime".into(),
            Value::Time(_) => "Time".into(),
            Value::Blob(_) => "Blob".into(),
            Value::Instance(i) => self.canonical_type_name(&i.borrow().class),
            Value::Record(r) => self.canonical_type_name(&r.borrow().typ),
            Value::Exception(e) => self.canonical_type_name(&e.borrow().typ),
            Value::Enum { typ, .. } => self.canonical_type_name(typ),
            Value::List(xs) | Value::Iterable(xs) => format!(
                "List<{}>",
                self.list_types
                    .borrow()
                    .get(&(Rc::as_ptr(xs) as usize))
                    .map(|s| self.canonical_type_name(s))
                    .unwrap_or("Object".into())
            ),
            Value::Set(_) => "Set<Object>".into(),
            Value::Map(_) => "Map<Object,Object>".into(),
            Value::Type(_) => "System.Type".into(),
            Value::Namespace(s) => s.clone(),
        }
    }
    pub(super) fn expression_type(&mut self, node: &Node) -> Option<String> {
        match node.kind.as_str() {
            "identifier" => {
                let key = node.text().to_ascii_lowercase();
                for scope in self.local_types.iter().rev() {
                    if let Some(t) = scope.get(&key) {
                        return Some(t.clone());
                    }
                }
                self.static_types
                    .get(&format!("{}.{key}", self.current_class))
                    .cloned()
                    .or_else(|| {
                        self.this
                            .as_ref()
                            .and_then(|i| i.borrow().field_types.get(&key).cloned())
                    })
            }
            "cast_expression" | "object_creation_expression" => {
                node.field("type").map(|n| n.text().into())
            }
            "field_access" => {
                let object = node.field("object")?;
                let key = node.field("field")?.text().to_ascii_lowercase();
                let value = if object.kind == "identifier" {
                    let name = object.text().to_ascii_lowercase();
                    self.locals
                        .iter()
                        .rev()
                        .find_map(|s| s.get(&name).cloned())
                        .or_else(|| {
                            self.statics
                                .get(&format!("{}.{name}", self.current_class))
                                .cloned()
                        })
                        .or_else(|| {
                            self.this
                                .as_ref()
                                .and_then(|i| i.borrow().fields.get(&name).cloned())
                        })?
                } else if object.kind == "this" {
                    Value::Instance(self.this.clone()?)
                } else {
                    return None;
                };
                match value {
                    Value::Instance(i) => i.borrow().field_types.get(&key).cloned(),
                    Value::Record(r) => self
                        .program
                        .schema
                        .field(&r.borrow().typ, &key)
                        .map(|f| f.apex_type().into()),
                    Value::Namespace(c) => self.static_types.get(&format!("{c}.{key}")).cloned(),
                    _ => None,
                }
            }
            _ => None,
        }
    }
    fn test_context(&self) -> bool {
        let mut scope = Some(self.current_class.as_str());
        while let Some(name) = scope {
            if self.program.classes.get(name).is_some_and(|c| {
                c.declaration
                    .child("modifiers")
                    .is_some_and(|n| n.text().to_ascii_lowercase().contains("@istest"))
            }) {
                return true;
            }
            scope = name.rsplit_once('.').map(|(parent, _)| parent);
        }
        false
    }
    pub(super) fn visible(&self, owner: &str, modifiers: &str) -> bool {
        self.current_class.is_empty()
            || self.current_class.split('.').next() == owner.split('.').next()
            || modifiers
                .split_whitespace()
                .any(|m| m == "public" || m == "global")
            || modifiers.contains("@testvisible") && self.test_context()
            || modifiers.split_whitespace().any(|m| m == "protected")
                && self.class_assignable(&self.current_class, owner)
    }
    pub(super) fn check_field_access(
        &self,
        class: &str,
        name: &str,
        is_static: bool,
        at: &Node,
    ) -> R<()> {
        if http::http_is_type(class)
            || platform_objects::is_type(class)
            || [
                "database.saveresult",
                "database.upsertresult",
                "database.deleteresult",
                "database.undeleteresult",
                "database.error",
            ]
            .contains(&class.to_ascii_lowercase().as_str())
        {
            return Ok(());
        }
        for owner in self.class_chain(class, at)? {
            let c = &self.program.classes[&owner];
            let fields = if is_static {
                &c.fields
            } else {
                &c.instance_fields
            };
            for field in fields {
                if field.fields("declarator").any(|d| {
                    d.field("name")
                        .is_some_and(|n| n.text().eq_ignore_ascii_case(name))
                }) {
                    let modifiers = field
                        .child("modifiers")
                        .map(|n| n.text().to_ascii_lowercase())
                        .unwrap_or_default();
                    if !self.visible(&owner, &modifiers) {
                        return block(at, "Non-public field access is not allowed in this context");
                    }
                    return Ok(());
                }
            }
        }
        Ok(())
    }
    pub(super) fn exec_switch(&mut self, n: &Node) -> R<Flow> {
        let value = self.eval(self.need(n, "condition")?)?;
        let mut otherwise = None;
        for rule in self.need(n, "body")?.named() {
            let Some(label) = rule.child("switch_label") else {
                return block(rule, "Missing switch label");
            };
            let Some(body) = rule.child("block") else {
                return block(rule, "Missing switch body");
            };
            let labels = label.named().collect::<Vec<_>>();
            if labels.is_empty() {
                otherwise = Some(body);
                continue;
            }
            for candidate in labels {
                if candidate.kind == "when_sobject_type" {
                    let pieces = candidate.named().collect::<Vec<_>>();
                    if pieces.len() != 2 {
                        return block(candidate, "Invalid typed switch label");
                    }
                    if let Value::Record(r) = &value {
                        if r.borrow().typ.eq_ignore_ascii_case(pieces[0].text()) {
                            let name = pieces[1].text().to_ascii_lowercase();
                            self.locals
                                .push(BTreeMap::from([(name.clone(), value.clone())]));
                            self.local_types
                                .push(BTreeMap::from([(name, pieces[0].text().into())]));
                            let out = self.exec(body);
                            self.locals.pop();
                            self.local_types.pop();
                            return out;
                        }
                    }
                    continue;
                }
                let matches = if let Value::Enum { name, .. } = &value {
                    candidate.kind == "identifier" && name.eq_ignore_ascii_case(candidate.text())
                } else {
                    let v = self.eval(candidate)?;
                    value.equal(&v)
                };
                if matches {
                    return self.exec(body);
                }
            }
        }
        if let Some(body) = otherwise {
            self.exec(body)
        } else {
            Ok(Flow::Next)
        }
    }
    pub(super) fn class_chain(&self, class: &str, at: &Node) -> R<Vec<String>> {
        let mut chain = vec![];
        let mut current = Some(class.to_ascii_lowercase());
        while let Some(name) = current {
            if chain.contains(&name) || chain.len() >= 64 {
                return block(at, "Cyclic/deep class inheritance");
            }
            let c = self.program.classes.get(&name).ok_or_else(|| EvalError {
                exception: None,
                status: "blocked",
                diagnostic: at.diagnostic("APX_UNSUPPORTED", format!("Unknown class {name}")),
            })?;
            chain.push(name.clone());
            current = if let Some(parent) = &c.superclass {
                if parent.eq_ignore_ascii_case("exception") {
                    None
                } else {
                    Some(
                        self.resolve_class_from(parent, &name)
                            .ok_or_else(|| EvalError {
                                exception: None,
                                status: "blocked",
                                diagnostic: at.diagnostic(
                                    "APX_UNSUPPORTED",
                                    format!("Unknown superclass {parent}"),
                                ),
                            })?,
                    )
                }
            } else {
                None
            };
        }
        Ok(chain)
    }
    pub(super) fn initialize_class(&mut self, class: &str, at: &Node) -> R<()> {
        let class = self
            .resolve_class(class)
            .unwrap_or_else(|| class.to_ascii_lowercase());
        if self.initialized.contains(&class) {
            return Ok(());
        }
        if self.initializing.contains(&class) {
            if self.current_class == class {
                return Ok(());
            }
            return block(at, "Cyclic static initialization is not implemented");
        }
        let c = self
            .program
            .classes
            .get(&class)
            .cloned()
            .ok_or_else(|| EvalError {
                exception: None,
                status: "blocked",
                diagnostic: at.diagnostic("APX_UNSUPPORTED", format!("Unknown class {class}")),
            })?;
        if !c.unsupported.is_empty() {
            return block(
                at,
                format!(
                    "Class {} requires unsupported features: {}",
                    c.name,
                    c.unsupported.join(", ")
                ),
            );
        }
        self.initializing.insert(class.clone());
        if let Some(parent) = &c.superclass {
            if let Some(parent) = self.resolve_class_from(parent, &class) {
                self.initialize(&parent, at)?;
            } else if !parent.eq_ignore_ascii_case("Exception") {
                return block(at, format!("Unknown superclass {parent}"));
            }
        }
        let newsharing = self.class_sharing(&class);
        let oldsharing = std::mem::replace(&mut self.sharing_context, newsharing);
        let old = std::mem::replace(&mut self.current_class, class.clone());
        let oldthis = self.this.take();
        let result = (|| {
            for field in &c.fields {
                let typ = self.need(field, "type")?.text();
                for d in field.fields("declarator") {
                    let name = self.need(d, "name")?.text().to_ascii_lowercase();
                    self.statics.insert(format!("{class}.{name}"), Value::Null);
                    self.static_types
                        .insert(format!("{class}.{name}"), typ.into());
                }
            }
            let mut initializers = c
                .fields
                .iter()
                .chain(
                    c.initializers
                        .iter()
                        .filter(|n| n.kind == "static_initializer"),
                )
                .collect::<Vec<_>>();
            initializers.sort_by_key(|n| n.start);
            for init in initializers {
                if init.kind == "field_declaration" {
                    let typ = self.need(init, "type")?.text();
                    for d in init.fields("declarator") {
                        if let Some(v) = d.field("value") {
                            let value = self.eval(v)?;
                            let value = self.coerce(value, typ, d)?;
                            self.statics.insert(
                                format!(
                                    "{class}.{}",
                                    self.need(d, "name")?.text().to_ascii_lowercase()
                                ),
                                value,
                            );
                        }
                    }
                } else if let Some(body) = init.child("block") {
                    match self.exec(body)? {
                        Flow::Next => {}
                        _ => return fail(body, "Control flow cannot escape a static initializer"),
                    }
                }
            }
            Ok(())
        })();
        self.current_class = old;
        self.this = oldthis;
        self.sharing_context = oldsharing;
        self.initializing.remove(&class);
        if result.is_ok() {
            self.initialized.insert(class);
        }
        result
    }
    fn argument_score(&self, value: &Value, typ: &str, hint: Option<&str>) -> Option<u32> {
        let typ = typ.to_ascii_lowercase().replace(' ', "");
        if let Some(hint) = hint {
            if hint.to_ascii_lowercase().replace(' ', "") == typ {
                return Some(0);
            }
        }
        match value {
            Value::Type(_)
                if typ == "type" || typ == "system.type" || typ.starts_with("schema.") =>
            {
                Some(0)
            }
            Value::Date(_) if typ == "date" => Some(0),
            Value::Datetime(_) if typ == "datetime" => Some(0),
            Value::Time(_) if typ == "time" => Some(0),
            Value::Null => Some(10),
            Value::Str(_) if typ == "string" || typ == "id" => Some(0),
            Value::Bool(_) if typ == "boolean" => Some(0),
            Value::Num(n) => match (n.kind, typ.as_str()) {
                (NumKind::Int, "integer")
                | (NumKind::Long, "long")
                | (NumKind::Decimal, "decimal")
                | (NumKind::Double, "double") => Some(0),
                (NumKind::Int, "long") => Some(1),
                (_, "decimal" | "double") => Some(2),
                _ if typ == "object" => Some(20),
                _ => None,
            },
            Value::Blob(_) if typ == "blob" => Some(0),
            Value::List(_) if typ.starts_with("list<") || typ.ends_with("[]") => Some(1),
            Value::Map(_) if typ.starts_with("map<") => Some(1),
            Value::Set(_) if typ.starts_with("set<") => Some(1),
            Value::Record(r) if r.borrow().typ.eq_ignore_ascii_case(&typ) => Some(0),
            Value::Record(_) if typ == "sobject" => Some(2),
            Value::Exception(e) if self.exception_matches(&e.borrow().typ, &typ) => {
                Some(if e.borrow().typ.eq_ignore_ascii_case(&typ) {
                    0
                } else {
                    2
                })
            }
            Value::Instance(i) if self.class_assignable(&i.borrow().class, &typ) => {
                Some(if i.borrow().class == typ { 0 } else { 2 })
            }
            Value::Enum { typ: actual, .. }
                if self.resolve_class(&typ).as_deref() == Some(actual)
                    || typ.trim_start_matches("system.")
                        == actual.to_ascii_lowercase().trim_start_matches("system.") =>
            {
                Some(0)
            }
            _ if typ == "object" => Some(20),
            _ => None,
        }
    }
    pub(super) fn find_method(
        &self,
        class: &str,
        name: &str,
        args: &[Value],
        static_method: bool,
        at: &Node,
    ) -> R<(String, Method)> {
        let chain = self.class_chain(class, at)?;
        let hints = at
            .field("arguments")
            .map(|n| {
                n.named()
                    .map(|arg| {
                        if arg.kind == "cast_expression" {
                            return arg.field("type").map(|n| n.text().to_string());
                        }
                        if arg.kind == "identifier" {
                            for scope in self.local_types.iter().rev() {
                                if let Some(t) = scope.get(&arg.text().to_ascii_lowercase()) {
                                    return Some(t.clone());
                                }
                            }
                        }
                        None
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut candidates = vec![];
        let mut signatures = BTreeSet::new();
        for owner in chain {
            let c = &self.program.classes[&owner];
            for m in &c.methods {
                if !m.name.eq_ignore_ascii_case(name)
                    || m.params.len() != args.len()
                    || m.static_method != static_method
                {
                    continue;
                }
                let signature = m
                    .params
                    .iter()
                    .map(|(_, t)| t.to_ascii_lowercase())
                    .collect::<Vec<_>>();
                if !signatures.insert(signature) {
                    continue;
                }
                let score = m
                    .params
                    .iter()
                    .zip(args)
                    .enumerate()
                    .map(|(i, ((_, t), v))| {
                        self.argument_score(
                            v,
                            &self
                                .resolve_class_from(t, &owner)
                                .unwrap_or_else(|| t.clone()),
                            hints.get(i).and_then(Option::as_deref),
                        )
                    })
                    .collect::<Option<Vec<_>>>();
                if let Some(scores) = score {
                    candidates.push((scores.iter().sum::<u32>(), owner.clone(), m.clone()));
                }
            }
        }
        candidates.sort_by_key(|x| x.0);
        if candidates.is_empty() {
            return block(
                at,
                format!(
                    "Method {class}.{name}/{} is missing or has no compatible signature",
                    args.len()
                ),
            );
        }
        if candidates.len() > 1 && candidates[0].0 == candidates[1].0 {
            return block(
                at,
                format!("Method {class}.{name} requires ambiguous overload resolution"),
            );
        }
        let (_, owner, m) = candidates.remove(0);
        Ok((owner, m))
    }
    pub(super) fn check_method_access(&self, owner: &str, m: &Method, at: &Node) -> R<()> {
        let modifiers = m
            .declaration
            .child("modifiers")
            .map(|n| n.text().to_ascii_lowercase())
            .unwrap_or_default();
        if !self.visible(owner, &modifiers) {
            return block(
                at,
                "Non-public cross-class method access requires qualified visibility rules",
            );
        }
        Ok(())
    }
    pub(super) fn invoke_method(
        &mut self,
        class: &str,
        m: &Method,
        args: Vec<Value>,
        receiver: Option<Rc<RefCell<Instance>>>,
    ) -> R<Value> {
        if let Some(value) = self.async_enqueue_future(class, m, &args, &m.declaration)? {
            return Ok(value);
        }
        if std::env::var_os("APX_TRACE_CALLS").is_some() {
            eprintln!("APX depth {}: {class}.{}", self.depth, m.name);
        }
        if self.depth >= 64 {
            return block(&m.declaration, "Host call-depth limit exceeded");
        }
        let body = m.body.as_ref().ok_or_else(|| EvalError {
            exception: None,
            status: "blocked",
            diagnostic: m.declaration.diagnostic(
                "APX_UNSUPPORTED",
                "Abstract/interface methods cannot execute",
            ),
        })?;
        let newsharing = self.class_sharing(class);
        let oldsharing = std::mem::replace(&mut self.sharing_context, newsharing);
        let old = std::mem::replace(&mut self.current_class, class.to_ascii_lowercase());
        let oldthis = std::mem::replace(&mut self.this, receiver);
        let oldlocals = std::mem::replace(&mut self.locals, vec![BTreeMap::new()]);
        let oldtypes = std::mem::replace(&mut self.local_types, vec![BTreeMap::new()]);
        self.depth += 1;
        let result = (|| {
            for ((name, typ), value) in m.params.iter().zip(args) {
                let value = self.coerce(value, typ, &m.declaration)?;
                self.locals[0].insert(name.clone(), value);
                self.local_types[0].insert(name.clone(), typ.clone());
            }
            match self.exec(body)? {
                Flow::Return(v) => self.coerce(v, &m.return_type, &m.declaration),
                Flow::Next if m.return_type.eq_ignore_ascii_case("void") => Ok(Value::Null),
                Flow::Next => fail(&m.declaration, "Non-void method completed without return"),
                _ => fail(&m.declaration, "Loop control escaped method"),
            }
        })();
        if let Err(EvalError {
            exception: Some(Value::Exception(exception)),
            ..
        }) = &result
        {
            exception.borrow_mut().stack.push_str(&format!(
                "\nClass.{class}.{}: line {}",
                m.name,
                m.declaration.line()
            ));
        }
        self.depth -= 1;
        self.locals = oldlocals;
        self.local_types = oldtypes;
        self.current_class = old;
        self.this = oldthis;
        self.sharing_context = oldsharing;
        result
    }
    pub(super) fn call_instance(
        &mut self,
        instance: Rc<RefCell<Instance>>,
        name: &str,
        args: Vec<Value>,
        at: &Node,
    ) -> R<Value> {
        self.tick(at)?;
        if let Some(value) = self.stub_call(instance.clone(), name, &args, at)? {
            return Ok(value);
        }
        let class = instance.borrow().class.clone();
        let (owner, method) = self.find_method(&class, name, &args, false, at)?;
        self.check_method_access(&owner, &method, at)?;
        self.invoke_method(&owner, &method, args, Some(instance))
    }
    pub(super) fn construct_instance(
        &mut self,
        class: &str,
        args: Vec<Value>,
        at: &Node,
    ) -> R<Value> {
        if self.depth >= 64 {
            return block(at, "Host construction-depth limit exceeded");
        }
        self.initialize(class, at)?;
        let c = self.program.classes[class].clone();
        if c.kind != "class_declaration" {
            return fail(at, "Only classes can be constructed");
        }
        let instance = Rc::new(RefCell::new(Instance {
            class: c.name.clone(),
            fields: BTreeMap::new(),
            field_types: BTreeMap::new(),
        }));
        let mut chain = self.class_chain(class, at)?;
        chain.reverse();
        for owner in &chain {
            let c = self.program.classes[owner].clone();
            for field in &c.instance_fields {
                let typ = self.qualified_type(self.need(field, "type")?.text(), owner);
                for d in field.fields("declarator") {
                    let key = self.need(d, "name")?.text().to_ascii_lowercase();
                    if instance.borrow().fields.contains_key(&key) {
                        return block(d, "Inherited field shadowing needs qualified storage slots");
                    }
                    instance
                        .borrow_mut()
                        .fields
                        .insert(key.clone(), Value::Null);
                    instance.borrow_mut().field_types.insert(key, typ.clone());
                }
            }
        }
        self.constructor_phase(
            instance.clone(),
            class,
            args,
            at,
            &mut BTreeSet::new(),
            &mut BTreeSet::new(),
        )?;
        Ok(Value::Instance(instance))
    }
    fn constructor_phase(
        &mut self,
        instance: Rc<RefCell<Instance>>,
        class: &str,
        args: Vec<Value>,
        at: &Node,
        initialized: &mut BTreeSet<String>,
        active: &mut BTreeSet<String>,
    ) -> R<()> {
        if self.depth >= 64 {
            return block(at, "Host construction-depth limit exceeded");
        }
        let c = self.program.classes[class].clone();
        let selected = if c.constructors.is_empty() {
            if !args.is_empty() {
                return fail(at, "Default constructor takes no arguments");
            }
            None
        } else {
            let hints = at
                .field("arguments")
                .map(|n| {
                    n.named()
                        .map(|n| n.field("type").map(|t| t.text()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let mut candidates = c
                .constructors
                .iter()
                .filter(|m| m.params.len() == args.len())
                .filter_map(|m| {
                    m.params
                        .iter()
                        .zip(&args)
                        .enumerate()
                        .map(|(i, ((_, t), v))| {
                            self.argument_score(
                                v,
                                &self
                                    .resolve_class_from(t, class)
                                    .unwrap_or_else(|| t.clone()),
                                hints.get(i).copied().flatten(),
                            )
                        })
                        .collect::<Option<Vec<_>>>()
                        .map(|s| (s.iter().sum::<u32>(), m.clone()))
                })
                .collect::<Vec<_>>();
            candidates.sort_by_key(|x| x.0);
            if candidates.is_empty() || candidates.len() > 1 && candidates[0].0 == candidates[1].0 {
                return block(at, "Constructor overload cannot be resolved");
            }
            let selected = candidates.remove(0).1;
            self.check_method_access(class, &selected, at)?;
            Some(selected)
        };
        let signature = format!(
            "{class}({})",
            selected
                .as_ref()
                .map(|m| m
                    .params
                    .iter()
                    .map(|(_, t)| t.clone())
                    .collect::<Vec<_>>()
                    .join(","))
                .unwrap_or_default()
        );
        if !active.insert(signature.clone()) {
            return block(at, "Recursive constructor delegation");
        }
        let newsharing = self.class_sharing(class);
        let oldsharing = std::mem::replace(&mut self.sharing_context, newsharing);
        let old = std::mem::replace(&mut self.current_class, class.into());
        let oldthis = std::mem::replace(&mut self.this, Some(instance.clone()));
        let oldlocals = std::mem::replace(&mut self.locals, vec![BTreeMap::new()]);
        let oldtypes = std::mem::replace(&mut self.local_types, vec![BTreeMap::new()]);
        self.depth += 1;
        let result = (|| {
            if let Some(m) = &selected {
                for ((name, typ), value) in m.params.iter().zip(args) {
                    let value = self.coerce(value, typ, &m.declaration)?;
                    self.locals[0].insert(name.clone(), value);
                    self.local_types[0].insert(name.clone(), typ.clone());
                }
            }
            let body = selected.as_ref().and_then(|m| m.body.as_ref());
            let delegation = body
                .and_then(|body| body.named().next())
                .filter(|n| n.kind == "explicit_constructor_invocation");
            let delegated_this = delegation
                .is_some_and(|n| n.field("constructor").is_some_and(|n| n.kind == "this"));
            let mut arguments = vec![];
            if let Some(delegation) = delegation {
                for arg in self.need(delegation, "arguments")?.named() {
                    arguments.push(self.eval(arg)?);
                }
            }
            if delegated_this {
                self.constructor_phase(
                    instance.clone(),
                    class,
                    arguments,
                    delegation.unwrap(),
                    initialized,
                    active,
                )?;
            } else {
                if let Some(parent) = c
                    .superclass
                    .as_ref()
                    .and_then(|p| self.resolve_class_from(p, class))
                {
                    self.constructor_phase(
                        instance.clone(),
                        &parent,
                        arguments,
                        delegation.unwrap_or(at),
                        initialized,
                        active,
                    )?;
                } else if delegation.is_some() {
                    return block(at, "Explicit super constructor has no user superclass");
                }
                if initialized.insert(class.into()) {
                    let mut steps = c
                        .instance_fields
                        .iter()
                        .chain(c.initializers.iter().filter(|n| n.kind == "block"))
                        .collect::<Vec<_>>();
                    steps.sort_by_key(|n| n.start);
                    for step in steps {
                        if step.kind == "field_declaration" {
                            let typ = self.need(step, "type")?.text();
                            for d in step.fields("declarator") {
                                if let Some(value) = d.field("value") {
                                    let value = self.eval(value)?;
                                    let value = self.coerce(value, typ, d)?;
                                    instance.borrow_mut().fields.insert(
                                        self.need(d, "name")?.text().to_ascii_lowercase(),
                                        value,
                                    );
                                }
                            }
                        } else {
                            self.exec(step)?;
                        }
                    }
                }
            }
            if let Some(body) = body {
                for statement in body
                    .named()
                    .filter(|n| n.kind != "explicit_constructor_invocation")
                {
                    match self.exec(statement)? {
                        Flow::Next => {}
                        Flow::Return(Value::Null) => break,
                        _ => return fail(statement, "Invalid constructor control flow"),
                    }
                }
            }
            Ok(())
        })();
        self.depth -= 1;
        self.current_class = old;
        self.this = oldthis;
        self.sharing_context = oldsharing;
        self.locals = oldlocals;
        self.local_types = oldtypes;
        active.remove(&signature);
        result
    }
    pub(super) fn resolve_class(&self, name: &str) -> Option<String> {
        self.resolve_class_from(name, &self.current_class)
    }
    pub(super) fn resolve_class_from(&self, name: &str, scope: &str) -> Option<String> {
        let name = name.to_ascii_lowercase();
        let mut scope = Some(scope.to_ascii_lowercase());
        while let Some(s) = scope {
            let nested = format!("{s}.{name}");
            if self.program.classes.contains_key(&nested) {
                return Some(nested);
            }
            scope = s.rsplit_once('.').map(|(p, _)| p.to_string());
        }
        self.program.classes.contains_key(&name).then_some(name)
    }
    pub(super) fn class_assignable(&self, actual: &str, expected: &str) -> bool {
        if cache::cache_assignable(actual, expected) {
            return true;
        }
        let expected = self.qualified_type(expected, &self.current_class);
        let mut pending = vec![actual.to_ascii_lowercase()];
        let mut seen = BTreeSet::new();
        while let Some(name) = pending.pop() {
            if name.trim_start_matches("system.") == expected.trim_start_matches("system.") {
                return true;
            }
            if !seen.insert(name.clone()) || seen.len() > 64 {
                continue;
            }
            if let Some(c) = self.program.classes.get(&name) {
                for parent in c.superclass.iter().chain(c.interfaces.iter()) {
                    if self
                        .qualified_type(parent, &name)
                        .trim_start_matches("system.")
                        == expected.trim_start_matches("system.")
                    {
                        return true;
                    }
                    if let Some(p) = self.resolve_class_from(parent, &name) {
                        pending.push(p);
                    }
                }
            }
        }
        false
    }
    pub(super) fn qualified_type(&self, typ: &str, scope: &str) -> String {
        let typ = typ.to_ascii_lowercase().replace(' ', "");
        if typ.len() > 4096 || typ.bytes().filter(|c| *c == b'<').count() > 64 {
            return typ;
        }
        if let Some((base, params)) = collections::generic(&typ) {
            return format!(
                "{}<{}>",
                base.trim_start_matches("system."),
                params
                    .iter()
                    .map(|p| self.qualified_type(p, scope))
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
        self.resolve_class_from(&typ, scope).unwrap_or(typ)
    }
    pub(super) fn implements_generic(&self, class: &str, kind: &str, at: &Node) -> R<bool> {
        for owner in self.class_chain(class, at)? {
            if self.program.classes[&owner].interfaces.iter().any(|i| {
                collections::generic(&i.to_ascii_lowercase().replace(' ', ""))
                    .is_some_and(|(k, _)| k.trim_start_matches("system.") == kind)
            }) {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub(super) fn exec_custom_iterable(
        &mut self,
        source: Rc<RefCell<Instance>>,
        n: &Node,
    ) -> R<Flow> {
        if !self.implements_generic(&source.borrow().class, "iterable", n)? {
            return fail(n, "For-each instance must implement Iterable<T>");
        }
        let iterator = self.call_instance(source, "iterator", vec![], n)?;
        let Value::Instance(iterator) = iterator else {
            return block(n, "Custom Iterable requires a user Iterator instance");
        };
        if !self.implements_generic(&iterator.borrow().class, "iterator", n)? {
            return fail(n, "Iterable.iterator() must return an Iterator<T>");
        }
        let key = self.need(n, "name")?.text().to_ascii_lowercase();
        let typ = self.need(n, "type")?.text().to_string();
        self.locals.push(BTreeMap::new());
        self.local_types
            .push(BTreeMap::from([(key.clone(), typ.clone())]));
        let result = (|| {
            loop {
                let has_next = self.call_instance(iterator.clone(), "hasnext", vec![], n)?;
                if !self.boolean(has_next, n)? {
                    break;
                }
                let item = self.call_instance(iterator.clone(), "next", vec![], n)?;
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
        result
    }
    pub(super) fn type_method(
        &mut self,
        receiver: &Value,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        match receiver {
            Value::Namespace(ns) if ns == "type" => match (name, args) {
                ("forname", [Value::Str(name)]) => Ok(Some(if self.known_type_literal(name) {
                    Value::Type(self.qualified_type(name, &self.current_class))
                } else {
                    Value::Null
                })),
                ("forname", [Value::Str(namespace), Value::Str(name)]) if namespace.is_empty() => {
                    Ok(Some(
                        self.resolve_class(name)
                            .map(Value::Type)
                            .unwrap_or(Value::Null),
                    ))
                }
                _ => Ok(None),
            },
            Value::Type(class) => match (name, args) {
                ("newinstance", []) => {
                    if let Some((kind, _)) = collections::generic(class) {
                        let v = match kind {
                            "list" => Value::list(vec![]),
                            "set" => Value::Set(Rc::new(RefCell::new(vec![]))),
                            "map" => Value::Map(Rc::new(RefCell::new(vec![]))),
                            _ => return block(at, "Type cannot be instantiated"),
                        };
                        return Ok(Some(self.coerce(v, class, at)?));
                    }
                    if self.program.schema.object(class).is_some() {
                        return Ok(Some(Value::Record(Rc::new(RefCell::new(Record {
                            typ: class.clone(),
                            fields: BTreeMap::new(),
                            projected: false,
                        })))));
                    }
                    Ok(Some(self.construct_instance(class, vec![], at)?))
                }
                ("getname", []) => Ok(Some(Value::Str(
                    self.program
                        .classes
                        .get(&class.to_ascii_lowercase())
                        .map(|c| c.name.clone())
                        .unwrap_or_else(|| class.clone()),
                ))),
                _ => Ok(None),
            },
            _ => Ok(None),
        }
    }
    pub(super) fn enum_method(
        &self,
        receiver: &Value,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        if let Value::Enum {
            typ,
            name: member,
            ordinal,
            ..
        } = receiver
        {
            return Ok(Some(match (name, args) {
                ("name" | "tostring", []) => Value::Str(member.clone()),
                ("ordinal", [])
                    if !self
                        .program
                        .classes
                        .get(&typ.to_ascii_lowercase())
                        .is_some_and(|c| !c.enum_members.is_empty()) =>
                {
                    return block(
                        at,
                        "Built-in enum ordinal requires qualified platform declaration order",
                    )
                }
                ("ordinal", []) => Value::Num(Number::integer(*ordinal as i64)),
                _ => return Ok(None),
            }));
        }
        if let Value::Namespace(typ) = receiver {
            if let Some(c) = self.program.classes.get(typ) {
                if !c.enum_members.is_empty() {
                    return Ok(Some(match (name, args) {
                        ("values", []) => {
                            let values = c
                                .enum_members
                                .iter()
                                .enumerate()
                                .map(|(ordinal, name)| Value::Enum {
                                    typ: typ.clone(),
                                    name: name.clone(),
                                    ordinal,
                                })
                                .collect();
                            self.coerce(Value::list(values), &format!("list<{typ}>"), at)?
                        }
                        ("valueof", [Value::Str(member)]) => {
                            let Some(ordinal) = c
                                .enum_members
                                .iter()
                                .position(|x| x.eq_ignore_ascii_case(member))
                            else {
                                return exceptions::raise(
                                    at,
                                    "System.TypeException",
                                    "Invalid enum constant",
                                );
                            };
                            Value::Enum {
                                typ: typ.clone(),
                                name: c.enum_members[ordinal].clone(),
                                ordinal,
                            }
                        }
                        _ => return Ok(None),
                    }));
                }
            }
        }
        Ok(None)
    }
}
