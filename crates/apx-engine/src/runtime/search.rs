//! Test-scoped SOSL fixed results and real permission-group recalculation.
//! Search terms are evaluated, but no production search index is simulated.
use super::*;
use crate::{
    record_store::{DataValue, StoreError},
    soql::{self, Bindings, Predicate, QueryAccess, QueryMode, QueryResult, Selection, ValueExpr},
};

const FIXED: &str = "search.fixedIds";
const COUNT: &str = "search.count";
const SAVED: &str = "search.savedCount";

fn error(at: &Node, e: StoreError) -> EvalError {
    if e.governor.is_some() {
        EvalError {
            exception: None,
            status: "failed",
            diagnostic: at.diagnostic("APX_GOVERNOR", e.to_string()),
        }
    } else if e.unsupported {
        EvalError {
            exception: None,
            status: "blocked",
            diagnostic: at.diagnostic("APX_UNSUPPORTED", e.to_string()),
        }
    } else {
        EvalError {
            exception: Some(Value::Exception(Rc::new(RefCell::new(ExceptionValue {
                typ: "System.QueryException".into(),
                message: e.to_string(),
                stack: format!("APX local {}:{}", at.path, at.line()),
            })))),
            status: "failed",
            diagnostic: at.diagnostic("APX_SEARCH", e.to_string()),
        }
    }
}

