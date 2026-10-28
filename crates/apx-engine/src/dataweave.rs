//! Bounded source interpreter for a deliberately explicit DataWeave 2 subset.
//! This module owns no Apex context, filesystem, network, or script-name dispatch.
//! Unsupported syntax/functions remain blocked rather than returning placeholders.
use crate::value::{NumKind, Number};
use chrono::{DateTime, Datelike, Timelike, Utc};
use num_rational::BigRational;
use num_traits::Zero;
use std::{collections::BTreeMap, rc::Rc};

const MAX_BYTES: usize = 1_048_576;
const MAX_ITEMS: usize = 10_000;
const MAX_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub enum DwValue {
    Null,
    Bool(bool),
    Number(BigRational),
    String(String),
    Array(Vec<DwValue>),
    Object(Vec<DwField>),
    TypedObject { class: String, fields: Vec<DwField> },
}
#[derive(Clone, Debug, PartialEq)]
pub struct DwField {
    pub key: String,
    pub attributes: Vec<(String, DwValue)>,
    pub value: DwValue,
}
impl DwField {
    pub fn new(key: impl Into<String>, value: DwValue) -> Self {
        Self {
            key: key.into(),
            attributes: vec![],
            value,
        }
    }
}
impl DwValue {
    pub fn number(value: &str) -> Result<Self, DwError> {
        if let Some((_, exponent)) = value.split_once(['e', 'E']) {
            let exponent = exponent
                .parse::<i64>()
                .map_err(|_| DwError::runtime("Invalid numeric exponent"))?;
            if !(-1000..=1000).contains(&exponent) {
                return Err(DwError::unsupported("Numeric exponent exceeds host bound"));
            }
        }
        Ok(Self::Number(
            Number::parse(value).map_err(DwError::runtime)?.q,
        ))
    }
    pub fn from_json(value: &serde_json::Value) -> Result<Self, DwError> {
        Self::json_at(value, 0)
    }
    fn json_at(value: &serde_json::Value, depth: usize) -> Result<Self, DwError> {
        if depth > MAX_DEPTH {
            return Err(DwError::unsupported("JSON nesting exceeds host bound"));
        }
        Ok(match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(v) => Self::Bool(*v),
            serde_json::Value::Number(v) => Self::number(&v.to_string())?,
            serde_json::Value::String(v) => Self::String(v.clone()),
            serde_json::Value::Array(v) => {
                if v.len() > MAX_ITEMS {
                    return Err(DwError::unsupported("JSON array exceeds host bound"));
                }
                Self::Array(
                    v.iter()
                        .map(|x| Self::json_at(x, depth + 1))
                        .collect::<Result<_, _>>()?,
                )
            }
            serde_json::Value::Object(v) => Self::Object(
                v.iter()
                    .map(|(k, v)| Ok(DwField::new(k, Self::json_at(v, depth + 1)?)))
                    .collect::<Result<_, DwError>>()?,
            ),
        })
    }
    fn fields(&self) -> Option<&[DwField]> {
        match self {
            Self::Object(v) | Self::TypedObject { fields: v, .. } => Some(v),
            _ => None,
        }
    }
    fn scalar(&self) -> Result<String, DwError> {
        match self {
            Self::String(v) => Ok(v.clone()),
            Self::Number(v) => Ok(number_text(v)),
            Self::Bool(v) => Ok(v.to_string()),
            Self::Null => Ok("null".into()),
            _ => Err(DwError::runtime("Expected scalar value")),
        }
    }
}
#[derive(Debug)]
pub struct DwResult {
    pub value: DwValue,
    pub text: String,
    pub logs: Vec<String>,
    pub media_type: String,
}
#[derive(Debug, Clone)]
pub struct DwError {
    pub message: String,
    pub unsupported: bool,
}
impl DwError {
    fn runtime(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            unsupported: false,
        }
    }
    fn unsupported(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            unsupported: true,
        }
    }
}
impl std::fmt::Display for DwError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for DwError {}
type R<T> = Result<T, DwError>;
fn number_text(value: &BigRational) -> String {
    Number {
        q: value.clone(),
        kind: NumKind::Decimal,
    }
    .display()
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Word(String),
    String(String),
    Number(String),
    Symbol(char),
    Op(String),
    Delimiter,
}
fn tokenize(source: &str) -> R<Vec<Token>> {
    if source.len() > MAX_BYTES {
        return Err(DwError::unsupported("DataWeave source exceeds 1 MiB"));
    }
    let chars: Vec<char> = source.chars().collect();
    let mut i = 0;
    let mut out = vec![];
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            if i + 1 >= chars.len() {
                return Err(DwError::runtime("Unterminated comment"));
            }
            i += 2;
            continue;
        }
        if c == '-' && chars.get(i + 1) == Some(&'-') && chars.get(i + 2) == Some(&'-') {
            out.push(Token::Delimiter);
            i += 3;
            continue;
        }
        if c == '\'' || c == '"' {
            let quote = c;
            i += 1;
            let mut text = String::new();
            let mut closed = false;
            while i < chars.len() {
                let c = chars[i];
                i += 1;
                if c == quote {
                    closed = true;
                    break;
                }
                if c == '\\' {
                    let Some(e) = chars.get(i).copied() else {
                        return Err(DwError::runtime("Unterminated string escape"));
                    };
                    i += 1;
                    match e {
                        'n' => text.push('\n'),
                        'r' => text.push('\r'),
                        't' => text.push('\t'),
                        'b' => text.push('\u{0008}'),
                        'f' => text.push('\u{000c}'),
                        '\\' | '\'' | '"' | '/' => text.push(e),
                        'u' => {
                            let digits: String = chars.iter().skip(i).take(4).collect();
                            if digits.len() != 4 {
                                return Err(DwError::runtime("Invalid Unicode escape"));
                            }
                            let code = u32::from_str_radix(&digits, 16)
                                .map_err(|_| DwError::runtime("Invalid Unicode escape"))?;
                            text.push(char::from_u32(code).ok_or_else(|| {
                                DwError::unsupported("Unpaired UTF-16 escape is unsupported")
                            })?);
                            i += 4;
                        }
                        _ => return Err(DwError::unsupported("Unsupported string escape")),
                    }
                } else {
                    text.push(c);
                }
            }
            if !closed {
                return Err(DwError::runtime("Unterminated string"));
            }
            out.push(Token::String(text));
        } else if c.is_ascii_digit() {
            let start = i;
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            if matches!(chars.get(i), Some('e' | 'E')) {
                i += 1;
                if matches!(chars.get(i), Some('+' | '-')) {
                    i += 1;
                }
                while chars.get(i).is_some_and(|c| c.is_ascii_digit()) {
                    i += 1;
                }
            }
            out.push(Token::Number(chars[start..i].iter().collect()));
        } else if c.is_ascii_alphabetic() || c == '_' || c == '$' {
            let start = i;
            i += 1;
            while chars
                .get(i)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
            {
                i += 1;
            }
            out.push(Token::Word(chars[start..i].iter().collect()));
        } else {
            let pair: String = chars.iter().skip(i).take(2).collect();
            if ["->", "==", "!=", ">=", "<=", ">>", "::", "++"].contains(&pair.as_str()) {
                out.push(Token::Op(pair));
                i += 2;
            } else if "{}[](),:.@%=+-*/><".contains(c) {
                out.push(Token::Symbol(c));
                i += 1;
            } else {
                return Err(DwError::unsupported(format!(
                    "Unsupported DataWeave character {c}"
                )));
            }
        }
        if out.len() > 20_000 {
            return Err(DwError::unsupported("DataWeave exceeds 20000 tokens"));
        }
    }
    Ok(out)
}
#[derive(Clone, Debug)]
enum Expr {
    Group(Box<Expr>),
    Literal(DwValue),
    Variable(String),
    Select(Box<Expr>, String),
    Array(Vec<Expr>),
    Object(Vec<Entry>),
    Call(String, Vec<Expr>),
    Binary(String, Box<Expr>, Box<Expr>),
    Lambda(Vec<String>, Box<Expr>),
    Transform(String, Box<Expr>, Box<Expr>),
    Cast(Box<Expr>, String, Vec<(String, Expr)>),
    Match(Box<Expr>, Vec<(Expr, Expr)>, Box<Expr>),
}
#[derive(Clone, Debug)]
enum Entry {
    Field {
        key: Expr,
        attributes: Vec<(String, Expr)>,
        value: Expr,
    },
    Spread(Expr),
}
#[derive(Debug)]
struct Function {
    params: Vec<String>,
    body: Expr,
}
#[derive(Debug)]
struct Input {
    name: String,
    media: String,
    separator: char,
}
#[derive(Debug)]
struct Script {
    inputs: Vec<Input>,
    output: String,
    functions: BTreeMap<String, Function>,
    strings_imported: bool,
    body: Expr,
}
struct Parser {
    tokens: Vec<Token>,
    at: usize,
    depth: usize,
    nodes: usize,
}
impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            at: 0,
            depth: 0,
            nodes: 0,
        }
    }
    fn symbol(&mut self, c: char) -> bool {
        if self.tokens.get(self.at) == Some(&Token::Symbol(c)) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn op(&mut self, s: &str) -> bool {
        if self.tokens.get(self.at) == Some(&Token::Op(s.into())) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn word(&mut self, w: &str) -> bool {
        if self.tokens.get(self.at) == Some(&Token::Word(w.into())) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn require(&mut self, c: char) -> R<()> {
        if self.symbol(c) {
            Ok(())
        } else {
            Err(DwError::runtime(format!(
                "Expected '{c}' at token {}",
                self.at
            )))
        }
    }
    fn ident(&mut self) -> R<String> {
        let Some(Token::Word(s)) = self.tokens.get(self.at) else {
            return Err(DwError::runtime("Expected identifier"));
        };
        let s = s.clone();
        self.at += 1;
        Ok(s)
    }
    fn media(&mut self) -> R<String> {
        let a = self.ident()?;
        self.require('/')?;
        Ok(format!("{a}/{}", self.ident()?))
    }
    fn script(&mut self) -> R<Script> {
        self.require('%')?;
        if !self.word("dw") {
            return Err(DwError::runtime("Expected %dw version header"));
        }
        if self.tokens.get(self.at) != Some(&Token::Number("2.0".into())) {
            return Err(DwError::unsupported(
                "Only DataWeave 2.0 source is implemented",
            ));
        }
        self.at += 1;
        let mut script = Script {
            inputs: vec![],
            output: "application/json".into(),
            functions: BTreeMap::new(),
            strings_imported: false,
            body: Expr::Literal(DwValue::Null),
        };
        while self.tokens.get(self.at) != Some(&Token::Delimiter) {
            if self.word("input") {
                let name = self.ident()?;
                let media = self.media()?;
                let mut separator = ',';
                if self.word("separator") {
                    self.require('=')?;
                    let Some(Token::String(s)) = self.tokens.get(self.at) else {
                        return Err(DwError::runtime("CSV separator must be a string"));
                    };
                    let mut chars = s.chars();
                    separator = chars
                        .next()
                        .ok_or_else(|| DwError::runtime("CSV separator is empty"))?;
                    if chars.next().is_some() || ['\r', '\n', '"'].contains(&separator) {
                        return Err(DwError::runtime("Invalid CSV separator"));
                    }
                    self.at += 1;
                }
                if script.inputs.iter().any(|i| i.name == name) {
                    return Err(DwError::runtime("Duplicate input name"));
                }
                script.inputs.push(Input {
                    name,
                    media,
                    separator,
                });
            } else if self.word("output") {
                script.output = self.media()?;
            } else if self.word("import") {
                let mut module = self.ident()?;
                while self.op("::") {
                    module.push_str("::");
                    module.push_str(&self.ident()?);
                }
                if module != "dw::core::Strings" {
                    return Err(DwError::unsupported(format!(
                        "Module {module} is not implemented"
                    )));
                }
                script.strings_imported = true;
            } else if self.word("fun") {
                let name = self.ident()?;
                let params = self.parameters(true)?;
                self.require('=')?;
                let body = self.expr(0)?;
                if script
                    .functions
                    .insert(name, Function { params, body })
                    .is_some()
                {
                    return Err(DwError::runtime("Duplicate function definition"));
                }
            } else {
                return Err(DwError::unsupported(format!(
                    "Unsupported DataWeave header at token {}",
                    self.at
                )));
            }
        }
        self.at += 1;
        script.body = self.expr(0)?;
        if self.at != self.tokens.len() {
            return Err(DwError::unsupported(format!(
                "Unsupported trailing DataWeave syntax at token {}",
                self.at
            )));
        }
        Ok(script)
    }
    fn parameters(&mut self, typed: bool) -> R<Vec<String>> {
        self.require('(')?;
        let mut params = vec![];
        if !self.symbol(')') {
            loop {
                params.push(self.ident()?);
                if typed && self.symbol(':') {
                    let typ = self.ident()?;
                    if !["Key", "String", "Number", "Boolean", "Any"].contains(&typ.as_str()) {
                        return Err(DwError::unsupported(
                            "Function parameter type is not implemented",
                        ));
                    }
                }
                if self.symbol(')') {
                    break;
                }
                self.require(',')?;
            }
        }
        if params.len() > 8 {
            return Err(DwError::unsupported("Function arity exceeds host bound"));
        }
        Ok(params)
    }
    fn expr(&mut self, min: u8) -> R<Expr> {
        self.depth += 1;
        self.nodes += 1;
        if self.depth > MAX_DEPTH || self.nodes > 512 {
            return Err(DwError::unsupported(
                "DataWeave expression exceeds host bound",
            ));
        }
        let result = self.expr_inner(min);
        self.depth -= 1;
        result
    }
    fn expr_inner(&mut self, min: u8) -> R<Expr> {
        let mut left = self.primary()?;
        loop {
            if min <= 90 && self.symbol('.') {
                left = Expr::Select(Box::new(left), self.ident()?);
                continue;
            }
            if min <= 75 && self.word("as") {
                let typ = self.ident()?;
                let mut options = vec![];
                if self.symbol('{') {
                    if !self.symbol('}') {
                        loop {
                            let key = self.ident()?;
                            self.require(':')?;
                            options.push((key, self.expr(0)?));
                            if self.symbol('}') {
                                break;
                            }
                            self.require(',')?;
                        }
                    }
                }
                left = Expr::Cast(Box::new(left), typ, options);
                continue;
            }
            if min <= 5 && self.word("match") {
                self.require('{')?;
                let mut cases = vec![];
                while self.word("case") {
                    let key = self.expr(6)?;
                    if !self.op("->") {
                        return Err(DwError::runtime("Expected match arrow"));
                    }
                    let value = self.expr(0)?;
                    cases.push((key, value));
                }
                if !self.word("else") || !self.op("->") {
                    return Err(DwError::unsupported(
                        "Match requires an explicit else branch",
                    ));
                }
                let fallback = self.expr(0)?;
                self.require('}')?;
                left = Expr::Match(Box::new(left), cases, Box::new(fallback));
                continue;
            }
            let operation = match self.tokens.get(self.at) {
                Some(Token::Word(w)) if ["map", "mapObject", "filter"].contains(&w.as_str()) => {
                    Some((10, w.clone()))
                }
                Some(Token::Op(w)) if ["==", "!=", "<=", ">="].contains(&w.as_str()) => {
                    Some((30, w.clone()))
                }
                Some(Token::Symbol(c)) if ['<', '>'].contains(c) => Some((30, c.to_string())),
                Some(Token::Symbol(c)) if ['+', '-'].contains(c) => Some((40, c.to_string())),
                Some(Token::Symbol(c)) if ['*', '/'].contains(c) => Some((50, c.to_string())),
                Some(Token::Op(w)) if w == ">>" => Some((60, w.clone())),
                _ => None,
            };
            let Some((precedence, operation)) = operation else {
                break;
            };
            if precedence < min {
                break;
            }
            self.at += 1;
            if precedence == 10 {
                let callback = self.primary()?;
                left = Expr::Transform(operation, Box::new(left), Box::new(callback));
            } else {
                let right = self.expr(precedence + 1)?;
                left = Expr::Binary(operation, Box::new(left), Box::new(right));
            }
        }
        if min == 0 && self.symbol(':') {
            let key = match left {
                Expr::Variable(name) => Expr::Literal(DwValue::String(name)),
                other => other,
            };
            left = Expr::Object(vec![Entry::Field {
                key,
                attributes: vec![],
                value: self.expr(0)?,
            }]);
        }
        Ok(left)
    }
    fn primary(&mut self) -> R<Expr> {
        if self.symbol('-') {
            return Ok(Expr::Binary(
                "-".into(),
                Box::new(Expr::Literal(DwValue::Number(BigRational::zero()))),
                Box::new(self.expr(80)?),
            ));
        }
        if self.tokens.get(self.at) == Some(&Token::Symbol('(')) {
            let saved = self.at;
            if let Ok(params) = self.parameters(false) {
                if self.op("->") {
                    return Ok(Expr::Lambda(params, Box::new(self.expr(0)?)));
                }
            }
            self.at = saved;
            self.require('(')?;
            let value = self.expr(0)?;
            self.require(')')?;
            return Ok(Expr::Group(Box::new(value)));
        }
        if self.symbol('[') {
            let mut values = vec![];
            if !self.symbol(']') {
                loop {
                    values.push(self.expr(0)?);
                    if self.symbol(']') {
                        break;
                    }
                    self.require(',')?;
                }
            }
            return Ok(Expr::Array(values));
        }
        if self.symbol('{') {
            let mut entries = vec![];
            if !self.symbol('}') {
                loop {
                    entries.push(self.entry()?);
                    if self.symbol('}') {
                        break;
                    }
                    self.require(',')?;
                }
            }
            return Ok(Expr::Object(entries));
        }
        let token = self
            .tokens
            .get(self.at)
            .cloned()
            .ok_or_else(|| DwError::runtime("Unexpected end of expression"))?;
        self.at += 1;
        match token {
            Token::String(v) => {
                if v.contains("$(") {
                    return Err(DwError::unsupported(
                        "String interpolation is not implemented",
                    ));
                }
                Ok(Expr::Literal(DwValue::String(v)))
            }
            Token::Number(v) => Ok(Expr::Literal(DwValue::number(&v)?)),
            Token::Word(v) if v == "true" || v == "false" => {
                Ok(Expr::Literal(DwValue::Bool(v == "true")))
            }
            Token::Word(v) if v == "null" => Ok(Expr::Literal(DwValue::Null)),
            Token::Word(mut name) => {
                while self.op("::") {
                    name.push_str("::");
                    name.push_str(&self.ident()?);
                }
                if self.symbol('(') {
                    let mut args = vec![];
                    if !self.symbol(')') {
                        loop {
                            args.push(self.expr(0)?);
                            if self.symbol(')') {
                                break;
                            }
                            self.require(',')?;
                        }
                    }
                    Ok(Expr::Call(name, args))
                } else {
                    Ok(Expr::Variable(name))
                }
            }
            other => Err(DwError::unsupported(format!(
                "Unsupported expression token {other:?}"
            ))),
        }
    }
    fn entry(&mut self) -> R<Entry> {
        let key = if self.symbol('(') {
            let value = self.expr(0)?;
            self.require(')')?;
            if !matches!(self.tokens.get(self.at), Some(Token::Symbol(':' | '@'))) {
                return Ok(Entry::Spread(value));
            }
            value
        } else {
            let token = self.tokens.get(self.at).cloned();
            self.at += 1;
            match token {
                Some(Token::Word(v) | Token::String(v)) => Expr::Literal(DwValue::String(v)),
                _ => return Err(DwError::runtime("Expected object key or dynamic element")),
            }
        };
        let mut attributes = vec![];
        if self.symbol('@') {
            self.require('(')?;
            loop {
                let name = self.ident()?;
                self.require(':')?;
                attributes.push((name, self.expr(0)?));
                if self.symbol(')') {
                    break;
                }
                self.require(',')?;
            }
        }
        self.require(':')?;
        Ok(Entry::Field {
            key,
            attributes,
            value: self.expr(0)?,
        })
    }
}

struct Evaluator<'a> {
    script: &'a Script,
    fuel: usize,
    depth: usize,
    logs: Vec<String>,
    allocated: usize,
}
type Scope = BTreeMap<String, Rc<DwValue>>;
impl Evaluator<'_> {
    fn eval(&mut self, expr: &Expr, scope: &Scope) -> R<DwValue> {
        if self.fuel == 0 || self.depth >= MAX_DEPTH {
            return Err(DwError::unsupported(
                "DataWeave evaluation exceeded host budget",
            ));
        }
        self.fuel -= 1;
        self.depth += 1;
        let value = self.eval_inner(expr, scope);
        self.depth -= 1;
        if let Ok(value) = &value {
            self.allocated = self.allocated.saturating_add(value_cost(value, 0)?);
            if self.allocated > 16 * MAX_BYTES {
                return Err(DwError::unsupported(
                    "DataWeave allocation exceeded host budget",
                ));
            }
        }
        value
    }
    fn eval_inner(&mut self, expr: &Expr, scope: &Scope) -> R<DwValue> {
        match expr {
            Expr::Literal(v) => Ok(v.clone()),
            Expr::Group(v) => self.eval(v, scope),
            Expr::Variable(name) => scope
                .get(name)
                .map(|value| value.as_ref().clone())
                .ok_or_else(|| DwError::runtime(format!("Unknown variable {name}"))),
            Expr::Select(base, key) => {
                let value = self.eval(base, scope)?;
                if let Some(fields) = value.fields() {
                    Ok(fields
                        .iter()
                        .find(|f| f.key == *key)
                        .map(|f| f.value.clone())
                        .unwrap_or(DwValue::Null))
                } else if value == DwValue::Null {
                    Ok(DwValue::Null)
                } else {
                    Err(DwError::unsupported(
                        "Selector on this value type is not implemented",
                    ))
                }
            }
            Expr::Array(items) => Ok(DwValue::Array(
                items
                    .iter()
                    .map(|v| self.eval(v, scope))
                    .collect::<R<_>>()?,
            )),
            Expr::Object(entries) => {
                let mut fields = vec![];
                for entry in entries {
                    match entry {
                        Entry::Field {
                            key,
                            attributes,
                            value,
                        } => {
                            let key = self.eval(key, scope)?.scalar()?;
                            let attrs = attributes
                                .iter()
                                .map(|(k, v)| Ok((k.clone(), self.eval(v, scope)?)))
                                .collect::<R<_>>()?;
                            fields.push(DwField {
                                key,
                                attributes: attrs,
                                value: self.eval(value, scope)?,
                            });
                        }
                        Entry::Spread(expr) => {
                            let value = self.eval(expr, scope)?;
                            spread(value, &mut fields)?;
                        }
                    }
                    if fields.len() > MAX_ITEMS {
                        return Err(DwError::unsupported("Object exceeds host entry bound"));
                    }
                }
                Ok(DwValue::Object(fields))
            }
            Expr::Binary(op, left, right) => {
                let left = self.eval(left, scope)?;
                let right = self.eval(right, scope)?;
                binary(op, left, right)
            }
            Expr::Lambda(_, _) => Err(DwError::unsupported(
                "First-class function values are not implemented",
            )),
            Expr::Transform(operation, input, callback) => {
                let input = self.eval(input, scope)?;
                let entries = match input {
                    DwValue::Array(v) if operation != "mapObject" => v
                        .into_iter()
                        .enumerate()
                        .map(|(i, v)| (v, DwValue::Number(BigRational::from_integer(i.into())), i))
                        .collect::<Vec<_>>(),
                    DwValue::Object(v) | DwValue::TypedObject { fields: v, .. }
                        if operation == "mapObject" =>
                    {
                        v.into_iter()
                            .enumerate()
                            .map(|(i, v)| (v.value, DwValue::String(v.key), i))
                            .collect()
                    }
                    DwValue::Null => return Ok(DwValue::Null),
                    _ => {
                        return Err(DwError::runtime(format!(
                            "{operation} received an incompatible input"
                        )))
                    }
                };
                if entries.len() > MAX_ITEMS {
                    return Err(DwError::unsupported(
                        "Transformation exceeds host iteration bound",
                    ));
                }
                let mut result = vec![];
                let mut fields = vec![];
                for (value, key, index) in entries {
                    let mut inner = scope.clone();
                    let mut callback = callback.as_ref();
                    while let Expr::Group(inner) = callback {
                        callback = inner;
                    }
                    let (params, body) = if let Expr::Lambda(params, body) = callback {
                        (params.as_slice(), body.as_ref())
                    } else {
                        (&[][..], callback)
                    };
                    if params.len() > if operation == "mapObject" { 3 } else { 2 } {
                        return Err(DwError::unsupported(
                            "Transformation callback arity is not implemented",
                        ));
                    }
                    let args = [
                        value.clone(),
                        key.clone(),
                        DwValue::Number(BigRational::from_integer(index.into())),
                    ];
                    for (name, arg) in params.iter().zip(args.iter()) {
                        inner.insert(name.clone(), Rc::new(arg.clone()));
                    }
                    inner.insert("$".into(), Rc::new(value.clone()));
                    inner.insert("$$".into(), Rc::new(key));
                    inner.insert("$$$".into(), Rc::new(args[2].clone()));
                    let evaluated = self.eval(body, &inner)?;
                    match operation.as_str() {
                        "map" => result.push(evaluated),
                        "filter" => match evaluated {
                            DwValue::Bool(true) => result.push(value),
                            DwValue::Bool(false) => {}
                            _ => {
                                return Err(DwError::runtime(
                                    "filter predicate must return Boolean",
                                ))
                            }
                        },
                        "mapObject" => spread(evaluated, &mut fields)?,
                        _ => return Err(DwError::unsupported("Unknown transformation")),
                    }
                }
                if operation == "mapObject" {
                    Ok(DwValue::Object(fields))
                } else {
                    Ok(DwValue::Array(result))
                }
            }
            Expr::Call(name, args) => {
                let args = args
                    .iter()
                    .map(|v| self.eval(v, scope))
                    .collect::<R<Vec<_>>>()?;
                if name == "log" && args.len() == 1 {
                    if self.logs.len() >= 1000 {
                        return Err(DwError::unsupported("DataWeave log bound exceeded"));
                    }
                    self.logs.push(render_json(&args[0], false)?);
                    return Ok(args[0].clone());
                }
                if name == "Strings::pluralize" && self.script.strings_imported && args.len() == 1 {
                    return match &args[0] {
                        DwValue::Null => Ok(DwValue::Null),
                        DwValue::String(value) => Ok(DwValue::String(pluralize(value)?)),
                        _ => Err(DwError::runtime("pluralize requires String")),
                    };
                }
                let function = self.script.functions.get(name).ok_or_else(|| {
                    DwError::unsupported(format!("Function {name} is not implemented"))
                })?;
                if args.len() != function.params.len() {
                    return Err(DwError::runtime("Function argument count mismatch"));
                }
                let mut inner = scope.clone();
                for (name, value) in function.params.iter().zip(args) {
                    inner.insert(name.clone(), Rc::new(value));
                }
                self.eval(&function.body, &inner)
            }
            Expr::Cast(expr, typ, options) => {
                let value = self.eval(expr, scope)?;
                let options = options
                    .iter()
                    .map(|(k, v)| Ok((k.clone(), self.eval(v, scope)?)))
                    .collect::<R<BTreeMap<_, _>>>()?;
                match typ.as_str() {
                    "Object" => {
                        if options.keys().any(|k| k != "class") {
                            return Err(DwError::unsupported(
                                "Object coercion option is not implemented",
                            ));
                        }
                        let class = options
                            .get("class")
                            .ok_or_else(|| DwError::unsupported("Object coercion requires class"))?
                            .scalar()?;
                        let fields = value
                            .fields()
                            .ok_or_else(|| DwError::runtime("Object coercion requires object"))?
                            .to_vec();
                        Ok(DwValue::TypedObject { class, fields })
                    }
                    "String" => {
                        if let Some(format) = options.get("format") {
                            if options.len() != 1 {
                                return Err(DwError::unsupported(
                                    "String formatting options are not implemented",
                                ));
                            }
                            Ok(DwValue::String(format_datetime(
                                &value.scalar()?,
                                &format.scalar()?,
                            )?))
                        } else if options.is_empty() {
                            Ok(DwValue::String(value.scalar()?))
                        } else {
                            Err(DwError::unsupported(
                                "String coercion option is not implemented",
                            ))
                        }
                    }
                    _ => Err(DwError::unsupported(format!(
                        "Coercion to {typ} is not implemented"
                    ))),
                }
            }
            Expr::Match(input, cases, fallback) => {
                let value = self.eval(input, scope)?;
                for (key, body) in cases {
                    if self.eval(key, scope)? == value {
                        return self.eval(body, scope);
                    }
                }
                self.eval(fallback, scope)
            }
        }
    }
}
fn spread(value: DwValue, fields: &mut Vec<DwField>) -> R<()> {
    match value {
        DwValue::Object(v) | DwValue::TypedObject { fields: v, .. } => fields.extend(v),
        DwValue::Array(values) => {
            for value in values {
                spread(value, fields)?;
            }
        }
        DwValue::Null => {}
        _ => return Err(DwError::runtime("Dynamic object elements must be objects")),
    }
    if fields.len() > MAX_ITEMS {
        return Err(DwError::unsupported("Object exceeds host entry bound"));
    }
    Ok(())
}
fn value_cost(value: &DwValue, depth: usize) -> R<usize> {
    if depth > MAX_DEPTH {
        return Err(DwError::unsupported("Value depth exceeds host bound"));
    }
    let cost = match value {
        DwValue::String(s) => s.len(),
        DwValue::Number(n) => {
            if n.numer().bits() > 4096 || n.denom().bits() > 4096 {
                return Err(DwError::unsupported("Numeric magnitude exceeds host bound"));
            }
            number_text(n).len()
        }
        DwValue::Array(v) => v.iter().try_fold(0usize, |n, v| {
            Ok(n.saturating_add(value_cost(v, depth + 1)?))
        })?,
        DwValue::Object(v) | DwValue::TypedObject { fields: v, .. } => {
            v.iter().try_fold(0usize, |n, f| {
                let mut size = n
                    .saturating_add(f.key.len())
                    .saturating_add(value_cost(&f.value, depth + 1)?);
                for (k, v) in &f.attributes {
                    size = size
                        .saturating_add(k.len())
                        .saturating_add(value_cost(v, depth + 1)?);
                }
                Ok::<_, DwError>(size)
            })?
        }
        _ => 8,
    };
    Ok(cost.saturating_add(32))
}
fn binary(op: &str, left: DwValue, right: DwValue) -> R<DwValue> {
    if op == "==" || op == "!=" {
        return Ok(DwValue::Bool(if op == "==" {
            left == right
        } else {
            left != right
        }));
    }
    if op == ">>" {
        if right != DwValue::String("UTC".into()) {
            return Err(DwError::unsupported(
                "Only explicit UTC timezone shift is implemented",
            ));
        }
        let date = DateTime::parse_from_rfc3339(&left.scalar()?)
            .map_err(|_| DwError::runtime("Invalid DateTime value"))?;
        return Ok(DwValue::String(date.with_timezone(&Utc).to_rfc3339()));
    }
    let (DwValue::Number(a), DwValue::Number(b)) = (left, right) else {
        return Err(DwError::unsupported(format!(
            "Operator {op} for these types is not implemented"
        )));
    };
    Ok(match op {
        "+" => DwValue::Number(a + b),
        "-" => DwValue::Number(a - b),
        "*" => DwValue::Number(a * b),
        "/" => {
            if b.is_zero() {
                return Err(DwError::runtime("Division by zero"));
            }
            DwValue::Number(a / b)
        }
        ">" => DwValue::Bool(a > b),
        "<" => DwValue::Bool(a < b),
        ">=" => DwValue::Bool(a >= b),
        "<=" => DwValue::Bool(a <= b),
        _ => {
            return Err(DwError::unsupported(format!(
                "Operator {op} is not implemented"
            )))
        }
    })
}

