//! Declared platform data carriers. These objects never call a remote service.
use super::*;

pub(super) fn fields(typ: &str) -> Option<Vec<(&'static str, &'static str)>> {
    let t = typ.to_ascii_lowercase();
    Some(match t.as_str() {
        "connectapi.followingpage" => vec![], // Opaque fixture result; page properties are not yet modeled.
        "messaging.inboundemail" => vec![
            ("fromAddress", "String"),
            ("fromName", "String"),
            ("subject", "String"),
            ("plainTextBody", "String"),
            ("htmlBody", "String"),
            ("replyTo", "String"),
            ("messageId", "String"),
            ("inReplyTo", "String"),
            ("references", "List<String>"),
            ("toAddresses", "List<String>"),
            ("ccAddresses", "List<String>"),
            ("headers", "List<Messaging.InboundEmail.Header>"),
            (
                "binaryAttachments",
                "List<Messaging.InboundEmail.BinaryAttachment>",
            ),
            (
                "textAttachments",
                "List<Messaging.InboundEmail.TextAttachment>",
            ),
            ("htmlBodyIsTruncated", "Boolean"),
            ("plainTextBodyIsTruncated", "Boolean"),
        ],
        "messaging.inboundemail.binaryattachment" => vec![
            ("body", "Blob"),
            ("fileName", "String"),
            ("mimeTypeSubType", "String"),
            ("headers", "List<Messaging.InboundEmail.Header>"),
        ],
        "messaging.inboundemail.textattachment" => vec![
            ("body", "String"),
            ("fileName", "String"),
            ("mimeTypeSubType", "String"),
            ("charset", "String"),
            ("headers", "List<Messaging.InboundEmail.Header>"),
        ],
        "messaging.inboundemail.header" => vec![("name", "String"), ("value", "String")],
        "messaging.inboundenvelope" => vec![("fromAddress", "String"), ("toAddress", "String")],
        "messaging.inboundemailresult" => vec![("success", "Boolean"), ("message", "String")],
        "connectapi.namedcredentialinput" => vec![
            ("developerName", "String"),
            ("masterLabel", "String"),
            ("calloutUrl", "String"),
            ("type", "ConnectApi.NamedCredentialType"),
            (
                "externalCredentials",
                "List<ConnectApi.ExternalCredentialInput>",
            ),
            (
                "calloutOptions",
                "ConnectApi.NamedCredentialCalloutOptionsInput",
            ),
        ],
        "connectapi.namedcredential" => vec![
            ("id", "String"),
            ("developerName", "String"),
            ("masterLabel", "String"),
            ("calloutUrl", "String"),
            ("type", "ConnectApi.NamedCredentialType"),
            ("externalCredentials", "List<ConnectApi.ExternalCredential>"),
            ("calloutOptions", "ConnectApi.NamedCredentialCalloutOptions"),
        ],
        "connectapi.namedcredentialcalloutoptionsinput"
        | "connectapi.namedcredentialcalloutoptions" => vec![
            ("allowMergeFieldsInBody", "Boolean"),
            ("allowMergeFieldsInHeader", "Boolean"),
            ("generateAuthorizationHeader", "Boolean"),
        ],
        "connectapi.externalcredentialinput" => vec![
            ("developerName", "String"),
            ("masterLabel", "String"),
            (
                "authenticationProtocol",
                "ConnectApi.CredentialAuthenticationProtocol",
            ),
            (
                "principals",
                "List<ConnectApi.ExternalCredentialPrincipalInput>",
            ),
        ],
        "connectapi.externalcredential" => vec![
            ("id", "String"),
            ("developerName", "String"),
            ("masterLabel", "String"),
            (
                "authenticationProtocol",
                "ConnectApi.CredentialAuthenticationProtocol",
            ),
            ("principals", "List<ConnectApi.ExternalCredentialPrincipal>"),
        ],
        "connectapi.externalcredentialprincipalinput"
        | "connectapi.externalcredentialprincipal" => vec![
            ("id", "String"),
            ("principalName", "String"),
            ("sequenceNumber", "Integer"),
            ("principalType", "ConnectApi.CredentialPrincipalType"),
        ],
        _ => return None,
    })
}
pub(super) fn is_type(typ: &str) -> bool {
    fields(typ).is_some()
}
/// Only the named members used by the supported credential protocols are bound.
/// Numeric ordinals and enumeration of the incomplete protocol catalog must block.
pub(super) fn enum_members(typ: &str) -> Option<&'static [&'static str]> {
    match typ.to_ascii_lowercase().as_str() {
        "connectapi.namedcredentialtype" => Some(&["SecuredEndpoint", "PrivateEndpoint", "Legacy"]),
        "connectapi.credentialauthenticationprotocol" => Some(&["Custom"]),
        "connectapi.credentialprincipaltype" => Some(&["NamedPrincipal", "PerUserPrincipal"]),
        _ => None,
    }
}
pub(super) fn is_namespace(typ: &str) -> bool {
    let t = typ.to_ascii_lowercase();
    matches!(
        t.as_str(),
        "messaging"
            | "connectapi"
            | "connectapi.chatterusers"
            | "applauncher"
            | "applauncher.appmenu"
    ) || is_type(&t)
        || enum_members(&t).is_some()
}
impl Context<'_> {
    pub(super) fn connect_api_namespace(
        &mut self,
        ns: &str,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        if ns == "applauncher.appmenu" {
            return self.app_menu_namespace(name, args, at);
        }
        if ns != "connectapi.chatterusers" || name != "getfollowings" {
            return Ok(None);
        }
        let [community, Value::Str(user)] = args else {
            return block(
                at,
                "Only the two-argument ChatterUsers.getFollowings overload is supported",
            );
        };
        if !matches!(community, Value::Null | Value::Str(_)) {
            return block(at, "Chatter communityId must be String or null");
        }
        if !matches!(
            self.native_state.get("test.seealldata"),
            Some(Value::Bool(true))
        ) {
            return exceptions::raise(
                at,
                "System.UnsupportedOperationException",
                "ConnectApi.ChatterUsers.getFollowings is unavailable in data-isolated tests",
            );
        }
        // A local engine has no ambient Chatter data. An explicit profile may provide
        // an opaque result for a precise call; missing calls never get invented data.
        let community = match community {
            Value::Str(s) => serde_json::Value::String(s.clone()),
            _ => serde_json::Value::Null,
        };
        let arguments = serde_json::json!([community, user]);
        let fixtures = self
            .program
            .environment
            .as_ref()
            .and_then(|e| e.capabilities.get("connectApiFixtures"))
            .and_then(serde_json::Value::as_array);
        let fixture = fixtures.and_then(|rows| {
            rows.iter().find(|r| {
                r["method"] == "ConnectApi.ChatterUsers.getFollowings"
                    && r["arguments"] == arguments
            })
        });
        let Some(fixture) = fixture else {
            return block(at, "ChatterUsers.getFollowings requires an explicit local fixture for these arguments; no network fallback");
        };
        if fixture["resultType"] != "ConnectApi.FollowingPage"
            || fixture["result"] != serde_json::json!({})
        {
            return block(at, "Only an explicitly opaque FollowingPage fixture is supported; page data is not yet modeled");
        }
        self.platform_object_construct("ConnectApi.FollowingPage", &[], at)
    }
    fn app_menu_namespace(&mut self, name: &str, args: &[Value], at: &Node) -> R<Option<Value>> {
        use crate::record_store::{DataValue, RecordData};
        if !matches!(name, "setorgsortorder" | "setappvisibility") {
            return block(at, format!("AppLauncher.AppMenu.{name} is not implemented"));
        }
        let permission = self.program.environment.as_ref().is_some_and(|e| {
            let caller = match self.native_state.get("userinfo.userid") {
                Some(Value::Str(id)) => id.as_str(),
                _ => e.current_user_id.as_str(),
            };
            (caller == e.current_user_id
                && e.capabilities
                    .get("appMenuCanCustomize")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true))
                || e.capabilities
                    .get("appMenuCustomizers")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(caller)))
        });
        if !permission {
            return block(at,"AppMenu changes require explicit local customization permission for the current user");
        }
        self.ensure_data_store(at)?;
        let store = self.data_store.as_ref().unwrap();
        if store.dml_rows != 0 {
            return block(
                at,
                "AppMenu changes after DML require mixed-setup transaction qualification",
            );
        }
        let rows = store
            .records("AppMenuItem", false)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let mut updates = vec![];
        match (name, args) {
            ("setorgsortorder", [Value::List(ids)]) => {
                let mut seen = BTreeSet::new();
                for (order, id) in ids.borrow().iter().enumerate() {
                    let Value::Str(id) = id else {
                        return block(at, "AppMenu application IDs must be non-null Id values");
                    };
                    let Some(row) = rows.iter().find(|r| {
                        r.get("ApplicationId")
                            .and_then(DataValue::text)
                            .is_some_and(|app| app == id || app.get(..15) == Some(id.as_str()))
                    }) else {
                        return block(at, "AppMenu application ID is absent from local metadata");
                    };
                    if !seen.insert(row.id().unwrap().to_string()) {
                        return block(at, "Duplicate AppMenu application IDs are not qualified");
                    }
                    updates.push(
                        RecordData::new("AppMenuItem")
                            .with("Id", DataValue::Text(row.id().unwrap().into()))
                            .with("SortOrder", DataValue::integer(order as i64)),
                    );
                }
                if seen.len() != rows.len() {
                    return block(at,"Partial AppMenu permutations are not qualified; supply all local application IDs");
                }
            }
            ("setappvisibility", [Value::Str(id), Value::Bool(visible)]) => {
                let Some(row) = rows.iter().find(|r| {
                    r.get("ApplicationId")
                        .and_then(DataValue::text)
                        .is_some_and(|app| app == id || app.get(..15) == Some(id.as_str()))
                }) else {
                    return block(at, "AppMenu application ID is absent from local metadata");
                };
                updates.push(
                    RecordData::new("AppMenuItem")
                        .with("Id", DataValue::Text(row.id().unwrap().into()))
                        .with("IsVisible", DataValue::Boolean(*visible)),
                );
            }
            _ => return block(at, "AppMenu overload is not implemented"),
        }
        // The local metadata state changes atomically and participates in the
        // enclosing async transaction. It can be queried by independent probes.
        let mut candidate = self.data_store.as_ref().unwrap().clone();
        for row in updates {
            candidate
                .upsert_platform_record(&row)
                .map_err(|e| EvalError {
                    exception: None,
                    status: "blocked",
                    diagnostic: at.diagnostic("APX_APP_MENU", e.message),
                })?;
        }
        self.data_store = Some(candidate);
        self.native_state
            .insert("appmenu.changed".into(), Value::Bool(true));
        Ok(Some(Value::Null))
    }
    pub(super) fn platform_object_construct(
        &self,
        typ: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        let Some(declared) = fields(typ) else {
            return Ok(None);
        };
        if !args.is_empty() {
            return block(at, "Platform data carrier constructors take no arguments");
        }
        let value = Value::Instance(Rc::new(RefCell::new(Instance {
            class: typ.to_ascii_lowercase(),
            fields: declared
                .iter()
                .map(|(name, _)| (name.to_ascii_lowercase(), Value::Null))
                .collect(),
            field_types: declared
                .iter()
                .map(|(name, typ)| (name.to_ascii_lowercase(), (*typ).into()))
                .collect(),
        })));
        self.charge(&value, at)?;
        Ok(Some(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn app_menu_updates_real_local_metadata_and_rejects_invalid_permutations_atomically() {
        use crate::record_store::{DataValue, RecordData};
        let n = crate::syntax::parse("class C{}", "menu.cls").unwrap();
        let mut p = Program::default();
        let fixture = serde_json::json!({"schemaVersion":1,"clock":{"now":"2026-01-15T12:00:00Z"},"currentUserId":"005000000000001AAA","records":[{"object":"User","fields":{"Id":"005000000000001AAA","LastName":"Local"}}],"capabilities":{"appMenuCanCustomize":true}});
        p.environment =
            Some(crate::environment::Environment::parse(&fixture, &p.schema, "test").unwrap());
        let mut c = Context::new(&p);
        c.ensure_data_store(&n).unwrap();
        let a = "02u000000000001AAA";
        let b = "02u000000000002AAA";
        let mut row_ids = vec![];
        for (name, id) in [("First", a), ("Second", b)] {
            c.data_store
                .as_mut()
                .unwrap()
                .upsert_platform_record(
                    &RecordData::new("AppDefinition")
                        .with("Id", DataValue::Text(id.into()))
                        .with("DeveloperName", DataValue::Text(name.into())),
                )
                .unwrap();
            row_ids.push(
                c.data_store
                    .as_mut()
                    .unwrap()
                    .upsert_platform_record(
                        &RecordData::new("AppMenuItem")
                            .with("Name", DataValue::Text(name.into()))
                            .with("ApplicationId", DataValue::Text(id.into())),
                    )
                    .unwrap(),
            );
        }
        let order = Value::list(vec![Value::Str(b.into()), Value::Str(a.into())]);
        c.connect_api_namespace("applauncher.appmenu", "setorgsortorder", &[order], &n)
            .unwrap();
        assert_eq!(
            c.data_store
                .as_ref()
                .unwrap()
                .find("AppMenuItem", &row_ids[0])
                .unwrap()
                .get("SortOrder"),
            Some(&DataValue::integer(1))
        );
        assert_eq!(
            c.data_store
                .as_ref()
                .unwrap()
                .find("AppMenuItem", &row_ids[1])
                .unwrap()
                .get("SortOrder"),
            Some(&DataValue::integer(0))
        );
        let invalid = Value::list(vec![Value::Str(a.into()), Value::Str(a.into())]);
        assert_eq!(
            c.connect_api_namespace("applauncher.appmenu", "setorgsortorder", &[invalid], &n)
                .unwrap_err()
                .status,
            "blocked"
        );
        assert_eq!(
            c.data_store
                .as_ref()
                .unwrap()
                .find("AppMenuItem", &row_ids[0])
                .unwrap()
                .get("SortOrder"),
            Some(&DataValue::integer(1))
        );
        c.connect_api_namespace(
            "applauncher.appmenu",
            "setappvisibility",
            &[Value::Str(a.into()), Value::Bool(false)],
            &n,
        )
        .unwrap();
        assert_eq!(
            c.data_store
                .as_ref()
                .unwrap()
                .find("AppMenuItem", &row_ids[0])
                .unwrap()
                .get("IsVisible"),
            Some(&DataValue::Boolean(false))
        );
    }
    #[test]
    fn apex_assignments_preserve_carrier_types_and_values() {
        let source = r#"@IsTest class Check { public static void run() {
          Messaging.InboundEmail email = new Messaging.InboundEmail();
          Messaging.InboundEmail.BinaryAttachment a = new Messaging.InboundEmail.BinaryAttachment();
          a.body = Blob.valueOf('changed body'); a.fileName = 'changed.txt';
          email.binaryAttachments = new List<Messaging.InboundEmail.BinaryAttachment>{a};
          Assert.areEqual('changed body', email.binaryAttachments[0].body.toString());
          ConnectApi.NamedCredentialInput credential = new ConnectApi.NamedCredentialInput();
          credential.type = ConnectApi.NamedCredentialType.SecuredEndpoint;
          Assert.areEqual(ConnectApi.NamedCredentialType.SecuredEndpoint, credential.type);
        } }"#;
        let n = crate::syntax::parse(source, "carrier.cls").unwrap();
        let mut p = Program::default();
        crate::syntax::declarations(&n, &mut p, &mut vec![]);
        Context::new(&p).call("check", "run", vec![], &n).unwrap();
        let bad = source.replace(
            "a.body = Blob.valueOf('changed body')",
            "a.body = 'wrong type'",
        );
        let n = crate::syntax::parse(&bad, "wrong.cls").unwrap();
        let mut p = Program::default();
        crate::syntax::declarations(&n, &mut p, &mut vec![]);
        assert!(Context::new(&p).call("check", "run", vec![], &n).is_err());
    }
    #[test]
    fn chatter_requires_explicit_response_and_respects_test_isolation() {
        let n = crate::syntax::parse("class C{}", "chatter.cls").unwrap();
        let mut p = Program::default();
        let mut c = Context::new(&p);
        let args = [Value::Null, Value::Str("005000000000001AAA".into())];
        assert!(c
            .connect_api_namespace("connectapi.chatterusers", "getfollowings", &args, &n)
            .unwrap_err()
            .exception
            .is_some());
        let fixture = serde_json::json!({"schemaVersion":1,"clock":{"now":"2026-01-15T12:00:00Z"},"currentUserId":"005000000000001AAA","records":[{"object":"User","fields":{"Id":"005000000000001AAA","LastName":"Local"}}],"capabilities":{"connectApiFixtures":[{"method":"ConnectApi.ChatterUsers.getFollowings","arguments":[null,"005000000000001AAA"],"resultType":"ConnectApi.FollowingPage","result":{}}]}});
        p.environment =
            Some(crate::environment::Environment::parse(&fixture, &p.schema, "test").unwrap());
        let mut c = Context::new(&p);
        c.native_state
            .insert("test.seealldata".into(), Value::Bool(true));
        assert!(c
            .connect_api_namespace("connectapi.chatterusers", "getfollowings", &args, &n)
            .unwrap()
            .is_some());
        assert_eq!(
            c.connect_api_namespace(
                "connectapi.chatterusers",
                "getfollowings",
                &[Value::Null, Value::Str("005000000000002AAA".into())],
                &n
            )
            .unwrap_err()
            .status,
            "blocked"
        );
    }
    #[test]
    fn data_carriers_have_declared_types_and_no_service_side_effects() {
        let n = crate::syntax::parse("class Host{}", "carrier.cls").unwrap();
        let p = Program::default();
        let c = Context::new(&p);
        let Value::Instance(i) = c
            .platform_object_construct("Messaging.InboundEmail.BinaryAttachment", &[], &n)
            .unwrap()
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(i.borrow().field_types["body"], "Blob");
        assert!(!i.borrow().field_types.contains_key("invented"));
        assert!(c
            .platform_object_construct("ConnectApi.Invented", &[], &n)
            .unwrap()
            .is_none());
        assert!(c
            .platform_object_construct("Messaging.InboundEmail", &[Value::Str("bad".into())], &n)
            .is_err());
    }
}
