use super::*;
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stripping_removes_projection_fields_without_mutating_source() {
        let source="@IsTest class SecurityProjection { @IsTest static void exercise() { Account input = new Account(Name='A', Description='secret'); SObjectAccessDecision decision = Security.stripInaccessible(AccessType.READABLE, new List<Account>{input}); List<SObject> safe = decision.getRecords(); System.assertEquals('A',safe[0].get('Name')); System.assertEquals('secret',input.Description); System.assertEquals(true,decision.getRemovedFields().get('Account').contains('Description')); try { Object hidden = safe[0].get('Description'); System.assert(false,'field should be absent'); } catch(System.SObjectException e) { } } }";
        let tree = crate::syntax::parse(source, "security-projection.cls").unwrap();
        let mut program = Program::default();
        let mut diagnostics = vec![];
        crate::syntax::declarations(&tree, &mut program, &mut diagnostics);
        assert!(diagnostics.is_empty());
        let json = serde_json::json!({"schemaVersion":1,"clock":{"now":"2026-01-15T12:00:00Z"},"currentUserId":"005000000000001AAA","records":[{"object":"User","fields":{"Id":"005000000000001AAA","ProfileId":"00e000000000001AAA"}},{"object":"Profile","fields":{"Id":"00e000000000001AAA","Name":"Synthetic"}}],"security":{"mode":"synthetic-local","defaultSharingModel":"Private","profilePolicies":[{"profileId":"00e000000000001AAA","objects":{"selector":"all-registered-objects","read":true},"fields":{"selector":"all-registered-fields","read":true,"exclude":["Account.Description"]}}]}});
        program.environment =
            Some(crate::environment::Environment::parse(&json, &program.schema, "unit").unwrap());
        let mut context = Context::new(&program);
        context
            .call("SecurityProjection", "exercise", vec![], &tree)
            .unwrap();
    }
}
impl Context<'_> {
    pub(super) fn security_method(
        &mut self,
        receiver: &Value,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        if let Value::Instance(instance) = receiver {
            let instance = instance.borrow();
            if instance.class == "system.sobjectaccessdecision" && args.is_empty() {
                return match name {
                    "getrecords" => Ok(instance.fields.get("records").cloned()),
                    "getremovedfields" => Ok(instance.fields.get("removedfields").cloned()),
                    _ => Ok(None),
                };
            }
        }
        let Value::Namespace(namespace) = receiver else {
            return Ok(None);
        };
        if namespace.trim_start_matches("system.") != "security" || name != "stripinaccessible" {
            return Ok(None);
        }
        if args.len() < 2 || args.len() > 3 {
            return fail(
                at,
                "stripInaccessible expects AccessType, records, and optional root CRUD flag",
            );
        }
        let Value::Enum {
            typ, name: mode, ..
        } = &args[0]
        else {
            return fail(at, "stripInaccessible requires AccessType");
        };
        if !typ.eq_ignore_ascii_case("System.AccessType") && !typ.eq_ignore_ascii_case("AccessType")
        {
            return fail(at, "stripInaccessible requires AccessType");
        }
        if !["READABLE", "CREATABLE", "UPDATABLE", "UPSERTABLE"].contains(&mode.as_str()) {
            return fail(at, "Invalid AccessType");
        }
        let Value::List(records) = &args[1] else {
            return fail(at, "stripInaccessible requires List<SObject>");
        };
        let enforce = match args.get(2) {
            None => true,
            Some(Value::Bool(v)) => *v,
            _ => return fail(at, "Root access flag must be Boolean"),
        };
        let access = self.data_access(at)?;
        let mut removed = BTreeMap::<String, BTreeSet<String>>::new();
        let mut output = vec![];
        for value in records.borrow().iter() {
            let Value::Record(record) = value else {
                return fail(at, "stripInaccessible records must be SObjects");
            };
            output.push(self.strip_record(record, mode, enforce, &access, &mut removed, at, 0)?);
        }
        let output = self.coerce(Value::list(output), "List<SObject>", at)?;
        let entries = removed
            .into_iter()
            .map(|(object, fields)| {
                (
                    Value::Str(object),
                    Value::Set(Rc::new(RefCell::new(
                        fields.into_iter().map(Value::Str).collect(),
                    ))),
                )
            })
            .collect();
        let removed = self.coerce(
            Value::Map(Rc::new(RefCell::new(entries))),
            "Map<String,Set<String>>",
            at,
        )?;
        Ok(Some(Value::Instance(Rc::new(RefCell::new(Instance {
            class: "system.sobjectaccessdecision".into(),
            fields: BTreeMap::from([
                ("records".into(), output),
                ("removedfields".into(), removed),
            ]),
            field_types: BTreeMap::new(),
        })))))
    }
    fn strip_record(
        &self,
        record: &Rc<RefCell<Record>>,
        mode: &str,
        enforce: bool,
        access: &crate::access::ResolvedAccess,
        removed: &mut BTreeMap<String, BTreeSet<String>>,
        at: &Node,
        depth: usize,
    ) -> R<Value> {
        if depth > 32 {
            return block(at, "stripInaccessible relationship depth exceeds32");
        }
        let record = record.borrow();
        let object = self
            .program
            .schema
            .object(&record.typ)
            .ok_or_else(|| EvalError {
                exception: None,
                status: "blocked",
                diagnostic: at.diagnostic("APX_ACCESS", "Record schema unavailable"),
            })?;
        let permission = access
            .objects
            .get(&object.name.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default();
        let allowed = match mode {
            "READABLE" => permission.read,
            "CREATABLE" => permission.create,
            "UPDATABLE" => permission.edit,
            _ => permission.create && permission.edit,
        };
        if enforce && !allowed {
            return exceptions::raise(
                at,
                "System.NoAccessException",
                format!("No access to entity {}", object.name),
            );
        }
        let mut fields = BTreeMap::new();
        for (key, value) in &record.fields {
            if let Some(field) = object.field(key) {
                let grant = access
                    .fields
                    .get(&format!("{}.{}", object.name.to_ascii_lowercase(), key))
                    .cloned()
                    .unwrap_or_default();
                let permitted = key == "id"
                    || if mode == "READABLE" {
                        grant.read
                    } else {
                        grant.edit && allowed
                    };
                if permitted {
                    fields.insert(key.clone(), value.clone());
                } else {
                    removed
                        .entry(object.name.clone())
                        .or_default()
                        .insert(field.name.clone());
                }
            } else {
                match value {
                    Value::Record(child) => {
                        fields.insert(
                            key.clone(),
                            self.strip_record(child, mode, false, access, removed, at, depth + 1)?,
                        );
                    }
                    Value::List(children) => {
                        let mut safe = vec![];
                        for child in children.borrow().iter() {
                            let Value::Record(child) = child else {
                                return block(
                                    at,
                                    "Non-record relationship value in stripInaccessible",
                                );
                            };
                            safe.push(self.strip_record(
                                child,
                                mode,
                                false,
                                access,
                                removed,
                                at,
                                depth + 1,
                            )?);
                        }
                        fields.insert(key.clone(), Value::list(safe));
                    }
                    _ => {
                        return block(
                            at,
                            format!("Unknown field {}.{key} in stripInaccessible", object.name),
                        )
                    }
                }
            }
        }
        Ok(Value::Record(Rc::new(RefCell::new(Record {
            typ: object.name.clone(),
            fields,
            projected: true,
        }))))
    }
}
