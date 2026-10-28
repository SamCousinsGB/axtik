//! Bounded, declarative Salesforce schema ingestion. No XML code or formulas execute here.
use roxmltree::{Document, Node, ParsingOptions};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Boolean,
    Integer,
    Long,
    Decimal,
    Date,
    DateTime,
    Id,
    Reference,
    StringList,
    Blob,
    Picklist,
    MultiPicklist,
    AutoNumber,
    Unsupported(String),
}
impl FieldKind {
    pub fn apex_type(&self) -> &'static str {
        match self {
            Self::Boolean => "Boolean",
            Self::Integer => "Integer",
            Self::Long => "Long",
            Self::Decimal => "Decimal",
            Self::Date => "Date",
            Self::DateTime => "Datetime",
            Self::Id | Self::Reference => "Id",
            Self::Blob => "Blob",
            Self::StringList => "List<String>",
            _ => "String",
        }
    }
}
#[derive(Clone, Debug)]
pub struct FieldSchema {
    pub name: String,
    pub kind: FieldKind,
    pub required: bool,
    pub writable: bool,
    pub unique: bool,
    pub external_id: bool,
    pub case_sensitive: bool,
    pub length: Option<usize>,
    pub precision: Option<u32>,
    pub scale: Option<u32>,
    pub default_value: Option<String>,
    pub formula: Option<String>,
    pub reference_to: Vec<String>,
    pub relationship_name: Option<String>,
    pub child_relationship_name: Option<String>,
    pub picklist_values: Vec<String>,
    pub restricted_picklist: bool,
    pub delete_behavior: Option<String>,
    pub metadata_relationship: bool,
    pub compound_components: Vec<String>,
}
impl FieldSchema {
    pub fn new(name: impl Into<String>, kind: FieldKind) -> Self {
        Self {
            name: name.into(),
            kind,
            required: false,
            writable: true,
            unique: false,
            external_id: false,
            case_sensitive: false,
            length: None,
            precision: None,
            scale: None,
            default_value: None,
            formula: None,
            reference_to: vec![],
            relationship_name: None,
            child_relationship_name: None,
            picklist_values: vec![],
            restricted_picklist: false,
            delete_behavior: None,
            metadata_relationship: false,
            compound_components: vec![],
        }
    }
    pub fn apex_type(&self) -> &'static str {
        if self.metadata_relationship {
            "String"
        } else {
            self.kind.apex_type()
        }
    }
}
#[derive(Clone, Debug)]
pub struct ObjectSchema {
    pub name: String,
    pub key_prefix: String,
    pub fields: BTreeMap<String, FieldSchema>,
    pub custom_metadata: bool,
    pub platform_event: bool,
    pub read_only: bool,
    pub auto_number_format: Option<String>,
    pub publish_behavior: String,
    pub sharing_model: Option<String>,
}
impl ObjectSchema {
    fn new(name: &str, prefix: &str) -> Self {
        Self {
            name: name.into(),
            key_prefix: prefix.into(),
            fields: BTreeMap::new(),
            custom_metadata: name.to_ascii_lowercase().ends_with("__mdt"),
            platform_event: name.to_ascii_lowercase().ends_with("__e"),
            read_only: name.to_ascii_lowercase().ends_with("__mdt"),
            auto_number_format: None,
            publish_behavior: String::new(),
            sharing_model: None,
        }
    }
    pub fn field(&self, name: &str) -> Option<&FieldSchema> {
        self.fields.get(&name.to_ascii_lowercase())
    }
    fn add(&mut self, f: FieldSchema) {
        self.fields.insert(f.name.to_ascii_lowercase(), f);
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MetadataScalar {
    Binary(std::sync::Arc<[u8]>),
    Null,
    Boolean(bool),
    Number(String),
    Text(String),
}
#[derive(Clone, Debug)]
pub struct MetadataRecord {
    pub object: String,
    pub developer_name: String,
    pub label: String,
    pub protected: bool,
    pub values: BTreeMap<String, MetadataScalar>,
}
#[derive(Clone, Debug, Default)]
pub struct ObjectPermission {
    pub read: bool,
    pub create: bool,
    pub edit: bool,
    pub delete: bool,
    pub view_all: bool,
    pub modify_all: bool,
}
#[derive(Clone, Debug, Default)]
pub struct FieldPermission {
    pub read: bool,
    pub edit: bool,
}
#[derive(Clone, Debug, Default)]
pub struct PermissionSet {
    pub name: String,
    pub label: String,
    pub objects: BTreeMap<String, ObjectPermission>,
    pub fields: BTreeMap<String, FieldPermission>,
    /// Source API names are metadata, including names outside the local execution schema.
    pub object_api_names: BTreeMap<String, String>,
    pub field_api_names: BTreeMap<String, String>,
    pub user_permissions: BTreeSet<String>,
}
#[derive(Clone, Debug)]
pub struct MetadataBlocker {
    pub path: String,
    pub object: Option<String>,
    pub capability: String,
    pub reason: String,
}
#[derive(Clone, Debug)]
pub struct SchemaError {
    pub path: String,
    pub message: String,
}
impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}
impl std::error::Error for SchemaError {}
#[derive(Clone, Debug)]
pub struct SchemaCatalog {
    pub objects: BTreeMap<String, ObjectSchema>,
    pub metadata_records: Vec<MetadataRecord>,
    pub permission_sets: BTreeMap<String, PermissionSet>,
    pub blockers: Vec<MetadataBlocker>,
    pub loaded_files: usize,
}
impl Default for SchemaCatalog {
    fn default() -> Self {
        Self::standard()
    }
}
impl SchemaCatalog {
    pub fn object(&self, name: &str) -> Option<&ObjectSchema> {
        self.objects.get(&name.to_ascii_lowercase())
    }
    pub fn field(&self, object: &str, field: &str) -> Option<&FieldSchema> {
        self.object(object)?.field(field)
    }
    pub fn blockers_for(&self, object: &str, capability: &str) -> Vec<&MetadataBlocker> {
        self.blockers
            .iter()
            .filter(|b| {
                (b.capability == capability || b.capability == "all")
                    && b.object
                        .as_ref()
                        .is_none_or(|o| o.eq_ignore_ascii_case(object))
            })
            .collect()
    }
    pub fn standard() -> Self {
        let mut c = Self {
            objects: BTreeMap::new(),
            metadata_records: vec![],
            permission_sets: BTreeMap::new(),
            blockers: vec![],
            loaded_files: 0,
        };
        // Explicit versioned local schema; unlisted standard fields remain unsupported.
        for (name,prefix,text,required) in [
            ("Account","001","Name Description Industry Phone Fax Website Type AccountNumber Site TickerSymbol Rating Ownership Sic SicDesc TradeStyle BillingStreet BillingCity BillingState BillingPostalCode BillingCountry ShippingStreet ShippingCity ShippingState ShippingPostalCode ShippingCountry","Name"),
            ("Contact","003","FirstName LastName Salutation Description Email Phone MobilePhone HomePhone OtherPhone Fax Title Department MailingStreet MailingCity MailingState MailingPostalCode MailingCountry OtherStreet OtherCity OtherState OtherPostalCode OtherCountry LeadSource AssistantName AssistantPhone","LastName"),
            ("Lead","00Q","FirstName LastName Company Status Email Phone Title Industry Description Street City State PostalCode Country LeadSource Rating Website","LastName Company"),
            ("Opportunity","006","Name StageName Type Description LeadSource NextStep","Name StageName CloseDate"),
            ("Task","00T","Subject Status Priority Description Type",""),
            ("Case","500","Subject Status Priority Origin Description Type Reason SuppliedEmail SuppliedName SuppliedPhone SuppliedCompany",""),
            ("Campaign","701","Name Type Status Description","Name"),
            ("User","005","Username Alias Email FirstName LastName CompanyName TimeZoneSidKey LocaleSidKey EmailEncodingKey LanguageLocaleKey FederationIdentifier Title Department","Username LastName Email Alias TimeZoneSidKey LocaleSidKey EmailEncodingKey LanguageLocaleKey ProfileId"),
            ("Profile","00e","Name UserType","Name"),("PermissionSet","0PS","Name Label Description","Name"),
            ("PermissionSetGroup","0PG","DeveloperName MasterLabel Status","DeveloperName MasterLabel"),
            ("PermissionSetAssignment","0Pa","","AssigneeId"),("PermissionSetGroupComponent","0PM","","PermissionSetGroupId PermissionSetId"),
            ("ContentVersion","068","Title PathOnClient Description ContentLocation FileType","Title PathOnClient VersionData"),
            ("ContentDocument","069","Title FileType",""),("ContentDocumentLink","06A","ShareType Visibility","ContentDocumentId LinkedEntityId"),
            ("EmailMessage","02s","Subject TextBody HtmlBody FromAddress FromName ToAddress CcAddress BccAddress Headers MessageIdentifier ThreadIdentifier Status",""),("EmailMessageRelation","0JL","RelationType RelationAddress","EmailMessageId"),
            ("AppMenuItem","0DS","Name Label Type",""),("AppDefinition","02u","DeveloperName Label",""),
            ("ContentDistribution","05D","Name ContentDownloadUrl DistributionPublicUrl","Name ContentVersionId"),
            ("StaticResource","081","Name ContentType Description CacheControl","Name Body"),
            ("EntityDefinition","01I","QualifiedApiName DeveloperName Label PluralLabel",""),("FieldDefinition","00N","QualifiedApiName DeveloperName Label DataType",""),
            ("AsyncApexJob","707","Status JobType MethodName ExtendedStatus",""),
            ("CronTrigger","08e","CronExpression State TimeZoneSidKey",""),
            ("CronJobDetail","08a","Name JobType",""),
            ("Organization","00D","Name OrganizationType InstanceName LanguageLocaleKey TimeZoneSidKey NamespacePrefix",""),
            ("ApexClass","01p","Name NamespacePrefix Body Status",""),
            ("PlatformCachePartition","0m8","DeveloperName MasterLabel NamespacePrefix",""),
            ("ObjectPermissions","110","SObjectType","ParentId SObjectType"),
            ("FieldPermissions","111","SObjectType Field","ParentId SObjectType Field"),
        ] { let mut o=ObjectSchema::new(name,prefix);system_fields(&mut o);for f in text.split_whitespace(){o.add(FieldSchema::new(f,FieldKind::Text));}for f in required.split_whitespace(){o.fields.entry(f.to_ascii_lowercase()).or_insert_with(||FieldSchema::new(f,FieldKind::Text)).required=true;}c.objects.insert(name.to_ascii_lowercase(),o); }
        for (obj, fields, kind) in [
            ("Account", "NumberOfEmployees", FieldKind::Integer),
            ("Account", "AnnualRevenue", FieldKind::Decimal),
            (
                "Account",
                "BillingLatitude BillingLongitude ShippingLatitude ShippingLongitude",
                FieldKind::Decimal,
            ),
            ("Contact", "Birthdate", FieldKind::Date),
            (
                "Opportunity",
                "Amount Probability ExpectedRevenue TotalOpportunityQuantity",
                FieldKind::Decimal,
            ),
            ("Opportunity", "CloseDate", FieldKind::Date),
            ("Task", "ActivityDate", FieldKind::Date),
            (
                "Campaign",
                "ActualCost BudgetedCost ExpectedRevenue",
                FieldKind::Decimal,
            ),
            ("Campaign", "StartDate EndDate", FieldKind::Date),
            ("User", "IsActive", FieldKind::Boolean),
            ("ContentVersion", "VersionData", FieldKind::Blob),
            ("StaticResource", "Body", FieldKind::Blob),
            (
                "AsyncApexJob",
                "JobItemsProcessed TotalJobItems NumberOfErrors",
                FieldKind::Integer,
            ),
        ] {
            for f in fields.split_whitespace() {
                let mut field = FieldSchema::new(f, kind.clone());
                if let Some(old) = c.field(obj, f) {
                    field.required = old.required;
                }
                c.objects
                    .get_mut(&obj.to_ascii_lowercase())
                    .unwrap()
                    .add(field);
            }
        }
        for (obj, field, targets, child) in [
            ("Account", "ParentId", "Account", "ChildAccounts"),
            ("Contact", "AccountId", "Account", "Contacts"),
            ("Opportunity", "AccountId", "Account", "Opportunities"),
            ("Case", "AccountId", "Account", "Cases"),
            ("Case", "ContactId", "Contact", "Cases"),
            ("Task", "WhoId", "Contact Lead", "Tasks"),
            ("Task", "WhatId", "Account Opportunity Case", "Tasks"),
            ("User", "ProfileId", "Profile", "Users"),
            ("User", "ManagerId", "User", "DirectReports"),
            (
                "PermissionSetAssignment",
                "AssigneeId",
                "User",
                "PermissionSetAssignments",
            ),
            (
                "PermissionSetAssignment",
                "PermissionSetId",
                "PermissionSet",
                "Assignments",
            ),
            (
                "PermissionSetAssignment",
                "PermissionSetGroupId",
                "PermissionSetGroup",
                "Assignments",
            ),
            (
                "PermissionSetGroupComponent",
                "PermissionSetId",
                "PermissionSet",
                "PermissionSetGroupComponents",
            ),
            (
                "PermissionSetGroupComponent",
                "PermissionSetGroupId",
                "PermissionSetGroup",
                "PermissionSetGroupComponents",
            ),
            (
                "ContentVersion",
                "ContentDocumentId",
                "ContentDocument",
                "ContentVersions",
            ),
            (
                "ContentVersion",
                "FirstPublishLocationId",
                "Account Contact User",
                "ContentVersions",
            ),
            (
                "ContentDocumentLink",
                "ContentDocumentId",
                "ContentDocument",
                "ContentDocumentLinks",
            ),
            (
                "ContentDocumentLink",
                "LinkedEntityId",
                "Account Contact User",
                "ContentDocumentLinks",
            ),
            (
                "EmailMessage",
                "RelatedToId",
                "Account Contact",
                "EmailMessages",
            ),
            (
                "EmailMessageRelation",
                "EmailMessageId",
                "EmailMessage",
                "EmailMessageRelations",
            ),
            (
                "EmailMessageRelation",
                "RelationId",
                "Contact User",
                "EmailMessageRelations",
            ),
        ] {
            let o = c.objects.get_mut(&obj.to_ascii_lowercase()).unwrap();
            let mut f = FieldSchema::new(field, FieldKind::Reference);
            f.required = o.field(field).is_some_and(|f| f.required);
            f.reference_to = targets.split_whitespace().map(str::to_string).collect();
            f.relationship_name = Some(field.trim_end_matches("Id").into());
            f.child_relationship_name = Some(child.into());
            f.delete_behavior = match (obj, field) {
                ("Contact", "AccountId")
                | ("Opportunity", "AccountId")
                | ("Task", "WhoId")
                | ("Task", "WhatId") => Some("Cascade".into()),
                ("Case", "AccountId") | ("Case", "ContactId") => Some("Restrict".into()),
                ("Account", "ParentId") | ("User", "ManagerId") => Some("SetNull".into()),
                _ => None,
            };
            o.add(f);
        }
        for obj in ["Contact", "Lead"] {
            let mut f = FieldSchema::new("Name", FieldKind::Text);
            f.writable = false;
            c.objects.get_mut(&obj.to_ascii_lowercase()).unwrap().add(f);
        }
        let mut case_number = FieldSchema::new("CaseNumber", FieldKind::AutoNumber);
        case_number.writable = false;
        c.objects.get_mut("case").unwrap().add(case_number);
        for (object,names,kind) in [("Organization","IsSandbox IsReadOnly",FieldKind::Boolean),("Organization","FiscalYearStartMonth",FieldKind::Integer),("ApexClass","ApiVersion",FieldKind::Decimal),("PlatformCachePartition","IsDefaultPartition",FieldKind::Boolean),("PlatformCachePartition","AllocatedCapacity AllocatedSessionCapacity",FieldKind::Decimal),("FieldDefinition","RelationshipName EntityDefinitionId",FieldKind::Text),("FieldDefinition","IsCalculated",FieldKind::Boolean),("ObjectPermissions","PermissionsRead PermissionsCreate PermissionsEdit PermissionsDelete PermissionsViewAllRecords PermissionsModifyAllRecords",FieldKind::Boolean),("FieldPermissions","PermissionsRead PermissionsEdit",FieldKind::Boolean)] {for name in names.split_whitespace(){c.objects.get_mut(&object.to_ascii_lowercase()).unwrap().add(FieldSchema::new(name,kind.clone()));}}
        for object in ["ObjectPermissions", "FieldPermissions"] {
            let mut f = FieldSchema::new("ParentId", FieldKind::Reference);
            f.required = true;
            f.reference_to = vec!["PermissionSet".into()];
            f.relationship_name = Some("Parent".into());
            c.objects
                .get_mut(&object.to_ascii_lowercase())
                .unwrap()
                .add(f);
        }
        let mut entity = FieldSchema::new("EntityDefinitionId", FieldKind::Reference);
        entity.writable = false;
        entity.reference_to = vec!["EntityDefinition".into()];
        entity.relationship_name = Some("EntityDefinition".into());
        entity.child_relationship_name = Some("Fields".into());
        c.objects.get_mut("fielddefinition").unwrap().add(entity);
        for object in [
            "Organization",
            "ApexClass",
            "PlatformCachePartition",
            "EntityDefinition",
            "FieldDefinition",
        ] {
            let o = c.objects.get_mut(&object.to_ascii_lowercase()).unwrap();
            o.read_only = true;
            for f in o.fields.values_mut() {
                f.writable = false;
            }
        }
        c.objects
            .get_mut("user")
            .unwrap()
            .fields
            .get_mut("isactive")
            .unwrap()
            .default_value = Some("true".into());
        for (object, names, kind) in [
            ("AsyncApexJob", "ParentJobId", FieldKind::Id),
            ("AsyncApexJob", "CompletedDate", FieldKind::DateTime),
            ("CronTrigger", "TimesTriggered", FieldKind::Integer),
            (
                "CronTrigger",
                "NextFireTime PreviousFireTime StartTime EndTime",
                FieldKind::DateTime,
            ),
            ("CronTrigger", "CronJobDetailId", FieldKind::Id),
        ] {
            for name in names.split_whitespace() {
                c.objects
                    .get_mut(&object.to_ascii_lowercase())
                    .unwrap()
                    .add(FieldSchema::new(name, kind.clone()));
            }
        }
        for object in ["AsyncApexJob", "CronTrigger", "CronJobDetail"] {
            let object = c.objects.get_mut(&object.to_ascii_lowercase()).unwrap();
            object.read_only = true;
            for field in object.fields.values_mut() {
                field.writable = false;
            }
        }
        let mut application = FieldSchema::new("ApplicationId", FieldKind::Reference);
        application.reference_to = vec!["AppDefinition".into()];
        application.relationship_name = Some("Application".into());
        application.writable = false;
        c.objects.get_mut("appmenuitem").unwrap().add(application);
        for name in ["SortOrder", "UserSortOrder"] {
            c.objects
                .get_mut("appmenuitem")
                .unwrap()
                .add(FieldSchema::new(name, FieldKind::Integer));
        }
        c.objects
            .get_mut("appmenuitem")
            .unwrap()
            .add(FieldSchema::new("IsVisible", FieldKind::Boolean));
        let mut owned = FieldSchema::new("IsOwnedByProfile", FieldKind::Boolean);
        owned.writable = false;
        c.objects.get_mut("permissionset").unwrap().add(owned);
        for name in ["ToIds", "CcIds", "BccIds"] {
            let mut field = FieldSchema::new(name, FieldKind::StringList);
            field.reference_to = vec!["User".into(), "Contact".into(), "Lead".into()];
            c.objects.get_mut("emailmessage").unwrap().add(field);
        }
        c.objects
            .get_mut("emailmessage")
            .unwrap()
            .add(FieldSchema::new("IsClientManaged", FieldKind::Boolean));
        for object in ["ContentVersion", "ContentDocument"] {
            c.objects
                .get_mut(&object.to_ascii_lowercase())
                .unwrap()
                .fields
                .get_mut("filetype")
                .unwrap()
                .writable = false;
        }
        c.objects
            .get_mut("permissionsetgroup")
            .unwrap()
            .fields
            .get_mut("status")
            .unwrap()
            .writable = false;
        for(object,names,kind)in[("ContentDocument","LatestPublishedVersionId",FieldKind::Id),("ContentDocument","ContentSize",FieldKind::Long),("ContentVersion","IsLatest",FieldKind::Boolean),("ContentVersion","VersionNumber FileExtension",FieldKind::Text),("ContentVersion","ContentSize",FieldKind::Long),("ContentDistribution","PreferencesAllowOriginalDownload PreferencesAllowPDFDownload PreferencesAllowViewInBrowser",FieldKind::Boolean),("ApexClass","LengthWithoutComments",FieldKind::Integer)]{for name in names.split_whitespace(){let mut field=FieldSchema::new(name,kind.clone());if object!="ContentDistribution"{field.writable=false;}c.objects.get_mut(&object.to_ascii_lowercase()).unwrap().add(field);}}
        for (name, targets) in [
            ("ContentVersionId", vec!["ContentVersion"]),
            (
                "RelatedRecordId",
                vec!["Account", "Contact", "Case", "Opportunity"],
            ),
        ] {
            let mut field = FieldSchema::new(name, FieldKind::Reference);
            field.required = name == "ContentVersionId";
            field.reference_to = targets.into_iter().map(str::to_string).collect();
            field.relationship_name = Some(name.trim_end_matches("Id").into());
            c.objects.get_mut("contentdistribution").unwrap().add(field);
        }
        for name in ["ContentDownloadUrl", "DistributionPublicUrl"] {
            c.objects
                .get_mut("contentdistribution")
                .unwrap()
                .fields
                .get_mut(&name.to_ascii_lowercase())
                .unwrap()
                .writable = false;
        }
        for (object, prefixes) in [
            ("Account", vec!["Billing", "Shipping"]),
            ("Contact", vec!["Mailing", "Other"]),
        ] {
            for prefix in prefixes {
                let mut field = FieldSchema::new(
                    format!("{prefix}Address"),
                    FieldKind::Unsupported("Address".into()),
                );
                field.compound_components = [
                    "Street",
                    "City",
                    "State",
                    "PostalCode",
                    "Country",
                    "Latitude",
                    "Longitude",
                ]
                .iter()
                .map(|s| format!("{prefix}{s}"))
                .filter(|s| c.field(object, s).is_some())
                .collect();
                c.objects
                    .get_mut(&object.to_ascii_lowercase())
                    .unwrap()
                    .add(field);
            }
        }
        c.objects
            .get_mut("contact")
            .unwrap()
            .fields
            .get_mut("name")
            .unwrap()
            .compound_components = vec!["FirstName".into(), "LastName".into(), "Salutation".into()];
        for object in ["AppMenuItem", "AppDefinition"] {
            let o = c.objects.get_mut(&object.to_ascii_lowercase()).unwrap();
            o.read_only = true;
            for field in o.fields.values_mut() {
                field.writable = false;
            }
        }
        c
    }
    /// Inputs are exact metadata paths collected by project discovery, not arbitrary traversals.
    pub fn load_files(paths: &[PathBuf]) -> Result<Self, SchemaError> {
        if paths.len() > 20_000 {
            return Err(err("", "Metadata file count exceeds 20000"));
        }
        let mut c = Self::standard();
        let mut parsed = vec![];
        let mut total = 0usize;
        for path in paths {
            let name = path
                .file_name()
                .and_then(|x| x.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !(name.ends_with(".xml")
                || name.ends_with(".object")
                || name.ends_with(".flow")
                || name.ends_with(".permissionset")
                || name.ends_with(".profile"))
            {
                continue;
            }
            let mut bytes = vec![];
            fs::File::open(path)
                .and_then(|f| f.take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes))
                .map_err(|e| err(path, e.to_string()))?;
            total += bytes.len();
            if bytes.len() > 2 * 1024 * 1024 || total > 32 * 1024 * 1024 {
                return Err(err(path, "Metadata exceeds 2MiB/file or 32MiB/project"));
            }
            let text =
                String::from_utf8(bytes).map_err(|_| err(path, "Metadata XML is not UTF-8"))?;
            // Empty DTDs are also rejected; no external entity resolver is installed.
            if text.to_ascii_lowercase().contains("<!doctype") {
                return Err(err(path, "DTD declarations are unsupported"));
            }
            parse_xml(&text, path)?;
            parsed.push((path.clone(), text));
        }
        // Object names must exist before decomposed fields and metadata rows resolve.
        for (path, text) in &parsed {
            let d = parse_xml(text, path)?;
            if d.root_element().tag_name().name() == "CustomObject" {
                c.load_object(path, d.root_element())?;
            }
        }
        for (path, text) in &parsed {
            let d = parse_xml(text, path)?;
            let n = d.root_element();
            match n.tag_name().name() {
                "CustomObject" => {}
                "CustomField" => {
                    let obj = object_from_path(path).ok_or_else(|| {
                        err(
                            path,
                            "Decomposed field has no objects/<object>/fields parent",
                        )
                    })?;
                    c.load_field(&obj, path, n)?;
                }
                "CustomMetadata" => {}
                "PermissionSet" => c.load_permission(path, n)?,
                "Flow" => {
                    let status = child_text(n, "status").unwrap_or("Unknown");
                    if status == "Active" {
                        let obj = n
                            .children()
                            .find(|n| n.has_tag_name("start"))
                            .and_then(|n| child_text(n, "object"))
                            .map(str::to_string);
                        c.blockers.push(MetadataBlocker {
                            path: display(path),
                            object: obj,
                            capability: "dml".into(),
                            reason: "Active Flow requires flow execution".into(),
                        });
                    }
                }
                "ValidationRule" => {
                    if bool_text(n, "active", true) {
                        c.blockers.push(MetadataBlocker {
                            path: display(path),
                            object: object_from_path(path),
                            capability: "dml".into(),
                            reason: "Active validation formula is not evaluated".into(),
                        });
                    }
                }
                "Workflow" | "DuplicateRule" | "MatchingRule" | "AssignmentRules"
                | "AutoResponseRules" | "EscalationRules" => c.blockers.push(MetadataBlocker {
                    path: display(path),
                    object: object_from_path(path),
                    capability: "dml".into(),
                    reason: format!("{} execution unsupported", n.tag_name().name()),
                }),
                "Profile" | "SharingRules" | "RestrictionRule" => {
                    c.blockers.push(MetadataBlocker {
                        path: display(path),
                        object: None,
                        capability: "security".into(),
                        reason: format!(
                            "{} access policy requires security context",
                            n.tag_name().name()
                        ),
                    })
                }
                _ => {}
            }
            c.loaded_files += 1;
        }
        for (path, text) in &parsed {
            let d = parse_xml(text, path)?;
            let n = d.root_element();
            match n.tag_name().name() {
                "CustomMetadata" => c.load_metadata(path, n)?,
                "PlatformCachePartition" => {
                    let name = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .split('.')
                        .next()
                        .unwrap_or("")
                        .to_string();
                    let mut values = BTreeMap::from([
                        (
                            "NamespacePrefix".into(),
                            MetadataScalar::Text(String::new()),
                        ),
                        (
                            "IsDefaultPartition".into(),
                            MetadataScalar::Boolean(bool_text(n, "isDefaultPartition", false)),
                        ),
                    ]);
                    for typ in n
                        .children()
                        .filter(|n| n.has_tag_name("platformCachePartitionTypes"))
                    {
                        let field = match child_text(typ, "cacheType") {
                            Some("Organization") => "AllocatedCapacity",
                            Some("Session") => "AllocatedSessionCapacity",
                            _ => return Err(err(path, "Unsupported cache partition type")),
                        };
                        let amount = child_text(typ, "allocatedCapacity").unwrap_or("0");
                        if amount.parse::<u64>().is_err() {
                            return Err(err(path, "Invalid cache allocation"));
                        }
                        values.insert(field.into(), MetadataScalar::Number(amount.into()));
                    }
                    c.metadata_records.push(MetadataRecord {
                        object: "PlatformCachePartition".into(),
                        developer_name: name.clone(),
                        label: child_text(n, "masterLabel").unwrap_or(&name).into(),
                        protected: false,
                        values,
                    });
                }
                "PermissionSetGroup" => {
                    let name = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .split('.')
                        .next()
                        .unwrap_or("")
                        .to_string();
                    c.metadata_records.push(MetadataRecord {
                        object: "PermissionSetGroup".into(),
                        developer_name: name.clone(),
                        label: child_text(n, "label").unwrap_or(&name).into(),
                        protected: false,
                        values: BTreeMap::from([(
                            "Status".into(),
                            MetadataScalar::Text(
                                child_text(n, "status").unwrap_or("Outdated").into(),
                            ),
                        )]),
                    });
                    if n.children().any(|n| n.has_tag_name("permissionSets")) {
                        c.blockers.push(MetadataBlocker {
                            path: display(path),
                            object: None,
                            capability: "security".into(),
                            reason: "Permission set group member calculation is not loaded".into(),
                        });
                    }
                }
                "CustomApplication" => {
                    let name = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .split('.')
                        .next()
                        .unwrap_or("")
                        .to_string();
                    let label = child_text(n, "label").unwrap_or(&name).to_string();
                    c.metadata_records.push(MetadataRecord {
                        object: "AppDefinition".into(),
                        developer_name: name.clone(),
                        label: label.clone(),
                        protected: false,
                        values: BTreeMap::new(),
                    });
                    c.metadata_records.push(MetadataRecord {
                        object: "AppMenuItem".into(),
                        developer_name: name.clone(),
                        label,
                        protected: false,
                        values: BTreeMap::from([
                            ("Name".into(), MetadataScalar::Text(name.clone())),
                            ("ApplicationId".into(), MetadataScalar::Text(name)),
                            ("Type".into(), MetadataScalar::Text("TabSet".into())),
                        ]),
                    });
                }
                _ => {}
            }
        }
        Ok(c)
    }
    fn load_object(&mut self, path: &Path, n: Node<'_, '_>) -> Result<(), SchemaError> {
        let name = object_from_path(path)
            .or_else(|| child_text(n, "fullName").map(str::to_string))
            .or_else(|| {
                path.file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.split('.').next().unwrap_or(s).to_string())
            })
            .ok_or_else(|| err(path, "Object name unavailable"))?;
        let key = name.to_ascii_lowercase();
        let next = self.objects.len();
        let o = self.objects.entry(key.clone()).or_insert_with(|| {
            let mut o = ObjectSchema::new(&name, &format!("a{:02}", next % 100));
            system_fields(&mut o);
            o
        });
        o.sharing_model = child_text(n, "sharingModel").map(str::to_string);
        if o.platform_event {
            o.publish_behavior = child_text(n, "publishBehavior").unwrap_or("").into();
            if !["", "PublishImmediately", "PublishAfterCommit"]
                .contains(&o.publish_behavior.as_str())
            {
                return Err(err(path, "Unsupported platform-event publishBehavior"));
            }
            for (name, kind) in [
                ("EventUuid", FieldKind::Text),
                ("ReplayId", FieldKind::Long),
            ] {
                let mut field = FieldSchema::new(name, kind);
                field.writable = false;
                o.add(field);
            }
        }
        if o.custom_metadata {
            for f in [
                "DeveloperName",
                "MasterLabel",
                "Label",
                "NamespacePrefix",
                "QualifiedApiName",
            ] {
                let mut f = FieldSchema::new(f, FieldKind::Text);
                f.writable = false;
                o.add(f);
            }
        }
        if let Some(nf) = n.children().find(|n| n.has_tag_name("nameField")) {
            let auto = child_text(nf, "type") == Some("AutoNumber");
            let mut f = FieldSchema::new(
                "Name",
                if auto {
                    FieldKind::AutoNumber
                } else {
                    FieldKind::Text
                },
            );
            f.required = !auto;
            f.writable = !auto;
            o.auto_number_format = child_text(nf, "displayFormat").map(str::to_string);
            o.add(f);
        }
        if !o.fields.contains_key("name") && !o.custom_metadata && !o.platform_event {
            let mut f = FieldSchema::new("Name", FieldKind::Text);
            f.required = true;
            o.add(f);
        }
        for field in n.children().filter(|n| n.has_tag_name("fields")) {
            self.load_field(&name, path, field)?;
        }
        for rule in n.children().filter(|n| n.has_tag_name("validationRules")) {
            if bool_text(rule, "active", true) {
                self.blockers.push(MetadataBlocker {
                    path: display(path),
                    object: Some(name.clone()),
                    capability: "dml".into(),
                    reason: "Active embedded validation formula is not evaluated".into(),
                });
            }
        }
        Ok(())
    }
    fn load_field(
        &mut self,
        object: &str,
        path: &Path,
        n: Node<'_, '_>,
    ) -> Result<(), SchemaError> {
        let name = child_text(n, "fullName").ok_or_else(|| err(path, "Field fullName missing"))?;
        let typ = child_text(n, "type").unwrap_or("Text");
        let kind = match typ {
            "Text" | "TextArea" | "LongTextArea" | "Html" | "Email" | "Phone" | "Url"
            | "EncryptedText" => FieldKind::Text,
            "Checkbox" => FieldKind::Boolean,
            "Number" | "Currency" | "Percent" => FieldKind::Decimal,
            "Date" => FieldKind::Date,
            "DateTime" => FieldKind::DateTime,
            "Lookup" | "MasterDetail" | "MetadataRelationship" => FieldKind::Reference,
            "Picklist" => FieldKind::Picklist,
            "MultiselectPicklist" => FieldKind::MultiPicklist,
            "AutoNumber" => FieldKind::AutoNumber,
            other => FieldKind::Unsupported(other.into()),
        };
        let mut f = FieldSchema::new(name, kind);
        f.metadata_relationship = typ == "MetadataRelationship";
        f.required = bool_text(n, "required", false) || typ == "MasterDetail";
        f.unique = bool_text(n, "unique", false);
        f.external_id = bool_text(n, "externalId", false);
        f.case_sensitive = bool_text(n, "caseSensitive", false);
        f.length = child_text(n, "length").and_then(|s| s.parse().ok());
        f.precision = child_text(n, "precision").and_then(|s| s.parse().ok());
        f.scale = child_text(n, "scale").and_then(|s| s.parse().ok());
        f.default_value = child_text(n, "defaultValue").map(str::to_string);
        f.formula = child_text(n, "formula").map(str::to_string);
        f.writable = f.formula.is_none() && f.kind != FieldKind::AutoNumber;
        f.reference_to = n
            .children()
            .filter(|n| n.has_tag_name("referenceTo"))
            .filter_map(|n| n.text().map(str::to_string))
            .collect();
        if typ == "MasterDetail" {
            f.delete_behavior = Some("Cascade".into());
        } else if typ == "Lookup" {
            let behavior = child_text(n, "deleteConstraint").unwrap_or("SetNull");
            if !["SetNull", "Restrict", "Cascade"].contains(&behavior) {
                return Err(err(path, "Unknown lookup deleteConstraint"));
            }
            f.delete_behavior = Some(behavior.into());
        }
        if !f.reference_to.is_empty() {
            f.relationship_name = Some(if name.ends_with("__c") {
                format!("{}__r", name.trim_end_matches("__c"))
            } else {
                name.trim_end_matches("Id").into()
            });
            f.child_relationship_name = child_text(n, "relationshipName").map(|s| {
                if (object.ends_with("__c") || object.ends_with("__mdt")) && !s.ends_with("__r") {
                    format!("{s}__r")
                } else {
                    s.into()
                }
            });
        }
        if let Some(v) = n.children().find(|n| n.has_tag_name("valueSet")) {
            f.restricted_picklist = bool_text(v, "restricted", false);
            f.picklist_values = v
                .descendants()
                .filter(|n| n.has_tag_name("value"))
                .filter_map(|n| child_text(n, "fullName").map(str::to_string))
                .collect();
            if v.descendants().any(|n| n.has_tag_name("valueSetName")) {
                self.blockers.push(MetadataBlocker {
                    path: display(path),
                    object: Some(object.into()),
                    capability: "dml".into(),
                    reason: "Global picklist value set resolution unavailable".into(),
                });
            }
        }
        let o = self
            .objects
            .get_mut(&object.to_ascii_lowercase())
            .ok_or_else(|| err(path, format!("Field references unknown object {object}")))?;
        if o.fields.insert(name.to_ascii_lowercase(), f).is_some() && name.ends_with("__c") {
            return Err(err(path, format!("Duplicate custom field {object}.{name}")));
        }
        Ok(())
    }
    fn load_metadata(&mut self, path: &Path, n: Node<'_, '_>) -> Result<(), SchemaError> {
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| err(path, "Invalid metadata filename"))?;
        let stem = filename
            .strip_suffix(".md-meta.xml")
            .ok_or_else(|| err(path, "Expected <type>.<record>.md-meta.xml"))?;
        let (typ, name) = stem
            .split_once('.')
            .ok_or_else(|| err(path, "Custom metadata name lacks type"))?;
        let object = if typ.ends_with("__mdt") {
            typ.into()
        } else {
            format!("{typ}__mdt")
        };
        let schema = self
            .object(&object)
            .ok_or_else(|| err(path, format!("Unknown custom metadata type {object}")))?;
        let mut values = BTreeMap::new();
        for value in n.children().filter(|n| n.has_tag_name("values")) {
            let field = child_text(value, "field")
                .ok_or_else(|| err(path, "Custom metadata field missing"))?;
            if schema.field(field).is_none() {
                return Err(err(
                    path,
                    format!("Unknown metadata field {object}.{field}"),
                ));
            }
            let v = value
                .children()
                .find(|n| n.has_tag_name("value"))
                .ok_or_else(|| err(path, "Metadata value missing"))?;
            let typ = v
                .attributes()
                .find(|a| a.name() == "type")
                .map(|a| a.value())
                .unwrap_or("string");
            let nil = v
                .attributes()
                .any(|a| a.name() == "nil" && a.value() == "true");
            let text = v.text().unwrap_or("");
            let scalar = if nil {
                MetadataScalar::Null
            } else if typ.ends_with(":boolean") || typ == "boolean" {
                match text {
                    "true" => MetadataScalar::Boolean(true),
                    "false" => MetadataScalar::Boolean(false),
                    _ => return Err(err(path, "Invalid metadata boolean")),
                }
            } else if ["decimal", "double", "int", "integer"]
                .iter()
                .any(|x| typ.ends_with(x))
            {
                MetadataScalar::Number(text.into())
            } else {
                MetadataScalar::Text(text.into())
            };
            if values.insert(field.to_ascii_lowercase(), scalar).is_some() {
                return Err(err(path, "Duplicate custom metadata value"));
            }
        }
        if self.metadata_records.iter().any(|r| {
            r.object.eq_ignore_ascii_case(&object) && r.developer_name.eq_ignore_ascii_case(name)
        }) {
            return Err(err(path, "Duplicate custom metadata record"));
        }
        self.metadata_records.push(MetadataRecord {
            object,
            developer_name: name.into(),
            label: child_text(n, "label").unwrap_or(name).into(),
            protected: bool_text(n, "protected", false),
            values,
        });
        Ok(())
    }
    fn load_permission(&mut self, path: &Path, n: Node<'_, '_>) -> Result<(), SchemaError> {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .split('.')
            .next()
            .unwrap_or("")
            .to_string();
        let mut p = PermissionSet {
            name: name.clone(),
            label: child_text(n, "label").unwrap_or(&name).into(),
            ..Default::default()
        };
        for item in n.children() {
            match item.tag_name().name() {
                "objectPermissions" => {
                    let object = child_text(item, "object")
                        .ok_or_else(|| err(path, "Permission object missing"))?;
                    if !api_identifier(object) {
                        return Err(err(path, "Permission object API name is invalid"));
                    }
                    p.object_api_names
                        .insert(object.to_ascii_lowercase(), object.into());
                    p.objects.insert(
                        object.to_ascii_lowercase(),
                        ObjectPermission {
                            read: bool_text(item, "allowRead", false),
                            create: bool_text(item, "allowCreate", false),
                            edit: bool_text(item, "allowEdit", false),
                            delete: bool_text(item, "allowDelete", false),
                            view_all: bool_text(item, "viewAllRecords", false),
                            modify_all: bool_text(item, "modifyAllRecords", false),
                        },
                    );
                }
                "fieldPermissions" => {
                    let field = child_text(item, "field")
                        .ok_or_else(|| err(path, "Permission field missing"))?;
                    if !field.split_once('.').is_some_and(|(object, name)| {
                        api_identifier(object) && api_identifier(name)
                    }) {
                        return Err(err(path, "Permission field must be Object.Field API names"));
                    }
                    p.field_api_names
                        .insert(field.to_ascii_lowercase(), field.into());
                    p.fields.insert(
                        field.to_ascii_lowercase(),
                        FieldPermission {
                            read: bool_text(item, "readable", false),
                            edit: bool_text(item, "editable", false),
                        },
                    );
                }
                "userPermissions" => {
                    if bool_text(item, "enabled", false) {
                        if let Some(name) = child_text(item, "name") {
                            p.user_permissions.insert(name.to_ascii_lowercase());
                        }
                    }
                }
                _ => {}
            }
        }
        if self
            .permission_sets
            .insert(name.to_ascii_lowercase(), p)
            .is_some()
        {
            return Err(err(path, "Duplicate permission set"));
        }
        Ok(())
    }
}
fn api_identifier(name: &str) -> bool {
    name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
fn system_fields(o: &mut ObjectSchema) {
    for (name, kind) in [
        ("Id", FieldKind::Id),
        ("IsDeleted", FieldKind::Boolean),
        ("CreatedDate", FieldKind::DateTime),
        ("LastModifiedDate", FieldKind::DateTime),
        ("SystemModstamp", FieldKind::DateTime),
        ("CreatedById", FieldKind::Id),
        ("LastModifiedById", FieldKind::Id),
    ] {
        let mut f = FieldSchema::new(name, kind);
        f.writable = false;
        o.add(f);
    }
    let mut owner = FieldSchema::new("OwnerId", FieldKind::Reference);
    owner.reference_to = vec!["User".into()];
    owner.relationship_name = Some("Owner".into());
    o.add(owner);
}
fn child_text<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<&'a str> {
    n.children()
        .find(|n| n.has_tag_name(name))
        .and_then(|n| n.text())
}
fn bool_text(n: Node<'_, '_>, name: &str, default: bool) -> bool {
    child_text(n, name).map(|s| s == "true").unwrap_or(default)
}
fn parse_xml<'a>(text: &'a str, path: &Path) -> Result<Document<'a>, SchemaError> {
    Document::parse_with_options(
        text,
        ParsingOptions {
            allow_dtd: false,
            nodes_limit: 100_000,
            entity_resolver: None,
        },
    )
    .map_err(|e| err(path, e.to_string()))
}
fn object_from_path(path: &Path) -> Option<String> {
    let parts = path.iter().filter_map(|s| s.to_str()).collect::<Vec<_>>();
    parts
        .windows(2)
        .rev()
        .find(|p| p[0].eq_ignore_ascii_case("objects"))
        .map(|p| p[1].split('.').next().unwrap_or(p[1]).to_string())
}
fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
fn err(path: impl AsRef<Path>, message: impl Into<String>) -> SchemaError {
    SchemaError {
        path: display(path.as_ref()),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standard_schema_is_case_insensitive_and_typed() {
        let c = SchemaCatalog::standard();
        assert_eq!(
            c.field("ACCOUNT", "numberofemployees").unwrap().kind,
            FieldKind::Integer
        );
        assert_eq!(
            c.field("Contact", "AccountId").unwrap().reference_to,
            vec!["Account"]
        );
        assert!(c.field("Account", "Missing__c").is_none());
    }
    #[test]
    fn xml_entities_are_not_resolved() {
        assert!(parse_xml(
            "<!DOCTYPE x [<!ENTITY ext SYSTEM 'file:///secret'>]><x>&ext;</x>",
            Path::new("input.xml")
        )
        .is_err());
    }
    #[test]
    fn bounded_xml_rejects_oversized_node_graph() {
        let text = format!("<x>{}</x>", "<v/>".repeat(100_001));
        assert!(parse_xml(&text, Path::new("input.xml")).is_err());
    }
    #[test]
    fn exact_metadata_records_have_typed_values() {
        let mut c = SchemaCatalog::standard();
        let object="<CustomObject><nameField><type>Text</type></nameField><fields><fullName>Enabled__c</fullName><type>Checkbox</type><defaultValue>false</defaultValue></fields></CustomObject>";
        let path = Path::new("objects/Feature__mdt/Feature__mdt.object-meta.xml");
        let d = parse_xml(object, path).unwrap();
        c.load_object(path, d.root_element()).unwrap();
        let xml="<CustomMetadata xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' xmlns:xsd='http://www.w3.org/2001/XMLSchema'><label>Enabled</label><values><field>Enabled__c</field><value xsi:type='xsd:boolean'>true</value></values></CustomMetadata>";
        let p = Path::new("customMetadata/Feature.One.md-meta.xml");
        let d = parse_xml(xml, p).unwrap();
        c.load_metadata(p, d.root_element()).unwrap();
        assert_eq!(
            c.metadata_records[0].values["enabled__c"],
            MetadataScalar::Boolean(true)
        );
        assert!(c.object("Feature__mdt").unwrap().read_only);
    }
    #[test]
    fn permission_names_preserve_source_spelling_without_inventing_runtime_schema() {
        let mut c = SchemaCatalog::standard();
        let path = Path::new("permissionsets/Unmodeled.permissionset-meta.xml");
        let xml = "<PermissionSet><fieldPermissions><field>Account.UnmodeledFeature__c</field><readable>true</readable><editable>true</editable></fieldPermissions><objectPermissions><object>UnmodeledObject__c</object><allowRead>true</allowRead></objectPermissions></PermissionSet>";
        let d = parse_xml(xml, path).unwrap();
        c.load_permission(path, d.root_element()).unwrap();
        let p = &c.permission_sets["unmodeled"];
        assert_eq!(
            p.field_api_names["account.unmodeledfeature__c"],
            "Account.UnmodeledFeature__c"
        );
        assert_eq!(
            p.object_api_names["unmodeledobject__c"],
            "UnmodeledObject__c"
        );
        assert!(c.field("Account", "UnmodeledFeature__c").is_none());
        assert!(c.object("UnmodeledObject__c").is_none());
        let invalid = xml.replace("Account.UnmodeledFeature__c", "Account.Bad.Field");
        let d = parse_xml(&invalid, path).unwrap();
        assert!(c.load_permission(path, d.root_element()).is_err());
    }
}
