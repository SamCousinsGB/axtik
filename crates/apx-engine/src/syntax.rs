use crate::model::Diagnostic;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug)]
pub struct Node {
    pub kind: String,
    pub named: bool,
    pub start: usize,
    pub end: usize,
    pub source: Arc<str>,
    pub path: Arc<str>,
    pub children: Vec<(Option<String>, Node)>,
}
impl Node {
    pub fn text(&self) -> &str {
        &self.source[self.start..self.end]
    }
    pub fn field(&self, name: &str) -> Option<&Node> {
        self.children
            .iter()
            .find(|(f, _)| f.as_deref() == Some(name))
            .map(|(_, n)| n)
    }
    pub fn fields<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> {
        self.children
            .iter()
            .filter(move |(f, _)| f.as_deref() == Some(name))
            .map(|(_, n)| n)
    }
    pub fn named(&self) -> impl Iterator<Item = &Node> {
        self.children
            .iter()
            .map(|(_, n)| n)
            .filter(|n| n.named && !matches!(n.kind.as_str(), "line_comment" | "block_comment"))
    }
    pub fn child(&self, kind: &str) -> Option<&Node> {
        self.named().find(|n| n.kind == kind)
    }
    pub fn line(&self) -> usize {
        self.source[..self.start]
            .bytes()
            .filter(|b| *b == b'\n')
            .count()
            + 1
    }
    pub fn diagnostic(&self, code: &str, message: impl Into<String>) -> Diagnostic {
        let prefix = &self.source[..self.start];
        let endprefix = &self.source[..self.end];
        let col = prefix
            .rsplit('\n')
            .next()
            .unwrap_or("")
            .encode_utf16()
            .count()
            + 1;
        let ec = endprefix
            .rsplit('\n')
            .next()
            .unwrap_or("")
            .encode_utf16()
            .count()
            + 1;
        let end_line = endprefix.bytes().filter(|b| *b == b'\n').count() + 1;
        Diagnostic {
            code: code.into(),
            severity: "error".into(),
            message: crate::model::bounded_diagnostic(message),
            path: self.path.to_string(),
            line: self.line(),
            column: col,
            end_line,
            end_column: if end_line == self.line() {
                ec.max(col + 1)
            } else {
                ec
            },
        }
    }
}
fn own(n: tree_sitter::Node, source: &Arc<str>, path: &Arc<str>) -> Node {
    let mut children = vec![];
    for i in 0..n.child_count() {
        if let Some(c) = n.child(i) {
            children.push((
                n.field_name_for_child(i as u32).map(str::to_string),
                own(c, source, path),
            ))
        }
    }
    Node {
        kind: n.kind().into(),
        named: n.is_named(),
        start: n.start_byte(),
        end: n.end_byte(),
        source: source.clone(),
        path: path.clone(),
        children,
    }
}
pub fn parse(source: &str, path: &str) -> Result<Node, Vec<Diagnostic>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_sfapex::apex::LANGUAGE.into())
        .expect("bundled Apex grammar");
    parser.set_timeout_micros(2_000_000);
    let tree = parser.parse(source, None).ok_or_else(|| {
        vec![Diagnostic::plain(
            "APX_PARSE_BUDGET",
            "Parser time budget exceeded",
        )]
    })?;
    let mut stack = vec![(tree.root_node(), 0usize)];
    let mut count = 0usize;
    while let Some((node, depth)) = stack.pop() {
        count += 1;
        if depth > 128 || count > 100_000 {
            return Err(vec![Diagnostic::plain(
                "APX_PARSE_BUDGET",
                "Syntax tree exceeds preview bound: depth 128 / 100000 nodes per file",
            )]);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push((child, depth + 1));
        }
    }
    let root = own(tree.root_node(), &Arc::from(source), &Arc::from(path));
    if tree.root_node().has_error() {
        let mut ds = vec![];
        fn errors(n: tree_sitter::Node, root: &Node, ds: &mut Vec<Diagnostic>) {
            if ds.len() >= 50 {
                return;
            }
            if n.is_error() || n.is_missing() {
                let mut at = root.clone();
                at.start = n.start_byte();
                at.end = n.end_byte();
                ds.push(at.diagnostic(
                    "APX_SYNTAX",
                    if n.is_missing() {
                        format!("Expected {}", n.kind())
                    } else {
                        "Invalid Apex syntax".into()
                    },
                ));
                return;
            }
            let mut c = n.walk();
            for x in n.children(&mut c) {
                if x.has_error() || x.is_missing() {
                    errors(x, root, ds)
                }
            }
        }
        errors(tree.root_node(), &root, &mut ds);
        return Err(ds);
    }
    Ok(root)
}
#[derive(Clone, Debug)]
pub struct Method {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub return_type: String,
    pub body: Option<Node>,
    pub declaration: Node,
    pub test: bool,
    pub setup: bool,
    pub static_method: bool,
}
#[derive(Clone, Debug)]
pub struct Class {
    pub name: String,
    pub declaration: Node,
    pub methods: Vec<Method>,
    pub fields: Vec<Node>,
    pub instance_fields: Vec<Node>,
    pub initializers: Vec<Node>,
    pub constructors: Vec<Method>,
    pub parent: Option<String>,
    pub superclass: Option<String>,
    pub interfaces: Vec<String>,
    pub enum_members: Vec<String>,
    pub kind: String,
    pub deferred_names: std::collections::BTreeSet<String>,
    pub unsupported: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct Trigger {
    pub name: String,
    pub object: String,
    pub events: Vec<String>,
    pub body: Node,
}
#[derive(Default, Clone, Debug)]
pub struct Program {
    pub schema: crate::schema::SchemaCatalog,
    pub dataweave_scripts: BTreeMap<String, String>,
    pub api_versions: BTreeMap<String, u32>,
    pub environment: Option<crate::environment::Environment>,
    pub classes: BTreeMap<String, Class>,
    pub triggers: Vec<Trigger>,
    pub automation_blockers: Vec<String>,
}
fn modifiers(n: &Node) -> String {
    n.named()
        .filter(|n| n.kind == "modifiers" || n.kind == "annotation")
        .map(|n| n.text())
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}
pub fn declarations(
    root: &Node,
    p: &mut Program,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<String> {
    declarations_in(root, p, diagnostics, None)
}
fn declarations_in(
    root: &Node,
    p: &mut Program,
    diagnostics: &mut Vec<Diagnostic>,
    parent: Option<&str>,
) -> Vec<String> {
    let mut names = vec![];
    for n in root.named() {
        match n.kind.as_str() {
            "class_declaration" | "interface_declaration" | "enum_declaration" => {
                let Some(name) = n.field("name") else {
                    continue;
                };
                let cname = parent
                    .map(|p| format!("{p}.{}", name.text()))
                    .unwrap_or_else(|| name.text().to_string());
                names.push(cname.clone());
                let mut class = Class {
                    name: cname.clone(),
                    declaration: n.clone(),
                    methods: vec![],
                    fields: vec![],
                    instance_fields: vec![],
                    initializers: vec![],
                    constructors: vec![],
                    parent: parent.map(str::to_string),
                    superclass: n
                        .field("superclass")
                        .and_then(|n| n.named().next())
                        .map(|n| n.text().into()),
                    interfaces: n
                        .field("interfaces")
                        .and_then(|n| n.named().next())
                        .map(|n| n.named().map(|n| n.text().to_string()).collect())
                        .unwrap_or_default(),
                    enum_members: vec![],
                    kind: n.kind.clone(),
                    deferred_names: Default::default(),
                    unsupported: vec![],
                };
                if let Some(body) = n.field("body") {
                    for x in body.named() {
                        match x.kind.as_str() {
                            "method_declaration" | "constructor_declaration" => {
                                let mods = modifiers(x);
                                let nm = x.field("name").map(|n| n.text()).unwrap_or("");
                                let params = x
                                    .field("parameters")
                                    .map(|p| {
                                        p.named()
                                            .filter(|n| n.kind == "formal_parameter")
                                            .map(|v| {
                                                (
                                                    v.field("name")
                                                        .unwrap()
                                                        .text()
                                                        .to_ascii_lowercase(),
                                                    v.field("type").unwrap().text().to_string(),
                                                )
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                let method = Method {
                                    name: nm.into(),
                                    params,
                                    return_type: x
                                        .field("type")
                                        .map(|n| n.text())
                                        .unwrap_or("void")
                                        .into(),
                                    body: x.field("body").cloned(),
                                    declaration: x.clone(),
                                    test: mods.contains("@istest") || mods.contains("testmethod"),
                                    setup: mods.contains("@testsetup"),
                                    static_method: mods
                                        .split_whitespace()
                                        .any(|m| m == "static" || m == "testmethod"),
                                };
                                if x.kind == "constructor_declaration" {
                                    class.constructors.push(method);
                                } else {
                                    class.methods.push(method);
                                }
                            }
                            "field_declaration" => {
                                if modifiers(x).split_whitespace().any(|m| m == "static") {
                                    class.fields.push(x.clone())
                                } else {
                                    class.instance_fields.push(x.clone());
                                    for d in x.fields("declarator") {
                                        if let Some(name) = d.field("name") {
                                            class
                                                .deferred_names
                                                .insert(name.text().to_ascii_lowercase());
                                        }
                                    }
                                }
                            }
                            "class_declaration" | "interface_declaration" | "enum_declaration" => {
                                // Declaring an inner type does not execute its body. Reaching it
                                // still blocks until nested type binding and construction exist.
                                if let Some(name) = x.field("name") {
                                    class
                                        .deferred_names
                                        .insert(name.text().to_ascii_lowercase());
                                }
                                let mut container = body.clone();
                                container.children = vec![(None, x.clone())];
                                declarations_in(&container, p, diagnostics, Some(&cname));
                            }
                            "static_initializer" | "block" => {
                                class.initializers.push(x.clone());
                            }
                            "enum_constant" => {
                                if let Some(name) = x.field("name") {
                                    class.enum_members.push(name.text().into());
                                }
                            }
                            _ => {
                                class.unsupported.push(x.kind.clone());
                            }
                        }
                    }
                }
                let mut methodkeys = std::collections::BTreeSet::new();
                for m in &class.methods {
                    let sig = format!(
                        "{}({})",
                        m.name.to_ascii_lowercase(),
                        m.params
                            .iter()
                            .map(|p| p.1.to_ascii_lowercase())
                            .collect::<Vec<_>>()
                            .join(",")
                    );
                    if !methodkeys.insert(sig) {
                        diagnostics.push(m.declaration.diagnostic(
                            "APX_DUPLICATE_METHOD",
                            format!("Duplicate method {}.{}", cname, m.name),
                        ))
                    }
                }
                if p.classes.contains_key(&cname.to_ascii_lowercase()) {
                    diagnostics.push(
                        name.diagnostic("APX_DUPLICATE_TYPE", format!("Duplicate type {cname}")),
                    )
                } else {
                    p.classes.insert(cname.to_ascii_lowercase(), class);
                }
            }
            "trigger_declaration" => {
                let name = n.field("name").unwrap().text().to_string();
                names.push(name.clone());
                p.triggers.push(Trigger {
                    name,
                    object: n.field("object").unwrap().text().to_ascii_lowercase(),
                    events: n
                        .fields("events")
                        .filter(|x| x.kind == "trigger_event")
                        .map(|x| x.text().to_ascii_lowercase())
                        .collect(),
                    body: n.field("body").unwrap().clone(),
                })
            }
            _ => {
                diagnostics.push(n.diagnostic(
                    "APX_SOURCE_UNIT",
                    "Expected a class, interface, enum or trigger source unit",
                ));
            }
        }
    }
    names
}
