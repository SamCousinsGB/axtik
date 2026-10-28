use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{One, Signed, ToPrimitive, Zero};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc, str::FromStr};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NumKind {
    Int,
    Long,
    Decimal,
    Double,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Number {
    pub q: BigRational,
    pub kind: NumKind,
}
impl Number {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > 1024 {
            return Err("Numeric literal exceeds preview resource bound".into());
        }
        let long = text.ends_with(['l', 'L']);
        let double = text.ends_with(['d', 'D']);
        let s = text.trim_end_matches(['l', 'L', 'd', 'D']);
        let (body, exp) = if let Some((b, e)) = s.split_once(['e', 'E']) {
            (b, e.parse::<i32>().map_err(|_| "Invalid exponent")?)
        } else {
            (s, 0)
        };
        if !(-1000..=1000).contains(&exp) {
            return Err("Exponent exceeds preview resource bound".into());
        }
        let scale = body.split('.').nth(1).map(str::len).unwrap_or(0) as i32 - exp;
        let digits = body.replace('.', "");
        let mut num = BigInt::from_str(&digits).map_err(|_| "Invalid number")?;
        let mut den = BigInt::one();
        if scale >= 0 {
            den = BigInt::from(10).pow(scale as u32)
        } else {
            num *= BigInt::from(10).pow((-scale) as u32)
        }
        let kind = if long {
            NumKind::Long
        } else if body.contains('.') || exp != 0 {
            NumKind::Decimal
        } else {
            NumKind::Int
        };
        let value = Self {
            q: BigRational::new(num, den),
            kind,
        };
        if double {
            Self::from_double(value.q.to_f64().ok_or("Double out of range")?)
        } else {
            Ok(value)
        }
    }
    pub fn from_double(value: f64) -> Result<Self, String> {
        if !value.is_finite() {
            return Err("Non-finite Double semantics are not implemented".into());
        }
        if value == 0.0 && value.is_sign_negative() {
            return Err("Signed-zero Double semantics are not implemented".into());
        }
        Ok(Self {
            q: BigRational::from_float(value).ok_or("Invalid Double")?,
            kind: NumKind::Double,
        })
    }
    pub fn integer(v: i64) -> Self {
        Self {
            q: BigRational::from_integer(v.into()),
            kind: NumKind::Int,
        }
    }
    pub fn display(&self) -> String {
        if self.kind == NumKind::Double {
            let mut text = self.q.to_f64().unwrap().to_string();
            if !text.contains(['.', 'e', 'E']) {
                text.push_str(".0");
            }
            return text;
        }
        if self.q.is_integer() {
            return self.q.to_integer().to_string();
        }
        let sign = if self.q.is_negative() { "-" } else { "" };
        let q = self.q.abs();
        let mut n = q.numer().clone();
        let d = q.denom().clone();
        let integer = &n / &d;
        n %= &d;
        let mut frac = String::new();
        for _ in 0..100 {
            if n.is_zero() {
                break;
            }
            n *= 10;
            frac.push_str(&(&n / &d).to_string());
            n %= &d;
        }
        if !n.is_zero() {
            frac.push_str("...")
        };
        format!("{sign}{integer}.{frac}")
    }
    pub fn index(&self) -> Option<usize> {
        if self.q.is_integer() {
            self.q.to_integer().to_usize()
        } else {
            None
        }
    }
    pub fn checked(self) -> Result<Self, String> {
        let ok = match self.kind {
            NumKind::Decimal | NumKind::Double => true,
            NumKind::Int => self.q.is_integer() && self.q.to_integer().to_i32().is_some(),
            NumKind::Long => self.q.is_integer() && self.q.to_integer().to_i64().is_some(),
        };
        if ok {
            Ok(self)
        } else {
            Err("Numeric overflow or invalid integer conversion".into())
        }
    }
}
#[derive(Clone, Debug)]
pub struct Record {
    pub typ: String,
    pub fields: BTreeMap<String, Value>,
    pub projected: bool,
}
#[derive(Clone, Debug)]
pub struct Instance {
    pub class: String,
    pub fields: BTreeMap<String, Value>,
    pub field_types: BTreeMap<String, String>,
}
#[derive(Clone, Debug)]
pub struct ExceptionValue {
    pub typ: String,
    pub message: String,
    pub stack: String,
}
#[derive(Clone, Debug)]
pub enum Value {
    Null,
    Bool(bool),
    Num(Number),
    Str(String),
    Blob(Vec<u8>),
    Date(i32),
    Datetime(i64),
    Time(u32),
    List(Rc<RefCell<Vec<Value>>>),
    Iterable(Rc<RefCell<Vec<Value>>>),
    Set(Rc<RefCell<Vec<Value>>>),
    Map(Rc<RefCell<Vec<(Value, Value)>>>),
    Instance(Rc<RefCell<Instance>>),
    Enum {
        typ: String,
        name: String,
        ordinal: usize,
    },
    Type(String),
    Exception(Rc<RefCell<ExceptionValue>>),
    Record(Rc<RefCell<Record>>),
    Namespace(String),
}
impl Value {
    pub fn list(v: Vec<Value>) -> Self {
        Self::List(Rc::new(RefCell::new(v)))
    }
    pub fn display(&self) -> String {
        self.display_checked()
            .unwrap_or_else(|message| format!("<{message}>"))
    }
    pub fn display_checked(&self) -> Result<String, String> {
        let mut out = String::new();
        self.write_display(&mut out, 0)?;
        Ok(out)
    }
    fn write_display(&self, out: &mut String, depth: usize) -> Result<(), String> {
        fn push(out: &mut String, text: &str) -> Result<(), String> {
            if out.len().saturating_add(text.len()) > 1024 * 1024 {
                return Err("Value display exceeds 1 MiB expansion bound".into());
            }
            out.push_str(text);
            Ok(())
        }
        if depth > 32 {
            return Err("Value display exceeds depth bound".into());
        }
        match self {
            Self::Null => push(out, "null"),
            Self::Bool(b) => push(out, if *b { "true" } else { "false" }),
            Self::Num(n) => push(out, &n.display()),
            Self::Str(s) => push(out, s),
            Self::Blob(bytes) => push(out, &format!("Blob[{}]", bytes.len())),
            Self::Date(_) | Self::Datetime(_) | Self::Time(_) => {
                push(out, &crate::dates::display(self).unwrap_or_default())
            }
            Self::Enum { name, .. } | Self::Type(name) => push(out, name),
            Self::Namespace(name) => push(out, &format!("<type {name}>")),
            Self::Exception(e) => {
                let e = e.borrow();
                push(out, &e.typ)?;
                push(out, ": ")?;
                push(out, &e.message)
            }
            Self::List(values) | Self::Iterable(values) | Self::Set(values) => {
                push(out, "[")?;
                for (i, v) in values.borrow().iter().enumerate() {
                    if i > 0 {
                        push(out, ", ")?;
                    }
                    v.write_display(out, depth + 1)?;
                }
                push(out, "]")
            }
            Self::Map(entries) => {
                push(out, "{")?;
                for (i, (k, v)) in entries.borrow().iter().enumerate() {
                    if i > 0 {
                        push(out, ", ")?;
                    }
                    k.write_display(out, depth + 1)?;
                    push(out, "=")?;
                    v.write_display(out, depth + 1)?;
                }
                push(out, "}")
            }
            Self::Instance(instance) => {
                let instance = instance.borrow();
                push(
                    out,
                    instance.class.rsplit('.').next().unwrap_or(&instance.class),
                )?;
                push(out, ":{")?;
                for (i, (k, v)) in instance.fields.iter().enumerate() {
                    if i > 0 {
                        push(out, ", ")?;
                    }
                    push(out, k)?;
                    push(out, "=")?;
                    v.write_display(out, depth + 1)?;
                }
                push(out, "}")
            }
            Self::Record(record) => {
                let record = record.borrow();
                push(out, &record.typ)?;
                push(out, "{")?;
                for (i, (k, v)) in record.fields.iter().enumerate() {
                    if i > 0 {
                        push(out, ", ")?;
                    }
                    push(out, k)?;
                    push(out, "=")?;
                    v.write_display(out, depth + 1)?;
                }
                push(out, "}")
            }
        }
    }
    pub fn equal(&self, b: &Self) -> bool {
        match (self, b) {
            (Self::Null, Self::Null) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Num(a), Self::Num(b)) => a.q == b.q,
            (Self::Str(a), Self::Str(b)) => a == b,
            (Self::Blob(a), Self::Blob(b)) => a == b,
            (Self::Date(a), Self::Date(b)) => a == b,
            (Self::Datetime(a), Self::Datetime(b)) => a == b,
            (Self::Time(a), Self::Time(b)) => a == b,
            (
                Self::Enum {
                    typ: a, name: an, ..
                },
                Self::Enum {
                    typ: b, name: bn, ..
                },
            ) => a.eq_ignore_ascii_case(b) && an.eq_ignore_ascii_case(bn),
            (Self::Type(a), Self::Type(b)) => a.eq_ignore_ascii_case(b),
            (Self::Instance(a), Self::Instance(b)) => Rc::ptr_eq(a, b),
            (Self::List(a), Self::List(b)) => {
                let aa = a.borrow();
                let bb = b.borrow();
                aa.len() == bb.len() && aa.iter().zip(bb.iter()).all(|(a, b)| a.equal(b))
            }
            (Self::Record(a), Self::Record(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
    pub fn is_scalar(&self) -> bool {
        matches!(
            self,
            Self::Null
                | Self::Bool(_)
                | Self::Num(_)
                | Self::Str(_)
                | Self::Blob(_)
                | Self::Date(_)
                | Self::Datetime(_)
                | Self::Time(_)
                | Self::Enum { .. }
                | Self::Type(_)
        )
    }
}
