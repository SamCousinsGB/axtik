use super::*;
impl Context<'_> {
    fn property(
        &self,
        receiver: &Value,
        name: &str,
        at: &Node,
    ) -> R<Option<(String, Node, String)>> {
        let (class, is_static) = match receiver {
            Value::Instance(i) => (i.borrow().class.clone(), false),
            Value::Namespace(s) => (s.clone(), true),
            _ => return Ok(None),
        };
        if !self
            .program
            .classes
            .contains_key(&class.to_ascii_lowercase())
        {
            return Ok(None);
        }
        for owner in self.class_chain(&class, at)? {
            let c = &self.program.classes[&owner];
            for field in if is_static {
                &c.fields
            } else {
                &c.instance_fields
            } {
                if field.child("accessor_list").is_some()
                    && field.fields("declarator").any(|d| {
                        d.field("name")
                            .is_some_and(|n| n.text().eq_ignore_ascii_case(name))
                    })
                {
                    let typ = self.need(field, "type")?.text().into();
                    return Ok(Some((owner, field.clone(), typ)));
                }
            }
        }
        Ok(None)
    }
    fn property_key(receiver: &Value, owner: &str, name: &str) -> String {
        format!(
            "{}:{owner}.{name}",
            match receiver {
                Value::Instance(i) => Rc::as_ptr(i) as usize,
                _ => 0,
            }
        )
    }
    pub(super) fn property_get(
        &mut self,
        receiver: &Value,
        name: &str,
        at: &Node,
    ) -> R<Option<Value>> {
        let Some((owner, field, typ)) = self.property(receiver, name, at)? else {
            return Ok(None);
        };
        let key = Self::property_key(receiver, &owner, name);
        if matches!(receiver, Value::Namespace(_)) {
            self.initialize(&owner, at)?;
        }
        let getter = field
            .child("accessor_list")
            .unwrap()
            .named()
            .find(|a| {
                a.field("accessor")
                    .is_some_and(|n| n.text().eq_ignore_ascii_case("get"))
            })
            .ok_or_else(|| EvalError {
                exception: None,
                status: "failed",
                diagnostic: at.diagnostic("APX_PROPERTY", "Property has no getter"),
            })?;
        self.check_property_access(&owner, &field, getter, at)?;
        if self.current_class.eq_ignore_ascii_case(&owner) && self.active_properties.contains(&key)
        {
            return Ok(None);
        }
        let Some(body) = getter.field("body") else {
            return Ok(Some(match receiver {
                Value::Instance(i) => i.borrow().fields.get(name).cloned().unwrap_or(Value::Null),
                _ => self
                    .statics
                    .get(&format!("{owner}.{name}"))
                    .cloned()
                    .unwrap_or(Value::Null),
            }));
        };
        let method = Method {
            name: format!("get_{name}"),
            params: vec![],
            return_type: typ,
            body: Some(body.clone()),
            declaration: getter.clone(),
            test: false,
            setup: false,
            static_method: matches!(receiver, Value::Namespace(_)),
        };
        self.active_properties.push(key);
        let result = self.invoke_method(
            &owner,
            &method,
            vec![],
            if let Value::Instance(i) = receiver {
                Some(i.clone())
            } else {
                None
            },
        );
        self.active_properties.pop();
        result.map(Some)
    }
    pub(super) fn property_set(
        &mut self,
        receiver: &Value,
        name: &str,
        value: &Value,
        at: &Node,
    ) -> R<bool> {
        let Some((owner, field, typ)) = self.property(receiver, name, at)? else {
            return Ok(false);
        };
        let key = Self::property_key(receiver, &owner, name);
        let setter = field
            .child("accessor_list")
            .unwrap()
            .named()
            .find(|a| {
                a.field("accessor")
                    .is_some_and(|n| n.text().eq_ignore_ascii_case("set"))
            })
            .ok_or_else(|| EvalError {
                exception: None,
                status: "failed",
                diagnostic: at.diagnostic("APX_PROPERTY", "Property has no setter"),
            })?;
        self.check_property_access(&owner, &field, setter, at)?;
        if self.current_class.eq_ignore_ascii_case(&owner) && self.active_properties.contains(&key)
        {
            return Ok(false);
        }
        let Some(body) = setter.field("body") else {
            return Ok(false);
        };
        let method = Method {
            name: format!("set_{name}"),
            params: vec![("value".into(), typ)],
            return_type: "void".into(),
            body: Some(body.clone()),
            declaration: setter.clone(),
            test: false,
            setup: false,
            static_method: matches!(receiver, Value::Namespace(_)),
        };
        self.active_properties.push(key);
        let result = self.invoke_method(
            &owner,
            &method,
            vec![value.clone()],
            if let Value::Instance(i) = receiver {
                Some(i.clone())
            } else {
                None
            },
        );
        self.active_properties.pop();
        result.map(|_| true)
    }
    fn check_property_access(
        &self,
        owner: &str,
        field: &Node,
        accessor: &Node,
        at: &Node,
    ) -> R<()> {
        let modifiers = accessor
            .child("modifiers")
            .or_else(|| field.child("modifiers"))
            .map(|m| m.text().to_ascii_lowercase())
            .unwrap_or_default();
        if !self.visible(owner, &modifiers) {
            return block(at, "Non-public property accessor is not accessible");
        }
        Ok(())
    }
}
