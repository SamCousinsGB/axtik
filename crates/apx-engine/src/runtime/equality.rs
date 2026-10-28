//! Bounded Apex value comparison. Lists compare elements in order; record values
//! compare populated fields. Explicit identity operators never invoke user code.
use super::*;

impl Context<'_> {
    pub(super) fn identity_equal(&self, a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::List(a) | Value::Iterable(a), Value::List(b) | Value::Iterable(b)) => {
                Rc::ptr_eq(a, b)
            }
            (Value::Set(a), Value::Set(b)) => Rc::ptr_eq(a, b),
            (Value::Map(a), Value::Map(b)) => Rc::ptr_eq(a, b),
            (Value::Record(a), Value::Record(b)) => Rc::ptr_eq(a, b),
            (Value::Instance(a), Value::Instance(b)) => Rc::ptr_eq(a, b),
            (Value::Exception(a), Value::Exception(b)) => Rc::ptr_eq(a, b),
            _ => a.equal(b),
        }
    }
    pub(super) fn apex_equal(
        &mut self,
        a: &Value,
        b: &Value,
        ignore_string_case: bool,
        at: &Node,
    ) -> R<bool> {
        self.equal_inner(a, b, ignore_string_case, at, 0)
    }
    fn equal_inner(
        &mut self,
        a: &Value,
        b: &Value,
        ignore_string_case: bool,
        at: &Node,
        depth: usize,
    ) -> R<bool> {
        self.tick(at)?;
        if depth >= 64 {
            return block(at, "Equality graph exceeds host depth bound");
        }
        match (a, b) {
            (Value::Num(a), Value::Num(b))
                if (a.kind == NumKind::Double && b.kind == NumKind::Decimal
                    || a.kind == NumKind::Decimal && b.kind == NumKind::Double)
                    && (!a.q.is_integer() || !b.q.is_integer()) =>
            {
                block(
                    at,
                    "Mixed fractional Double/Decimal equality requires qualified promotion policy",
                )
            }
            (Value::Str(a), Value::Str(b)) if ignore_string_case => {
                if !a.is_ascii() || !b.is_ascii() {
                    return block(at, "Non-ASCII String comparison is not qualified");
                }
                Ok(a.eq_ignore_ascii_case(b))
            }
            (Value::List(a) | Value::Iterable(a), Value::List(b) | Value::Iterable(b)) => {
                let (a, b) = (a.borrow().clone(), b.borrow().clone());
                if a.len() != b.len() {
                    return Ok(false);
                }
                for (a, b) in a.iter().zip(&b) {
                    if !self.equal_inner(a, b, false, at, depth + 1)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (Value::Record(a), Value::Record(b)) => {
                let (a, b) = (a.borrow().clone(), b.borrow().clone());
                if !a.typ.eq_ignore_ascii_case(&b.typ) {
                    return Ok(false);
                }
                let keys = a
                    .fields
                    .keys()
                    .chain(b.fields.keys())
                    .collect::<BTreeSet<_>>();
                for key in keys {
                    if !self.equal_inner(
                        a.fields.get(key).unwrap_or(&Value::Null),
                        b.fields.get(key).unwrap_or(&Value::Null),
                        false,
                        at,
                        depth + 1,
                    )? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (Value::Set(a), Value::Set(b)) => {
                let (a, b) = (a.borrow().clone(), b.borrow().clone());
                if a.len() != b.len() {
                    return Ok(false);
                }
                for item in &a {
                    let mut found = false;
                    for other in &b {
                        if self.equal_inner(item, other, false, at, depth + 1)? {
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (Value::Map(a), Value::Map(b)) => {
                let (a, b) = (a.borrow().clone(), b.borrow().clone());
                if a.len() != b.len() {
                    return Ok(false);
                }
                for (key, value) in &a {
                    let mut found = false;
                    for (other_key, other_value) in &b {
                        if self.equal_inner(key, other_key, false, at, depth + 1)? {
                            if !self.equal_inner(value, other_value, false, at, depth + 1)? {
                                return Ok(false);
                            }
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (Value::Instance(a), Value::Instance(_)) => {
                let class = a.borrow().class.clone();
                if self
                    .program
                    .classes
                    .contains_key(&class.to_ascii_lowercase())
                {
                    if self.class_chain(&class, at)?.iter().any(|c| {
                        self.program.classes[c]
                            .methods
                            .iter()
                            .any(|m| m.name.eq_ignore_ascii_case("equals"))
                    }) {
                        let (owner, method) =
                            self.find_method(&class, "equals", &[b.clone()], false, at)?;
                        let result =
                            self.invoke_method(&owner, &method, vec![b.clone()], Some(a.clone()))?;
                        return self.boolean(result, at);
                    }
                    return Ok(self.identity_equal(&Value::Instance(a.clone()), b));
                }
                if self.identity_equal(&Value::Instance(a.clone()), b) {
                    Ok(true)
                } else {
                    block(at, "Native object content equality is not implemented")
                }
            }
            _ => Ok(a.equal(b)),
        }
    }
}
