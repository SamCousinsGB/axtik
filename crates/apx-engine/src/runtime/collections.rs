use super::*;

pub(super) fn generic(typ: &str) -> Option<(&str, Vec<String>)> {
    let open = typ.find('<')?;
    let inside = typ.get(open + 1..typ.len().checked_sub(1)?)?;
    if !typ.ends_with('>') {
        return None;
    }
    let mut depth = 0i32;
    let mut start = 0;
    let mut parts = vec![];
    for (i, c) in inside.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(inside[start..i].trim().into());
                start = i + 1;
            }
            _ => {}
        }
        if depth < 0 {
            return None;
        }
    }
    if depth != 0 {
        return None;
    }
    parts.push(inside[start..].trim().into());
    Some((&typ[..open], parts))
}
impl Context<'_> {
    fn can_widen_collection_type(&self, actual: &str, expected: &str) -> bool {
        if actual == expected
            || expected == "object"
            || expected == "sobject" && self.program.schema.object(actual).is_some()
            || self.class_assignable(actual, expected)
        {
            return true;
        }
        match (generic(actual), generic(expected)) {
            (Some(("list", a)), Some(("list", e))) if a.len() == 1 && e.len() == 1 => {
                self.can_widen_collection_type(&a[0], &e[0])
            }
            _ => false,
        }
    }
    pub(super) fn register_list_contract(&self, items: &Rc<RefCell<Vec<Value>>>, typ: &str) {
        let key = Rc::as_ptr(items) as usize;
        self.list_types
            .borrow_mut()
            .insert(key, self.qualified_type(typ, &self.current_class));
        self.list_owners
            .borrow_mut()
            .insert(key, Rc::downgrade(items));
    }
    pub(super) fn register_set_contract(&self, items: &Rc<RefCell<Vec<Value>>>, typ: &str) {
        let key = Rc::as_ptr(items) as usize;
        self.set_types
            .borrow_mut()
            .insert(key, self.qualified_type(typ, &self.current_class));
        self.set_owners
            .borrow_mut()
            .insert(key, Rc::downgrade(items));
    }
    pub(super) fn register_map_contract(
        &self,
        items: &Rc<RefCell<Vec<(Value, Value)>>>,
        key_type: &str,
        value_type: &str,
    ) {
        let key = Rc::as_ptr(items) as usize;
        self.map_types.borrow_mut().insert(
            key,
            (
                self.qualified_type(key_type, &self.current_class),
                self.qualified_type(value_type, &self.current_class),
            ),
        );
        self.map_owners
            .borrow_mut()
            .insert(key, Rc::downgrade(items));
    }
    pub(super) fn coerce_collection(&self, v: Value, typ: &str, at: &Node) -> R<Option<Value>> {
        let array_type;
        let typ = if let Some(t) = typ.strip_suffix("[]") {
            array_type = format!("list<{t}>");
            array_type.as_str()
        } else {
            typ
        };
        let Some((kind, params)) = generic(typ) else {
            return Ok(None);
        };
        let params = params
            .iter()
            .map(|p| self.qualified_type(p, &self.current_class))
            .collect::<Vec<_>>();
        if !["list", "set", "map", "iterable"].contains(&kind) {
            return Ok(None);
        }
        if matches!(v, Value::Null) {
            return Ok(Some(v));
        }
        if (kind == "map" && params.len() != 2) || (kind != "map" && params.len() != 1) {
            return block(at, "Invalid generic collection arity");
        }
        match (kind, &v) {
            ("list", Value::List(items)) | ("set", Value::Set(items)) => {
                let types = if kind == "list" {
                    &self.list_types
                } else {
                    &self.set_types
                };
                let owners = if kind == "list" {
                    &self.list_owners
                } else {
                    &self.set_owners
                };
                let key = Rc::as_ptr(items) as usize;
                if owners
                    .borrow()
                    .get(&key)
                    .is_some_and(|owner| owner.upgrade().is_none())
                {
                    types.borrow_mut().remove(&key);
                }
                owners.borrow_mut().insert(key, Rc::downgrade(items));
                let old_type = types.borrow().get(&key).cloned();
                let mut retained_type = params[0].clone();
                if let Some(old) = old_type.as_ref().filter(|old| *old != &params[0]) {
                    let is_record =
                        |t: &str| t == "sobject" || self.program.schema.object(t).is_some();
                    if self.can_widen_collection_type(old, &params[0]) {
                        retained_type = old.clone();
                    } else if is_record(old) && is_record(&params[0])
                        || self.can_widen_collection_type(&params[0], old)
                    {
                    } else {
                        return block(
                            at,
                            format!(
                                "Collection covariance/casting from {old} to {} is not implemented",
                                params[0]
                            ),
                        );
                    }
                }
                let mut converted = items
                    .borrow()
                    .iter()
                    .map(|v| self.coerce(v.clone(), &params[0], at))
                    .collect::<R<Vec<_>>>()?;
                if kind == "set" {
                    let mut unique = vec![];
                    for item in converted {
                        if !item.is_scalar() {
                            return block(
                                at,
                                "Reference-valued Set entries require qualified hashing",
                            );
                        }
                        if !unique.iter().any(|v: &Value| v.equal(&item)) {
                            unique.push(item);
                        }
                    }
                    converted = unique;
                }
                *items.borrow_mut() = converted;
                types.borrow_mut().insert(key, retained_type);
            }
            ("map", Value::Map(items)) => {
                let key = Rc::as_ptr(items) as usize;
                if self
                    .map_owners
                    .borrow()
                    .get(&key)
                    .is_some_and(|owner| owner.upgrade().is_none())
                {
                    self.map_types.borrow_mut().remove(&key);
                }
                self.map_owners
                    .borrow_mut()
                    .insert(key, Rc::downgrade(items));
                let mut expected = (params[0].clone(), params[1].clone());
                if let Some(old) = self
                    .map_types
                    .borrow()
                    .get(&key)
                    .filter(|old| *old != &expected)
                {
                    if old.0 != expected.0 {
                        return block(at, "Map key covariance/casting is not implemented");
                    }
                    if self.can_widen_collection_type(&old.1, &expected.1) {
                        expected = old.clone();
                    } else if self.can_widen_collection_type(&expected.1, &old.1) {
                    } else {
                        return block(
                            at,
                            format!(
                                "Map value covariance from {} to {} is not implemented",
                                old.1, expected.1
                            ),
                        );
                    }
                }
                let converted = items
                    .borrow()
                    .iter()
                    .map(|(k, v)| {
                        Ok((
                            self.coerce(k.clone(), &params[0], at)?,
                            self.coerce(v.clone(), &params[1], at)?,
                        ))
                    })
                    .collect::<R<Vec<_>>>()?;
                *items.borrow_mut() = converted;
                self.map_types.borrow_mut().insert(key, expected);
            }
            ("iterable", Value::List(items) | Value::Iterable(items)) => {
                if self.list_types.borrow().get(&(Rc::as_ptr(items) as usize)) != Some(&params[0]) {
                    return block(
                        at,
                        "Iterable element type must exactly match the underlying List",
                    );
                }
                return Ok(Some(Value::Iterable(items.clone())));
            }
            ("iterable", Value::Instance(instance))
                if self.class_assignable(&instance.borrow().class, typ) =>
            {
                return Ok(Some(v.clone()))
            }
            _ => return fail(at, format!("Expected {typ}")),
        }
        Ok(Some(v))
    }
    pub(super) fn construct_collection(
        &mut self,
        typ: &str,
        args: &Node,
        at: &Node,
    ) -> R<Option<Value>> {
        let typ = typ.replace(' ', "");
        let Some((kind, _)) = generic(&typ) else {
            return Ok(None);
        };
        if !["list", "set", "map"].contains(&kind) {
            return Ok(None);
        }
        let values = args.named().map(|n| self.eval(n)).collect::<R<Vec<_>>>()?;
        let v = match (kind, values.as_slice()) {
            ("list", []) => Value::list(vec![]),
            ("set", []) => Value::Set(Rc::new(RefCell::new(vec![]))),
            ("map", []) => Value::Map(Rc::new(RefCell::new(vec![]))),
            ("list", [Value::List(v) | Value::Set(v) | Value::Iterable(v)]) => {
                Value::list(v.borrow().clone())
            }
            ("set", [Value::List(v) | Value::Set(v) | Value::Iterable(v)]) => {
                let mut unique = vec![];
                for x in v.borrow().iter() {
                    if !unique.iter().any(|y: &Value| y.equal(x)) {
                        unique.push(x.clone());
                    }
                }
                Value::Set(Rc::new(RefCell::new(unique)))
            }
            ("map", [Value::Map(v)]) => Value::Map(Rc::new(RefCell::new(v.borrow().clone()))),
            ("map", [Value::List(records)]) => {
                let mut values = vec![];
                for record in records.borrow().iter() {
                    let Value::Record(r) = record else {
                        return fail(at, "Map record constructor requires SObjects");
                    };
                    let id = r.borrow().fields.get("id").cloned().unwrap_or(Value::Null);
                    if matches!(id, Value::Null) {
                        return exceptions::raise(
                            at,
                            "System.ListException",
                            "Row with null Id at index in Map constructor",
                        );
                    }
                    if let Some((_, old)) =
                        values.iter_mut().find(|(key, _)| Value::equal(key, &id))
                    {
                        *old = record.clone();
                    } else {
                        values.push((id, record.clone()));
                    }
                }
                Value::Map(Rc::new(RefCell::new(values)))
            }
            _ => return block(at, "Unsupported collection constructor arguments"),
        };
        Ok(Some(self.coerce(v, &typ, at)?))
    }
    pub(super) fn map_literal(&mut self, n: &Node) -> R<Value> {
        let typ = self
            .need(n, "type")?
            .text()
            .to_ascii_lowercase()
            .replace(' ', "");
        if self.need(n, "value")?.named().next().is_none() {
            if typ.starts_with("list<") {
                return self.coerce(Value::list(vec![]), &typ, n);
            }
            if typ.starts_with("set<") {
                return self.coerce(Value::Set(Rc::new(RefCell::new(vec![]))), &typ, n);
            }
        }
        let mut values: Vec<(Value, Value)> = vec![];
        for entry in self.need(n, "value")?.named() {
            let pair = entry.named().collect::<Vec<_>>();
            if pair.len() != 2 {
                return block(entry, "Invalid Map initializer pair");
            }
            let k = self.eval(pair[0])?;
            let v = self.eval(pair[1])?;
            if let Some(old) = values.iter_mut().find(|(key, _)| key.equal(&k)) {
                old.1 = v;
            } else {
                values.push((k, v));
            }
        }
        self.coerce(Value::Map(Rc::new(RefCell::new(values))), &typ, n)
    }
    pub(super) fn collection_method(
        &mut self,
        receiver: &Value,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        if name == "tostring"
            && args.is_empty()
            && matches!(receiver, Value::List(_) | Value::Set(_) | Value::Map(_))
        {
            return Ok(Some(Value::Str(self.display_value(receiver, at)?)));
        }
        let result = match receiver {
            Value::Map(items) => {
                let identity = Rc::as_ptr(items) as usize;
                if ["put", "putall", "remove", "clear"].contains(&name)
                    && self.iteration_locks.contains_key(&identity)
                {
                    return exceptions::raise(
                        at,
                        "System.FinalException",
                        "Collection cannot be modified during iteration/trigger execution",
                    );
                }
                let (kt, vt) =
                    self.map_types
                        .borrow()
                        .get(&identity)
                        .cloned()
                        .ok_or_else(|| EvalError {
                            exception: None,
                            status: "blocked",
                            diagnostic: at
                                .diagnostic("APX_UNSUPPORTED", "Map type contract unavailable"),
                        })?;
                match (name, args) {
                    ("size", []) => Value::Num(Number::integer(items.borrow().len() as i64)),
                    ("isempty", []) => Value::Bool(items.borrow().is_empty()),
                    ("get", [k]) => (items
                        .borrow()
                        .iter()
                        .find(|(key, _)| key.equal(k))
                        .map(|(_, v)| v.clone()))
                    .unwrap_or(Value::Null),
                    ("containskey", [k]) => {
                        Value::Bool(items.borrow().iter().any(|(key, _)| key.equal(k)))
                    }
                    ("put", [k, v]) => {
                        let k = self.coerce(k.clone(), &kt, at)?;
                        let v = self.coerce(v.clone(), &vt, at)?;
                        if !k.is_scalar() {
                            return block(
                                at,
                                "Reference-valued Map keys require qualified hashing",
                            );
                        }
                        self.reject_cycle(&v, identity, at)?;
                        let mut items = items.borrow_mut();
                        if let Some((_, old)) = items.iter_mut().find(|(key, _)| key.equal(&k)) {
                            std::mem::replace(old, v)
                        } else {
                            items.push((k, v));
                            Value::Null
                        }
                    }
                    ("putall", [Value::Map(source)]) => {
                        let entries = source.borrow().clone();
                        for (k, v) in entries {
                            self.collection_method(receiver, "put", &[k, v], at)?;
                        }
                        Value::Null
                    }
                    ("remove", [k]) => {
                        let mut items = items.borrow_mut();
                        if let Some(i) = items.iter().position(|(key, _)| key.equal(k)) {
                            items.remove(i).1
                        } else {
                            Value::Null
                        }
                    }
                    ("clear", []) => {
                        items.borrow_mut().clear();
                        Value::Null
                    }
                    ("keyset", []) => {
                        let keys = items.borrow().iter().map(|(k, _)| k.clone()).collect();
                        self.coerce(
                            Value::Set(Rc::new(RefCell::new(keys))),
                            &format!("set<{kt}>"),
                            at,
                        )?
                    }
                    ("values", []) => {
                        let values = items.borrow().iter().map(|(_, v)| v.clone()).collect();
                        self.coerce(Value::list(values), &format!("list<{vt}>"), at)?
                    }
                    ("clone", []) => self.coerce(
                        Value::Map(Rc::new(RefCell::new(items.borrow().clone()))),
                        &format!("map<{kt},{vt}>"),
                        at,
                    )?,
                    _ => return Ok(None),
                }
            }
            Value::Set(items) => {
                let identity = Rc::as_ptr(items) as usize;
                let typ = self
                    .set_types
                    .borrow()
                    .get(&identity)
                    .cloned()
                    .ok_or_else(|| EvalError {
                        exception: None,
                        status: "blocked",
                        diagnostic: at
                            .diagnostic("APX_UNSUPPORTED", "Set type contract unavailable"),
                    })?;
                match (name, args) {
                    ("size", []) => Value::Num(Number::integer(items.borrow().len() as i64)),
                    ("isempty", []) => Value::Bool(items.borrow().is_empty()),
                    ("contains", [v]) => Value::Bool(items.borrow().iter().any(|old| old.equal(v))),
                    ("add", [v]) => {
                        self.ensure_list_writable(items, at)?;
                        let v = self.coerce(v.clone(), &typ, at)?;
                        if !v.is_scalar() {
                            return block(
                                at,
                                "Reference-valued Set entries require qualified hashing",
                            );
                        }
                        let mut items = items.borrow_mut();
                        let new = !items.iter().any(|old| old.equal(&v));
                        if new {
                            items.push(v);
                        }
                        Value::Bool(new)
                    }
                    ("remove", [v]) => {
                        self.ensure_list_writable(items, at)?;
                        let mut items = items.borrow_mut();
                        if let Some(i) = items.iter().position(|old| old.equal(v)) {
                            items.remove(i);
                            Value::Bool(true)
                        } else {
                            Value::Bool(false)
                        }
                    }
                    ("clear", []) => {
                        self.ensure_list_writable(items, at)?;
                        items.borrow_mut().clear();
                        Value::Null
                    }
                    ("clone", []) => self.coerce(
                        Value::Set(Rc::new(RefCell::new(items.borrow().clone()))),
                        &format!("set<{typ}>"),
                        at,
                    )?,
                    _ => return Ok(None),
                }
            }
            Value::List(items) => match (name, args) {
                ("getsobjecttype", []) => {
                    let typ = self
                        .list_types
                        .borrow()
                        .get(&(Rc::as_ptr(items) as usize))
                        .cloned()
                        .unwrap_or_default();
                    let Some(object) = self.program.schema.object(&typ) else {
                        return block(at, "List runtime SObject type is unavailable");
                    };
                    Value::Type(format!("Schema.SObjectType:{}", object.name))
                }
                ("addall", [Value::List(source) | Value::Set(source)]) => {
                    self.ensure_list_writable(items, at)?;
                    let typ = self
                        .list_types
                        .borrow()
                        .get(&(Rc::as_ptr(items) as usize))
                        .cloned()
                        .ok_or_else(|| EvalError {
                            exception: None,
                            status: "blocked",
                            diagnostic: at.diagnostic("APX_UNSUPPORTED", "List type unavailable"),
                        })?;
                    let values = source.borrow().clone();
                    let mut converted = vec![];
                    for v in values {
                        self.reject_cycle(&v, Rc::as_ptr(items) as usize, at)?;
                        converted.push(self.coerce(v, &typ, at)?);
                    }
                    if items.borrow().len() + converted.len() > 100000 {
                        return block(at, "Host list bound exceeded");
                    }
                    items.borrow_mut().extend(converted);
                    Value::Null
                }
                ("sort", []) | ("sort", [Value::Instance(_)]) => {
                    self.ensure_list_writable(items, at)?;
                    let mut values = items.borrow().clone();
                    for i in 1..values.len() {
                        let mut j = i;
                        while j > 0 {
                            self.tick(at)?;
                            let greater = if let Some(Value::Instance(comparator)) = args.first() {
                                let result = self.call_instance(
                                    comparator.clone(),
                                    "compare",
                                    vec![values[j - 1].clone(), values[j].clone()],
                                    at,
                                )?;
                                let Value::Num(result) = result else {
                                    return fail(at, "Comparator must return Integer");
                                };
                                result.q > BigRational::zero()
                            } else {
                                match (&values[j - 1], &values[j]) {
                                    (Value::Null, _) => false,
                                    (_, Value::Null) => true,
                                    (Value::Num(a), Value::Num(b)) => a.q > b.q,
                                    (Value::Str(a), Value::Str(b))
                                        if a.is_ascii() && b.is_ascii() =>
                                    {
                                        a > b
                                    }
                                    _ => {
                                        return block(
                                            at,
                                            "Natural sort for these values is not implemented",
                                        )
                                    }
                                }
                            };
                            if !greater {
                                break;
                            }
                            values.swap(j - 1, j);
                            j -= 1;
                        }
                    }
                    *items.borrow_mut() = values;
                    Value::Null
                }
                ("contains", [v]) => {
                    let values = items.borrow().clone();
                    let mut found = false;
                    for old in &values {
                        if self.apex_equal(old, v, false, at)? {
                            found = true;
                            break;
                        }
                    }
                    Value::Bool(found)
                }
                ("indexof", [v]) => {
                    let values = items.borrow().clone();
                    let mut index = -1;
                    for (i, old) in values.iter().enumerate() {
                        if self.apex_equal(old, v, false, at)? {
                            index = i as i64;
                            break;
                        }
                    }
                    Value::Num(Number::integer(index))
                }
                ("clone", []) => {
                    let typ = self
                        .list_types
                        .borrow()
                        .get(&(Rc::as_ptr(items) as usize))
                        .cloned()
                        .unwrap_or("object".into());
                    self.coerce(
                        Value::list(items.borrow().clone()),
                        &format!("list<{typ}>"),
                        at,
                    )?
                }
                _ => return Ok(None),
            },
            _ => return Ok(None),
        };
        Ok(Some(result))
    }
    pub(super) fn reject_cycle(&self, value: &Value, target: usize, at: &Node) -> R<()> {
        fn visit(v: &Value, target: usize, seen: &mut BTreeSet<usize>, depth: usize) -> bool {
            if depth > 64 {
                return true;
            }
            match v {
                Value::List(v) | Value::Set(v) | Value::Iterable(v) => {
                    let id = Rc::as_ptr(v) as usize;
                    if id == target {
                        return true;
                    }
                    if !seen.insert(id) {
                        return false;
                    }
                    v.borrow().iter().any(|v| visit(v, target, seen, depth + 1))
                }
                Value::Map(v) => {
                    let id = Rc::as_ptr(v) as usize;
                    if id == target {
                        return true;
                    }
                    if !seen.insert(id) {
                        return false;
                    }
                    v.borrow().iter().any(|(k, v)| {
                        visit(k, target, seen, depth + 1) || visit(v, target, seen, depth + 1)
                    })
                }
                Value::Instance(v) => {
                    let id = Rc::as_ptr(v) as usize;
                    if id == target {
                        return true;
                    }
                    if !seen.insert(id) {
                        return false;
                    }
                    v.borrow()
                        .fields
                        .values()
                        .any(|v| visit(v, target, seen, depth + 1))
                }
                Value::Record(v) => {
                    let id = Rc::as_ptr(v) as usize;
                    if id == target {
                        return true;
                    }
                    if !seen.insert(id) {
                        return false;
                    }
                    v.borrow()
                        .fields
                        .values()
                        .any(|v| visit(v, target, seen, depth + 1))
                }
                _ => false,
            }
        }
        if visit(value, target, &mut BTreeSet::new(), 0) {
            return block(at, "Nested/reference-valued cyclic mutation unsupported");
        }
        Ok(())
    }
}
