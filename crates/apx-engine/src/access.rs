//! Explicit local access policy. No grants are inferred from display names.
use crate::{
    environment::Environment,
    record_store::{DataValue, RecordStore, StoreError},
    schema::{FieldPermission, ObjectPermission},
    soql::AccessPolicy,
};
use std::collections::BTreeMap;
#[derive(Clone, Debug, Default)]
pub struct ResolvedAccess {
    pub objects: BTreeMap<String, ObjectPermission>,
    pub fields: BTreeMap<String, FieldPermission>,
    pub records: BTreeMap<String, RecordPermission>,
    pub query: AccessPolicy,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct RecordPermission {
    pub read: bool,
    pub edit: bool,
    pub delete: bool,
}
pub fn business_object(name: &str) -> bool {
    [
        "account",
        "contact",
        "lead",
        "opportunity",
        "task",
        "case",
        "campaign",
        "contentversion",
        "contentdocument",
        "contentdocumentlink",
        "contentdistribution",
        "emailmessage",
        "emailmessagerelation",
    ]
    .contains(&name.to_ascii_lowercase().as_str())
        || name.to_ascii_lowercase().ends_with("__c")
}
fn sharing_model<'a>(
    object: &'a crate::schema::ObjectSchema,
    environment: &'a Environment,
) -> Option<&'a str> {
    object
        .sharing_model
        .as_deref()
        .or_else(|| {
            environment.security["objectSharingModels"]
                .as_object()
                .and_then(|models| {
                    models
                        .iter()
                        .find(|(name, _)| name.eq_ignore_ascii_case(&object.name))
                        .and_then(|(_, value)| value.as_str())
                })
        })
        .or_else(|| environment.security["defaultSharingModel"].as_str())
}
fn union_object(a: &mut ObjectPermission, b: &ObjectPermission) {
    a.read |= b.read;
    a.create |= b.create;
    a.edit |= b.edit;
    a.delete |= b.delete;
    a.view_all |= b.view_all;
    a.modify_all |= b.modify_all;
}
fn flag(v: &serde_json::Value, name: &str) -> bool {
    v[name].as_bool() == Some(true)
}
fn object_grant(v: &serde_json::Value) -> ObjectPermission {
    ObjectPermission {
        read: flag(v, "read"),
        create: flag(v, "create"),
        edit: flag(v, "edit"),
        delete: flag(v, "delete"),
        view_all: flag(v, "viewAll"),
        modify_all: flag(v, "modifyAll"),
    }
}
pub fn resolve(
    store: &RecordStore,
    environment: &Environment,
    user_id: &str,
) -> Result<ResolvedAccess, StoreError> {
    if environment.security["mode"] != "synthetic-local" {
        return Err(StoreError::unsupported(
            "No explicit access policy supplied",
        ));
    }
    let user = store
        .find("User", user_id)
        .ok_or_else(|| StoreError::unsupported("Current User is absent from local environment"))?;
    let profile = user
        .get("ProfileId")
        .and_then(DataValue::text)
        .ok_or_else(|| StoreError::unsupported("Current User has no ProfileId"))?;
    let profiles = environment.security["profilePolicies"]
        .as_array()
        .ok_or_else(|| StoreError::unsupported("Profile policies missing"))?;
    let policy = profiles
        .iter()
        .find(|p| p["profileId"].as_str() == Some(profile))
        .ok_or_else(|| StoreError::unsupported("No explicit policy for current ProfileId"))?;
    if environment
        .assignments
        .iter()
        .any(|a| a.unresolved && a.assignee_id == user_id)
        && !(policy["objects"]["selector"] == "all-registered-objects"
            && ["read", "create", "edit", "delete", "viewAll", "modifyAll"]
                .iter()
                .all(|s| flag(&policy["objects"], s))
            && policy["fields"]["selector"] == "all-registered-fields"
            && flag(&policy["fields"], "read")
            && flag(&policy["fields"], "edit"))
    {
        return Err(StoreError::unsupported(
            "An unresolved permission-set assignment may change this user's grants",
        ));
    }
    let mut result = ResolvedAccess::default();
    let object_selector = policy["objects"]["selector"].as_str().unwrap_or("");
    let field_selector = policy["fields"]["selector"].as_str().unwrap_or("");
    if !["all-registered-objects", "registered-business-objects"].contains(&object_selector)
        || ![
            "all-registered-fields",
            "all-permissionable-fields",
            "registered-business-fields",
        ]
        .contains(&field_selector)
    {
        return Err(StoreError::unsupported("Unknown access selector"));
    }
    for object in store.schema.objects.values() {
        let key = object.name.to_ascii_lowercase();
        let applies = object_selector == "all-registered-objects" || business_object(&key);
        result.objects.insert(
            key.clone(),
            if applies {
                object_grant(&policy["objects"])
            } else {
                ObjectPermission::default()
            },
        );
        for field in object.fields.values() {
            let qualified = format!("{key}.{}", field.name.to_ascii_lowercase());
            let excluded = policy["fields"]["exclude"]
                .as_array()
                .is_some_and(|values| {
                    values
                        .iter()
                        .filter_map(|v| v.as_str())
                        .any(|s| s.eq_ignore_ascii_case(&qualified))
                });
            let applies = (field_selector != "registered-business-fields" || business_object(&key))
                && !excluded;
            result.fields.insert(
                qualified,
                FieldPermission {
                    read: applies && flag(&policy["fields"], "read"),
                    edit: applies && flag(&policy["fields"], "edit"),
                },
            );
        }
    }
    let mut permission_ids = std::collections::BTreeSet::new();
    for assignment in store
        .records("PermissionSetAssignment", false)
        .into_iter()
        .filter(|a| a.get("AssigneeId").and_then(DataValue::text) == Some(user_id))
    {
        if let Some(id) = assignment.get("PermissionSetId").and_then(DataValue::text) {
            permission_ids.insert(id.to_string());
        }
        if let Some(id) = assignment
            .get("PermissionSetGroupId")
            .and_then(DataValue::text)
        {
            if let Some(blocker) = store
                .schema
                .blockers
                .iter()
                .find(|b| b.capability == "security")
            {
                return Err(StoreError::unsupported(format!(
                    "Unresolved permission group policy: {} ({})",
                    blocker.reason, blocker.path
                )));
            }
            let group = store
                .find("PermissionSetGroup", id)
                .ok_or_else(|| StoreError::unsupported("Assigned permission set group missing"))?;
            if group.get("Status").and_then(DataValue::text) != Some("Updated") {
                return Err(StoreError::unsupported(
                    "Permission set group requires recalculation",
                ));
            }
            for member in store.records("PermissionSetGroupComponent", false) {
                if member.get("PermissionSetGroupId").and_then(DataValue::text) == Some(id) {
                    if let Some(id) = member.get("PermissionSetId").and_then(DataValue::text) {
                        permission_ids.insert(id.to_string());
                    }
                }
            }
        }
    }
    for id in permission_ids {
        store
            .find("PermissionSet", &id)
            .ok_or_else(|| StoreError::unsupported("Assigned permission set missing"))?;
        // Source permission metadata is materialized into these transaction
        // rows at store creation. Reading the immutable source again would
        // resurrect grants after a permission row was edited or deleted.
        for p in store
            .records("ObjectPermissions", false)
            .into_iter()
            .filter(|p| p.get("ParentId").and_then(DataValue::text) == Some(&id))
        {
            let object = p
                .get("SObjectType")
                .and_then(DataValue::text)
                .ok_or_else(|| StoreError::unsupported("ObjectPermissions.SObjectType missing"))?
                .to_ascii_lowercase();
            let b = |name| p.get(name) == Some(&DataValue::Boolean(true));
            union_object(
                result.objects.entry(object).or_default(),
                &ObjectPermission {
                    read: b("PermissionsRead"),
                    create: b("PermissionsCreate"),
                    edit: b("PermissionsEdit"),
                    delete: b("PermissionsDelete"),
                    view_all: b("PermissionsViewAllRecords"),
                    modify_all: b("PermissionsModifyAllRecords"),
                },
            );
        }
        for p in store
            .records("FieldPermissions", false)
            .into_iter()
            .filter(|p| p.get("ParentId").and_then(DataValue::text) == Some(&id))
        {
            let field = p
                .get("Field")
                .and_then(DataValue::text)
                .ok_or_else(|| StoreError::unsupported("FieldPermissions.Field missing"))?
                .to_ascii_lowercase();
            let f = result.fields.entry(field).or_default();
            f.read |= p.get("PermissionsRead") == Some(&DataValue::Boolean(true));
            f.edit |= p.get("PermissionsEdit") == Some(&DataValue::Boolean(true));
        }
    }
    for object in store.schema.objects.values() {
        for field in object.fields.values() {
            if !field.compound_components.is_empty() {
                if let Some(grant) = result
                    .fields
                    .get(&format!(
                        "{}.{}",
                        object.name.to_ascii_lowercase(),
                        field.name.to_ascii_lowercase()
                    ))
                    .cloned()
                {
                    for component in &field.compound_components {
                        let target = result
                            .fields
                            .entry(format!(
                                "{}.{}",
                                object.name.to_ascii_lowercase(),
                                component.to_ascii_lowercase()
                            ))
                            .or_default();
                        target.read |= grant.read;
                        target.edit |= grant.edit;
                    }
                }
            }
        }
    }
    // Imported permission rows may describe platform fields outside the bounded
    // execution schema. Preserve those rows for metadata queries, but they must
    // never create runtime object/field capabilities.
    result
        .objects
        .retain(|name, _| store.schema.object(name).is_some());
    result.fields.retain(|name, _| {
        name.split_once('.')
            .is_some_and(|(object, field)| store.schema.field(object, field).is_some())
    });
    // Apply platform schema caps after the union: one false grant never removes another grant.
    for object in store.schema.objects.values() {
        let key = object.name.to_ascii_lowercase();
        let p = result.objects.entry(key.clone()).or_default();
        if object.read_only {
            p.create = false;
            p.edit = false;
            p.delete = false;
            p.modify_all = false;
        }
        if object.platform_event {
            p.read = false;
            p.edit = false;
            p.delete = false;
        }
        for field in object.fields.values() {
            let f = result
                .fields
                .entry(format!("{key}.{}", field.name.to_ascii_lowercase()))
                .or_default();
            if field.required
                || [
                    "id",
                    "createddate",
                    "createdbyid",
                    "lastmodifieddate",
                    "lastmodifiedbyid",
                    "systemmodstamp",
                    "isdeleted",
                ]
                .contains(&field.name.to_ascii_lowercase().as_str())
            {
                f.read = p.read;
                if field.required {
                    f.edit = p.create || p.edit;
                }
            }
            f.read &= p.read;
            f.edit &= field.writable && (p.create || p.edit);
            if f.read {
                result
                    .query
                    .readable_fields
                    .entry(key.clone())
                    .or_default()
                    .insert(field.name.to_ascii_lowercase());
            }
        }
        if p.view_all || p.modify_all || object.read_only {
            result.query.view_all_objects.insert(key.clone());
        }
        // ModifyAll already authorizes every row; read-only schema cannot have
        // writable rows. Avoid enumerating entire tables for either case.
        if p.modify_all || object.read_only {
            continue;
        }
        for row in store.records(&object.name, true) {
            let rights = shared_row_access(store, environment, user_id, &result.objects, row, 0)?;
            if let Some(id) = row.id() {
                if rights.read {
                    result.query.visible_record_ids.insert(id.into());
                }
                result.records.insert(id.into(), rights);
            }
        }
    }
    Ok(result)
}
fn shared_row_access(
    store: &RecordStore,
    environment: &Environment,
    user: &str,
    grants: &BTreeMap<String, ObjectPermission>,
    row: &crate::record_store::RecordData,
    depth: usize,
) -> Result<RecordPermission, StoreError> {
    if depth > 16 {
        return Err(StoreError::unsupported("Sharing parent chain exceeds16"));
    }
    let object = store
        .schema
        .object(&row.object)
        .ok_or_else(|| StoreError::unsupported("Sharing schema unavailable"))?;
    let grant = grants
        .get(&row.object.to_ascii_lowercase())
        .cloned()
        .unwrap_or_default();
    if object.read_only {
        return Ok(RecordPermission {
            read: true,
            ..Default::default()
        });
    }
    if grant.modify_all || row.get("OwnerId").and_then(DataValue::text) == Some(user) {
        return Ok(RecordPermission {
            read: true,
            edit: true,
            delete: true,
        });
    }
    let sharing = sharing_model(object, environment)
        .ok_or_else(|| StoreError::unsupported("Sharing model unavailable"))?;
    let mut rights = match sharing {
        "Private" => RecordPermission::default(),
        "Read" | "PublicReadOnly" => RecordPermission {
            read: true,
            ..Default::default()
        },
        // Sharing read/write access does not transfer the owner's delete right.
        "ReadWrite" | "PublicReadWrite" => RecordPermission {
            read: true,
            edit: true,
            delete: false,
        },
        "ControlledByParent" => {
            let parents = if object.name.eq_ignore_ascii_case("Contact") {
                vec!["AccountId".into()]
            } else {
                object
                    .fields
                    .values()
                    .filter(|f| {
                        f.kind == crate::schema::FieldKind::Reference
                            && f.required
                            && f.delete_behavior.as_deref() == Some("Cascade")
                    })
                    .map(|f| f.name.clone())
                    .collect::<Vec<String>>()
            };
            if parents.is_empty() {
                return Err(StoreError::unsupported(
                    "ControlledByParent has no known parent relationship",
                ));
            }
            let mut inherited = RecordPermission {
                read: true,
                edit: true,
                delete: true,
            };
            for field in parents {
                let Some(parent) = row
                    .get(&field)
                    .and_then(DataValue::text)
                    .and_then(|id| store.find_any(id))
                else {
                    return Ok(RecordPermission {
                        read: grant.view_all,
                        ..Default::default()
                    });
                };
                let parent =
                    shared_row_access(store, environment, user, grants, parent, depth + 1)?;
                inherited.read &= parent.read;
                inherited.edit &= parent.edit;
                inherited.delete &= parent.delete;
            }
            inherited
        }
        _ => {
            return Err(StoreError::unsupported(format!(
                "Unknown sharing model {sharing}"
            )))
        }
    };
    rights.read |= grant.view_all;
    Ok(rights)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        record_store::{DmlOperation, RecordData},
        schema::{PermissionSet, SchemaCatalog},
    };
    fn environment() -> Environment {
        let json = serde_json::json!({"schemaVersion":1,"clock":{"now":"2026-01-15T12:00:00Z"},"currentUserId":"005000000000001AAA","records":[{"object":"User","fields":{"Id":"005000000000001AAA","ProfileId":"00e000000000001AAA"}},{"object":"Profile","fields":{"Id":"00e000000000001AAA","Name":"Display name is not a grant"}}],"security":{"mode":"synthetic-local","defaultSharingModel":"Private","profilePolicies":[{"profileId":"00e000000000001AAA","objects":{"selector":"all-registered-objects","read":false,"create":false,"edit":false,"delete":false},"fields":{"selector":"all-permissionable-fields","read":false,"edit":false}}]}});
        Environment::parse(&json, &SchemaCatalog::standard(), "unit").unwrap()
    }
    #[test]
    fn grants_union_then_schema_caps_and_transactional_permission_revocation() {
        let env = environment();
        let mut schema = SchemaCatalog::standard();
        schema.permission_sets.insert(
            "grants".into(),
            PermissionSet {
                name: "grants".into(),
                objects: BTreeMap::from([(
                    "account".into(),
                    ObjectPermission {
                        read: true,
                        create: true,
                        edit: true,
                        ..Default::default()
                    },
                )]),
                fields: BTreeMap::from([
                    (
                        "account.description".into(),
                        FieldPermission {
                            read: true,
                            edit: true,
                        },
                    ),
                    (
                        "account.createddate".into(),
                        FieldPermission {
                            read: true,
                            edit: true,
                        },
                    ),
                ]),
                ..Default::default()
            },
        );
        let mut store =
            RecordStore::with_environment(schema, &env.now, Some(env.current_user_id.clone()))
                .unwrap();
        store.seed_environment(&env).unwrap();
        assert!(!resolve(&store, &env, &env.current_user_id).unwrap().objects["account"].read);
        let permission = store.records("PermissionSet", false)[0]
            .id()
            .unwrap()
            .to_string();
        store
            .apply(
                DmlOperation::Insert,
                &[RecordData::new("PermissionSetAssignment")
                    .with("AssigneeId", DataValue::Text(env.current_user_id.clone()))
                    .with("PermissionSetId", DataValue::Text(permission))],
                true,
            )
            .unwrap();
        let access = resolve(&store, &env, &env.current_user_id).unwrap();
        assert!(access.objects["account"].create);
        assert!(access.fields["account.description"].edit);
        assert!(!access.fields["account.createddate"].edit);
        assert!(!access.fields["account.tradestyle"].read);
        assert!(access.fields["account.name"].edit);
        let field_permission = store
            .records("FieldPermissions", false)
            .into_iter()
            .find(|row| row.get("Field").and_then(DataValue::text) == Some("Account.Description"))
            .unwrap()
            .id()
            .unwrap()
            .to_string();
        store
            .apply(
                DmlOperation::Update,
                &[RecordData::new("FieldPermissions")
                    .with("Id", DataValue::Text(field_permission))
                    .with("PermissionsRead", DataValue::Boolean(false))
                    .with("PermissionsEdit", DataValue::Boolean(false))],
                true,
            )
            .unwrap();
        let access = resolve(&store, &env, &env.current_user_id).unwrap();
        assert!(store.schema.permission_sets["grants"].fields["account.description"].edit);
        assert!(!access.fields["account.description"].read);
        assert!(!access.fields["account.description"].edit);
        let object_permission = store.records("ObjectPermissions", false)[0]
            .id()
            .unwrap()
            .to_string();
        store
            .apply(
                DmlOperation::Delete,
                &[RecordData::new("ObjectPermissions")
                    .with("Id", DataValue::Text(object_permission))],
                true,
            )
            .unwrap();
        let access = resolve(&store, &env, &env.current_user_id).unwrap();
        assert!(!access.objects["account"].read);
        assert!(!access.objects["account"].create);
        assert!(!access.fields["account.name"].edit);
    }
    #[test]
    fn invalid_fixture_reference_is_atomic() {
        let mut env = environment();
        env.records[0].set("ProfileId", DataValue::Text("00e999999999999AAA".into()));
        let mut store = RecordStore::new(SchemaCatalog::standard()).unwrap();
        assert!(store.seed_environment(&env).is_err());
        assert!(store.records("User", false).is_empty());
    }
    #[test]
    fn imported_permissions_remain_queryable_without_granting_unmodeled_schema() {
        let env = environment();
        let mut schema = SchemaCatalog::standard();
        schema.permission_sets.insert(
            "source".into(),
            PermissionSet {
                name: "Source".into(),
                objects: BTreeMap::from([
                    (
                        "account".into(),
                        ObjectPermission {
                            read: true,
                            ..Default::default()
                        },
                    ),
                    (
                        "unmodeledobject__c".into(),
                        ObjectPermission {
                            read: true,
                            ..Default::default()
                        },
                    ),
                ]),
                fields: BTreeMap::from([
                    (
                        "account.description".into(),
                        FieldPermission {
                            read: true,
                            edit: false,
                        },
                    ),
                    (
                        "account.unmodeledfeature__c".into(),
                        FieldPermission {
                            read: true,
                            edit: true,
                        },
                    ),
                ]),
                field_api_names: BTreeMap::from([(
                    "account.unmodeledfeature__c".into(),
                    "Account.UnmodeledFeature__c".into(),
                )]),
                object_api_names: BTreeMap::from([(
                    "unmodeledobject__c".into(),
                    "UnmodeledObject__c".into(),
                )]),
                ..Default::default()
            },
        );
        let mut store =
            RecordStore::with_environment(schema, &env.now, Some(env.current_user_id.clone()))
                .unwrap();
        store.seed_environment(&env).unwrap();
        let permission = store.records("PermissionSet", false)[0]
            .id()
            .unwrap()
            .to_string();
        store
            .apply(
                DmlOperation::Insert,
                &[RecordData::new("PermissionSetAssignment")
                    .with("AssigneeId", DataValue::Text(env.current_user_id.clone()))
                    .with("PermissionSetId", DataValue::Text(permission.clone()))],
                true,
            )
            .unwrap();
        let access = resolve(&store, &env, &env.current_user_id).unwrap();
        assert!(access.fields["account.description"].read);
        assert!(!access.fields.contains_key("account.unmodeledfeature__c"));
        assert!(!access.objects.contains_key("unmodeledobject__c"));
        assert!(!access.query.readable_fields["account"].contains("unmodeledfeature__c"));
        let metadata = store
            .records("FieldPermissions", false)
            .into_iter()
            .find(|row| {
                row.get("Field").and_then(DataValue::text) == Some("Account.UnmodeledFeature__c")
            })
            .unwrap();
        assert_eq!(
            metadata.get("PermissionsEdit"),
            Some(&DataValue::Boolean(true))
        );
        assert_eq!(
            metadata.get("ParentId").and_then(DataValue::text),
            Some(permission.as_str())
        );
        for query in [
            "SELECT UnmodeledFeature__c FROM Account",
            "SELECT Id FROM UnmodeledObject__c",
        ] {
            assert!(store
                .query(
                    &crate::soql::parse(query).unwrap(),
                    &crate::soql::Bindings::new(),
                    crate::soql::QueryAccess::User(&access.query)
                )
                .is_err());
        }
        assert!(store
            .schema
            .field("Account", "UnmodeledFeature__c")
            .is_none());
    }
    #[test]
    fn business_fixtures_are_isolated_unless_see_all_data_is_explicit() {
        let mut env = environment();
        env.records.push(
            RecordData::new("Account")
                .with("Id", DataValue::Text("001000000000001AAA".into()))
                .with("Name", DataValue::Text("Existing customer".into())),
        );
        let mut isolated = RecordStore::new(SchemaCatalog::standard()).unwrap();
        isolated.seed_environment_for_test(&env, false).unwrap();
        assert!(isolated.records("Account", false).is_empty());
        assert_eq!(isolated.records("Profile", false).len(), 1);
        let mut visible = RecordStore::new(SchemaCatalog::standard()).unwrap();
        visible.seed_environment_for_test(&env, true).unwrap();
        assert_eq!(visible.records("Account", false).len(), 1);
    }
}