impl Context<'_> {
    pub(super) fn search_test_method(
        &mut self,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        match name {
            "setfixedsearchresults" => {
                let [Value::List(values)] = args else {
                    return block(at, "Test.setFixedSearchResults requires List<Id>");
                };
                if values.borrow().len() > 2000 {
                    return block(
                        at,
                        "Fixed search results exceed the supported 2000-ID bound",
                    );
                }
                let mut ids = vec![];
                let mut seen = BTreeSet::new();
                for value in values.borrow().iter() {
                    let Value::Str(id) = self.coerce(value.clone(), "Id", at)? else {
                        return exceptions::raise(
                            at,
                            "System.StringException",
                            "Fixed search IDs cannot be null",
                        );
                    };
                    if seen.insert(id.clone()) {
                        ids.push(Value::Str(id));
                    }
                }
                self.native_state.insert(FIXED.into(), Value::list(ids));
                self.native_state.remove("search.unqualifiedAsyncFixed");
                Ok(Some(Value::Null))
            }
            "calculatepermissionsetgroup" => {
                let [id] = args else {
                    return block(at, "Test.calculatePermissionSetGroup requires one Id");
                };
                let Value::Str(id) = self.coerce(id.clone(), "Id", at)? else {
                    return exceptions::raise(
                        at,
                        "System.NullPointerException",
                        "Permission set group Id cannot be null",
                    );
                };
                self.ensure_data_store(at)?;
                let store = self.data_store.as_ref().unwrap();
                if store.find("PermissionSetGroup", &id).is_none() {
                    return exceptions::raise(
                        at,
                        "System.InvalidParameterValueException",
                        "Permission set group does not exist",
                    );
                }
                if store
                    .schema
                    .blockers
                    .iter()
                    .any(|b| b.capability == "security")
                {
                    return block(
                        at,
                        "Permission set group calculation depends on unresolved security metadata",
                    );
                }
                let members = store
                    .records("PermissionSetGroupComponent", false)
                    .into_iter()
                    .filter(|m| {
                        m.get("PermissionSetGroupId").and_then(DataValue::text) == Some(&id)
                    })
                    .collect::<Vec<_>>();
                if members.len() > 1000 {
                    return block(at, "Permission set group exceeds 1000-member host bound");
                }
                for member in members {
                    let Some(member_id) = member.get("PermissionSetId").and_then(DataValue::text)
                    else {
                        return block(at, "Permission set group member has no PermissionSetId");
                    };
                    let Some(permission) = store.find("PermissionSet", member_id) else {
                        return block(
                            at,
                            "Permission set group member references missing permissions",
                        );
                    };
                    let Some(name) = permission.get("Name").and_then(DataValue::text) else {
                        return block(at, "Permission set identity is unavailable");
                    };
                    if !store
                        .schema
                        .permission_sets
                        .contains_key(&name.to_ascii_lowercase())
                    {
                        return block(
                            at,
                            "Permission set group calculation requires loaded permission metadata",
                        );
                    }
                }
                // Access resolution unions the actual member metadata only when
                // this status is Updated. Membership edits invalidate it in DML.
                let group = crate::record_store::RecordData::new("PermissionSetGroup")
                    .with("Id", DataValue::Text(id))
                    .with("Status", DataValue::Text("Updated".into()));
                self.data_store
                    .as_mut()
                    .unwrap()
                    .upsert_platform_record(&group)
                    .map_err(|e| error(at, e))?;
                Ok(Some(Value::Null))
            }
            _ => Ok(None),
        }
    }
    pub(super) fn search_limits_method(
        &self,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        if !["getsoslqueries", "getlimitsoslqueries"].contains(&name) {
            return Ok(None);
        }
        if !args.is_empty() {
            return block(at, "SOSL Limits getter requires no arguments");
        }
        Ok(Some(if name == "getlimitsoslqueries" {
            Value::Num(Number::integer(20))
        } else {
            self.native_state
                .get(COUNT)
                .cloned()
                .unwrap_or(Value::Num(Number::integer(0)))
        }))
    }
    pub(super) fn search_start_test(&mut self) {
        let old = self
            .native_state
            .get(COUNT)
            .cloned()
            .unwrap_or(Value::Num(Number::integer(0)));
        self.native_state.insert(SAVED.into(), old);
        self.native_state
            .insert(COUNT.into(), Value::Num(Number::integer(0)));
    }
    pub(super) fn search_stop_test(&mut self) {
        if let Some(old) = self.native_state.remove(SAVED) {
            self.native_state.insert(COUNT.into(), old);
        }
    }
    pub(super) fn search_query(&mut self, at: &Node) -> R<Value> {
        if self
            .native_state
            .contains_key("search.unqualifiedAsyncFixed")
        {
            return block(at,"SOSL fixed-result inheritance across asynchronous test transactions is not qualified");
        }
        if at.text().len() > 100_000 {
            return block(at, "SOSL exceeds 100000-character bound");
        }
        let body = at.child("sosl_query_body").unwrap_or(at);
        for child in body.named() {
            if !["find_clause", "in_clause", "returning_clause"].contains(&child.kind.as_str()) {
                return block(child, format!("SOSL {} is not implemented", child.kind));
            }
        }
        let find = body
            .child("find_clause")
            .ok_or_else(|| error(at, StoreError::unsupported("SOSL FIND clause missing")))?;
        if let Some(bound) = find.child("bound_apex_expression") {
            let expression = bound
                .named()
                .next()
                .ok_or_else(|| error(bound, StoreError::unsupported("Missing search bind")))?;
            let value = self.eval(expression)?;
            match value {
                Value::Str(s) if s.len() <= 10_000 && !s.is_empty() => {}
                Value::Null => {
                    return exceptions::raise(
                        find,
                        "System.QueryException",
                        "Search term cannot be null",
                    )
                }
                Value::Str(_) => {
                    return block(
                        find,
                        "Search term length outside qualified test-search range",
                    )
                }
                _ => return block(find, "SOSL FIND bind requires String"),
            }
        } else if find.child("term").is_none() {
            return block(find, "Unsupported SOSL FIND expression");
        }
        let returns = body
            .named()
            .filter(|n| n.kind == "returning_clause")
            .flat_map(|n| n.named().filter(|n| n.kind == "sobject_return"))
            .collect::<Vec<_>>();
        if returns.is_empty() || returns.len() > 100 {
            return block(
                at,
                "SOSL requires 1..100 explicit RETURNING object projections",
            );
        }
        let mut queries = vec![];
        for returned in returns {
            for child in returned.named() {
                if ![
                    "identifier",
                    "selected_fields",
                    "where_clause",
                    "order_by_clause",
                    "limit_clause",
                    "offset_clause",
                ]
                .contains(&child.kind.as_str())
                {
                    return block(
                        child,
                        format!("SOSL projection {} is not implemented", child.kind),
                    );
                }
            }
            let object = returned
                .child("identifier")
                .ok_or_else(|| {
                    error(
                        returned,
                        StoreError::unsupported("Missing returning object"),
                    )
                })?
                .text();
            if ["contentdocument", "contentnote"].contains(&object.to_ascii_lowercase().as_str()) {
                return block(returned,"SOSL file/note results require ContentVersion-to-document search-index mapping, which is not implemented");
            }
            let fields = returned
                .child("selected_fields")
                .map(Node::text)
                .unwrap_or("Id");
            let mut text = format!("SELECT {fields} FROM {object}");
            for clause in [
                "where_clause",
                "order_by_clause",
                "limit_clause",
                "offset_clause",
            ] {
                if let Some(n) = returned.child(clause) {
                    text.push(' ');
                    text.push_str(n.text());
                }
            }
            let query = soql::parse(&text).map_err(|e| error(returned, e))?;
            if query
                .selections
                .iter()
                .any(|s| !matches!(s, Selection::Field(_)))
            {
                return block(
                    returned,
                    "SOSL RETURNING supports explicit scalar/relationship fields only",
                );
            }
            let mut names = BTreeSet::new();
            data::query_binds(&query, &mut names);
            let mut bindings = Bindings::new();
            for name in names {
                let value = self.resolve_data_bind(&name, returned)?;
                bindings.insert(name, data::to_data(&value, returned, 0)?);
            }
            queries.push((query, bindings));
        }
        self.ensure_data_store(at)?;
        let count = match self.native_state.get(COUNT) {
            Some(Value::Num(n)) => n.q.to_integer().to_i64().unwrap_or(20),
            _ => 0,
        } + 1;
        self.native_state
            .insert(COUNT.into(), Value::Num(Number::integer(count)));
        if count > 20 {
            return exceptions::raise(at, "System.LimitException", "Too many SOSL queries: 21");
        }
        let ids = match self.native_state.get(FIXED) {
            Some(Value::List(ids)) => ids.borrow().clone(),
            _ => vec![],
        };
        let fixed = ValueExpr::List(
            ids.iter()
                .map(|id| ValueExpr::Literal(DataValue::Text(id.display())))
                .collect(),
        );
        let user_mode = self
            .program
            .api_versions
            .get(at.path.as_ref())
            .is_some_and(|v| *v >= 67);
        let mut policy = if user_mode || self.effective_sharing() {
            Some(self.data_access(at)?.query)
        } else {
            None
        };
        if !user_mode {
            // System mode still obeys with-sharing row visibility; CRUD/FLS are
            // not enforced. Supply complete field access while retaining rows.
            if let Some(policy) = &mut policy {
                policy.readable_fields = self
                    .program
                    .schema
                    .objects
                    .iter()
                    .map(|(name, obj)| (name.clone(), obj.fields.keys().cloned().collect()))
                    .collect();
            }
        }
        let mut lists = vec![];
        let mut total = 0;
        for (mut query, bindings) in queries {
            let selected = Predicate::Compare {
                field: "Id".into(),
                operator: "IN".into(),
                value: fixed.clone(),
            };
            query.predicate = Some(match query.predicate.take() {
                Some(other) => Predicate::And(Box::new(other), Box::new(selected)),
                None => selected,
            });
            query.mode = if policy.is_some() {
                QueryMode::User
            } else {
                QueryMode::System
            };
            let store = self.data_store.as_mut().unwrap();
            let previous = store.query_rows;
            store.query_rows = 0;
            let result = store.query(
                &query,
                &bindings,
                policy
                    .as_ref()
                    .map(QueryAccess::User)
                    .unwrap_or(QueryAccess::System),
            );
            store.query_rows = previous;
            let QueryResult::Rows(rows) = result.map_err(|e| error(at, e))? else {
                return block(at, "Unexpected aggregate SOSL result");
            };
            total += rows.len();
            if total > 2000 {
                return block(at, "SOSL results exceed the qualified 2000-record bound");
            }
            let values = rows
                .iter()
                .map(|r| data::from_data(&DataValue::Record(Box::new(r.clone())), at, 0))
                .collect::<R<Vec<_>>>()?;
            let list = Rc::new(RefCell::new(values));
            self.register_list_contract(&list, &query.object);
            lists.push(Value::List(list));
        }
        let outer = Rc::new(RefCell::new(lists));
        self.register_list_contract(&outer, "list<sobject>");
        Ok(Value::List(outer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn program(source: &str) -> (Program, Node) {
        let n = crate::syntax::parse(source, "search.cls").unwrap();
        let mut p = Program::default();
        crate::syntax::declarations(&n, &mut p, &mut vec![]);
        (p, n)
    }
    #[test]
    fn fixed_results_apply_actual_projection_filter_limit_and_bind_values() {
        let (p,n)=program("class Probe{static void run(){Account a=new Account(Name='alpha');Account b=new Account(Name='beta');insert new List<Account>{a,b};Contact c=new Contact(LastName='different term',AccountId=a.Id);insert c;List<Id> fixed=new List<Id>{a.Id,b.Id,c.Id};Test.setFixedSearchResults(fixed);fixed.clear();String term='does not match';List<List<SObject>> rows=[FIND :term IN ALL FIELDS RETURNING Account(Name WHERE Name='beta' LIMIT 1),Contact(LastName,Account.Name)];System.assertEquals(1,rows[0].size());System.assertEquals('beta',((Account)rows[0][0]).Name);System.assertEquals('alpha',((Contact)rows[1][0]).Account.Name);System.assertEquals(1,Limits.getSoslQueries());System.assertEquals(0,Limits.getQueries());Test.setFixedSearchResults(new List<Id>{a.Id});rows=[FIND :term RETURNING Account(Name WHERE Name='beta'),Contact(LastName)];System.assertEquals(0,rows[0].size());System.assertEquals(0,rows[1].size());}}");
        Context::new(&p).call("probe", "run", vec![], &n).unwrap();
    }
    #[test]
    fn no_fixed_results_are_empty_but_bad_fields_and_terms_are_not_hidden() {
        let (p,n)=program("class Probe{static void run(){insert new Account(Name='alpha');String term='alpha';List<List<SObject>> rows=[FIND :term RETURNING Account(Name)];System.assertEquals(0,rows[0].size());}}");
        Context::new(&p).call("probe", "run", vec![], &n).unwrap();
        for source in ["class Probe{static void run(){String term='x';Object rows=[FIND :term RETURNING Account(NotAField)];}}","class Probe{static void run(){Integer term=1;Object rows=[FIND :term RETURNING Account(Name)];}}"] {
            let(p,n)=program(source);assert!(Context::new(&p).call("probe","run",vec![],&n).is_err());
        }
    }
    #[test]
    fn search_limits_reset_inside_start_stop_and_cannot_be_caught() {
        let(p,n)=program("class Probe{static void run(){String term='x';Object rows=[FIND :term RETURNING Account(Id)];Test.startTest();System.assertEquals(0,Limits.getSoslQueries());rows=[FIND :term RETURNING Account(Id)];Test.stopTest();System.assertEquals(1,Limits.getSoslQueries());try{for(Integer i=0;i<20;i++){rows=[FIND :term RETURNING Account(Id)];}}catch(Exception ignored){return;}}}");
        let e = Context::new(&p)
            .call("probe", "run", vec![], &n)
            .unwrap_err();
        assert!(e.diagnostic.message.contains("Too many SOSL"));
        assert_eq!(e.status, "failed");
        assert_eq!(e.diagnostic.code, "APX_GOVERNOR");
        assert!(e.exception.is_none());
    }
    #[test]
    fn permission_calculation_enables_real_member_grants_and_removed_member_removes_access() {
        use crate::{
            environment::Environment,
            record_store::{DmlOperation, RecordData},
            schema::{FieldPermission, ObjectPermission, PermissionSet},
        };
        let (mut p, n) = program("class Probe{}");
        p.schema.permission_sets.insert(
            "reader".into(),
            PermissionSet {
                name: "Reader".into(),
                label: "Reader".into(),
                objects: BTreeMap::from([(
                    "account".into(),
                    ObjectPermission {
                        read: true,
                        ..Default::default()
                    },
                )]),
                fields: BTreeMap::from([(
                    "account.name".into(),
                    FieldPermission {
                        read: true,
                        edit: false,
                    },
                )]),
                ..Default::default()
            },
        );
        let json = serde_json::json!({"schemaVersion":1,"clock":{"now":"2026-01-15T12:00:00Z"},"currentUserId":"005000000000001AAA","records":[{"object":"User","fields":{"Id":"005000000000001AAA","ProfileId":"00e000000000001AAA"}},{"object":"Profile","fields":{"Id":"00e000000000001AAA","Name":"Explicit synthetic minimum"}}],"security":{"mode":"synthetic-local","defaultSharingModel":"Private","profilePolicies":[{"profileId":"00e000000000001AAA","objects":{"selector":"all-registered-objects","read":false},"fields":{"selector":"all-permissionable-fields","read":false}}]}});
        p.environment = Some(Environment::parse(&json, &p.schema, "search unit fixture").unwrap());
        let mut c = Context::new(&p);
        c.ensure_data_store(&n).unwrap();
        let store = c.data_store.as_mut().unwrap();
        let permission = store.records("PermissionSet", false)[0]
            .id()
            .unwrap()
            .to_string();
        let group = store
            .upsert_platform_record(
                &RecordData::new("PermissionSetGroup")
                    .with("DeveloperName", DataValue::Text("Group".into()))
                    .with("Status", DataValue::Text("Outdated".into())),
            )
            .unwrap();
        let member = RecordData::new("PermissionSetGroupComponent")
            .with("PermissionSetId", DataValue::Text(permission))
            .with("PermissionSetGroupId", DataValue::Text(group.clone()));
        let member = store.apply(DmlOperation::Insert, &[member], true).unwrap()[0]
            .id
            .clone()
            .unwrap();
        c.search_test_method(
            "calculatepermissionsetgroup",
            &[Value::Str(group.clone())],
            &n,
        )
        .unwrap();
        c.data_store
            .as_mut()
            .unwrap()
            .apply(
                DmlOperation::Insert,
                &[RecordData::new("PermissionSetAssignment")
                    .with("PermissionSetGroupId", DataValue::Text(group.clone()))
                    .with("AssigneeId", DataValue::Text("005000000000001AAA".into()))],
                true,
            )
            .unwrap();
        assert!(c.data_access(&n).unwrap().objects["account"].read);
        c.data_store
            .as_mut()
            .unwrap()
            .apply(
                DmlOperation::Delete,
                &[RecordData::new("PermissionSetGroupComponent")
                    .with("Id", DataValue::Text(member))],
                true,
            )
            .unwrap();
        c.search_test_method("calculatepermissionsetgroup", &[Value::Str(group)], &n)
            .unwrap();
        assert!(!c.data_access(&n).unwrap().objects["account"].read);
    }
}
