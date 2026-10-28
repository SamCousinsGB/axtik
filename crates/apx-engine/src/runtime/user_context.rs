//! Explicit local user identity. No org identity or capability is guessed.
use super::*;
use crate::record_store::{DataValue, DmlOperation};
use rand::RngCore;

pub(super) fn request_is_type(typ: &str) -> bool {
    typ.to_ascii_lowercase().trim_start_matches("system.") == "request"
}

impl Context<'_> {
    pub(super) fn request_namespace(
        &mut self,
        ns: &str,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        if !request_is_type(ns) {
            return Ok(None);
        }
        if name != "getcurrent" || !args.is_empty() {
            return block(at, "Request supports getCurrent() only");
        }
        if let Some(request) = self.native_state.get("request.current") {
            return Ok(Some(request.clone()));
        }
        let mut bytes = [0u8; 16];
        rand::rngs::OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| EvalError {
                exception: None,
                status: "blocked",
                diagnostic: at.diagnostic(
                    "APX_RANDOM",
                    "Host randomness unavailable for local request identity",
                ),
            })?;
        let request = Value::Instance(Rc::new(RefCell::new(Instance {
            class: "request".into(),
            field_types: BTreeMap::new(),
            fields: BTreeMap::from([(
                "\0id".into(),
                Value::Str(format!("apx-local-{}", hex::encode(bytes))),
            )]),
        })));
        self.native_state
            .insert("request.current".into(), request.clone());
        Ok(Some(request))
    }
    pub(super) fn request_method(
        &self,
        receiver: &Value,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        let Value::Instance(request) = receiver else {
            return Ok(None);
        };
        if !request_is_type(&request.borrow().class) {
            return Ok(None);
        }
        if !args.is_empty() {
            return block(at, "Request getters take no arguments");
        }
        match name {
            "getrequestid" => Ok(Some(request.borrow().fields["\0id"].clone())),
            "getquiddity" => {
                let value = match self.native_state.get("request.quiddity") {
                    Some(Value::Str(s)) => s.as_str(),
                    _ => self
                        .program
                        .environment
                        .as_ref()
                        .and_then(|e| e.capabilities.get("requestQuiddity"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("RUNTEST_SYNC"),
                };
                let members = objects::builtin_enum("quiddity").unwrap();
                let Some(ordinal) = members.iter().position(|m| m.eq_ignore_ascii_case(value))
                else {
                    return block(at, "Unknown explicit request quiddity");
                };
                Ok(Some(Value::Enum {
                    typ: "quiddity".into(),
                    name: members[ordinal].into(),
                    ordinal,
                }))
            }
            _ => block(at, format!("Request.{name} is not implemented")),
        }
    }
    fn active_user_id(&self, at: &Node) -> R<String> {
        if let Some(Value::Str(id)) = self.native_state.get("userinfo.userid") {
            return Ok(id.clone());
        }
        if let Some(e) = &self.program.environment {
            return Ok(e.current_user_id.clone());
        }
        block(
            at,
            "UserInfo requires an explicit local environment with currentUserId",
        )
    }
    pub(super) fn userinfo_namespace(
        &mut self,
        ns: &str,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        if ns.trim_start_matches("system.") != "userinfo" {
            return Ok(None);
        }
        if !args.is_empty() {
            return block(at, "UserInfo getters take no arguments");
        }
        if name == "getuserid" {
            return self.active_user_id(at).map(Value::Str).map(Some);
        }
        if name == "ismulticurrencyorganization" || name == "getuithemedisplayed" {
            let key = if name == "ismulticurrencyorganization" {
                "multiCurrency"
            } else {
                "uiThemeDisplayed"
            };
            let configured = self
                .program
                .environment
                .as_ref()
                .and_then(|e| e.capabilities.get(key));
            return Ok(Some(match (name, configured) {
                ("ismulticurrencyorganization", Some(serde_json::Value::Bool(b))) => {
                    Value::Bool(*b)
                }
                ("getuithemedisplayed", Some(serde_json::Value::String(s))) => {
                    Value::Str(s.clone())
                }
                _ => {
                    return block(
                        at,
                        format!("UserInfo.{name} requires explicit environment capability {key}"),
                    )
                }
            }));
        }
        self.ensure_data_store(at)?;
        if name == "getorganizationid" || name == "getorganizationname" {
            let rows = self
                .data_store
                .as_ref()
                .unwrap()
                .records("Organization", false);
            if rows.len() != 1 {
                return block(
                    at,
                    "UserInfo requires exactly one explicit Organization record",
                );
            }
            let field = if name == "getorganizationid" {
                "Id"
            } else {
                "Name"
            };
            let Some(value) = rows[0].get(field) else {
                return block(
                    at,
                    format!("Organization.{field} is absent from environment"),
                );
            };
            return data::from_data(value, at, 0).map(Some);
        }
        let id = self.active_user_id(at)?;
        let Some(user) = self.data_store.as_ref().unwrap().find("User", &id) else {
            return block(at, "Current user is not present in the local record store");
        };
        let field = match name {
            "getusername" => "Username",
            "getfirstname" => "FirstName",
            "getlastname" => "LastName",
            "getuseremail" => "Email",
            "getprofileid" => "ProfileId",
            "getlocale" => "LocaleSidKey",
            "getlanguage" => "LanguageLocaleKey",
            "getusertype" => "UserType",
            "getdefaultcurrency" => "DefaultCurrencyIsoCode",
            "getname" => {
                let first = match user.get("FirstName") {
                    Some(DataValue::Text(s)) => s.as_str(),
                    _ => "",
                };
                let Some(DataValue::Text(last)) = user.get("LastName") else {
                    return block(at, "Current user LastName is missing");
                };
                return Ok(Some(Value::Str(if first.is_empty() {
                    last.clone()
                } else {
                    format!("{first} {last}")
                })));
            }
            _ => return block(at, format!("UserInfo.{name} is not implemented")),
        };
        let Some(value) = user.get(field) else {
            // FirstName is optional; other identity settings must be explicit.
            if field == "FirstName" {
                return Ok(Some(Value::Null));
            }
            return block(
                at,
                format!("UserInfo.{name} requires an explicit User.{field}"),
            );
        };
        data::from_data(value, at, 0).map(Some)
    }

    pub(super) fn exec_run_as(&mut self, node: &Node) -> R<Flow> {
        let user = self.eval(self.need(node, "user")?)?;
        let Value::Record(record) = &user else {
            return exceptions::raise(node, "System.TypeException", "System.runAs requires a User");
        };
        if !record.borrow().typ.eq_ignore_ascii_case("User") {
            return exceptions::raise(node, "System.TypeException", "System.runAs requires a User");
        }
        self.ensure_data_store(node)?;
        let missing_id = record
            .borrow()
            .fields
            .get("id")
            .is_none_or(|v| matches!(v, Value::Null));
        if missing_id {
            self.data_dml(DmlOperation::Insert, &user, true, node)?;
        }
        let id = match record.borrow().fields.get("id") {
            Some(Value::Str(id)) => id.clone(),
            _ => {
                return exceptions::raise(
                    node,
                    "System.TypeException",
                    "System.runAs User requires an Id",
                )
            }
        };
        let Some(stored) = self.data_store.as_ref().unwrap().find("User", &id) else {
            return exceptions::raise(
                node,
                "System.TypeException",
                "System.runAs User does not exist",
            );
        };
        if matches!(stored.get("IsActive"), Some(DataValue::Boolean(false))) {
            return exceptions::raise(
                node,
                "System.TypeException",
                "System.runAs requires an active User",
            );
        }
        self.dml += 1;
        if self.dml > 150 {
            return exceptions::raise(
                node,
                "System.LimitException",
                "Too many DML statements: 151",
            );
        }
        let body = node.child("block").ok_or_else(|| EvalError {
            exception: None,
            status: "blocked",
            diagnostic: node.diagnostic("APX_UNSUPPORTED", "Missing runAs body"),
        })?;
        let old = self
            .native_state
            .insert("userinfo.userid".into(), Value::Str(id.clone()));
        let previous_store_user = std::mem::replace(
            &mut self.data_store.as_mut().unwrap().current_user,
            Some(id),
        );
        let result = self.exec(body);
        if let Some(old) = old {
            self.native_state.insert("userinfo.userid".into(), old);
        } else {
            self.native_state.remove("userinfo.userid");
        }
        self.data_store.as_mut().unwrap().current_user = previous_store_user;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_run_as_restores_user_on_exception_and_return() {
        let source = r#"@IsTest class Check { public static void run() {
          Assert.areEqual('005000000000001AAA', UserInfo.getUserId());
          try { System.runAs(new User(Id='005000000000002AAA')) {
            Assert.areEqual('other@example.invalid', UserInfo.getUserName());
            System.runAs(new User(Id='005000000000001AAA')) { Assert.areEqual('Original', UserInfo.getLastName()); }
            Assert.areEqual('Other', UserInfo.getLastName());
            throw new IllegalArgumentException('expected');
          } } catch (IllegalArgumentException e) {}
          early();
          Assert.areEqual('Original', UserInfo.getLastName());
          Assert.areEqual(3, Limits.getDmlStatements());
        } public static void early() { System.runAs(new User(Id='005000000000002AAA')) { return; } } }"#;
        let node = crate::syntax::parse(source, "identity.cls").unwrap();
        let mut p = Program::default();
        crate::syntax::declarations(&node, &mut p, &mut vec![]);
        let fixture = serde_json::json!({"schemaVersion":1, "clock":{"now":"2026-01-15T12:00:00Z"}, "currentUserId":"005000000000001AAA", "records":[
          {"object":"User", "fields":{"Id":"005000000000001AAA", "LastName":"Original", "Username":"original@example.invalid", "IsActive":true}},
          {"object":"User", "fields":{"Id":"005000000000002AAA", "LastName":"Other", "Username":"other@example.invalid", "IsActive":true}}
        ]});
        p.environment =
            Some(crate::environment::Environment::parse(&fixture, &p.schema, "test").unwrap());
        let mut context = Context::new(&p);
        context.call("check", "run", vec![], &node).unwrap();
        assert_eq!(
            context.data_store.unwrap().current_user.as_deref(),
            Some("005000000000001AAA")
        );
    }
    #[test]
    fn user_identity_is_never_fabricated_without_environment() {
        let p = Program::default();
        let n = crate::syntax::parse("class C{}", "noenv.cls").unwrap();
        let mut c = Context::new(&p);
        assert_eq!(
            c.userinfo_namespace("userinfo", "getuserid", &[], &n)
                .unwrap_err()
                .status,
            "blocked"
        );
        assert_eq!(
            c.userinfo_namespace("userinfo", "ismulticurrencyorganization", &[], &n)
                .unwrap_err()
                .status,
            "blocked"
        );
    }
    #[test]
    fn request_identity_is_transaction_local_and_origin_is_explicit() {
        let p = Program::default();
        let n = crate::syntax::parse("class C{}", "request.cls").unwrap();
        let mut a = Context::new(&p);
        let mut b = Context::new(&p);
        let first = a
            .request_namespace("request", "getcurrent", &[], &n)
            .unwrap()
            .unwrap();
        let second = a
            .request_namespace("request", "getcurrent", &[], &n)
            .unwrap()
            .unwrap();
        let other = b
            .request_namespace("request", "getcurrent", &[], &n)
            .unwrap()
            .unwrap();
        let id = a
            .request_method(&first, "getrequestid", &[], &n)
            .unwrap()
            .unwrap();
        assert!(id.equal(
            &a.request_method(&second, "getrequestid", &[], &n)
                .unwrap()
                .unwrap()
        ));
        assert!(!id.equal(
            &b.request_method(&other, "getrequestid", &[], &n)
                .unwrap()
                .unwrap()
        ));
        a.native_state
            .insert("request.quiddity".into(), Value::Str("QUEUEABLE".into()));
        assert!(
            matches!(a.request_method(&first,"getquiddity",&[],&n).unwrap(),Some(Value::Enum{name,..}) if name=="QUEUEABLE")
        );
    }
}
