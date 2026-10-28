use super::*;
impl Context<'_> {
    pub(super) fn data_record_writable(&self, record: &Rc<RefCell<Record>>, at: &Node) -> R<()> {
        if self
            .trigger_readonly
            .contains(&(Rc::as_ptr(record) as usize))
        {
            return exceptions::raise(
                at,
                "System.FinalException",
                "Record is read-only in this trigger context",
            );
        }
        Ok(())
    }
    pub(super) fn fire_data_triggers(
        &mut self,
        object: &str,
        event: &str,
        new: &[Rc<RefCell<Record>>],
        old: &[Rc<RefCell<Record>>],
        at: &Node,
    ) -> R<()> {
        let event = event.to_ascii_lowercase();
        let triggers = self
            .program
            .triggers
            .iter()
            .filter(|t| {
                t.object.eq_ignore_ascii_case(object)
                    && t.events
                        .iter()
                        .any(|e| e.split_whitespace().collect::<Vec<_>>().join(" ") == event)
            })
            .cloned()
            .collect::<Vec<_>>();
        if triggers.is_empty() {
            return Ok(());
        }
        if self.trigger_depth >= 16 {
            return block(at, "Trigger recursion exceeds16 nested invocations");
        }
        if new.len().max(old.len()) > 200 {
            return block(
                at,
                "Trigger batching above200 rows must be split by the DML coordinator",
            );
        }
        let before = event.starts_with("before ");
        let action = event.split_whitespace().nth(1).unwrap_or("");
        let has_new = action != "delete";
        let has_old = action == "update" || action == "delete";
        let has_new_map = !self
            .program
            .schema
            .object(object)
            .is_some_and(|o| o.platform_event)
            && (action == "update" || action == "undelete" || (action == "insert" && !before));
        let prior_context = std::mem::take(&mut self.trigger_context);
        let prior_sharing = self.sharing_context;
        self.sharing_context = false;
        let prior_readonly = self.trigger_readonly.clone();
        let prior_locals = std::mem::replace(&mut self.locals, vec![BTreeMap::new()]);
        let prior_types = std::mem::replace(&mut self.local_types, vec![BTreeMap::new()]);
        let prior_class = std::mem::take(&mut self.current_class);
        let prior_this = self.this.take();
        let mut locked = vec![];
        for r in old {
            self.trigger_readonly.insert(Rc::as_ptr(r) as usize);
        }
        if !before {
            for r in new {
                self.trigger_readonly.insert(Rc::as_ptr(r) as usize);
            }
        }
        let list = |records: &[Rc<RefCell<Record>>]| {
            Value::list(records.iter().cloned().map(Value::Record).collect())
        };
        let new_list = if has_new { list(new) } else { Value::Null };
        let old_list = if has_old { list(old) } else { Value::Null };
        for value in [&new_list, &old_list] {
            if let Value::List(v) = value {
                let id = Rc::as_ptr(v) as usize;
                *self.iteration_locks.entry(id).or_default() += 1;
                locked.push(id);
                self.register_list_contract(v, object);
            }
        }
        let map = |records: &[Rc<RefCell<Record>>]| {
            Value::Map(Rc::new(RefCell::new(
                records
                    .iter()
                    .filter_map(|record| {
                        record
                            .borrow()
                            .fields
                            .get("id")
                            .cloned()
                            .map(|id| (id, Value::Record(record.clone())))
                    })
                    .collect(),
            )))
        };
        let new_map = if has_new_map { map(new) } else { Value::Null };
        let old_map = if has_old { map(old) } else { Value::Null };
        for value in [&new_map, &old_map] {
            if let Value::Map(v) = value {
                let id = Rc::as_ptr(v) as usize;
                *self.iteration_locks.entry(id).or_default() += 1;
                locked.push(id);
                self.register_map_contract(v, "Id", object);
            }
        }
        self.trigger_context.insert("new".into(), new_list);
        self.trigger_context.insert("old".into(), old_list);
        self.trigger_context.insert("newmap".into(), new_map);
        self.trigger_context.insert("oldmap".into(), old_map);
        for (name, value) in [
            ("isexecuting", true),
            ("isbefore", before),
            ("isafter", !before),
            ("isinsert", action == "insert"),
            ("isupdate", action == "update"),
            ("isdelete", action == "delete"),
            ("isundelete", action == "undelete"),
        ] {
            self.trigger_context.insert(name.into(), Value::Bool(value));
        }
        self.trigger_context.insert(
            "size".into(),
            Value::Num(Number::integer(new.len().max(old.len()) as i64)),
        );
        let operation = event.replace(' ', "_").to_ascii_uppercase();
        let ordinal = [
            "BEFORE_INSERT",
            "BEFORE_UPDATE",
            "BEFORE_DELETE",
            "AFTER_INSERT",
            "AFTER_UPDATE",
            "AFTER_DELETE",
            "AFTER_UNDELETE",
        ]
        .iter()
        .position(|x| *x == operation)
        .unwrap_or(0);
        self.trigger_context.insert(
            "operationtype".into(),
            Value::Enum {
                typ: "System.TriggerOperation".into(),
                name: operation,
                ordinal,
            },
        );
        self.trigger_depth += 1;
        let outcome = (|| {
            for trigger in triggers {
                self.current_class = format!("<trigger:{}>", trigger.name);
                match self.exec(&trigger.body)? {
                    Flow::Next => {}
                    _ => return block(&trigger.body, "Control flow escaped trigger body"),
                }
            }
            Ok(())
        })();
        self.trigger_depth -= 1;
        for id in locked {
            if let Some(count) = self.iteration_locks.get_mut(&id) {
                *count -= 1;
                if *count == 0 {
                    self.iteration_locks.remove(&id);
                }
            }
        }
        self.trigger_context = prior_context;
        self.trigger_readonly = prior_readonly;
        self.locals = prior_locals;
        self.local_types = prior_types;
        self.current_class = prior_class;
        self.this = prior_this;
        self.sharing_context = prior_sharing;
        outcome
    }
}
