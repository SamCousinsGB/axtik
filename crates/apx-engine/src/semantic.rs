//! Side-effect-free declaration binding and static checks for the preview language.
//!
//! Every source body is traversed, including unreachable branches and methods
//! which are not selected for execution. Native APIs without a modeled return
//! signature remain unknown here; that is not evidence of full Apex compilation.
use crate::{
    model::Diagnostic,
    syntax::{Class, Method, Node, Program},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default)]
struct Expr {
    typ: Option<String>,
    namespace: bool,
    query: bool,
}
impl Expr {
    fn value(typ: impl Into<String>) -> Self {
        Self {
            typ: Some(typ.into()),
            ..Self::default()
        }
    }
    fn namespace(typ: impl Into<String>) -> Self {
        Self {
            typ: Some(typ.into()),
            namespace: true,
            query: false,
        }
    }
}

/// Return definite binding/type errors. No Apex statements or native calls run.
pub fn check(program: &Program) -> Vec<Diagnostic> {
    let mut checker = Checker {
        program,
        diagnostics: vec![],
        scope: String::new(),
        locals: vec![],
        return_type: "void".into(),
        static_context: false,
        nodes: 0,
        exhausted: false,
    };
    for (name, class) in &program.classes {
        checker.scope = name.clone();
        checker.check_class(class);
        if checker.exhausted {
            break;
        }
    }
    for trigger in &program.triggers {
        if checker.exhausted {
            break;
        }
        checker.scope.clear();
        checker.locals = vec![BTreeMap::new()];
        checker.return_type = "void".into();
        checker.statement(&trigger.body);
    }
    checker.diagnostics
}

struct Checker<'a> {
    program: &'a Program,
    diagnostics: Vec<Diagnostic>,
    scope: String,
    locals: Vec<BTreeMap<String, String>>,
    return_type: String,
    static_context: bool,
    nodes: usize,
    exhausted: bool,
}
fn normalized(typ: &str) -> String {
    let value = typ
        .to_ascii_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    let value = value.strip_prefix("system.").unwrap_or(&value);
    if let Some(element) = value.strip_suffix("[]") {
        format!("list<{}>", normalized(element))
    } else {
        value.into()
    }
}
fn generic(typ: &str) -> Option<(&str, Vec<String>)> {
    let (base, rest) = typ.split_once('<')?;
    let body = rest.strip_suffix('>')?;
    let mut depth = 0;
    let mut start = 0;
    let mut values = vec![];
    for (index, c) in body.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                values.push(body[start..index].to_string());
                start = index + 1;
            }
            _ => {}
        }
    }
    values.push(body[start..].to_string());
    Some((base, values))
}
fn modifiers(node: &Node) -> String {
    node.child("modifiers")
        .map(|n| n.text().to_ascii_lowercase())
        .unwrap_or_default()
}
fn numeric(typ: &str) -> bool {
    matches!(typ, "integer" | "long" | "decimal" | "double")
}
// Known platform record types can bind before their runtime metadata is
// implemented. Recognition does not install a permissive runtime schema.
fn opaque_record(typ: &str) -> bool {
    matches!(typ, "setupentityaccess")
}
fn native_type(typ: &str) -> bool {
    matches!(
        typ,
        "void"
            | "integer"
            | "long"
            | "decimal"
            | "double"
            | "boolean"
            | "string"
            | "id"
            | "object"
            | "sobject"
            | "blob"
            | "date"
            | "datetime"
            | "time"
            | "type"
            | "aggregateresult"
            | "exception"
            | "dmlexception"
            | "queryexception"
            | "sobjectexception"
            | "stringexception"
            | "listexception"
            | "mathexception"
            | "nullpointerexception"
            | "typeexception"
            | "securityexception"
            | "illegalargumentexception"
            | "calloutexception"
            | "jsonexception"
            | "finalexception"
            | "unsupportedoperationexception"
            | "aurahandledexception"
            | "limitexception"
            | "noaccessexception"
            | "dataweavescriptexception"
            | "http"
            | "httprequest"
            | "httpresponse"
            | "restrequest"
            | "restresponse"
            | "httpcalloutmock"
            | "webservicemock"
            | "stubprovider"
            | "request"
            | "quiddity"
            | "logginglevel"
            | "triggeroperation"
            | "accesslevel"
            | "accesstype"
            | "sobjectaccessdecision"
            | "savepoint"
            | "queueable"
            | "queueablecontext"
            | "schedulable"
            | "schedulablecontext"
            | "pattern"
            | "matcher"
            | "comparable"
            | "database.batchablecontext"
            | "database.querylocator"
            | "database.querylocatoriterator"
            | "database.stateful"
            | "database.allowscallouts"
            | "database.saveresult"
            | "database.upsertresult"
            | "database.deleteresult"
            | "database.undeleteresult"
            | "database.error"
            | "database.savepoint"
            | "database.dmloptions"
            | "schema.sobjecttype"
            | "schema.sobjectfield"
            | "schema.describesobjectresult"
            | "schema.describefieldresult"
            | "schema.sobjectdescribeoptions"
            | "schema.picklistentry"
            | "schema.childrelationship"
            | "schema.recordtypeinfo"
            | "schema.fieldset"
            | "schema.fieldsetmember"
            | "sobjecttype"
            | "sobjectfield"
            | "describesobjectresult"
            | "describefieldresult"
            | "cache.partition"
            | "cache.orgpartition"
            | "cache.sessionpartition"
            | "cache.cachebuilder"
            | "cache.visibility"
            | "cache.invalidparamexception"
            | "dataweave.script"
            | "dataweave.result"
            | "dataweave.dataweavescriptexception"
            | "eventbus.eventpublishsuccesscallback"
            | "eventbus.eventpublishfailurecallback"
            | "eventbus.successresult"
            | "eventbus.failureresult"
            | "messaging.inboundemailhandler"
            | "messaging.inboundemail"
            | "messaging.inboundenvelope"
            | "messaging.inboundemailresult"
            | "messaging.inboundemail.binaryattachment"
            | "messaging.inboundemail.textattachment"
            | "messaging.inboundemail.header"
            | "connectapi.followingpage"
            | "connectapi.namedcredentialinput"
            | "connectapi.namedcredential"
            | "connectapi.namedcredentialcalloutoptionsinput"
            | "connectapi.namedcredentialcalloutoptions"
            | "connectapi.externalcredentialinput"
            | "connectapi.externalcredential"
            | "connectapi.externalcredentialprincipalinput"
            | "connectapi.externalcredentialprincipal"
            | "connectapi.namedcredentialtype"
            | "connectapi.credentialauthenticationprotocol"
            | "connectapi.credentialprincipaltype"
    )
}
fn namespace(typ: &str) -> bool {
    matches!(
        typ,
        "system"
            | "assert"
            | "test"
            | "limits"
            | "database"
            | "schema"
            | "security"
            | "userinfo"
            | "crypto"
            | "encodingutil"
            | "json"
            | "math"
            | "restcontext"
            | "trigger"
            | "cache"
            | "cache.org"
            | "cache.session"
            | "eventbus"
            | "search"
            | "connectapi"
            | "connectapi.chatterusers"
            | "connectapi.namedcredentials"
            | "applauncher"
            | "applauncher.appmenu"
            | "dataweave"
            | "dataweavescriptresource"
            | "messaging"
    )
}