pub fn execute(source: &str, inputs: &BTreeMap<String, DwValue>) -> R<DwResult> {
    let script = Parser::new(tokenize(source)?).script()?;
    if !["application/json", "application/apex", "application/xml"]
        .contains(&script.output.as_str())
    {
        return Err(if script.output == "application/xlsx" {
            DwError::runtime("Unknown content type `application/xlsx`")
        } else {
            DwError::unsupported(format!(
                "Output content type {} is not implemented",
                script.output
            ))
        });
    }
    let mut scope = BTreeMap::new();
    let mut input_size = 0usize;
    for input in &script.inputs {
        let value = inputs
            .get(&input.name)
            .ok_or_else(|| DwError::runtime(format!("Missing input {}", input.name)))?;
        input_size = input_size.saturating_add(value_cost(value, 0)?);
        if input_size > 4 * MAX_BYTES {
            return Err(DwError::unsupported("DataWeave input exceeds host bound"));
        }
        let value = match input.media.as_str() {
            "application/java" | "application/apex" => value.clone(),
            "application/json" => {
                if let DwValue::String(text) = value {
                    if text.len() > MAX_BYTES {
                        return Err(DwError::unsupported("JSON input exceeds host bound"));
                    }
                    parse_json_input(text)?
                } else {
                    value.clone()
                }
            }
            "application/csv" => {
                let DwValue::String(text) = value else {
                    return Err(DwError::runtime("CSV input must be String"));
                };
                parse_csv(text, input.separator)?
            }
            _ => {
                return Err(DwError::unsupported(format!(
                    "Input content type {} is not implemented",
                    input.media
                )))
            }
        };
        scope.insert(input.name.clone(), Rc::new(value));
    }
    let mut evaluator = Evaluator {
        script: &script,
        fuel: 100_000,
        depth: 0,
        logs: vec![],
        allocated: 0,
    };
    let value = evaluator.eval(&script.body, &scope)?;
    let text = match script.output.as_str() {
        "application/xml" => render_xml(&value)?,
        _ => render_json(&value, true)?,
    };
    Ok(DwResult {
        value,
        text,
        logs: evaluator.logs,
        media_type: script.output,
    })
}

