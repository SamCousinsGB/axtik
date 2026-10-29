//! A bounded SOQL parser and evaluator over the owned local record store.
//! Unsupported clauses fail explicitly; they are never absorbed into string literals.
use crate::{
    record_store::{value_equal, DataValue, RecordData, RecordStore, StoreError},
    schema::{FieldKind, FieldSchema},
};
use num_rational::BigRational;
use num_traits::{ToPrimitive, Zero};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};
pub type Bindings = BTreeMap<String, DataValue>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryMode {
    System,
    User,
    SecurityEnforced,
}
#[derive(Clone, Debug)]
pub enum ValueExpr {
    Literal(DataValue),
    Bind(String),
    List(Vec<ValueExpr>),
    Subquery(Box<Query>),
}
#[derive(Clone, Debug)]
pub enum Predicate {
    And(Box<Predicate>, Box<Predicate>),
    Or(Box<Predicate>, Box<Predicate>),
    Not(Box<Predicate>),
    Compare {
        field: String,
        operator: String,
        value: ValueExpr,
    },
}
#[derive(Clone, Debug)]
pub enum Selection {
    Field(String),
    StandardFields,
    Aggregate {
        function: String,
        field: Option<String>,
        alias: String,
    },
    Subquery(Box<Query>),
}
#[derive(Clone, Debug)]
pub struct OrderBy {
    pub field: String,
    pub descending: bool,
    pub nulls_first: bool,
}
#[derive(Clone, Debug)]
pub struct Query {
    pub object: String,
    pub selections: Vec<Selection>,
    pub predicate: Option<Predicate>,
    pub order: Vec<OrderBy>,
    pub limit: Option<ValueExpr>,
    pub offset: Option<ValueExpr>,
    pub all_rows: bool,
    pub mode: QueryMode,
    pub explicit_mode: bool,
}
#[derive(Clone, Debug)]
pub enum QueryResult {
    Count(usize),
    Rows(Vec<RecordData>),
}
/// Every visible object/field and row must come from a resolved security context.
/// An empty policy denies access. System mode is a separate explicit choice.
#[derive(Clone, Debug, Default)]
pub struct AccessPolicy {
    pub readable_fields: BTreeMap<String, BTreeSet<String>>,
    pub visible_record_ids: BTreeSet<String>,
    pub view_all_objects: BTreeSet<String>,
}
#[derive(Clone, Copy)]
pub enum QueryAccess<'a> {
    System,
    User(&'a AccessPolicy),
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Word(String),
    String(String),
    Number(String),
    Symbol(char),
    Operator(String),
}
fn tokens(source: &str) -> Result<Vec<Token>, StoreError> {
    if source.len() > 256 * 1024 {
        return Err(StoreError::unsupported("SOQL exceeds 256KiB"));
    }
    let mut source = source.trim();
    if source.starts_with('[') && source.ends_with(']') {
        source = &source[1..source.len() - 1];
    }
    let mut out = vec![];
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        if c == '/' && chars.peek() == Some(&'/') {
            chars.next();
            for c in chars.by_ref() {
                if c == '\n' || c == '\r' {
                    break;
                }
            }
            continue;
        }
        if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut closed = false;
            while let Some(c) = chars.next() {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    closed = true;
                    break;
                }
            }
            if !closed {
                return Err(StoreError::unsupported("Unterminated SOQL comment"));
            }
            continue;
        }
        if c == '\'' {
            let mut s = String::new();
            let mut closed = false;
            while let Some(c) = chars.next() {
                if c == '\'' {
                    closed = true;
                    break;
                }
                if c == '\\' {
                    let e = chars
                        .next()
                        .ok_or_else(|| StoreError::unsupported("Unterminated SOQL escape"))?;
                    match e {
                        '\'' | '\\' => s.push(e),
                        'n' => s.push('\n'),
                        'r' => s.push('\r'),
                        't' => s.push('\t'),
                        '%' | '_' => {
                            s.push('\\');
                            s.push(e);
                        }
                        _ => return Err(StoreError::unsupported("Unsupported SOQL escape")),
                    }
                } else {
                    s.push(c);
                }
            }
            if !closed {
                return Err(StoreError::unsupported("Unterminated SOQL string"));
            }
            out.push(Token::String(s));
        } else if c.is_ascii_alphabetic() || c == '_' {
            let mut word = String::from(c);
            while chars
                .peek()
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
            {
                word.push(chars.next().unwrap());
            }
            out.push(Token::Word(word));
        } else if c.is_ascii_digit()
            || (c == '-' && chars.peek().is_some_and(|c| c.is_ascii_digit()))
        {
            let mut number = String::from(c);
            while chars
                .peek()
                .is_some_and(|c| c.is_ascii_digit() || *c == '.')
            {
                number.push(chars.next().unwrap());
            }
            out.push(Token::Number(number));
        } else if ['=', '!', '<', '>'].contains(&c) {
            let mut op = String::from(c);
            if chars
                .peek()
                .is_some_and(|p| *p == '=' || (c == '<' && *p == '>'))
            {
                op.push(chars.next().unwrap());
            }
            if !["=", "!=", "<>", "<", ">", "<=", ">="].contains(&op.as_str()) {
                return Err(StoreError::unsupported("Unknown SOQL operator"));
            }
            out.push(Token::Operator(op));
        } else if [',', '(', ')', '.', ':'].contains(&c) {
            out.push(Token::Symbol(c));
        } else {
            return Err(StoreError::unsupported(format!(
                "Unsupported SOQL character {c}"
            )));
        }
        if out.len() > 20_000 {
            return Err(StoreError::unsupported("SOQL exceeds 20000 tokens"));
        }
    }
    Ok(out)
}
struct Parser {
    tokens: Vec<Token>,
    at: usize,
    depth: usize,
    predicate_nodes: usize,
}
impl Parser {
    fn predicate_node(&mut self) -> Result<(), StoreError> {
        self.predicate_nodes += 1;
        if self.predicate_nodes > 128 {
            return Err(StoreError::unsupported("SOQL exceeds 128 predicate nodes"));
        }
        Ok(())
    }
    fn word(&mut self, w: &str) -> bool {
        if matches!(self.tokens.get(self.at),Some(Token::Word(s)) if s.eq_ignore_ascii_case(w)) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn expect_word(&mut self, w: &str) -> Result<(), StoreError> {
        if self.word(w) {
            Ok(())
        } else {
            Err(StoreError::unsupported(format!("Expected SOQL {w}")))
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
    fn expect_symbol(&mut self, c: char) -> Result<(), StoreError> {
        if self.symbol(c) {
            Ok(())
        } else {
            Err(StoreError::unsupported(format!("Expected SOQL {c}")))
        }
    }
    fn ident(&mut self) -> Result<String, StoreError> {
        if let Some(Token::Word(s)) = self.tokens.get(self.at) {
            let s = s.clone();
            self.at += 1;
            Ok(s)
        } else {
            Err(StoreError::unsupported("Expected SOQL identifier"))
        }
    }
    fn path(&mut self) -> Result<String, StoreError> {
        let mut s = self.ident()?;
        let mut count = 1;
        while self.symbol('.') {
            count += 1;
            if count > 6 {
                return Err(StoreError::unsupported(
                    "SOQL relationship traversal exceeds five levels",
                ));
            }
            s.push('.');
            s.push_str(&self.ident()?);
        }
        Ok(s)
    }
    fn value(&mut self) -> Result<ValueExpr, StoreError> {
        if self.symbol(':') {
            let mut path = self.path()?.to_ascii_lowercase();
            if self.symbol('(') {
                self.expect_symbol(')')?;
                path.push_str("()");
            }
            return Ok(ValueExpr::Bind(path));
        }
        if self.symbol('(') {
            if matches!(self.tokens.get(self.at),Some(Token::Word(s))if s.eq_ignore_ascii_case("SELECT"))
            {
                let query = self.query()?;
                self.expect_symbol(')')?;
                return Ok(ValueExpr::Subquery(Box::new(query)));
            }
            self.depth += 1;
            if self.depth > 32 {
                return Err(StoreError::unsupported("SOQL expression nesting exceeds32"));
            }
            let mut values = vec![];
            if !self.symbol(')') {
                loop {
                    values.push(self.value()?);
                    if self.symbol(')') {
                        break;
                    }
                    self.expect_symbol(',')?;
                }
            }
            self.depth -= 1;
            return Ok(ValueExpr::List(values));
        }
        if self.word("null") {
            return Ok(ValueExpr::Literal(DataValue::Null));
        }
        if self.word("true") {
            return Ok(ValueExpr::Literal(DataValue::Boolean(true)));
        }
        if self.word("false") {
            return Ok(ValueExpr::Literal(DataValue::Boolean(false)));
        }
        let t = self.tokens.get(self.at).cloned();
        self.at += 1;
        match t {
            Some(Token::String(s)) => Ok(ValueExpr::Literal(DataValue::Text(s))),
            Some(Token::Number(s)) => Ok(ValueExpr::Literal(DataValue::number(&s)?)),
            _ => Err(StoreError::unsupported(
                "Unsupported SOQL literal or expression",
            )),
        }
    }
    fn predicate(&mut self) -> Result<Predicate, StoreError> {
        self.or()
    }
    fn or(&mut self) -> Result<Predicate, StoreError> {
        let mut p = self.and()?;
        while self.word("OR") {
            self.predicate_node()?;
            p = Predicate::Or(Box::new(p), Box::new(self.and()?));
        }
        Ok(p)
    }
    fn and(&mut self) -> Result<Predicate, StoreError> {
        let mut p = self.atom()?;
        while self.word("AND") {
            self.predicate_node()?;
            p = Predicate::And(Box::new(p), Box::new(self.atom()?));
        }
        Ok(p)
    }
    fn atom(&mut self) -> Result<Predicate, StoreError> {
        self.depth += 1;
        if self.depth > 32 {
            return Err(StoreError::unsupported("SOQL predicate depth exceeds32"));
        }
        let result = (|| {
            if self.word("NOT") {
                self.predicate_node()?;
                return Ok(Predicate::Not(Box::new(self.atom()?)));
            }
            if self.symbol('(') {
                let p = self.predicate()?;
                self.expect_symbol(')')?;
                return Ok(p);
            }
            let field = self.path()?;
            self.predicate_node()?;
            let op = if self.word("NOT") {
                self.expect_word("IN")?;
                "NOT IN".into()
            } else if self.word("IN") {
                "IN".into()
            } else if self.word("LIKE") {
                "LIKE".into()
            } else if let Some(Token::Operator(op)) = self.tokens.get(self.at) {
                let op = op.clone();
                self.at += 1;
                op
            } else {
                return Err(StoreError::unsupported(
                    "Unsupported SOQL predicate operator",
                ));
            };
            Ok(Predicate::Compare {
                field,
                operator: op,
                value: self.value()?,
            })
        })();
        self.depth -= 1;
        result
    }
    fn query(&mut self) -> Result<Query, StoreError> {
        self.depth += 1;
        if self.depth > 8 {
            return Err(StoreError::unsupported("SOQL subquery nesting exceeds8"));
        }
        self.expect_word("SELECT")?;
        let mut selections = vec![];
        loop {
            if self.symbol('(') {
                let q = self.query()?;
                self.expect_symbol(')')?;
                selections.push(Selection::Subquery(Box::new(q)));
            } else {
                let field = self.path()?;
                if self.symbol('(') {
                    let function = field.to_ascii_uppercase();
                    if function == "FIELDS" {
                        if !self.word("STANDARD") {
                            return Err(StoreError::unsupported(
                                "Apex supports the bounded FIELDS(STANDARD) group only",
                            ));
                        }
                        self.expect_symbol(')')?;
                        selections.push(Selection::StandardFields);
                        if self.symbol(',') {
                            continue;
                        }
                        break;
                    }
                    if !["COUNT", "COUNT_DISTINCT", "SUM", "AVG", "MIN", "MAX"]
                        .contains(&function.as_str())
                    {
                        return Err(StoreError::unsupported(format!(
                            "Aggregate {function} unsupported"
                        )));
                    }
                    let field = if self.symbol(')') {
                        None
                    } else {
                        let f = self.path()?;
                        self.expect_symbol(')')?;
                        Some(f)
                    };
                    if field.is_none() && function != "COUNT" {
                        return Err(StoreError::unsupported("Only COUNT accepts no field"));
                    }
                    let alias = if matches!(self.tokens.get(self.at),Some(Token::Word(s)) if !s.eq_ignore_ascii_case("FROM"))
                    {
                        self.ident()?
                    } else {
                        format!("expr{}", selections.len())
                    };
                    selections.push(Selection::Aggregate {
                        function,
                        field,
                        alias,
                    });
                } else {
                    selections.push(Selection::Field(field));
                }
            }
            if !self.symbol(',') {
                break;
            }
        }
        self.expect_word("FROM")?;
        let object = self.path()?;
        let mut q = Query {
            object,
            selections,
            predicate: None,
            order: vec![],
            limit: None,
            offset: None,
            all_rows: false,
            mode: QueryMode::System,
            explicit_mode: false,
        };
        if self.word("WHERE") {
            q.predicate = Some(self.predicate()?);
        }
        if self.word("WITH") {
            q.explicit_mode = true;
            q.mode = if self.word("USER_MODE") {
                QueryMode::User
            } else if self.word("SECURITY_ENFORCED") {
                QueryMode::SecurityEnforced
            } else if self.word("SYSTEM_MODE") {
                QueryMode::System
            } else {
                return Err(StoreError::unsupported("Unsupported WITH clause"));
            };
        }
        if self.word("ORDER") {
            self.expect_word("BY")?;
            loop {
                let field = self.path()?;
                let descending = if self.word("DESC") {
                    true
                } else {
                    self.word("ASC");
                    false
                };
                let nulls_first = if self.word("NULLS") {
                    if self.word("FIRST") {
                        true
                    } else {
                        self.expect_word("LAST")?;
                        false
                    }
                } else {
                    true
                };
                q.order.push(OrderBy {
                    field,
                    descending,
                    nulls_first,
                });
                if !self.symbol(',') {
                    break;
                }
            }
        }
        if self.word("LIMIT") {
            q.limit = Some(self.value()?);
        }
        if self.word("OFFSET") {
            q.offset = Some(self.value()?);
        }
        if self.word("ALL") {
            self.expect_word("ROWS")?;
            q.all_rows = true;
        }
        self.depth -= 1;
        Ok(q)
    }
}
pub fn parse(source: &str) -> Result<Query, StoreError> {
    let mut p = Parser {
        tokens: tokens(source)?,
        at: 0,
        depth: 0,
        predicate_nodes: 0,
    };
    let q = p.query()?;
    if p.at != p.tokens.len() {
        return Err(StoreError::unsupported(format!(
            "Unsupported trailing SOQL clause at token {}",
            p.at
        )));
    }
    Ok(q)
}
fn validate_semijoin_layout(
    p: &Predicate,
    restricted: bool,
    count: &mut usize,
) -> Result<(), StoreError> {
    match p {
        Predicate::And(a, b) => {
            validate_semijoin_layout(a, restricted, count)?;
            validate_semijoin_layout(b, restricted, count)?;
        }
        Predicate::Or(a, b) => {
            validate_semijoin_layout(a, true, count)?;
            validate_semijoin_layout(b, true, count)?;
        }
        Predicate::Not(p) => validate_semijoin_layout(p, true, count)?,
        Predicate::Compare {
            value: ValueExpr::Subquery(child),
            ..
        } => {
            *count += 1;
            if restricted || *count > 2 {
                return Err(StoreError::new(
                    "MALFORMED_QUERY",
                    "Semi-joins must be top-level AND conditions, at most two",
                    vec![],
                ));
            }
            if let Some(p) = &child.predicate {
                let mut inner = 0;
                validate_semijoin_layout(p, true, &mut inner)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn bind(v: &ValueExpr, bindings: &Bindings) -> Result<DataValue, StoreError> {
    match v {
        ValueExpr::Literal(v) => Ok(v.clone()),
        ValueExpr::Bind(name) => bindings
            .get(&name.to_ascii_lowercase())
            .cloned()
            .ok_or_else(|| {
                StoreError::new(
                    "QUERY_BIND",
                    format!("Unresolved query bind {name}"),
                    vec![],
                )
            }),
        ValueExpr::List(v) => v
            .iter()
            .map(|v| bind(v, bindings))
            .collect::<Result<Vec<_>, _>>()
            .map(DataValue::List),
        ValueExpr::Subquery(_) => Err(StoreError::unsupported("Unresolved semi-join subquery")),
    }
}
fn bound(
    v: &Option<ValueExpr>,
    bindings: &Bindings,
    default: usize,
    maximum: usize,
) -> Result<usize, StoreError> {
    if let Some(v) = v {
        let DataValue::Number(n) = bind(v, bindings)? else {
            return Err(StoreError::new(
                "MALFORMED_QUERY",
                "Query bound must be Integer",
                vec![],
            ));
        };
        if !n.is_integer() {
            return Err(StoreError::new(
                "MALFORMED_QUERY",
                "Query bound must be Integer",
                vec![],
            ));
        }
        let n = n.to_integer().to_usize().ok_or_else(|| {
            StoreError::new("MALFORMED_QUERY", "Query bound is out of range", vec![])
        })?;
        if n > maximum {
            return Err(StoreError::unsupported(format!(
                "Query bound exceeds {maximum}"
            )));
        }
        Ok(n)
    } else {
        Ok(default)
    }
}
fn charge_query_rows(total: &mut usize, count: usize) -> Result<(), StoreError> {
    *total = total.saturating_add(count);
    if *total > 50_000 {
        return Err(StoreError::governor(
            crate::record_store::GovernorLimit::QueryRows,
            *total,
        ));
    }
    Ok(())
}
fn validate_comparison(
    kind: &FieldKind,
    operator: &str,
    value: &DataValue,
) -> Result<(), StoreError> {
    if matches!(
        kind,
        FieldKind::Blob | FieldKind::MultiPicklist | FieldKind::Unsupported(_)
    ) {
        return Err(StoreError::unsupported(format!(
            "Query comparison for field type {kind:?} is not implemented"
        )));
    }
    if operator == "IN"
        || operator == "NOT IN"
        || (["=", "!=", "<>"].contains(&operator) && matches!(value, DataValue::List(_)))
    {
        let DataValue::List(values) = value else {
            return Err(StoreError::new(
                "MALFORMED_QUERY",
                "IN requires a list bind or literal list",
                vec![],
            ));
        };
        if values.len() > 10_000 {
            return Err(StoreError::unsupported("SOQL IN exceeds 10000 values"));
        }
        for item in values {
            validate_comparison(kind, "=", item)?;
        }
        return Ok(());
    }
    if operator == "LIKE" {
        if !matches!(kind, FieldKind::Text) {
            return Err(StoreError::new(
                "MALFORMED_QUERY",
                "LIKE requires a text field",
                vec![],
            ));
        }
        let DataValue::Text(pattern) = value else {
            return Err(StoreError::new(
                "MALFORMED_QUERY",
                "LIKE requires text operands",
                vec![],
            ));
        };
        like_pattern(pattern)?;
        return Ok(());
    }
    if !["=", "!=", "<>", "<", ">", "<=", ">="].contains(&operator) {
        return Err(StoreError::unsupported("Unknown comparison operator"));
    }
    if matches!(kind, FieldKind::Boolean) && !["=", "!=", "<>"].contains(&operator) {
        return Err(StoreError::new(
            "MALFORMED_QUERY",
            "Boolean fields support equality comparisons",
            vec![],
        ));
    }
    if value.is_null() {
        return Ok(());
    }
    let compatible = matches!(
        (kind, value),
        (FieldKind::Boolean, DataValue::Boolean(_))
            | (
                FieldKind::Integer | FieldKind::Long | FieldKind::Decimal,
                DataValue::Number(_)
            )
            | (
                FieldKind::Text
                    | FieldKind::Picklist
                    | FieldKind::Id
                    | FieldKind::Reference
                    | FieldKind::AutoNumber,
                DataValue::Text(_)
            )
            | (FieldKind::Date, DataValue::Date(_))
            | (FieldKind::DateTime, DataValue::DateTime(_))
    );
    if compatible {
        Ok(())
    } else {
        Err(StoreError::new(
            "MALFORMED_QUERY",
            "Incompatible query bind/literal type",
            vec![],
        ))
    }
}
fn like_pattern(pattern: &str) -> Result<regex::Regex, StoreError> {
    if !pattern.is_ascii() {
        return Err(StoreError::unsupported(
            "Non-ASCII LIKE requires locale policy",
        ));
    }
    if pattern.len() > 256 * 1024 {
        return Err(StoreError::unsupported("LIKE pattern exceeds 256KiB"));
    }
    let mut regex = String::from("(?is)^");
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        match c {
            '%' => regex.push_str(".*"),
            '_' => regex.push('.'),
            '\\' => {
                let escaped = chars.next().ok_or_else(|| {
                    StoreError::new("MALFORMED_QUERY", "Unterminated LIKE escape", vec![])
                })?;
                regex.push_str(&regex::escape(&escaped.to_string()));
            }
            _ => regex.push_str(&regex::escape(&c.to_string())),
        }
    }
    regex.push('$');
    regex::Regex::new(&regex)
        .map_err(|_| StoreError::unsupported("LIKE pattern compilation failed"))
}
impl RecordStore {
    pub fn query(
        &mut self,
        query: &Query,
        bindings: &Bindings,
        access: QueryAccess<'_>,
    ) -> Result<QueryResult, StoreError> {
        // Validate the entire query before scanning rows. Empty tables, LIMIT 0
        // and short-circuited predicates must not hide bad binds or child fields.
        let mut resolved = query.clone();
        self.expand_standard_fields(&mut resolved, access)?;
        self.resolve_semijoins(&mut resolved, bindings, access, 0)?;
        let query = &resolved;
        self.validate_query(query, bindings, access, 0, &mut 0)?;
        let mut queried_rows = self.query_rows;
        let result = self.query_inner(query, bindings, access, None, 0, &mut queried_rows);
        self.query_rows = queried_rows;
        result
    }
    fn expand_standard_fields(
        &self,
        query: &mut Query,
        access: QueryAccess<'_>,
    ) -> Result<(), StoreError> {
        let mut fields = vec![];
        for selection in std::mem::take(&mut query.selections) {
            match selection {
                Selection::StandardFields => {
                    let object = self.schema.object(&query.object).ok_or_else(|| {
                        StoreError::unsupported("FIELDS object schema unavailable")
                    })?;
                    for field in object.fields.values().filter(|f| !f.name.ends_with("__c")) {
                        if let QueryAccess::User(policy) = access {
                            if !policy
                                .readable_fields
                                .get(&object.name.to_ascii_lowercase())
                                .is_some_and(|fields| {
                                    fields.contains(&field.name.to_ascii_lowercase())
                                })
                            {
                                continue;
                            }
                        }
                        fields.push(Selection::Field(field.name.clone()));
                    }
                }
                Selection::Subquery(mut child) => {
                    let relationship = child.object.clone();
                    child.object = self.child_relation(&query.object, &relationship)?.0;
                    self.expand_standard_fields(&mut child, access)?;
                    child.object = relationship;
                    fields.push(Selection::Subquery(child));
                }
                other => fields.push(other),
            }
        }
        query.selections = fields;
        Ok(())
    }
    fn resolve_semijoins(
        &mut self,
        query: &mut Query,
        bindings: &Bindings,
        access: QueryAccess<'_>,
        depth: usize,
    ) -> Result<(), StoreError> {
        if depth > 5 {
            return Err(StoreError::unsupported("Semi-join nesting exceeds five"));
        }
        if let Some(p) = &query.predicate {
            let mut count = 0;
            validate_semijoin_layout(p, false, &mut count)?;
        }
        if let Some(predicate) = &mut query.predicate {
            self.resolve_semijoin_predicate(
                predicate,
                &query.object,
                &query.mode,
                bindings,
                access,
                depth,
            )?;
        }
        for selection in &mut query.selections {
            if let Selection::Subquery(child) = selection {
                self.resolve_semijoins(child, bindings, access, depth + 1)?;
            }
        }
        Ok(())
    }
    fn resolve_semijoin_predicate(
        &mut self,
        predicate: &mut Predicate,
        object: &str,
        mode: &QueryMode,
        bindings: &Bindings,
        access: QueryAccess<'_>,
        depth: usize,
    ) -> Result<(), StoreError> {
        match predicate {
            Predicate::And(a, b) | Predicate::Or(a, b) => {
                self.resolve_semijoin_predicate(a, object, mode, bindings, access, depth)?;
                self.resolve_semijoin_predicate(b, object, mode, bindings, access, depth)?;
            }
            Predicate::Not(p) => {
                self.resolve_semijoin_predicate(p, object, mode, bindings, access, depth)?
            }
            Predicate::Compare {
                field,
                operator,
                value: ValueExpr::Subquery(child),
            } => {
                if !["IN", "NOT IN"].contains(&operator.as_str())
                    || child.object.eq_ignore_ascii_case(object)
                    || child.selections.len() != 1
                    || !child.order.is_empty()
                    || child.limit.is_some()
                    || child.offset.is_some()
                {
                    return Err(StoreError::new(
                        "MALFORMED_QUERY",
                        "Invalid semi-join shape",
                        vec![],
                    ));
                }
                let Selection::Field(selected) = &child.selections[0] else {
                    return Err(StoreError::new(
                        "MALFORMED_QUERY",
                        "Semi-join must select one Id/reference field",
                        vec![],
                    ));
                };
                let selected = selected.clone();
                let outer = self
                    .schema
                    .field(object, field)
                    .ok_or_else(|| StoreError::unsupported("Semi-join outer field missing"))?;
                let inner = self
                    .schema
                    .field(&child.object, &selected)
                    .ok_or_else(|| StoreError::unsupported("Semi-join inner field missing"))?;
                if !matches!(outer.kind, FieldKind::Id | FieldKind::Reference)
                    || !matches!(inner.kind, FieldKind::Id | FieldKind::Reference)
                {
                    return Err(StoreError::new(
                        "MALFORMED_QUERY",
                        "Semi-join requires Id/reference fields",
                        vec![],
                    ));
                }
                if !child.explicit_mode {
                    child.mode = mode.clone();
                }
                self.resolve_semijoins(child, bindings, access, depth + 1)?;
                self.validate_query(child, bindings, access, depth + 1, &mut 0)?;
                let mut rows = self.query_rows;
                let result = self.query_inner(child, bindings, access, None, depth + 1, &mut rows);
                self.query_rows = rows;
                let QueryResult::Rows(results) = result? else {
                    return Err(StoreError::unsupported("Semi-join aggregate unsupported"));
                };
                let values = results
                    .into_iter()
                    .filter_map(|r| r.get(&selected).cloned())
                    .filter(|v| !v.is_null())
                    .map(ValueExpr::Literal)
                    .collect();
                if let Predicate::Compare { value, .. } = predicate {
                    *value = ValueExpr::List(values);
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn query_inner(
        &self,
        q: &Query,
        bindings: &Bindings,
        access: QueryAccess<'_>,
        restriction: Option<(&str, &str)>,
        depth: usize,
        queried_rows: &mut usize,
    ) -> Result<QueryResult, StoreError> {
        if depth > 5 {
            return Err(StoreError::unsupported(
                "Relationship subquery depth exceeds5",
            ));
        }
        let object = self.schema.object(&q.object).ok_or_else(|| {
            StoreError::unsupported(format!("Object schema {} is unavailable", q.object))
        })?;
        if q.mode != QueryMode::System && matches!(access, QueryAccess::System) {
            return Err(StoreError::unsupported(
                "User/security query requires resolved CRUD/FLS/sharing policy",
            ));
        }
        let effective_access = if q.mode == QueryMode::System {
            QueryAccess::System
        } else {
            access
        };
        if let QueryAccess::User(policy) = effective_access {
            if !policy
                .readable_fields
                .contains_key(&object.name.to_ascii_lowercase())
            {
                return Err(StoreError::new(
                    "INSUFFICIENT_ACCESS",
                    format!("sObject type '{}' is not supported", object.name),
                    vec![],
                ));
            }
        }
        for s in &q.selections {
            match s {
                Selection::Field(f) => {
                    self.validate_query_field(&q.object, f, effective_access)?;
                }
                Selection::Aggregate { field: Some(f), .. } => {
                    self.validate_query_field(&q.object, f, effective_access)?;
                }
                _ => {}
            }
        }
        for order in &q.order {
            self.validate_query_field(&q.object, &order.field, effective_access)?;
        }
        let mut rows = vec![];
        for row in self.records(&q.object, q.all_rows) {
            if let Some((field, id)) = restriction {
                if row.get(field).and_then(DataValue::text) != Some(id) {
                    continue;
                }
            }
            if let QueryAccess::User(policy) = effective_access {
                if !policy
                    .view_all_objects
                    .contains(&q.object.to_ascii_lowercase())
                    && !row
                        .id()
                        .is_some_and(|id| policy.visible_record_ids.contains(id))
                {
                    continue;
                }
            }
            if q.predicate
                .as_ref()
                .map(|p| self.matches(row, p, bindings))
                .transpose()?
                .unwrap_or(true)
            {
                rows.push(row);
            }
        }
        if !q.order.is_empty() {
            let mut keyed = rows
                .into_iter()
                .map(|row| {
                    let keys = q
                        .order
                        .iter()
                        .map(|o| self.resolve_field(row, &o.field))
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok((row, keys))
                })
                .collect::<Result<Vec<_>, StoreError>>()?;
            let mut failure = None;
            keyed.sort_by(|a, b| {
                for (i, o) in q.order.iter().enumerate() {
                    let left = &a.1[i];
                    let right = &b.1[i];
                    let ordering = if left.is_null() || right.is_null() {
                        match (left.is_null(), right.is_null()) {
                            (true, true) => Ordering::Equal,
                            (true, false) => {
                                if o.nulls_first {
                                    Ordering::Less
                                } else {
                                    Ordering::Greater
                                }
                            }
                            (false, true) => {
                                if o.nulls_first {
                                    Ordering::Greater
                                } else {
                                    Ordering::Less
                                }
                            }
                            _ => unreachable!(),
                        }
                    } else {
                        match compare(left, right) {
                            Ok(v) => {
                                if o.descending {
                                    v.reverse()
                                } else {
                                    v
                                }
                            }
                            Err(e) => {
                                failure = Some(e);
                                Ordering::Equal
                            }
                        }
                    };
                    if ordering != Ordering::Equal {
                        return ordering;
                    }
                }
                Ordering::Equal
            });
            if let Some(e) = failure {
                return Err(e);
            }
            rows = keyed.into_iter().map(|x| x.0).collect();
        }
        let limit = bound(&q.limit, bindings, 50_000, 50_000)?;
        let offset = bound(&q.offset, bindings, 0, 2_000)?;
        let aggregate = q
            .selections
            .iter()
            .any(|s| matches!(s, Selection::Aggregate { .. }));
        if aggregate {
            if q.selections
                .iter()
                .any(|s| !matches!(s, Selection::Aggregate { .. }))
            {
                return Err(StoreError::unsupported(
                    "Mixed aggregate and field selection requires GROUP BY",
                ));
            }
            if !q.order.is_empty() || q.offset.is_some() {
                return Err(StoreError::unsupported(
                    "Aggregate ORDER/OFFSET semantics are not implemented",
                ));
            }
            if q.selections.len() == 1
                && matches!(&q.selections[0],Selection::Aggregate{function,field:None,..} if function=="COUNT")
            {
                charge_query_rows(queried_rows, rows.len())?;
                return Ok(QueryResult::Count(rows.len()));
            }
            let mut result = RecordData::new("AggregateResult");
            for s in &q.selections {
                let Selection::Aggregate {
                    function,
                    field,
                    alias,
                } = s
                else {
                    unreachable!()
                };
                let mut values = vec![];
                if let Some(field) = field {
                    for row in &rows {
                        let v = self.resolve_field(row, field)?;
                        if !v.is_null() {
                            values.push(v);
                        }
                    }
                }
                let value = match function.as_str() {
                    "COUNT" => DataValue::integer(if field.is_some() {
                        values.len()
                    } else {
                        rows.len()
                    } as i64),
                    "COUNT_DISTINCT" => {
                        let mut distinct = vec![];
                        for v in values {
                            let mut exists = false;
                            for old in &distinct {
                                if value_equal(old, &v, false)? {
                                    exists = true;
                                    break;
                                }
                            }
                            if !exists {
                                distinct.push(v);
                            }
                        }
                        DataValue::integer(distinct.len() as i64)
                    }
                    "SUM" | "AVG" => {
                        if values.is_empty() {
                            DataValue::Null
                        } else {
                            let count = values.len();
                            let mut sum = BigRational::zero();
                            for v in values {
                                let DataValue::Number(n) = v else {
                                    return Err(StoreError::new(
                                        "MALFORMED_QUERY",
                                        "SUM/AVG requires a numeric field",
                                        vec![],
                                    ));
                                };
                                sum += n;
                            }
                            if function == "AVG" {
                                sum /= BigRational::from_integer(count.into());
                            }
                            DataValue::Number(sum)
                        }
                    }
                    "MIN" | "MAX" => {
                        let mut best = DataValue::Null;
                        for v in values {
                            if best.is_null()
                                || (function == "MIN" && compare(&v, &best)? == Ordering::Less)
                                || (function == "MAX" && compare(&v, &best)? == Ordering::Greater)
                            {
                                best = v;
                            }
                        }
                        best
                    }
                    _ => return Err(StoreError::unsupported("Unknown aggregate")),
                };
                result.set(alias, value);
            }
            charge_query_rows(queried_rows, usize::from(limit != 0))?;
            return Ok(QueryResult::Rows(if limit == 0 {
                vec![]
            } else {
                vec![result]
            }));
        }
        let mut results = vec![];
        for row in rows.into_iter().skip(offset).take(limit) {
            charge_query_rows(queried_rows, 1)?;
            let mut projected = RecordData::new(&row.object);
            if let Some(id) = row.id() {
                projected.set("Id", DataValue::Text(id.into()));
            }
            for s in &q.selections {
                match s {
                    Selection::Field(path) => self.project_field(row, &mut projected, path)?,
                    Selection::Subquery(child) => {
                        let (child_object, field) =
                            self.child_relation(&row.object, &child.object)?;
                        let mut child = (**child).clone();
                        child.object = child_object;
                        child.mode = q.mode.clone();
                        let result = self.query_inner(
                            &child,
                            bindings,
                            effective_access,
                            Some((
                                &field,
                                row.id().ok_or_else(|| {
                                    StoreError::unsupported("Parent query row has no Id")
                                })?,
                            )),
                            depth + 1,
                            queried_rows,
                        )?;
                        let QueryResult::Rows(children) = result else {
                            return Err(StoreError::unsupported(
                                "Aggregate relationship subqueries unsupported",
                            ));
                        };
                        let relationship = if let Selection::Subquery(original) = s {
                            &original.object
                        } else {
                            unreachable!()
                        };
                        projected.set(
                            relationship.rsplit('.').next().unwrap_or(relationship),
                            DataValue::List(
                                children
                                    .into_iter()
                                    .map(|r| DataValue::Record(Box::new(r)))
                                    .collect(),
                            ),
                        );
                    }
                    _ => unreachable!(),
                }
            }
            results.push(projected);
        }
        Ok(QueryResult::Rows(results))
    }
    fn validate_query(
        &self,
        query: &Query,
        bindings: &Bindings,
        access: QueryAccess<'_>,
        depth: usize,
        predicate_nodes: &mut usize,
    ) -> Result<(), StoreError> {
        if depth > 5 {
            return Err(StoreError::unsupported(
                "Relationship subquery depth exceeds5",
            ));
        }
        let object = self.schema.object(&query.object).ok_or_else(|| {
            StoreError::unsupported(format!("Object schema {} is unavailable", query.object))
        })?;
        if object.platform_event {
            return Err(StoreError::new(
                "INVALID_TYPE",
                "Platform events cannot be queried using SOQL",
                vec![],
            ));
        }
        if query.mode != QueryMode::System && matches!(access, QueryAccess::System) {
            return Err(StoreError::unsupported(
                "User/security query requires resolved CRUD/FLS/sharing policy",
            ));
        }
        let effective = if query.mode == QueryMode::System {
            QueryAccess::System
        } else {
            access
        };
        if let QueryAccess::User(policy) = effective {
            if !policy
                .readable_fields
                .contains_key(&object.name.to_ascii_lowercase())
            {
                return Err(StoreError::new(
                    "INSUFFICIENT_ACCESS",
                    format!("sObject type '{}' is not supported", object.name),
                    vec![],
                ));
            }
        }
        if query.selections.len() > 128 {
            return Err(StoreError::unsupported(
                "SOQL exceeds 128 selected expressions",
            ));
        }
        let aggregate = query
            .selections
            .iter()
            .any(|s| matches!(s, Selection::Aggregate { .. }));
        if aggregate
            && query
                .selections
                .iter()
                .any(|s| !matches!(s, Selection::Aggregate { .. }))
        {
            return Err(StoreError::unsupported(
                "Mixed aggregate and field selection requires GROUP BY",
            ));
        }
        let mut aliases = BTreeSet::new();
        for selection in &query.selections {
            match selection {
                Selection::Field(field) => {
                    self.validate_query_field(&query.object, field, effective)?;
                }
                Selection::Aggregate {
                    function,
                    field,
                    alias,
                } => {
                    if !aliases.insert(alias.to_ascii_lowercase()) {
                        return Err(StoreError::new(
                            "MALFORMED_QUERY",
                            "Duplicate aggregate alias",
                            vec![alias.clone()],
                        ));
                    }
                    if let Some(field) = field {
                        let schema = self.validate_query_field(&query.object, field, effective)?;
                        match function.as_str() {
                            "SUM" | "AVG"
                                if !matches!(
                                    schema.kind,
                                    FieldKind::Integer | FieldKind::Long | FieldKind::Decimal
                                ) =>
                            {
                                return Err(StoreError::new(
                                    "MALFORMED_QUERY",
                                    "SUM/AVG requires a numeric field",
                                    vec![field.clone()],
                                ))
                            }
                            "MIN" | "MAX" | "COUNT_DISTINCT"
                                if matches!(
                                    schema.kind,
                                    FieldKind::Blob
                                        | FieldKind::MultiPicklist
                                        | FieldKind::Unsupported(_)
                                ) =>
                            {
                                return Err(StoreError::unsupported(format!(
                                    "Aggregate over field type {:?} is not implemented",
                                    schema.kind
                                )))
                            }
                            "COUNT" | "COUNT_DISTINCT" | "SUM" | "AVG" | "MIN" | "MAX" => {}
                            _ => return Err(StoreError::unsupported("Unknown aggregate")),
                        }
                    } else if function != "COUNT" {
                        return Err(StoreError::new(
                            "MALFORMED_QUERY",
                            "Only COUNT accepts no field",
                            vec![],
                        ));
                    }
                }
                Selection::Subquery(child) => {
                    if child
                        .selections
                        .iter()
                        .any(|s| matches!(s, Selection::Aggregate { .. }))
                    {
                        return Err(StoreError::unsupported(
                            "Aggregate relationship subqueries unsupported",
                        ));
                    }
                    let (object, _) = self.child_relation(&query.object, &child.object)?;
                    let mut child = (**child).clone();
                    child.object = object;
                    child.mode = query.mode.clone();
                    self.validate_query(&child, bindings, effective, depth + 1, predicate_nodes)?;
                }
                Selection::StandardFields => {
                    return Err(StoreError::unsupported("FIELDS group was not expanded"))
                }
            }
        }
        for order in &query.order {
            self.validate_query_field(&query.object, &order.field, effective)?;
        }
        bound(&query.limit, bindings, 50_000, 50_000)?;
        bound(&query.offset, bindings, 0, 2_000)?;
        if let Some(predicate) = &query.predicate {
            self.validate_predicate(
                &query.object,
                predicate,
                bindings,
                effective,
                predicate_nodes,
            )?;
        }
        Ok(())
    }
    fn validate_predicate(
        &self,
        object: &str,
        p: &Predicate,
        bindings: &Bindings,
        access: QueryAccess<'_>,
        nodes: &mut usize,
    ) -> Result<(), StoreError> {
        *nodes += 1;
        if *nodes > 128 {
            return Err(StoreError::unsupported("SOQL exceeds 128 predicate nodes"));
        }
        match p {
            Predicate::And(a, b) | Predicate::Or(a, b) => {
                self.validate_predicate(object, a, bindings, access, nodes)?;
                self.validate_predicate(object, b, bindings, access, nodes)
            }
            Predicate::Not(p) => self.validate_predicate(object, p, bindings, access, nodes),
            Predicate::Compare {
                field,
                operator,
                value,
            } => {
                let schema = self.validate_query_field(object, field, access)?;
                let value = reference_bind(&schema.kind, bind(value, bindings)?);
                validate_comparison(&schema.kind, operator, &value)
            }
        }
    }
    fn validate_query_field(
        &self,
        object: &str,
        path: &str,
        access: QueryAccess<'_>,
    ) -> Result<&FieldSchema, StoreError> {
        let mut object = object.to_string();
        let parts = path.split('.').collect::<Vec<_>>();
        for (i, part) in parts.iter().enumerate() {
            let schema = self
                .schema
                .object(&object)
                .ok_or_else(|| StoreError::unsupported(format!("Unknown object {object}")))?;
            let field = if i + 1 == parts.len() {
                schema.field(part)
            } else {
                schema.fields.values().find(|f| {
                    f.relationship_name
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(part))
                })
            }
            .ok_or_else(|| {
                StoreError::new(
                    "INVALID_FIELD",
                    format!("Unknown query field {object}.{part}"),
                    vec![path.into()],
                )
            })?;
            if field.formula.is_some() {
                return Err(StoreError::unsupported(format!(
                    "Formula field {} requires formula evaluation",
                    field.name
                )));
            }
            if let QueryAccess::User(p) = access {
                let fields = p
                    .readable_fields
                    .get(&object.to_ascii_lowercase())
                    .ok_or_else(|| {
                        StoreError::new(
                            "INSUFFICIENT_ACCESS",
                            "Relationship object is not readable",
                            vec![path.into()],
                        )
                    })?;
                if !fields.contains("*") && !fields.contains(&field.name.to_ascii_lowercase()) {
                    return Err(StoreError::new(
                        "INSUFFICIENT_ACCESS",
                        format!("Field {}.{} is not readable", object, field.name),
                        vec![path.into()],
                    ));
                }
            }
            if i + 1 < parts.len() {
                if field.reference_to.len() != 1 {
                    return Err(StoreError::unsupported(
                        "Polymorphic relationship traversal requires TYPEOF semantics",
                    ));
                }
                object = field.reference_to[0].clone();
            } else {
                return Ok(field);
            }
        }
        Err(StoreError::new(
            "INVALID_FIELD",
            "Empty query field path",
            vec![],
        ))
    }
    fn relationship<'a>(
        &'a self,
        row: &RecordData,
        name: &str,
    ) -> Result<Option<&'a RecordData>, StoreError> {
        let schema = self
            .schema
            .object(&row.object)
            .ok_or_else(|| StoreError::unsupported("Unknown row object"))?;
        let field = schema
            .fields
            .values()
            .find(|f| {
                f.relationship_name
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(name))
            })
            .ok_or_else(|| {
                StoreError::new(
                    "INVALID_FIELD",
                    format!("Unknown relationship {}.{name}", row.object),
                    vec![name.into()],
                )
            })?;
        let Some(id) = row.get(&field.name).and_then(DataValue::text) else {
            return Ok(None);
        };
        if let Some(target) = self.find_any(id) {
            return Ok(Some(target));
        }
        if field.reference_to.len() != 1 {
            return Err(StoreError::unsupported(
                "Unresolved polymorphic relationship",
            ));
        }
        let target = &field.reference_to[0];
        let lookup = id.rsplit('.').next().unwrap_or(id);
        let values = self.records(target, false);
        let found = values
            .into_iter()
            .filter(|r| {
                r.get(
                    if target == "EntityDefinition" || target == "FieldDefinition" {
                        "QualifiedApiName"
                    } else {
                        "DeveloperName"
                    },
                )
                .and_then(DataValue::text)
                .is_some_and(|s| s.eq_ignore_ascii_case(lookup))
                    && (target != "FieldDefinition"
                        || !id.contains('.')
                        || r.get("EntityDefinitionId")
                            .and_then(DataValue::text)
                            .and_then(|entity| self.find("EntityDefinition", entity))
                            .and_then(|entity| entity.get("QualifiedApiName"))
                            .and_then(DataValue::text)
                            .is_some_and(|name| {
                                name.eq_ignore_ascii_case(id.split('.').next().unwrap_or(""))
                            }))
            })
            .collect::<Vec<_>>();
        if found.len() == 1 {
            Ok(Some(found[0]))
        } else {
            Err(StoreError::unsupported(format!(
                "Metadata relationship {name} has ambiguous/unresolved target"
            )))
        }
    }
    pub fn resolve_field(&self, row: &RecordData, path: &str) -> Result<DataValue, StoreError> {
        if let Some((parent, rest)) = path.split_once('.') {
            if let Some(parent) = self.relationship(row, parent)? {
                self.resolve_field(parent, rest)
            } else {
                Ok(DataValue::Null)
            }
        } else {
            let f = self.schema.field(&row.object, path).ok_or_else(|| {
                StoreError::new(
                    "INVALID_FIELD",
                    format!("Unknown field {}.{path}", row.object),
                    vec![path.into()],
                )
            })?;
            if f.formula.is_some() {
                return Err(StoreError::unsupported(
                    "Formula evaluation not implemented",
                ));
            }
            Ok(row.get(path).cloned().unwrap_or(DataValue::Null))
        }
    }
    fn project_field(
        &self,
        source: &RecordData,
        target: &mut RecordData,
        path: &str,
    ) -> Result<(), StoreError> {
        if let Some((parent, rest)) = path.split_once('.') {
            if let Some(record) = self.relationship(source, parent)? {
                let mut projected = match target.get(parent) {
                    Some(DataValue::Record(r)) => (**r).clone(),
                    _ => RecordData::new(&record.object),
                };
                self.project_field(record, &mut projected, rest)?;
                target.set(parent, DataValue::Record(Box::new(projected)));
            } else {
                target.set(parent, DataValue::Null);
            }
        } else {
            target.set(path, self.resolve_field(source, path)?);
        }
        Ok(())
    }
    fn child_relation(&self, parent: &str, name: &str) -> Result<(String, String), StoreError> {
        let name = if let Some((owner, relationship)) = name.split_once('.') {
            if !owner.eq_ignore_ascii_case(parent) {
                return Err(StoreError::new(
                    "INVALID_FIELD",
                    "Child relationship qualifier does not match parent",
                    vec![],
                ));
            }
            relationship
        } else {
            name
        };
        let mut found = vec![];
        for object in self.schema.objects.values() {
            for field in object.fields.values() {
                if field
                    .reference_to
                    .iter()
                    .any(|p| p.eq_ignore_ascii_case(parent))
                    && field
                        .child_relationship_name
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(name))
                {
                    found.push((object.name.clone(), field.name.clone()));
                }
            }
        }
        if found.len() == 1 {
            Ok(found.remove(0))
        } else {
            Err(StoreError::unsupported(format!(
                "Child relationship {parent}.{name} is missing or ambiguous"
            )))
        }
    }
    fn matches(
        &self,
        row: &RecordData,
        p: &Predicate,
        bindings: &Bindings,
    ) -> Result<bool, StoreError> {
        match p {
            Predicate::And(a, b) => {
                Ok(self.matches(row, a, bindings)? && self.matches(row, b, bindings)?)
            }
            Predicate::Or(a, b) => {
                Ok(self.matches(row, a, bindings)? || self.matches(row, b, bindings)?)
            }
            Predicate::Not(p) => Ok(!self.matches(row, p, bindings)?),
            Predicate::Compare {
                field,
                operator,
                value,
            } => {
                let actual = self.resolve_field(row, field)?;
                let schema = self.validate_query_field(&row.object, field, QueryAccess::System)?;
                let expected = reference_bind(&schema.kind, bind(value, bindings)?);
                let case_sensitive = self
                    .validate_query_field(&row.object, field, QueryAccess::System)?
                    .case_sensitive;
                if ["=", "!=", "<>"].contains(&operator.as_str()) {
                    if let DataValue::List(values) = &expected {
                        let mut contains = false;
                        for value in values {
                            if value_equal(&actual, value, case_sensitive)? {
                                contains = true;
                                break;
                            }
                        }
                        return Ok(if operator == "=" { contains } else { !contains });
                    }
                }
                match operator.as_str() {
                    "=" => value_equal(&actual, &expected, case_sensitive),
                    "!=" | "<>" => Ok(!value_equal(&actual, &expected, case_sensitive)?),
                    "IN" | "NOT IN" => {
                        let DataValue::List(values) = expected else {
                            return Err(StoreError::new(
                                "MALFORMED_QUERY",
                                "IN requires a list bind or literal list",
                                vec![],
                            ));
                        };
                        let mut contains = false;
                        for value in values {
                            if value_equal(&actual, &value, case_sensitive)? {
                                contains = true;
                                break;
                            }
                        }
                        Ok(if operator == "IN" {
                            contains
                        } else {
                            !contains
                        })
                    }
                    "LIKE" => {
                        if actual.is_null() {
                            return Ok(false);
                        }
                        let (DataValue::Text(value), DataValue::Text(pattern)) = (actual, expected)
                        else {
                            return Err(StoreError::new(
                                "MALFORMED_QUERY",
                                "LIKE requires text operands",
                                vec![],
                            ));
                        };
                        if !value.is_ascii() || !pattern.is_ascii() {
                            return Err(StoreError::unsupported(
                                "Non-ASCII LIKE requires locale policy",
                            ));
                        }
                        Ok(like_pattern(&pattern)?.is_match(&value))
                    }
                    op => {
                        if actual.is_null() || expected.is_null() {
                            return Ok(false);
                        }
                        let cmp = compare(&actual, &expected)?;
                        Ok(match op {
                            "<" => cmp == Ordering::Less,
                            ">" => cmp == Ordering::Greater,
                            "<=" => cmp != Ordering::Greater,
                            ">=" => cmp != Ordering::Less,
                            _ => {
                                return Err(StoreError::unsupported("Unknown comparison operator"))
                            }
                        })
                    }
                }
            }
        }
    }
}
fn reference_bind(kind: &FieldKind, value: DataValue) -> DataValue {
    if !matches!(kind, FieldKind::Id | FieldKind::Reference) {
        return value;
    }
    match value {
        DataValue::Record(record) => record.get("Id").cloned().unwrap_or(DataValue::Null),
        DataValue::List(values) => DataValue::List(
            values
                .into_iter()
                .map(|v| reference_bind(kind, v))
                .collect(),
        ),
        DataValue::Text(id) if id.len() == 15 => DataValue::Text(crate::record_store::id18(&id)),
        v => v,
    }
}
fn compare(a: &DataValue, b: &DataValue) -> Result<Ordering, StoreError> {
    match (a, b) {
        (DataValue::Number(a), DataValue::Number(b)) => Ok(a.cmp(b)),
        (DataValue::Text(a), DataValue::Text(b)) => {
            if !a.is_ascii() || !b.is_ascii() {
                return Err(StoreError::unsupported(
                    "Non-ASCII ordering requires locale policy",
                ));
            }
            Ok(a.to_ascii_uppercase().cmp(&b.to_ascii_uppercase()))
        }
        (DataValue::Date(a), DataValue::Date(b))
        | (DataValue::DateTime(a), DataValue::DateTime(b)) => Ok(a.cmp(b)),
        (DataValue::Boolean(a), DataValue::Boolean(b)) => Ok(a.cmp(b)),
        _ => Err(StoreError::new(
            "MALFORMED_QUERY",
            "Incompatible comparison operands",
            vec![],
        )),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn semijoin_filters_real_references_and_rejects_invalid_shapes() {
        use crate::record_store::{DataValue, DmlOperation, RecordData, RecordStore};
        use crate::schema::SchemaCatalog;
        let mut store = RecordStore::new(SchemaCatalog::standard()).unwrap();
        let ids = store
            .apply(
                DmlOperation::Insert,
                &[
                    RecordData::new("Account").with("Name", DataValue::Text("A".into())),
                    RecordData::new("Account").with("Name", DataValue::Text("B".into())),
                ],
                true,
            )
            .unwrap();
        store
            .apply(
                DmlOperation::Insert,
                &[RecordData::new("Contact")
                    .with("LastName", DataValue::Text("One".into()))
                    .with("AccountId", DataValue::Text(ids[0].id.clone().unwrap()))],
                true,
            )
            .unwrap();
        let query=super::parse("SELECT Id FROM Account WHERE Id NOT IN (SELECT AccountId FROM Contact WHERE LastName = :label)").unwrap();
        let result = store
            .query(
                &query,
                &super::Bindings::from([("label".into(), DataValue::Text("One".into()))]),
                super::QueryAccess::System,
            )
            .unwrap();
        let super::QueryResult::Rows(rows) = result else {
            panic!()
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id(), ids[1].id.as_deref());
        assert!(store.query(&super::parse("SELECT Id FROM Account WHERE Id IN (SELECT AccountId FROM Contact) OR Name = 'B'").unwrap(),&Default::default(),super::QueryAccess::System).is_err());
        assert!(
            super::parse("SELECT Id FROM Account WHERE Name = :UserInfo.getUsername()").is_ok()
        );
    }
    #[test]
    fn comments_do_not_change_literals_or_hide_unterminated_input() {
        assert!(super::parse("SELECT /* selected fields */ Id // line\n FROM Account WHERE Name = 'https://example.invalid/*x*/'").is_ok());
        assert!(super::parse("SELECT Id FROM Account /* missing close").is_err());
        assert!(
            super::parse("SELECT Id FROM Account WITH SYSTEM_MODE")
                .unwrap()
                .explicit_mode
        );
        assert!(
            !super::parse("SELECT Id FROM Account")
                .unwrap()
                .explicit_mode
        );
    }
    #[test]
    fn standard_field_group_expands_bounded_schema() {
        let mut store = super::RecordStore::new(crate::schema::SchemaCatalog::standard()).unwrap();
        store
            .apply(
                crate::record_store::DmlOperation::Insert,
                &[crate::record_store::RecordData::new("Account")
                    .with("Name", crate::record_store::DataValue::Text("A".into()))],
                true,
            )
            .unwrap();
        let query = super::parse("SELECT FIELDS(STANDARD) FROM Account LIMIT 1").unwrap();
        let super::QueryResult::Rows(rows) = store
            .query(&query, &Default::default(), super::QueryAccess::System)
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(
            rows[0].get("Name"),
            Some(&crate::record_store::DataValue::Text("A".into()))
        );
        assert!(rows[0].fields.contains_key("description"));
        assert!(super::parse("SELECT FIELDS(ALL) FROM Account LIMIT 200").is_err());
    }
    use super::*;
    use crate::{record_store::DmlOperation, schema::SchemaCatalog};
    fn data() -> RecordStore {
        let mut s = RecordStore::new(SchemaCatalog::standard()).unwrap();
        s.apply(
            DmlOperation::Insert,
            &[
                RecordData::new("Account")
                    .with("Name", DataValue::Text("Alpha".into()))
                    .with("NumberOfEmployees", DataValue::integer(3)),
                RecordData::new("Account")
                    .with("Name", DataValue::Text("Beta".into()))
                    .with("NumberOfEmployees", DataValue::integer(7)),
            ],
            true,
        )
        .unwrap();
        s
    }
    #[test]
    fn parser_does_not_swallow_compound_predicates() {
        let mut s = data();
        let q=parse("[SELECT COUNT() FROM Account WHERE Name='alpha' OR (NumberOfEmployees>=7 AND Name != 'Z')]").unwrap();
        assert!(matches!(
            s.query(&q, &Bindings::new(), QueryAccess::System).unwrap(),
            QueryResult::Count(2)
        ));
        assert!(parse("SELECT Name FROM Account WHERE Name='A' garbage").is_err());
    }
    #[test]
    fn ordering_offset_and_bind_limits() {
        let mut s = data();
        let q =
            parse("SELECT Name FROM Account ORDER BY NumberOfEmployees DESC LIMIT :limit OFFSET 1")
                .unwrap();
        let binds = BTreeMap::from([("limit".into(), DataValue::integer(1))]);
        let QueryResult::Rows(rows) = s.query(&q, &binds, QueryAccess::System).unwrap() else {
            panic!()
        };
        assert_eq!(rows[0].get("Name"), Some(&DataValue::Text("Alpha".into())));
    }
    #[test]
    fn aggregates_preserve_exact_decimal_values() {
        let mut s = data();
        let q=parse("SELECT SUM(NumberOfEmployees) total, AVG(NumberOfEmployees) average, COUNT(Id) records FROM Account").unwrap();
        let QueryResult::Rows(rows) = s.query(&q, &Bindings::new(), QueryAccess::System).unwrap()
        else {
            panic!()
        };
        assert_eq!(rows[0].get("total"), Some(&DataValue::integer(10)));
        assert_eq!(rows[0].get("average"), Some(&DataValue::integer(5)));
    }
    #[test]
    fn parent_and_child_queries_resolve_owned_records() {
        let mut s = data();
        let id = s.records("Account", false)[0].id().unwrap().to_string();
        s.apply(
            DmlOperation::Insert,
            &[RecordData::new("Contact")
                .with("LastName", DataValue::Text("Child".into()))
                .with("AccountId", DataValue::Text(id))],
            true,
        )
        .unwrap();
        let q =
            parse("SELECT Name, (SELECT LastName FROM Contacts) FROM Account WHERE Name='Alpha'")
                .unwrap();
        let QueryResult::Rows(rows) = s.query(&q, &Bindings::new(), QueryAccess::System).unwrap()
        else {
            panic!()
        };
        assert!(
            matches!(rows[0].get("Contacts"),Some(DataValue::List(children))if children.len()==1)
        );
        let q = parse("SELECT Account.Name FROM Contact WHERE Account.Name='Alpha'").unwrap();
        let QueryResult::Rows(rows) = s.query(&q, &Bindings::new(), QueryAccess::System).unwrap()
        else {
            panic!()
        };
        assert!(matches!(rows[0].get("Account"), Some(DataValue::Record(_))));
    }
    #[test]
    fn user_mode_cannot_silently_execute_in_system_context() {
        let mut s = data();
        let q = parse("SELECT Name FROM Account WITH USER_MODE").unwrap();
        assert!(
            s.query(&q, &Bindings::new(), QueryAccess::System)
                .unwrap_err()
                .unsupported
        );
        let policy = AccessPolicy::default();
        assert!(s
            .query(&q, &Bindings::new(), QueryAccess::User(&policy))
            .is_err());
    }
    #[test]
    fn missing_bind_and_unknown_fields_fail_even_for_empty_tables() {
        let mut s = data();
        assert!(s
            .query(
                &parse("SELECT Typo__c FROM Contact").unwrap(),
                &Bindings::new(),
                QueryAccess::System
            )
            .is_err());
        assert!(s
            .query(
                &parse("SELECT Name FROM Account WHERE Name=:missing").unwrap(),
                &Bindings::new(),
                QueryAccess::System
            )
            .is_err());
    }
    #[test]
    fn huge_limit_and_deep_predicates_are_bounded() {
        assert!(parse(&format!(
            "SELECT Id FROM Account WHERE {}Name='A'{}",
            "(".repeat(50),
            ")".repeat(50)
        ))
        .is_err());
        let mut s = data();
        assert!(s
            .query(
                &parse("SELECT Id FROM Account LIMIT 99999999999999999999999").unwrap(),
                &Bindings::new(),
                QueryAccess::System
            )
            .is_err());
    }
    #[test]
    fn flat_boolean_chains_have_a_total_node_budget() {
        let within = format!(
            "SELECT Id FROM Account WHERE {}",
            vec!["Name='A'"; 64].join(" AND ")
        );
        assert!(parse(&within).is_ok());
        for operator in [" AND ", " OR "] {
            let too_long = format!(
                "SELECT Id FROM Account WHERE {}",
                vec!["Name='A'"; 65].join(operator)
            );
            assert!(parse(&too_long)
                .unwrap_err()
                .message
                .contains("128 predicate nodes"));
        }
    }
    #[test]
    fn empty_tables_and_short_circuiting_cannot_hide_invalid_binds() {
        let mut store = data();
        for query in [
            "SELECT Id FROM Contact WHERE LastName=:missing",
            "SELECT Id FROM Contact WHERE LastName=123",
            "SELECT Id FROM Contact WHERE LastName IN :wrong",
            "SELECT Id FROM Contact WHERE LastName LIKE :wrong",
            "SELECT Id FROM Account WHERE Name='Alpha' OR Name=:missing",
        ] {
            let bindings = BTreeMap::from([("wrong".into(), DataValue::integer(1))]);
            assert!(
                store
                    .query(&parse(query).unwrap(), &bindings, QueryAccess::System)
                    .is_err(),
                "{query}"
            );
        }
        let bad_pattern = BTreeMap::from([("pattern".into(), DataValue::Text("broken\\".into()))]);
        assert!(store
            .query(
                &parse("SELECT Id FROM Contact WHERE LastName LIKE :pattern").unwrap(),
                &bad_pattern,
                QueryAccess::System
            )
            .is_err());
    }
    #[test]
    fn child_queries_are_validated_when_parent_table_is_empty() {
        let mut store = RecordStore::new(SchemaCatalog::standard()).unwrap();
        for query in [
            "SELECT Id, (SELECT MissingField FROM Contacts) FROM Account",
            "SELECT Id, (SELECT Id FROM Contacts WHERE LastName=:missing) FROM Account",
            "SELECT Id, (SELECT Id FROM MissingRelationship) FROM Account",
            "SELECT Id, (SELECT COUNT() FROM Contacts) FROM Account",
        ] {
            assert!(
                store
                    .query(
                        &parse(query).unwrap(),
                        &Bindings::new(),
                        QueryAccess::System
                    )
                    .is_err(),
                "{query}"
            );
        }
        let policy = AccessPolicy {
            readable_fields: BTreeMap::from([("account".into(), BTreeSet::from(["id".into()]))]),
            ..Default::default()
        };
        let query =
            parse("SELECT Id, (SELECT Id FROM Contacts) FROM Account WITH USER_MODE").unwrap();
        assert_eq!(
            store
                .query(&query, &Bindings::new(), QueryAccess::User(&policy))
                .unwrap_err()
                .code,
            "INSUFFICIENT_ACCESS"
        );
    }
    #[test]
    fn aggregate_type_validation_does_not_depend_on_matching_data() {
        let mut store = RecordStore::new(SchemaCatalog::standard()).unwrap();
        for query in [
            "SELECT SUM(Name) total FROM Account",
            "SELECT AVG(LastName) average FROM Contact",
            "SELECT COUNT(Id) same, SUM(NumberOfEmployees) same FROM Account",
        ] {
            assert_eq!(
                store
                    .query(
                        &parse(query).unwrap(),
                        &Bindings::new(),
                        QueryAccess::System
                    )
                    .unwrap_err()
                    .code,
                "MALFORMED_QUERY"
            );
        }
        let QueryResult::Rows(rows) = store
            .query(
                &parse("SELECT SUM(NumberOfEmployees) total FROM Account").unwrap(),
                &Bindings::new(),
                QueryAccess::System,
            )
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(rows[0].get("total"), Some(&DataValue::Null));
    }
    #[test]
    fn child_result_rows_count_toward_the_same_query_budget() {
        let mut store = data();
        let parent_id = store.records("Account", false)[0].id().unwrap().to_string();
        store
            .apply(
                DmlOperation::Insert,
                &[RecordData::new("Contact")
                    .with("LastName", DataValue::Text("Child".into()))
                    .with("AccountId", DataValue::Text(parent_id))],
                true,
            )
            .unwrap();
        let query =
            parse("SELECT Id, (SELECT Id FROM Contacts) FROM Account WHERE Name='Alpha'").unwrap();
        store
            .query(&query, &Bindings::new(), QueryAccess::System)
            .unwrap();
        assert_eq!(
            store.query_rows, 2,
            "One parent plus one descendant must be charged"
        );
        store.query_rows = 49_999;
        assert_eq!(
            store
                .query(&query, &Bindings::new(), QueryAccess::System)
                .unwrap_err()
                .code,
            "LIMIT_EXCEEDED"
        );
        assert_eq!(store.query_rows, 50_001);
    }
    #[test]
    fn relationship_predicates_preserve_terminal_field_case_policy() {
        let mut store = data();
        let parent_id = store.records("Account", false)[0].id().unwrap().to_string();
        store
            .apply(
                DmlOperation::Insert,
                &[RecordData::new("Contact")
                    .with("LastName", DataValue::Text("Child".into()))
                    .with("AccountId", DataValue::Text(parent_id))],
                true,
            )
            .unwrap();
        store
            .schema
            .objects
            .get_mut("account")
            .unwrap()
            .fields
            .get_mut("name")
            .unwrap()
            .case_sensitive = true;
        let query = parse("SELECT Id FROM Contact WHERE Account.Name='alpha'").unwrap();
        let QueryResult::Rows(rows) = store
            .query(&query, &Bindings::new(), QueryAccess::System)
            .unwrap()
        else {
            panic!()
        };
        assert!(rows.is_empty());
    }
}