impl Checker<'_> {
    fn error(&mut self, at: &Node, code: &str, message: impl Into<String>) {
        if self.diagnostics.len() < 100 {
            self.diagnostics.push(at.diagnostic(code, message));
        }
    }
    fn tick(&mut self, at: &Node) -> bool {
        if self.exhausted {
            return false;
        }
        self.nodes += 1;
        if self.nodes > 2_000_000 {
            self.error(
                at,
                "APX_SEMANTIC_BUDGET",
                "Static checking exceeds the bounded project node budget",
            );
            self.exhausted = true;
        }
        !self.exhausted
    }
    fn resolve(&self, typ: &str, scope: &str) -> Option<String> {
        let key = normalized(typ);
        let mut current = Some(scope.to_ascii_lowercase());
        while let Some(owner) = current {
            let nested = format!("{owner}.{key}");
            if self.program.classes.contains_key(&nested) {
                return Some(nested);
            }
            current = owner.rsplit_once('.').map(|(parent, _)| parent.to_string());
        }
        self.program.classes.contains_key(&key).then_some(key)
    }
    fn qualify(&self, typ: &str, scope: &str) -> String {
        let typ = normalized(typ);
        if let Some((base, args)) = generic(&typ) {
            return format!(
                "{}<{}>",
                base,
                args.iter()
                    .map(|p| self.qualify(p, scope))
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
        self.resolve(&typ, scope).unwrap_or(typ)
    }
    fn known(&self, typ: &str) -> bool {
        if typ.len() > 4096 || typ.bytes().filter(|b| *b == b'<').count() > 64 {
            return false;
        }
        if let Some((base, args)) = generic(typ) {
            let arity = if base == "map" { 2 } else { 1 };
            return matches!(
                base,
                "list"
                    | "set"
                    | "map"
                    | "iterable"
                    | "iterator"
                    | "comparator"
                    | "database.batchable"
            ) && args.len() == arity
                && args.iter().all(|t| self.known(t));
        }
        native_type(typ)
            || opaque_record(typ)
            || self.program.classes.contains_key(typ)
            || self.program.schema.object(typ).is_some()
            || typ
                .strip_prefix("dataweavescriptresource.")
                .is_some_and(|s| {
                    self.program
                        .dataweave_scripts
                        .keys()
                        .any(|name| name.eq_ignore_ascii_case(s))
                })
    }
    fn declared(&mut self, typ: &str, at: &Node) -> String {
        let ty = self.qualify(typ, &self.scope);
        if !self.known(&ty) {
            self.error(
                at,
                "APX_BIND_TYPE",
                format!("Unknown or unsupported type {typ}"),
            );
        }
        ty
    }
    fn chain(&self, typ: &str) -> Vec<String> {
        let mut chain = vec![];
        let mut next = Some(typ.to_string());
        while let Some(owner) = next {
            if chain.contains(&owner) || chain.len() >= 64 {
                break;
            }
            let Some(class) = self.program.classes.get(&owner) else {
                break;
            };
            chain.push(owner.clone());
            next = class
                .superclass
                .as_ref()
                .and_then(|name| self.resolve(name, &owner));
        }
        chain
    }
    fn class_assignable(&self, actual: &str, expected: &str) -> bool {
        let mut pending = vec![actual.to_string()];
        let mut seen = BTreeSet::new();
        while let Some(typ) = pending.pop() {
            if typ == expected {
                return true;
            }
            if !seen.insert(typ.clone()) || seen.len() > 64 {
                continue;
            }
            if let Some(class) = self.program.classes.get(&typ) {
                for parent in class.superclass.iter().chain(&class.interfaces) {
                    pending.push(self.qualify(parent, &typ));
                }
            }
        }
        false
    }
    fn assignable(&self, expected: &str, actual: &str) -> bool {
        if actual == "null" || expected == actual || expected == "object" {
            return true;
        }
        if expected == "exception" && native_type(actual) && actual.ends_with("exception") {
            return true;
        }
        if expected == "dataweave.script"
            && actual.starts_with("dataweavescriptresource.")
            && self.known(actual)
        {
            return true;
        }
        if numeric(expected) && numeric(actual) {
            return match expected {
                "integer" => actual == "integer",
                "long" => matches!(actual, "integer" | "long"),
                _ => true,
            };
        }
        if matches!((expected, actual), ("string", "id") | ("id", "string")) {
            return true;
        }
        if expected == "sobject" && self.program.schema.object(actual).is_some() {
            return true;
        }
        if let (Some((eb, ea)), Some((ab, aa))) = (generic(expected), generic(actual)) {
            if (eb == ab || eb == "iterable" && matches!(ab, "list" | "set"))
                && ea.len() == aa.len()
            {
                // Apex permits typed SObject list conversion; the runtime checks
                // that individual rows have the destination record type.
                if eb == "list"
                    && ab == "list"
                    && aa[0] == "sobject"
                    && self.program.schema.object(&ea[0]).is_some()
                {
                    return true;
                }
                return ea.iter().zip(&aa).all(|(e, a)| self.assignable(e, a));
            }
        }
        self.class_assignable(actual, expected)
    }
    fn expect(&mut self, expected: &str, actual: &Expr, at: &Node) {
        let Some(actual_type) = actual.typ.as_deref() else {
            return;
        };
        if !self.compatible(expected, actual) {
            self.error(
                at,
                "APX_TYPE_MISMATCH",
                format!("Cannot assign {actual_type} to {expected}"),
            );
        }
    }
    fn compatible(&self, expected: &str, actual: &Expr) -> bool {
        !actual.namespace
            && actual.typ.as_deref().is_none_or(|typ| {
                self.assignable(expected, typ)
                    || actual.query && self.assignable(expected, &format!("list<{typ}>"))
            })
    }
    fn visible(&self, owner: &str, node: &Node) -> bool {
        if self.scope.is_empty() || self.scope.split('.').next() == owner.split('.').next() {
            return true;
        }
        let mods = modifiers(node);
        if mods
            .split_whitespace()
            .any(|m| matches!(m, "public" | "global"))
        {
            return true;
        }
        if mods.contains("@testvisible")
            && self
                .program
                .classes
                .get(self.scope.split('.').next().unwrap_or(""))
                .is_some_and(|c| modifiers(&c.declaration).contains("@istest"))
        {
            return true;
        }
        mods.split_whitespace().any(|m| m == "protected")
            && self.class_assignable(&self.scope, owner)
    }
    fn check_class(&mut self, class: &Class) {
        for parent in class.superclass.iter().chain(&class.interfaces) {
            self.declared(parent, &class.declaration);
        }
        for field in class.fields.iter().chain(&class.instance_fields) {
            self.locals = vec![BTreeMap::new()];
            self.static_context = class.fields.iter().any(|f| f.start == field.start);
            self.declaration(field);
            if let Some(accessors) = field.child("accessor_list") {
                let typ = field
                    .field("type")
                    .map(|t| self.qualify(t.text(), &self.scope))
                    .unwrap_or_default();
                for accessor in accessors.named() {
                    if accessor.kind != "accessor_declaration" {
                        continue;
                    }
                    self.locals = vec![BTreeMap::from([("value".into(), typ.clone())])];
                    self.return_type = if accessor
                        .field("accessor")
                        .is_some_and(|n| n.text().eq_ignore_ascii_case("get"))
                    {
                        typ.clone()
                    } else {
                        "void".into()
                    };
                    if let Some(body) = accessor.field("body") {
                        self.statement(body);
                    }
                }
            }
        }
        for initializer in &class.initializers {
            self.locals = vec![BTreeMap::new()];
            self.return_type = "void".into();
            self.static_context = initializer.kind == "static_initializer";
            self.statement(initializer);
        }
        for method in class.methods.iter().chain(&class.constructors) {
            self.check_method(method);
        }
    }
    fn check_method(&mut self, method: &Method) {
        self.return_type = self.declared(&method.return_type, &method.declaration);
        self.static_context = method.static_method;
        self.locals = vec![BTreeMap::new()];
        for (name, typ) in &method.params {
            let typ = self.declared(typ, &method.declaration);
            self.locals
                .last_mut()
                .unwrap()
                .insert(name.to_ascii_lowercase(), typ);
        }
        if let Some(body) = &method.body {
            self.statement(body);
        }
    }
    fn declaration(&mut self, node: &Node) {
        let Some(t) = node.field("type") else {
            return;
        };
        let typ = self.declared(t.text(), t);
        if typ == "void" {
            self.error(t, "APX_TYPE_MISMATCH", "A variable cannot have type void");
        }
        for declarator in node.fields("declarator") {
            if let Some(value) = declarator.field("value") {
                let v = self.expression(value);
                self.expect(&typ, &v, value);
            }
            if let Some(name) = declarator.field("name") {
                self.locals
                    .last_mut()
                    .unwrap()
                    .insert(name.text().to_ascii_lowercase(), typ.clone());
            }
        }
    }
    fn statement(&mut self, node: &Node) {
        if !self.tick(node) {
            return;
        }
        match node.kind.as_str() {
            "block" | "constructor_body" | "trigger_body" => {
                self.locals.push(BTreeMap::new());
                for c in node.named() {
                    self.statement(c);
                }
                self.locals.pop();
            }
            "local_variable_declaration" => self.declaration(node),
            "expression_statement" => {
                for c in node.named() {
                    self.expression(c);
                }
            }
            "return_statement" => {
                if let Some(value) = node.named().next() {
                    let v = self.expression(value);
                    let expected = self.return_type.clone();
                    if expected == "void" {
                        self.error(
                            value,
                            "APX_TYPE_MISMATCH",
                            "A void method cannot return a value",
                        );
                    } else {
                        self.expect(&expected, &v, value);
                    }
                } else if self.return_type != "void" {
                    self.error(
                        node,
                        "APX_RETURN_TYPE",
                        "Non-void method must return a value",
                    );
                }
            }
            "throw_statement" => {
                for value in node.named() {
                    let v = self.expression(value);
                    self.expect("exception", &v, value);
                }
            }
            "if_statement" | "while_statement" | "do_statement" => {
                if let Some(c) = node.field("condition") {
                    let v = self.expression(c);
                    self.expect("boolean", &v, c);
                }
                for (_, c) in &node.children {
                    if c.named && node.field("condition").is_none_or(|n| n.start != c.start) {
                        self.statement(c);
                    }
                }
            }
            "for_statement" => {
                self.locals.push(BTreeMap::new());
                for c in node.fields("init") {
                    self.statement(c);
                }
                if let Some(c) = node.field("condition") {
                    let v = self.expression(c);
                    self.expect("boolean", &v, c);
                }
                for c in node.fields("update") {
                    self.expression(c);
                }
                if let Some(body) = node.field("body") {
                    self.statement(body);
                }
                self.locals.pop();
            }
            "enhanced_for_statement" => {
                self.locals.push(BTreeMap::new());
                let typ = node.field("type").map(|t| self.declared(t.text(), t));
                if let Some(value) = node.field("value") {
                    let v = self.expression(value);
                    if let (Some(t), Some(actual)) = (&typ, &v.typ) {
                        if v.query {
                            if !self.assignable(t, actual)
                                && !self.assignable(t, &format!("list<{actual}>"))
                            {
                                self.error(
                                    value,
                                    "APX_TYPE_MISMATCH",
                                    format!("Cannot iterate {actual} as {t}"),
                                );
                            }
                        } else if let Some((_, args)) = generic(actual) {
                            if let Some(item) = args.first() {
                                self.expect(t, &Expr::value(item.clone()), value);
                            }
                        }
                    }
                }
                if let (Some(name), Some(typ)) = (node.field("name"), typ) {
                    self.locals
                        .last_mut()
                        .unwrap()
                        .insert(name.text().to_ascii_lowercase(), typ);
                }
                if let Some(body) = node.field("body") {
                    self.statement(body);
                }
                self.locals.pop();
            }
            "catch_clause" => {
                self.locals.push(BTreeMap::new());
                if let Some(p) = node.child("formal_parameter") {
                    if let (Some(t), Some(name)) = (p.field("type"), p.field("name")) {
                        let t = self.declared(t.text(), t);
                        self.locals
                            .last_mut()
                            .unwrap()
                            .insert(name.text().to_ascii_lowercase(), t);
                    }
                }
                if let Some(body) = node.field("body") {
                    self.statement(body);
                }
                self.locals.pop();
            }
            "switch_expression" => {
                if let Some(c) = node.field("condition") {
                    self.expression(c);
                }
                if let Some(body) = node.field("body") {
                    for rule in body.named() {
                        self.locals.push(BTreeMap::new());
                        if let Some(label) = rule.child("switch_label") {
                            for candidate in label.named() {
                                if candidate.kind == "when_sobject_type" {
                                    let parts = candidate.named().collect::<Vec<_>>();
                                    if parts.len() == 2 {
                                        let t = self.declared(parts[0].text(), parts[0]);
                                        self.locals
                                            .last_mut()
                                            .unwrap()
                                            .insert(parts[1].text().to_ascii_lowercase(), t);
                                    }
                                }
                            }
                        }
                        if let Some(body) = rule.child("block") {
                            self.statement(body);
                        }
                        self.locals.pop();
                    }
                }
            }
            "break_statement" | "continue_statement" | "empty_statement" | "line_comment"
            | "block_comment" => {}
            _ => {
                if is_expression(&node.kind) {
                    self.expression(node);
                } else {
                    for c in node.named() {
                        self.statement(c);
                    }
                }
            }
        }
    }
    fn field_lookup(
        &self,
        typ: &str,
        name: &str,
        static_only: bool,
    ) -> Option<(String, String, Node)> {
        for owner in self.chain(typ) {
            let class = &self.program.classes[&owner];
            for field in class.fields.iter().chain(if static_only {
                [].iter()
            } else {
                class.instance_fields.iter()
            }) {
                if field.fields("declarator").any(|d| {
                    d.field("name")
                        .is_some_and(|n| n.text().eq_ignore_ascii_case(name))
                }) {
                    let typ = field
                        .field("type")
                        .map(|n| self.qualify(n.text(), &owner))?;
                    return Some((owner, typ, field.clone()));
                }
            }
        }
        None
    }
    fn identifier(&mut self, node: &Node) -> Expr {
        let key = node.text().to_ascii_lowercase();
        for locals in self.locals.iter().rev() {
            if let Some(typ) = locals.get(&key) {
                return Expr::value(typ.clone());
            }
        }
        let mut owner = Some(self.scope.clone());
        while let Some(scope) = owner {
            if let Some((owner, typ, field)) = self.field_lookup(&scope, &key, false) {
                if !self.visible(&owner, &field) {
                    self.error(
                        node,
                        "APX_MEMBER_ACCESS",
                        format!("Field {owner}.{key} is not accessible here"),
                    );
                }
                if self.static_context
                    && !modifiers(&field).split_whitespace().any(|m| m == "static")
                {
                    self.error(
                        node,
                        "APX_STATIC_CONTEXT",
                        format!("Instance field {owner}.{key} is unavailable in a static context"),
                    );
                }
                return Expr::value(typ);
            }
            owner = scope.rsplit_once('.').map(|(p, _)| p.to_string());
        }
        let typ = self.qualify(&key, &self.scope);
        if self.known(&typ) || namespace(&typ) {
            return Expr::namespace(typ);
        }
        // Trigger context is represented by its native namespace; enum labels are
        // handled by the switch visitor and never reach lexical name lookup.
        self.error(
            node,
            "APX_BIND_NAME",
            format!("Unknown variable or type {}", node.text()),
        );
        Expr::default()
    }
    fn member(&mut self, receiver: Expr, name: &str, at: &Node) -> Expr {
        let Some(typ) = receiver.typ.as_deref() else {
            return Expr::default();
        };
        let key = name.to_ascii_lowercase();
        if receiver.namespace {
            if key == "class" && self.known(typ) {
                return Expr::value("type");
            }
            let full = if typ == "system" {
                key.clone()
            } else {
                format!("{typ}.{key}")
            };
            if self.known(&full) || namespace(&full) {
                return Expr::namespace(full);
            }
            if self
                .program
                .classes
                .get(typ)
                .is_some_and(|c| c.enum_members.iter().any(|n| n.eq_ignore_ascii_case(&key)))
            {
                return Expr::value(typ);
            }
            if matches!(
                typ,
                "accesslevel"
                    | "accesstype"
                    | "logginglevel"
                    | "quiddity"
                    | "triggeroperation"
                    | "schema.sobjectdescribeoptions"
                    | "cache.visibility"
            ) || typ.starts_with("connectapi.")
            {
                return Expr::value(typ);
            }
            if self.program.schema.object(typ).is_some() {
                return Expr::value(if key == "sobjecttype" {
                    "schema.sobjecttype"
                } else {
                    "schema.sobjectfield"
                });
            }
        }
        if let Some((owner, field_type, field)) = self.field_lookup(typ, &key, receiver.namespace) {
            if !self.visible(&owner, &field) {
                self.error(
                    at,
                    "APX_MEMBER_ACCESS",
                    format!("Field {owner}.{name} is not accessible here"),
                );
            }
            return Expr::value(field_type);
        }
        if let Some(field) = self.program.schema.field(typ, &key) {
            return Expr::value(normalized(field.apex_type()));
        }
        if let Some(object) = self.program.schema.object(typ) {
            for field in object.fields.values() {
                if field
                    .relationship_name
                    .as_ref()
                    .is_some_and(|r| r.eq_ignore_ascii_case(&key))
                {
                    return if field.reference_to.len() == 1 {
                        Expr::value(normalized(&field.reference_to[0]))
                    } else {
                        Expr::value("sobject")
                    };
                }
            }
            for child in self.program.schema.objects.values() {
                for field in child.fields.values() {
                    if field
                        .reference_to
                        .iter()
                        .any(|r| r.eq_ignore_ascii_case(typ))
                        && field
                            .child_relationship_name
                            .as_ref()
                            .is_some_and(|r| r.eq_ignore_ascii_case(&key))
                    {
                        return Expr::value(format!("list<{}>", normalized(&child.name)));
                    }
                }
            }
            // The standard schema is intentionally partial. Reached unknown
            // fields still block in the runtime rather than gaining a type here.
            return Expr::default();
        }
        if self.program.classes.contains_key(typ) {
            self.error(at, "APX_BIND_MEMBER", format!("Unknown field {typ}.{name}"));
        }
        match (typ, key.as_str()) {
            ("restcontext", "request") => Expr::value("restrequest"),
            ("restcontext", "response") => Expr::value("restresponse"),
            ("trigger", "new" | "old") => Expr::value("list<sobject>"),
            ("trigger", "newmap" | "oldmap") => Expr::value("map<id,sobject>"),
            ("trigger", "operationtype") => Expr::value("triggeroperation"),
            ("trigger", _) => Expr::default(),
            ("schema.sobjecttype", "fields") => Expr::default(),
            (_, "class") => Expr::value("type"),
            _ => Expr::default(),
        }
    }
    fn expression(&mut self, node: &Node) -> Expr {
        if !self.tick(node) {
            return Expr::default();
        }
        match node.kind.as_str() {
            "identifier" => self.identifier(node),
            "string_literal" => Expr::value("string"),
            "boolean" => Expr::value("boolean"),
            "null_literal" => Expr::value("null"),
            "int" => Expr::value(if node.text().to_ascii_lowercase().ends_with('l') {
                "long"
            } else {
                "integer"
            }),
            "decimal_floating_point_literal" => {
                Expr::value(if node.text().to_ascii_lowercase().ends_with('d') {
                    "double"
                } else {
                    "decimal"
                })
            }
            "this" => {
                if self.static_context {
                    self.error(
                        node,
                        "APX_STATIC_CONTEXT",
                        "this is unavailable in a static context",
                    );
                }
                Expr::value(self.scope.clone())
            }
            "super" => self
                .program
                .classes
                .get(&self.scope)
                .and_then(|c| c.superclass.as_ref())
                .map(|t| Expr::value(self.qualify(t, &self.scope)))
                .unwrap_or_default(),
            "parenthesized_expression" => node
                .named()
                .next()
                .map(|c| self.expression(c))
                .unwrap_or_default(),
            "field_access" => {
                let name = node.field("field").map(|n| n.text()).unwrap_or("");
                if name.eq_ignore_ascii_case("class") {
                    if let Some(typ) = node.field("object") {
                        self.declared(typ.text(), typ);
                    }
                    Expr::value("type")
                } else {
                    let receiver = node
                        .field("object")
                        .map(|n| self.expression(n))
                        .unwrap_or_default();
                    self.member(receiver, name, node)
                }
            }
            "class_literal" => {
                if let Some(t) = node.named().next() {
                    self.declared(t.text(), t);
                }
                Expr::value("type")
            }
            "cast_expression" => {
                let t = node.field("type").map(|n| self.declared(n.text(), n));
                for c in node.named() {
                    if node.field("type").is_none_or(|t| t.start != c.start) {
                        self.expression(c);
                    }
                }
                t.map(Expr::value).unwrap_or_default()
            }
            "assignment_expression" => {
                let left = node
                    .field("left")
                    .map(|n| self.expression(n))
                    .unwrap_or_default();
                let right = node
                    .field("right")
                    .map(|n| self.expression(n))
                    .unwrap_or_default();
                if let Some(t) = &left.typ {
                    let op = node.field("operator").map(|n| n.text()).unwrap_or("=");
                    if op == "=" {
                        self.expect(t, &right, node);
                    } else if !(op == "+=" && t == "string") {
                        if let Some(r) = &right.typ {
                            if !numeric(t) || !numeric(r) {
                                self.error(
                                    node,
                                    "APX_TYPE_MISMATCH",
                                    "Compound assignment requires compatible operands",
                                );
                            }
                        }
                    }
                }
                left
            }
            "binary_expression" => self.binary(node),
            "unary_expression" | "update_expression" => {
                let value = node
                    .field("operand")
                    .or_else(|| node.named().last())
                    .map(|n| self.expression(n))
                    .unwrap_or_default();
                if node.text().trim_start().starts_with('!') {
                    self.expect("boolean", &value, node);
                    Expr::value("boolean")
                } else {
                    if let Some(t) = &value.typ {
                        if !numeric(t) {
                            self.error(
                                node,
                                "APX_TYPE_MISMATCH",
                                "Numeric unary operator requires a numeric operand",
                            );
                        }
                    }
                    value
                }
            }
            "ternary_expression" => {
                if let Some(c) = node.field("condition") {
                    let v = self.expression(c);
                    self.expect("boolean", &v, c);
                }
                let a = node
                    .field("consequence")
                    .map(|n| self.expression(n))
                    .unwrap_or_default();
                let b = node
                    .field("alternative")
                    .map(|n| self.expression(n))
                    .unwrap_or_default();
                if a.typ == b.typ {
                    a
                } else if a.typ.as_deref() == Some("null") {
                    b
                } else if b.typ.as_deref() == Some("null") {
                    a
                } else {
                    Expr::default()
                }
            }
            "instanceof_expression" => {
                for c in node.named() {
                    if matches!(
                        c.kind.as_str(),
                        "type_identifier"
                            | "generic_type"
                            | "array_type"
                            | "scoped_type_identifier"
                    ) {
                        self.declared(c.text(), c);
                    } else {
                        self.expression(c);
                    }
                }
                Expr::value("boolean")
            }
            "method_invocation" => self.call(node),
            "object_creation_expression"
            | "array_creation_expression"
            | "map_creation_expression" => self.construct(node),
            "array_access" => {
                let array = node
                    .field("array")
                    .or_else(|| node.named().next())
                    .map(|n| self.expression(n))
                    .unwrap_or_default();
                if let Some(index) = node.field("index") {
                    let v = self.expression(index);
                    self.expect("integer", &v, index);
                }
                array
                    .typ
                    .and_then(|t| generic(&t).and_then(|(_, args)| args.first().cloned()))
                    .map(Expr::value)
                    .unwrap_or_default()
            }
            "query_expression" => {
                // Bind variables are Apex expressions and must be checked even if
                // the query is unreachable. SOQL identifiers are a separate scope.
                fn binds<'a>(n: &'a Node, out: &mut Vec<&'a Node>) {
                    if n.kind == "bound_apex_expression" {
                        out.extend(n.named());
                    } else {
                        for c in n.named() {
                            binds(c, out);
                        }
                    }
                }
                let mut expressions = vec![];
                binds(node, &mut expressions);
                for e in expressions {
                    self.expression(e);
                }
                if let Ok(query) = crate::soql::parse(node.text()) {
                    if query.selections.len() == 1
                        && matches!(&query.selections[0],crate::soql::Selection::Aggregate{function,field:None,..}if function.eq_ignore_ascii_case("count"))
                    {
                        Expr::value("integer")
                    } else {
                        let aggregate = query
                            .selections
                            .iter()
                            .any(|s| matches!(s, crate::soql::Selection::Aggregate { .. }));
                        Expr {
                            typ: Some(if aggregate {
                                "aggregateresult".into()
                            } else {
                                normalized(&query.object)
                            }),
                            query: true,
                            namespace: false,
                        }
                    }
                } else {
                    Expr::default()
                }
            }
            "type_identifier" | "generic_type" | "array_type" | "scoped_type_identifier" => {
                Expr::namespace(self.declared(node.text(), node))
            }
            _ => {
                for c in node.named() {
                    if is_expression(&c.kind) {
                        self.expression(c);
                    } else {
                        self.statement(c);
                    }
                }
                Expr::default()
            }
        }
    }
    fn binary(&mut self, node: &Node) -> Expr {
        let left = node
            .field("left")
            .map(|n| self.expression(n))
            .unwrap_or_default();
        let right = node
            .field("right")
            .map(|n| self.expression(n))
            .unwrap_or_default();
        let op = node.field("operator").map(|n| n.text()).unwrap_or("");
        if matches!(op, "&&" | "||") {
            self.expect("boolean", &left, node);
            self.expect("boolean", &right, node);
            return Expr::value("boolean");
        }
        if matches!(op, "==" | "!=" | "===" | "!==" | "instanceof") {
            return Expr::value("boolean");
        }
        if matches!(op, "<" | ">" | "<=" | ">=") {
            return Expr::value("boolean");
        }
        if op == "+"
            && (left.typ.as_deref() == Some("string") || right.typ.as_deref() == Some("string"))
        {
            return Expr::value("string");
        }
        if let (Some(l), Some(r)) = (&left.typ, &right.typ) {
            if numeric(l) && numeric(r) {
                return Expr::value(if l == "double" || r == "double" {
                    "double"
                } else if l == "decimal" || r == "decimal" {
                    "decimal"
                } else if l == "long" || r == "long" {
                    "long"
                } else {
                    "integer"
                });
            }
            if l == "boolean" && r == "boolean" && matches!(op, "&" | "|" | "^") {
                return Expr::value("boolean");
            }
            if matches!(op, "+" | "-") && matches!(l.as_str(), "date" | "datetime") && numeric(r) {
                return Expr::value(l);
            }
            self.error(
                node,
                "APX_TYPE_MISMATCH",
                format!("Operator {op} cannot combine {l} and {r}"),
            );
        }
        Expr::default()
    }
    fn construct(&mut self, node: &Node) -> Expr {
        let Some(type_node) = node.field("type") else {
            return Expr::default();
        };
        let typ = self.declared(type_node.text(), type_node);
        if let Some(values) = node.field("value") {
            let args = generic(&typ).map(|(_, args)| args).unwrap_or_default();
            for value in values.named() {
                if value.kind == "map_key_initializer" {
                    for (index, part) in value.named().enumerate() {
                        let v = self.expression(part);
                        if let Some(expected) = args.get(index) {
                            self.expect(expected, &v, part);
                        }
                    }
                } else {
                    let v = self.expression(value);
                    if let Some(expected) = args.first() {
                        self.expect(expected, &v, value);
                    }
                }
            }
        }
        let args = node
            .field("arguments")
            .map(|a| a.named().collect::<Vec<_>>())
            .unwrap_or_default();
        if self.program.schema.object(&typ).is_some() || opaque_record(&typ) {
            for arg in args {
                if arg.kind == "assignment_expression" {
                    let name = arg.field("left").map(|n| n.text()).unwrap_or("");
                    if let Some(value) = arg.field("right") {
                        let v = self.expression(value);
                        let field = self.member(Expr::value(typ.clone()), name, arg);
                        if let Some(t) = field.typ {
                            self.expect(&t, &v, arg);
                        }
                    }
                } else {
                    self.expression(arg);
                }
            }
        } else {
            let values = args.iter().map(|n| self.expression(n)).collect::<Vec<_>>();
            if let Some(class) = self.program.classes.get(&typ) {
                if !class.constructors.is_empty() {
                    self.check_overload(
                        &typ,
                        "constructor",
                        &class.constructors,
                        &values,
                        node,
                        false,
                    );
                } else if !values.is_empty() && !self.class_assignable(&typ, "exception") {
                    self.error(
                        node,
                        "APX_BIND_METHOD",
                        format!("No constructor {typ} accepts {} argument(s)", values.len()),
                    );
                }
            }
        }
        Expr::value(typ)
    }
    fn check_overload(
        &mut self,
        owner: &str,
        name: &str,
        methods: &[Method],
        args: &[Expr],
        at: &Node,
        static_only: bool,
    ) -> Option<String> {
        let methods = methods
            .iter()
            .map(|m| (owner.to_string(), m.clone()))
            .collect::<Vec<_>>();
        self.check_candidates(name, &methods, args, at, static_only)
    }
    fn check_candidates(
        &mut self,
        name: &str,
        methods: &[(String, Method)],
        args: &[Expr],
        at: &Node,
        static_only: bool,
    ) -> Option<String> {
        let mut signatures = BTreeSet::new();
        let candidates = methods
            .iter()
            .filter(|(owner, m)| {
                m.params.len() == args.len()
                    && (!static_only || m.static_method)
                    && m.params.iter().zip(args).all(|((_, expected), actual)| {
                        self.compatible(&self.qualify(expected, owner), actual)
                    })
            })
            .filter(|(owner, m)| {
                signatures.insert(
                    m.params
                        .iter()
                        .map(|(_, t)| self.qualify(t, owner))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            self.error(
                at,
                "APX_BIND_METHOD",
                format!(
                    "No compatible {name} overload accepts ({})",
                    args.iter()
                        .map(|a| a.typ.as_deref().unwrap_or("unresolved"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
            return None;
        }
        let candidates = candidates
            .iter()
            .filter(|(owner, m)| {
                !candidates.iter().any(|(other_owner, other)| {
                    let own = m
                        .params
                        .iter()
                        .map(|(_, t)| self.qualify(t, owner))
                        .collect::<Vec<_>>();
                    let different = other
                        .params
                        .iter()
                        .map(|(_, t)| self.qualify(t, other_owner))
                        .collect::<Vec<_>>();
                    different
                        .iter()
                        .zip(&own)
                        .all(|(a, b)| self.assignable(b, a))
                        && different
                            .iter()
                            .zip(&own)
                            .any(|(a, b)| !self.assignable(a, b))
                })
            })
            .copied()
            .collect::<Vec<_>>();
        if candidates
            .iter()
            .all(|(owner, m)| !self.visible(owner, &m.declaration))
        {
            self.error(
                at,
                "APX_MEMBER_ACCESS",
                format!("Method {name} is not accessible here"),
            );
        }
        if candidates.len() > 1 && args.iter().all(|a| a.typ.is_some()) {
            self.error(
                at,
                "APX_BIND_METHOD",
                format!("Ambiguous overload for {name}"),
            );
        }
        let returns = candidates
            .iter()
            .map(|(owner, m)| self.qualify(&m.return_type, owner))
            .collect::<BTreeSet<_>>();
        (returns.len() == 1).then(|| returns.into_iter().next().unwrap())
    }
    fn check_native(&mut self, typ: &str, name: &str, args: &[Expr], at: &Node) {
        let closed = match typ {
            "integer" | "long" | "decimal" | "double" => Some(matches!(
                name,
                "valueof"
                    | "abs"
                    | "divide"
                    | "format"
                    | "intvalue"
                    | "longvalue"
                    | "doublevalue"
                    | "round"
                    | "scale"
                    | "precision"
                    | "setscale"
                    | "striptrailingzeros"
                    | "pow"
                    | "min"
                    | "max"
                    | "signum"
                    | "equals"
                    | "hashcode"
                    | "tostring"
            )),
            "assert" => Some(matches!(
                name,
                "areequal"
                    | "arenotequal"
                    | "istrue"
                    | "isfalse"
                    | "isnull"
                    | "isnotnull"
                    | "isinstanceoftype"
                    | "isnotinstanceoftype"
                    | "fail"
            )),
            "test" => Some(matches!(
                name,
                "starttest"
                    | "stoptest"
                    | "isrunningtest"
                    | "setmock"
                    | "createstub"
                    | "loaddata"
                    | "setfixedsearchresults"
                    | "getstandardpricebookid"
                    | "setcreateddate"
                    | "geteventbus"
                    | "getrunid"
                    | "getorgnamespace"
                    | "enablechangeddatacapture"
                    | "enablechangedatacapture"
                    | "setcurrentpage"
                    | "setcurrentpagereference"
                    | "getflexqueuorder"
                    | "enqueuebatchjobs"
                    | "getexternalservicemocks"
                    | "setcontinuationresponse"
                    | "invokecontinuationmethod"
                    | "calculatepermissionSetGroup"
                    | "calculatepermissionsetgroup"
            )),
            "system" => Some(matches!(
                name,
                "assert"
                    | "assertequals"
                    | "assertnotequals"
                    | "debug"
                    | "enqueuejob"
                    | "schedule"
                    | "schedulebatch"
                    | "abortjob"
                    | "runas"
                    | "isbatch"
                    | "isfuture"
                    | "isqueueable"
                    | "isscheduled"
                    | "attachfinalizer"
                    | "currenttimemillis"
                    | "requestversion"
                    | "now"
                    | "today"
                    | "resetpassword"
                    | "setpassword"
                    | "testinstall"
                    | "testupgrade"
                    | "testuninstall"
                    | "istest"
                    | "isrunningtest"
                    | "purgeoldasynjobs"
            )),
            _ => None,
        };
        if closed == Some(false) {
            self.error(
                at,
                "APX_BIND_METHOD",
                format!("Unknown method {typ}.{name}"),
            );
            return;
        }
        if let Some((base, params)) = generic(typ) {
            if !matches!(base, "list" | "set" | "map" | "iterator" | "iterable") {
                return;
            }
            let item = params.first().cloned().unwrap_or_default();
            let value = params.get(1).cloned().unwrap_or_default();
            let signatures: Vec<Vec<String>> = match (base, name) {
                (_, "equals") => vec![vec!["object".into()]],
                (_, "hashcode" | "tostring") => vec![vec![]],
                ("list" | "set" | "map", "size" | "isempty" | "clear" | "clone") => vec![vec![]],
                ("list", "get" | "remove") => vec![vec!["integer".into()]],
                ("list", "set") => vec![vec!["integer".into(), item.clone()]],
                ("list", "add") => vec![vec![item.clone()], vec!["integer".into(), item.clone()]],
                ("set", "add") => vec![vec![item.clone()]],
                ("list" | "set", "contains") | ("set", "remove") => vec![vec!["object".into()]],
                ("list" | "set", "addall") | ("set", "containsall" | "removeall" | "retainall") => {
                    vec![vec![format!("iterable<{item}>")]]
                }
                ("list", "sort") => vec![vec![], vec![format!("comparator<{item}>")]],
                ("list" | "map", "deepclone") => {
                    (0..=4).map(|n| vec!["boolean".into(); n]).collect()
                }
                ("map", "get" | "remove" | "containskey") => vec![vec![item.clone()]],
                ("map", "put") => vec![vec![item.clone(), value.clone()]],
                ("map", "putall") => vec![vec![typ.into()], vec![format!("list<{value}>")]],
                ("map", "keyset" | "values") => vec![vec![]],
                ("list" | "set" | "iterable", "iterator") | ("iterator", "hasnext" | "next") => {
                    vec![vec![]]
                }
                ("list" | "map", "getsobjecttype") => vec![vec![]],
                _ => {
                    self.error(
                        at,
                        "APX_BIND_METHOD",
                        format!("Unknown collection method {base}.{name}"),
                    );
                    return;
                }
            };
            if !signatures.iter().any(|s| {
                s.len() == args.len() && s.iter().zip(args).all(|(e, a)| self.compatible(e, a))
            }) {
                self.error(
                    at,
                    "APX_BIND_METHOD",
                    format!("No compatible {typ}.{name} overload for supplied arguments"),
                );
            }
        } else if typ == "string" {
            if !string_method(name) {
                self.error(
                    at,
                    "APX_BIND_METHOD",
                    format!("Unknown String method {name}"),
                );
                return;
            }
            let signature: Option<Vec<&str>> = match name {
                "length" | "trim" | "tolowercase" | "touppercase" | "hashcode" | "tostring"
                | "tochararray" => Some(vec![]),
                "isblank"
                | "isnotblank"
                | "isempty"
                | "isnotempty"
                | "equalsignorecase"
                | "contains"
                | "containsignorecase"
                | "startswith"
                | "startswithignorecase"
                | "endswith"
                | "endswithignorecase"
                | "matches"
                | "split" => Some(vec!["string"]),
                "equals" | "valueof" => Some(vec!["object"]),
                "charat" => Some(vec!["integer"]),
                "replace" | "replaceall" | "replacefirst" => Some(vec!["string", "string"]),
                "substring" => Some(if args.len() == 1 {
                    vec!["integer"]
                } else {
                    vec!["integer", "integer"]
                }),
                _ => None,
            };
            // split(regex, limit) is an additional platform overload.
            if name == "split" && args.len() == 2 {
                self.expect("string", &args[0], at);
                self.expect("integer", &args[1], at);
                return;
            }
            if let Some(signature) = signature {
                if signature.len() != args.len() {
                    self.error(
                        at,
                        "APX_BIND_METHOD",
                        format!("Invalid argument count for String.{name}"),
                    );
                } else {
                    for (e, a) in signature.iter().zip(args) {
                        self.expect(e, a, at);
                    }
                }
            }
        }
    }
    fn call(&mut self, node: &Node) -> Expr {
        let name = node
            .field("name")
            .map(|n| n.text().to_ascii_lowercase())
            .unwrap_or_default();
        let receiver = node.field("object").map(|n| self.expression(n));
        let args = node
            .field("arguments")
            .map(|a| a.named().map(|n| self.expression(n)).collect::<Vec<_>>())
            .unwrap_or_default();
        let mut owners = vec![];
        if let Some(receiver) = &receiver {
            if let Some(t) = &receiver.typ {
                owners.extend(self.chain(t));
            }
        } else {
            let mut scope = Some(self.scope.clone());
            while let Some(owner) = scope {
                owners.extend(self.chain(&owner));
                scope = owner.rsplit_once('.').map(|(p, _)| p.to_string());
            }
        }
        let methods = owners
            .iter()
            .flat_map(|owner| {
                self.program.classes[owner]
                    .methods
                    .iter()
                    .filter(|m| m.name.eq_ignore_ascii_case(&name))
                    .map(|m| (owner.clone(), m.clone()))
            })
            .collect::<Vec<_>>();
        if !methods.is_empty() {
            return self
                .check_candidates(
                    &name,
                    &methods,
                    &args,
                    node,
                    receiver
                        .as_ref()
                        .map(|r| r.namespace)
                        .unwrap_or(self.static_context),
                )
                .map(Expr::value)
                .unwrap_or_default();
        }
        let typ = match &receiver {
            Some(receiver) => match receiver.typ.as_deref() {
                Some(typ) => typ,
                None => return Expr::default(),
            },
            None => "system",
        };
        if self.program.classes.contains_key(typ) {
            if self.program.classes[typ].kind == "enum_declaration" {
                let result = match (name.as_str(), args.len()) {
                    ("name", 0) => Some("string".into()),
                    ("ordinal", 0) => Some("integer".into()),
                    ("values", 0) => Some(format!("list<{typ}>")),
                    ("valueof", 1) => Some(typ.into()),
                    _ => None,
                };
                if let Some(result) = result {
                    return Expr::value(result);
                }
            }
            if matches!(name.as_str(), "equals" | "hashcode" | "tostring" | "clone") {
                return native_return(typ, &name, &args);
            }
            if self.class_assignable(typ, "exception") {
                return native_return("exception", &name, &args);
            }
            self.error(
                node,
                "APX_BIND_METHOD",
                format!("Unknown method {typ}.{name}"),
            );
            return Expr::default();
        }
        if receiver.is_none()
            && !matches!(
                name.as_str(),
                "assert"
                    | "assertequals"
                    | "assertnotequals"
                    | "debug"
                    | "enqueuejob"
                    | "schedule"
                    | "runas"
            )
        {
            self.error(
                node,
                "APX_BIND_METHOD",
                format!("Unknown method {}.{name}", self.scope),
            );
        }
        self.check_native(typ, &name, &args, node);
        native_return(typ, &name, &args)
    }
}
fn is_expression(kind: &str) -> bool {
    kind.ends_with("_expression")
        || matches!(
            kind,
            "identifier"
                | "field_access"
                | "method_invocation"
                | "array_access"
                | "string_literal"
                | "boolean"
                | "int"
                | "null_literal"
                | "decimal_floating_point_literal"
                | "this"
                | "super"
                | "class_literal"
        )
}
fn string_method(name: &str) -> bool {
    matches!(
        name,
        "abbreviate"
            | "capitalize"
            | "center"
            | "charat"
            | "codepointat"
            | "codepointbefore"
            | "codepointcount"
            | "compareto"
            | "comparetoignorecase"
            | "contains"
            | "containsany"
            | "containsignorecase"
            | "containsonly"
            | "containsnone"
            | "containswhitespace"
            | "countmatches"
            | "deletewhitespace"
            | "difference"
            | "endswith"
            | "endswithignorecase"
            | "equals"
            | "equalsignorecase"
            | "escapeecmascript"
            | "escapehtml3"
            | "escapehtml4"
            | "escapejava"
            | "escapesinglequotes"
            | "escapexml"
            | "format"
            | "fromchararray"
            | "getcommonprefix"
            | "getlevenshteindistance"
            | "hashcode"
            | "indexof"
            | "indexofany"
            | "indexofanybut"
            | "indexofdifference"
            | "indexofignorecase"
            | "isalllowercase"
            | "isalluppercase"
            | "isalpha"
            | "isalphaspace"
            | "isalphanumeric"
            | "isalphanumericspace"
            | "isasciiprintable"
            | "isblank"
            | "isempty"
            | "isnotblank"
            | "isnotempty"
            | "isnumeric"
            | "isnumericspace"
            | "iswhitespace"
            | "join"
            | "lastindexof"
            | "lastindexofignorecase"
            | "left"
            | "leftpad"
            | "length"
            | "matches"
            | "mid"
            | "normalizeSpace"
            | "normalizespace"
            | "offsetbycodepoints"
            | "remove"
            | "removeend"
            | "removeendignorecase"
            | "removestart"
            | "removestartignorecase"
            | "repeat"
            | "replace"
            | "replaceall"
            | "replacefirst"
            | "reverse"
            | "right"
            | "rightpad"
            | "split"
            | "splitbycharactertype"
            | "splitbycharactertypecamelcase"
            | "startswith"
            | "startswithignorecase"
            | "striphtmltags"
            | "substring"
            | "substringafter"
            | "substringafterlast"
            | "substringbefore"
            | "substringbeforelast"
            | "substringbetween"
            | "swapcase"
            | "tochararray"
            | "tolowercase"
            | "touppercase"
            | "trim"
            | "tostring"
            | "uncapitalize"
            | "unescapeecmascript"
            | "unescapehtml3"
            | "unescapehtml4"
            | "unescapejava"
            | "unescapexml"
            | "valueof"
            | "valueofgmt"
    )
}
fn native_return(typ: &str, name: &str, args: &[Expr]) -> Expr {
    if let Some((base, params)) = generic(typ) {
        return match name {
            "size" | "hashcode" => Expr::value("integer"),
            "isempty" | "contains" | "containskey" | "equals" | "addall" | "retainall"
            | "removeall" => Expr::value("boolean"),
            "get" | "remove" if base == "list" => {
                params.first().cloned().map(Expr::value).unwrap_or_default()
            }
            "set" | "clear" => Expr::value("void"),
            "add" if base == "list" => Expr::value("void"),
            "add" | "remove" if base == "set" => Expr::value("boolean"),
            "get" | "put" | "remove" if base == "map" => {
                params.get(1).cloned().map(Expr::value).unwrap_or_default()
            }
            "keyset" if base == "map" => Expr::value(format!("set<{}>", params[0])),
            "values" if base == "map" => Expr::value(format!("list<{}>", params[1])),
            "iterator" => Expr::value(format!("iterator<{}>", params[0])),
            "next" if base == "iterator" => Expr::value(params[0].clone()),
            "hasnext" => Expr::value("boolean"),
            "clone" | "deepclone" => Expr::value(typ),
            "tostring" => Expr::value("string"),
            _ => Expr::default(),
        };
    }
    if typ == "string" {
        return match name {
            "length"
            | "indexof"
            | "lastindexof"
            | "compareto"
            | "comparetoignorecase"
            | "charat"
            | "countmatches"
            | "codepointat"
            | "codepointbefore"
            | "codepointcount"
            | "getlevenshteindistance"
            | "hashcode"
            | "indexofany"
            | "indexofanybut"
            | "indexofdifference"
            | "indexofignorecase"
            | "lastindexofignorecase"
            | "offsetbycodepoints" => Expr::value("integer"),
            "isblank"
            | "isnotblank"
            | "isempty"
            | "isnotempty"
            | "equals"
            | "equalsignorecase"
            | "contains"
            | "containsignorecase"
            | "startswith"
            | "startswithignorecase"
            | "endswith"
            | "endswithignorecase"
            | "matches"
            | "isalpha"
            | "isalphanumeric"
            | "isnumeric" => Expr::value("boolean"),
            "isalllowercase"
            | "isalluppercase"
            | "isalphaspace"
            | "isalphanumericspace"
            | "isasciiprintable"
            | "isnumericspace"
            | "iswhitespace"
            | "containsany"
            | "containsonly"
            | "containsnone"
            | "containswhitespace" => Expr::value("boolean"),
            "split" | "splitbycharactertype" | "splitbycharactertypecamelcase" => {
                Expr::value("list<string>")
            }
            "tochararray" => Expr::value("list<integer>"),
            _ if string_method(name) => Expr::value("string"),
            _ => Expr::default(),
        };
    }
    if numeric(typ) {
        return Expr::value(match name {
            "format" | "tostring" => "string",
            "intvalue" | "scale" | "precision" | "hashcode" => "integer",
            "longvalue" => "long",
            "doublevalue" => "double",
            "equals" => "boolean",
            _ => typ,
        });
    }
    match (typ, name) {
        (_, "equals" | "haserrors" | "issuccess" | "iscreated") => Expr::value("boolean"),
        (_, "hashcode") => Expr::value("integer"),
        (_, "tostring") => Expr::value("string"),
        ("boolean", "valueof") => Expr::value("boolean"),
        ("blob", "valueof") => Expr::value("blob"),
        ("blob", "size") => Expr::value("integer"),
        ("encodingutil", "base64decode" | "convertfromhex") => Expr::value("blob"),
        ("encodingutil", _) => Expr::value("string"),
        ("json", "serialize" | "serializepretty") => Expr::value("string"),
        (
            "crypto",
            "encrypt"
            | "decrypt"
            | "encryptwithmanagediv"
            | "decryptwithmanagediv"
            | "generatedigest"
            | "generatemac"
            | "generateaeskey"
            | "sign",
        ) => Expr::value("blob"),
        ("crypto", "verifymac" | "verify") => Expr::value("boolean"),
        (
            "date" | "datetime" | "time",
            "valueof" | "valueofgmt" | "newinstance" | "newinstancegmt" | "today" | "now"
            | "adddays" | "addmonths" | "addyears" | "addhours" | "addminutes" | "addseconds"
            | "addmilliseconds",
        ) => Expr::value(typ),
        ("date" | "datetime" | "time", "format" | "formatgmt") => Expr::value("string"),
        ("date" | "datetime" | "time", "date" | "dategmt") => Expr::value("date"),
        ("date" | "datetime" | "time", "time" | "timegmt") => Expr::value("time"),
        ("date" | "datetime" | "time", _) => Expr::value("integer"),
        ("pattern", "compile") => Expr::value("pattern"),
        ("pattern", "matcher") => Expr::value("matcher"),
        ("pattern", "matches") => Expr::value("boolean"),
        ("matcher", "matches" | "find" | "lookingat") => Expr::value("boolean"),
        ("matcher", "group") => Expr::value("string"),
        ("matcher", "start" | "end" | "groupcount") => Expr::value("integer"),
        ("exception", "getmessage" | "gettypename" | "getstacktracestring") => {
            Expr::value("string")
        }
        ("exception", "getlinenumber") => Expr::value("integer"),
        ("http", "send") => Expr::value("httpresponse"),
        (
            "httprequest" | "httpresponse",
            "getbody" | "getheader" | "getendpoint" | "getmethod" | "getstatus",
        ) => Expr::value("string"),
        ("httprequest" | "httpresponse", "getbodyasblob") => Expr::value("blob"),
        ("httpresponse", "getstatuscode") => Expr::value("integer"),
        ("type", "forname") => Expr::value("type"),
        ("type", "getname") => Expr::value("string"),
        ("limits", _) => Expr::value("integer"),
        ("userinfo", _) => Expr::default(),
        ("database", "countquery") => Expr::value("integer"),
        ("database", "getquerylocator") => Expr::value("database.querylocator"),
        ("database", "setsavepoint") => Expr::value("savepoint"),
        ("security", "stripinaccessible") => Expr::value("sobjectaccessdecision"),
        ("sobjectaccessdecision", "getrecords") => Expr::value("list<sobject>"),
        ("sobjectaccessdecision", "getremovedfields") => Expr::value("map<string,set<string>>"),
        ("schema", "getglobaldescribe") => Expr::value("map<string,schema.sobjecttype>"),
        ("schema.sobjecttype", "getdescribe") => Expr::value("schema.describesobjectresult"),
        ("schema.sobjectfield", "getdescribe") => Expr::value("schema.describefieldresult"),
        (
            "schema.describesobjectresult" | "schema.describefieldresult",
            "getname" | "getlabel" | "getrelationshipname",
        ) => Expr::value("string"),
        ("schema.describesobjectresult" | "schema.describefieldresult", n)
            if n.starts_with("is") =>
        {
            Expr::value("boolean")
        }
        ("database.querylocator", "iterator") => Expr::value("database.querylocatoriterator"),
        ("database.querylocatoriterator", "next") => Expr::value("sobject"),
        ("database.querylocatoriterator", "hasnext") => Expr::value("boolean"),
        (_, "getsobjecttype") => Expr::value("schema.sobjecttype"),
        ("system", "enqueuejob" | "schedule") => Expr::value("id"),
        ("math", _) => args.first().cloned().unwrap_or_default(),
        _ => Expr::default(),
    }
}