fn parse_csv(text: &str, separator: char) -> R<DwValue> {
    if text.len() > MAX_BYTES {
        return Err(DwError::unsupported("CSV exceeds 1 MiB"));
    }
    let mut rows = vec![];
    let mut row = vec![];
    let mut value = String::new();
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    let mut after_quote = false;
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    value.push('"');
                } else {
                    quoted = false;
                    after_quote = true;
                }
            } else {
                value.push(c);
            }
            continue;
        }
        if after_quote && c != separator && c != '\r' && c != '\n' {
            return Err(DwError::runtime("Unexpected data after CSV quote"));
        }
        if c == '"' {
            if !value.is_empty() {
                return Err(DwError::runtime("CSV quote inside unquoted field"));
            }
            quoted = true;
        } else if c == separator {
            row.push(std::mem::take(&mut value));
            after_quote = false;
        } else if c == '\r' || c == '\n' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(std::mem::take(&mut value));
            rows.push(std::mem::take(&mut row));
            after_quote = false;
            if rows.len() > MAX_ITEMS + 1 {
                return Err(DwError::unsupported("CSV row bound exceeded"));
            }
        } else {
            value.push(c);
        }
    }
    if quoted {
        return Err(DwError::runtime("Unterminated CSV quoted field"));
    }
    if !row.is_empty() || !value.is_empty() || after_quote {
        row.push(value);
        rows.push(row);
    }
    if rows.is_empty() {
        return Ok(DwValue::Array(vec![]));
    }
    let header = rows.remove(0);
    if header.is_empty()
        || header.iter().any(|s| s.is_empty())
        || header
            .iter()
            .enumerate()
            .any(|(i, s)| header[..i].contains(s))
    {
        return Err(DwError::runtime("CSV headers must be nonempty and unique"));
    }
    let mut result = vec![];
    for row in rows {
        if row.len() != header.len() {
            return Err(DwError::runtime(
                "CSV row has different number of fields from header",
            ));
        }
        result.push(DwValue::Object(
            header
                .iter()
                .cloned()
                .zip(row)
                .map(|(k, v)| DwField::new(k, DwValue::String(v)))
                .collect(),
        ));
    }
    Ok(DwValue::Array(result))
}
fn parse_json_input(text: &str) -> R<DwValue> {
    // Validate strict JSON first, then retain source object order rather than
    // materializing it through serde_json's alphabetically ordered Map.
    serde_json::from_str::<serde_json::Value>(text)
        .map_err(|e| DwError::runtime(format!("Invalid JSON: {e}")))?;
    fn value(parser: &mut Parser, depth: usize) -> R<DwValue> {
        if depth > MAX_DEPTH {
            return Err(DwError::unsupported("JSON nesting exceeds host bound"));
        }
        if parser.symbol('{') {
            let mut fields = vec![];
            if !parser.symbol('}') {
                loop {
                    let Some(Token::String(key)) = parser.tokens.get(parser.at).cloned() else {
                        return Err(DwError::runtime("JSON key must be String"));
                    };
                    parser.at += 1;
                    parser.require(':')?;
                    fields.push(DwField::new(key, value(parser, depth + 1)?));
                    if fields.len() > MAX_ITEMS {
                        return Err(DwError::unsupported("JSON object exceeds host bound"));
                    }
                    if parser.symbol('}') {
                        break;
                    }
                    parser.require(',')?;
                }
            }
            return Ok(DwValue::Object(fields));
        }
        if parser.symbol('[') {
            let mut items = vec![];
            if !parser.symbol(']') {
                loop {
                    items.push(value(parser, depth + 1)?);
                    if items.len() > MAX_ITEMS {
                        return Err(DwError::unsupported("JSON array exceeds host bound"));
                    }
                    if parser.symbol(']') {
                        break;
                    }
                    parser.require(',')?;
                }
            }
            return Ok(DwValue::Array(items));
        }
        let negative = parser.symbol('-');
        let token = parser.tokens.get(parser.at).cloned();
        parser.at += 1;
        match token {
            Some(Token::Number(number)) => DwValue::number(&if negative {
                format!("-{number}")
            } else {
                number
            }),
            Some(Token::String(s)) if !negative => Ok(DwValue::String(s)),
            Some(Token::Word(s)) if !negative && s == "true" => Ok(DwValue::Bool(true)),
            Some(Token::Word(s)) if !negative && s == "false" => Ok(DwValue::Bool(false)),
            Some(Token::Word(s)) if !negative && s == "null" => Ok(DwValue::Null),
            _ => Err(DwError::runtime("Invalid JSON value")),
        }
    }
    let mut parser = Parser::new(tokenize(text)?);
    let out = value(&mut parser, 0)?;
    if parser.at != parser.tokens.len() {
        return Err(DwError::runtime("Trailing JSON data"));
    }
    Ok(out)
}
fn render_json(value: &DwValue, pretty: bool) -> R<String> {
    fn render(v: &DwValue, out: &mut String, depth: usize, pretty: bool) -> R<()> {
        if depth > MAX_DEPTH {
            return Err(DwError::unsupported("JSON depth bound exceeded"));
        }
        match v {
            DwValue::Null => out.push_str("null"),
            DwValue::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
            DwValue::Number(v) => out.push_str(&number_text(v)),
            DwValue::String(v) => out
                .push_str(&serde_json::to_string(v).map_err(|e| DwError::runtime(e.to_string()))?),
            DwValue::Array(items) => {
                out.push('[');
                for (i, value) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    if pretty {
                        out.push('\n');
                        out.push_str(&"  ".repeat(depth + 1));
                    }
                    render(value, out, depth + 1, pretty)?;
                }
                if pretty && !items.is_empty() {
                    out.push('\n');
                    out.push_str(&"  ".repeat(depth));
                }
                out.push(']');
            }
            DwValue::Object(fields) | DwValue::TypedObject { fields, .. } => {
                out.push('{');
                for (i, field) in fields.iter().enumerate() {
                    if !field.attributes.is_empty() {
                        return Err(DwError::unsupported(
                            "XML attributes cannot be serialized as JSON without a policy",
                        ));
                    }
                    if fields[..i].iter().any(|f| f.key == field.key) {
                        return Err(DwError::unsupported(
                            "Duplicate object keys require a JSON writer policy",
                        ));
                    }
                    if i > 0 {
                        out.push(',');
                    }
                    if pretty {
                        out.push('\n');
                        out.push_str(&"  ".repeat(depth + 1));
                    }
                    out.push_str(
                        &serde_json::to_string(&field.key)
                            .map_err(|e| DwError::runtime(e.to_string()))?,
                    );
                    out.push(':');
                    if pretty {
                        out.push(' ');
                    }
                    render(&field.value, out, depth + 1, pretty)?;
                }
                if pretty && !fields.is_empty() {
                    out.push('\n');
                    out.push_str(&"  ".repeat(depth));
                }
                out.push('}');
            }
        }
        if out.len() > MAX_BYTES {
            return Err(DwError::unsupported("JSON output exceeds 1 MiB"));
        }
        Ok(())
    }
    let mut out = String::new();
    render(value, &mut out, 0, pretty)?;
    Ok(out)
}
fn xml_escape(value: &str) -> R<String> {
    if value.chars().any(|c| !matches!(c, '\u{9}'|'\u{a}'|'\u{d}'|'\u{20}'..='\u{d7ff}'|'\u{e000}'..='\u{fffd}'|'\u{10000}'..='\u{10ffff}')) { return Err(DwError::runtime("Invalid XML 1.0 character")); }
    Ok(value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;"))
}
fn xml_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || ['_', '-', '.'].contains(&c))
}
fn render_xml(value: &DwValue) -> R<String> {
    fn field(f: &DwField, out: &mut String, depth: usize) -> R<()> {
        if depth > MAX_DEPTH {
            return Err(DwError::unsupported("XML depth bound exceeded"));
        }
        if !xml_name(&f.key) {
            return Err(DwError::runtime("Invalid XML element name"));
        }
        out.push('<');
        out.push_str(&f.key);
        for (k, v) in &f.attributes {
            if !xml_name(k) {
                return Err(DwError::runtime("Invalid XML attribute name"));
            }
            out.push(' ');
            out.push_str(k);
            out.push_str("=\"");
            out.push_str(&xml_escape(&v.scalar()?)?);
            out.push('"');
        }
        out.push('>');
        if let Some(children) = f.value.fields() {
            for child in children {
                field(child, out, depth + 1)?;
            }
        } else if let DwValue::Array(_) = f.value {
            return Err(DwError::unsupported(
                "Array-valued XML element requires explicit dynamic elements",
            ));
        } else if f.value != DwValue::Null {
            out.push_str(&xml_escape(&f.value.scalar()?)?);
        }
        out.push_str("</");
        out.push_str(&f.key);
        out.push('>');
        if out.len() > MAX_BYTES {
            return Err(DwError::unsupported("XML output exceeds 1 MiB"));
        }
        Ok(())
    }
    let fields = value
        .fields()
        .ok_or_else(|| DwError::runtime("XML output must be an object"))?;
    if fields.len() != 1 {
        return Err(DwError::runtime(
            "XML output requires exactly one root element",
        ));
    }
    let mut out = "<?xml version='1.0' encoding='UTF-8'?>\n".to_string();
    field(&fields[0], &mut out, 0)?;
    Ok(out)
}
fn format_datetime(value: &str, pattern: &str) -> R<String> {
    let date = DateTime::parse_from_rfc3339(value)
        .map_err(|_| DwError::runtime("Invalid DateTime value"))?
        .with_timezone(&Utc);
    let months = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\'' {
            i += 1;
            while i < chars.len() && chars[i] != '\'' {
                out.push(chars[i]);
                i += 1;
            }
            if i == chars.len() {
                return Err(DwError::runtime("Unterminated format literal"));
            }
            i += 1;
            continue;
        }
        let start = i;
        let c = chars[i];
        i += 1;
        while i < chars.len() && chars[i] == c {
            i += 1;
        }
        let n = i - start;
        let token: String = chars[start..i].iter().collect();
        out.push_str(&match token.as_str() {
            "uuuu" | "yyyy" => format!("{:04}", date.year()),
            "MMMM" => months[date.month0() as usize].into(),
            "MMM" => months[date.month0() as usize][..3].into(),
            "MM" => format!("{:02}", date.month()),
            "M" => date.month().to_string(),
            "dd" => format!("{:02}", date.day()),
            "d" => date.day().to_string(),
            "HH" => format!("{:02}", date.hour()),
            "hh" => format!(
                "{:02}",
                if date.hour() % 12 == 0 {
                    12
                } else {
                    date.hour() % 12
                }
            ),
            "mm" => format!("{:02}", date.minute()),
            "ss" => format!("{:02}", date.second()),
            "a" => {
                if date.hour() < 12 {
                    "AM".into()
                } else {
                    "PM".into()
                }
            }
            _ if !c.is_ascii_alphabetic() => std::iter::repeat(c).take(n).collect(),
            _ => {
                return Err(DwError::unsupported(format!(
                    "Date format token {token} is not implemented"
                )))
            }
        });
    }
    Ok(out)
}
fn pluralize(value: &str) -> R<String> {
    if value.is_empty() {
        return Ok(String::new());
    }
    if !value.is_ascii() || value.chars().any(|c| !c.is_ascii_alphabetic()) {
        return Err(DwError::unsupported(
            "Pluralization supports ASCII alphabetic words only",
        ));
    }
    let lower = value.to_ascii_lowercase();
    let irregular = [
        ("person", "people"),
        ("man", "men"),
        ("woman", "women"),
        ("child", "children"),
        ("mouse", "mice"),
        ("goose", "geese"),
        ("tooth", "teeth"),
        ("foot", "feet"),
        ("ox", "oxen"),
        ("die", "dice"),
        ("datum", "data"),
        ("analysis", "analyses"),
        ("basis", "bases"),
        ("crisis", "crises"),
        ("thesis", "theses"),
        ("criterion", "criteria"),
        ("phenomenon", "phenomena"),
    ];
    let result = if ["deer", "sheep", "fish", "species", "series"].contains(&lower.as_str())
        || irregular.iter().any(|(_, plural)| *plural == lower)
    {
        lower.clone()
    } else if let Some((_, plural)) = irregular.iter().find(|(single, _)| *single == lower) {
        (*plural).into()
    } else if lower.ends_with("ies")
        || lower.ends_with("xes")
        || lower.ends_with("ches")
        || lower.ends_with("shes")
        || lower.ends_with("ses")
    {
        lower.clone()
    } else if lower.ends_with('s')
        && !lower.ends_with("ss")
        && !lower.ends_with("us")
        && !lower.ends_with("is")
    {
        lower.clone()
    } else if lower.ends_with('y')
        && lower.len() > 1
        && !"aeiou".contains(lower.as_bytes()[lower.len() - 2] as char)
    {
        format!("{}ies", &lower[..lower.len() - 1])
    } else if lower.ends_with('x')
        || lower.ends_with('s')
        || lower.ends_with("ch")
        || lower.ends_with("sh")
        || lower.ends_with('z')
    {
        format!("{lower}es")
    } else {
        format!("{lower}s")
    };
    Ok(if value.chars().all(|c| c.is_ascii_uppercase()) {
        result.to_ascii_uppercase()
    } else if value.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        let mut chars = result.chars();
        chars.next().unwrap().to_uppercase().collect::<String>() + chars.as_str()
    } else {
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(source: &str, inputs: &[(&str, DwValue)]) -> R<DwResult> {
        execute(
            source,
            &inputs
                .iter()
                .map(|(k, v)| ((*k).into(), v.clone()))
                .collect(),
        )
    }
    #[test]
    fn arithmetic_and_input_are_evaluated_not_named() {
        let out = run(
            "%dw 2.0\ninput payload application/json\n---\npayload map (v) -> {answer: v * 3 + 2}",
            &[("payload", DwValue::String("[1,4]".into()))],
        )
        .unwrap();
        assert!(out.text.contains("14"));
        assert!(out.text.contains("5"));
        assert!(run("%dw 2.0\n---\n1/0", &[])
            .unwrap_err()
            .message
            .starts_with("Division by zero"));
    }
    #[test]
    fn csv_quotes_separator_and_mutated_fields() {
        let out=run("%dw 2.0\ninput data application/csv separator=';'\n---\ndata map (row) -> {name:row.name,changed:row.value}",&[("data",DwValue::String("name;value\r\n\"A;B\";\"quoted \"\"value\"\"\"".into()))]).unwrap();
        assert!(out.text.contains("A;B"));
        assert!(out.text.contains("quoted \\".trim_end_matches(' ')));
        assert!(parse_csv("a,b\n1", ',').is_err());
    }
    #[test]
    fn custom_functions_and_key_matching_are_generic() {
        let script="%dw 2.0\ninput payload application/json\nfun rename(k: Key)=k match {case \"old\" -> \"new\" else -> k}\n---\npayload mapObject (v,k)->{(rename(k)):v}";
        let out = run(
            script,
            &[("payload", DwValue::String("{\"old\":7,\"keep\":8}".into()))],
        )
        .unwrap();
        assert!(out.text.contains("\"new\": 7"));
        assert!(out.text.contains("\"keep\": 8"));
    }
    #[test]
    fn typed_object_annotations_preserve_actual_fields() {
        let out=run("%dw 2.0\ninput records application/json\noutput application/apex\n---\nrecords map(r)->{LastName:r.name} as Object {class:\"Contact\"}",&[("records",DwValue::String("[{\"name\":\"Different\"}]".into()))]).unwrap();
        assert!(
            matches!(&out.value,DwValue::Array(v)if matches!(&v[0],DwValue::TypedObject{class,fields}if class=="Contact"&&fields[0].value==DwValue::String("Different".into())))
        );
    }
    #[test]
    fn filter_logger_and_xml_attributes_follow_values() {
        let out=run("%dw 2.0\ninput payload application/json\noutput application/xml\n---\nitems:{(payload filter ($.winner == true) map (v)->{item @(id:v.id):v.name})}",&[("payload",DwValue::String("[{\"winner\":true,\"id\":2,\"name\":\"A&B\"},{\"winner\":false,\"id\":3,\"name\":\"hidden\"}]".into()))]).unwrap();
        assert!(out.text.contains("<item id=\"2\">A&amp;B</item>"));
        assert!(!out.text.contains("hidden"));
    }
    #[test]
    fn unsupported_constructs_and_recursive_functions_are_bounded() {
        assert!(
            run("%dw 2.0\n---\nunknown(1)", &[])
                .unwrap_err()
                .unsupported
        );
        assert!(
            run("%dw 2.0\nfun loop(x)=loop(x)\n---\nloop(1)", &[])
                .unwrap_err()
                .unsupported
        );
        assert!(run("%dw 2.0\noutput application/xlsx\n---\n[]", &[])
            .unwrap_err()
            .message
            .contains("Unknown content type `application/xlsx`"));
        assert!(
            run("%dw 2.0\noutput application/csv\n---\n[]", &[])
                .unwrap_err()
                .unsupported
        );
    }
    #[test]
    fn datetime_format_is_token_driven() {
        assert_eq!(
            format_datetime("2026-03-04T21:07:08Z", "hh:mm:ss a, MMMM dd, uuuu").unwrap(),
            "09:07:08 PM, March 04, 2026"
        );
        assert_eq!(
            format_datetime("2026-03-04T21:07:08Z", "uuuu-MM-dd HH:mm:ss").unwrap(),
            "2026-03-04 21:07:08"
        );
        assert!(format_datetime("invalid", "yyyy").is_err());
    }
    #[test]
    fn pluralization_applies_regular_and_irregular_language_rules() {
        assert_eq!(pluralize("box").unwrap(), "boxes");
        assert_eq!(pluralize("city").unwrap(), "cities");
        assert_eq!(pluralize("person").unwrap(), "people");
        assert_eq!(pluralize("boxes").unwrap(), "boxes");
        assert_eq!(pluralize("cats").unwrap(), "cats");
        assert_eq!(pluralize("analysis").unwrap(), "analyses");
        assert_eq!(pluralize("bus").unwrap(), "buses");
        assert_eq!(pluralize("").unwrap(), "");
    }
    #[test]
    fn source_and_csv_mutations_change_results_or_fail() {
        let source =
            "%dw 2.0\ninput payload application/csv\n---\npayload map(row)->{chosen:row.first}";
        let inputs = [(
            "payload",
            DwValue::String("first,second\noriginal,mutated".into()),
        )];
        let original = run(source, &inputs).unwrap();
        let changed = run(&source.replace("row.first", "row.second"), &inputs).unwrap();
        assert_ne!(original.value, changed.value);
        assert!(original.text.contains("original"));
        assert!(changed.text.contains("mutated"));
        for invalid in [
            "first,second\nonly-one",
            "first,second\n\"unclosed,value",
            "first,first\n1,2",
        ] {
            let error = run(source, &[("payload", DwValue::String(invalid.into()))]).unwrap_err();
            assert!(
                !error.unsupported,
                "Malformed input must be a catchable data error"
            );
        }
        let arithmetic =
            "%dw 2.0\ninput payload application/json\n---\npayload map(value)->value*3";
        let inputs = [("payload", DwValue::String("[2,5]".into()))];
        assert_ne!(
            run(arithmetic, &inputs).unwrap().value,
            run(&arithmetic.replace("*3", "+3"), &inputs).unwrap().value
        );
    }
    #[test]
    fn dynamic_keys_and_json_object_order_are_not_baked_into_output() {
        let result=run("%dw 2.0\ninput payload application/json\n---\npayload mapObject (value,key,index)->{(key):index}",&[("payload",DwValue::String("{\"z\":1,\"a\":2}".into()))]).unwrap();
        assert_eq!(result.text, "{\n  \"z\": 0,\n  \"a\": 1\n}");
        let result = run(
            "%dw 2.0\ninput payload application/json\n---\npayload map (value)->(value):value",
            &[("payload", DwValue::String("[\"independent\"]".into()))],
        )
        .unwrap();
        assert!(result.text.contains("\"independent\": \"independent\""));
    }
    #[test]
    fn extreme_exponents_flat_expression_trees_and_xml_controls_fail_boundedly() {
        assert!(
            run("%dw 2.0\n---\n1e-2147483648", &[])
                .unwrap_err()
                .unsupported
        );
        let source = format!("%dw 2.0\n---\n{}", vec!["1"; 1000].join("+"));
        assert!(run(&source, &[]).unwrap_err().unsupported);
        assert!(run(
            "%dw 2.0\ninput payload application/json\noutput application/xml\n---\nroot:payload",
            &[("payload", DwValue::String("\"\\u0000\"".into()))]
        )
        .is_err());
        assert!(
            run("%dw 2.0\nfun grow(x)=grow(x*x)\n---\ngrow(1000)", &[])
                .unwrap_err()
                .unsupported
        );
    }
}
