use super::*;
pub(super) enum Location {
    Local(usize, String, String),
    Static(String, String),
    Native(String),
    Instance(Rc<RefCell<Instance>>, String, String),
    Record(Rc<RefCell<Record>>, String, String),
    List(Rc<RefCell<Vec<Value>>>, usize, String),
}
impl Context<'_> {
    pub(super) fn resolve_location(&mut self, n: &Node) -> R<Location> {
        match n.kind.as_str() {
            "identifier" => {
                let name = n.text().to_ascii_lowercase();
                for i in (0..self.locals.len()).rev() {
                    if self.locals[i].contains_key(&name) {
                        return Ok(Location::Local(
                            i,
                            name.clone(),
                            self.local_types[i][&name].clone(),
                        ));
                    }
                }
                if let Some(i) = &self.this {
                    if let Some(typ) = i.borrow().field_types.get(&name) {
                        self.check_field_access(&i.borrow().class, &name, false, n)?;
                        return Ok(Location::Instance(i.clone(), name, typ.clone()));
                    }
                }
                let key = format!("{}.{name}", self.current_class);
                if let Some(typ) = self.static_types.get(&key) {
                    return Ok(Location::Static(key, typ.clone()));
                }
                fail(n, format!("Assignment to undeclared variable {}", n.text()))
            }
            "field_access" => {
                let receiver = self.eval(self.need(n, "object")?)?;
                let name = self.need(n, "field")?.text().to_ascii_lowercase();
                match receiver {
                    Value::Instance(i) => {
                        self.check_field_access(&i.borrow().class, &name, false, n)?;
                        let typ = i.borrow().field_types.get(&name).cloned().ok_or_else(|| {
                            EvalError {
                                exception: None,
                                status: "blocked",
                                diagnostic: n.diagnostic(
                                    "APX_UNSUPPORTED",
                                    format!("Unknown instance field {name}"),
                                ),
                            }
                        })?;
                        Ok(Location::Instance(i, name, typ))
                    }
                    Value::Record(r) => {
                        let typ = r.borrow().typ.clone();
                        let field =
                            self.record_field_type(&typ, &name)
                                .ok_or_else(|| EvalError {
                                    exception: None,
                                    status: "blocked",
                                    diagnostic: n.diagnostic(
                                        "APX_UNSUPPORTED",
                                        format!("Unknown SObject field {typ}.{name}"),
                                    ),
                                })?;
                        Ok(Location::Record(r, name, field))
                    }
                    Value::Namespace(class) if class == "restcontext" => Ok(Location::Native(name)),
                    Value::Namespace(class) => {
                        self.initialize(&class, n)?;
                        self.check_field_access(&class, &name, true, n)?;
                        let key = format!("{class}.{name}");
                        let typ =
                            self.static_types
                                .get(&key)
                                .cloned()
                                .ok_or_else(|| EvalError {
                                    exception: None,
                                    status: "blocked",
                                    diagnostic: n
                                        .diagnostic("APX_UNSUPPORTED", "Unknown static field"),
                                })?;
                        Ok(Location::Static(key, typ))
                    }
                    Value::Null => {
                        exceptions::raise(n, "System.NullPointerException", "Null field receiver")
                    }
                    _ => block(n, "Unsupported field assignment"),
                }
            }
            "array_access" => {
                let receiver = self.eval(self.need(n, "array")?)?;
                let index = self.eval(self.need(n, "index")?)?;
                let (Value::List(list), Value::Num(index)) = (receiver, index) else {
                    return fail(n, "Invalid list assignment");
                };
                let index = index.index().ok_or_else(|| EvalError {
                    exception: None,
                    status: "failed",
                    diagnostic: n.diagnostic("APX_INDEX", "Invalid list index"),
                })?;
                if index >= list.borrow().len() {
                    return exceptions::raise(
                        n,
                        "System.ListException",
                        "List index out of bounds",
                    );
                }
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
                Ok(Location::List(list, index, typ))
            }
            _ => block(n, "Unsupported assignment target"),
        }
    }
    pub(super) fn read_location(&mut self, loc: &Location, at: &Node) -> R<Value> {
        if let Location::Instance(i, k, _) = loc {
            if let Some(v) = self.property_get(&Value::Instance(i.clone()), k, at)? {
                return Ok(v);
            }
        }
        if let Location::Static(key, _) = loc {
            let (owner, k) = key.rsplit_once('.').unwrap();
            if let Some(v) = self.property_get(&Value::Namespace(owner.into()), k, at)? {
                return Ok(v);
            }
        }
        Ok(match loc {
            Location::Native(k) => self
                .native_state
                .get(&format!("restcontext.{k}"))
                .cloned()
                .unwrap_or(Value::Null),
            Location::Local(i, k, _) => self.locals[*i][k].clone(),
            Location::Static(k, _) => self.statics[k].clone(),
            Location::Instance(i, k, _) => i.borrow().fields.get(k).cloned().unwrap_or(Value::Null),
            Location::Record(r, k, _) => r.borrow().fields.get(k).cloned().unwrap_or(Value::Null),
            Location::List(list, i, _) => list.borrow()[*i].clone(),
        })
    }
    pub(super) fn write_location(&mut self, loc: Location, value: Value, at: &Node) -> R<Value> {
        if let Location::Native(name) = loc {
            self.http_context_set(&name, value.clone(), at)?;
            return Ok(value);
        }
        let typ = match &loc {
            Location::Local(_, _, t)
            | Location::Static(_, t)
            | Location::Instance(_, _, t)
            | Location::Record(_, _, t)
            | Location::List(_, _, t) => t,
            _ => unreachable!(),
        };
        let value = self.coerce(value, typ, at)?;
        if let Location::Instance(i, k, _) = &loc {
            if self.property_set(&Value::Instance(i.clone()), k, &value, at)? {
                return Ok(value);
            }
        }
        if let Location::Static(key, _) = &loc {
            let (owner, k) = key.rsplit_once('.').unwrap();
            if self.property_set(&Value::Namespace(owner.into()), k, &value, at)? {
                return Ok(value);
            }
        }
        match loc {
            Location::Local(i, k, _) => {
                self.locals[i].insert(k, value.clone());
            }
            Location::Static(k, _) => {
                self.statics.insert(k, value.clone());
            }
            Location::Instance(i, k, _) => {
                self.reject_cycle(&value, Rc::as_ptr(&i) as usize, at)?;
                i.borrow_mut().fields.insert(k, value.clone());
            }
            Location::Record(r, k, _) => {
                self.data_record_writable(&r, at)?;
                if !value.is_scalar() && !matches!(value, Value::Record(_)) {
                    return block(at, "Unsupported reference-valued SObject field");
                }
                self.reject_cycle(&value, Rc::as_ptr(&r) as usize, at)?;
                r.borrow_mut().fields.insert(k, value.clone());
            }
            Location::List(list, i, _) => {
                self.ensure_list_writable(&list, at)?;
                self.reject_cycle(&value, Rc::as_ptr(&list) as usize, at)?;
                list.borrow_mut()[i] = value.clone();
            }
            Location::Native(_) => unreachable!(),
        }
        Ok(value)
    }
}
