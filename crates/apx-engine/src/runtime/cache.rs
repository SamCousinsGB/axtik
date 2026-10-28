//! Transaction-isolated Platform Cache model. Partition allocation is metadata,
//! session identity is explicit environment configuration, and builders run Apex.
use super::*;
use crate::schema::MetadataScalar;
use num_traits::ToPrimitive;

const ENTRY: &str = "cache.entry\0";
const HOST_ENTRIES: usize = 10_000;
const HOST_VALUE_BYTES: usize = 1024 * 1024;

pub(super) fn cache_is_type(t: &str) -> bool {
    matches!(
        t,
        "cache.partition" | "cache.orgpartition" | "cache.sessionpartition"
    )
}
pub(super) fn cache_assignable(actual: &str, expected: &str) -> bool {
    let a = actual.to_ascii_lowercase();
    let e = expected.to_ascii_lowercase();
    a == e
        || e == "cache.partition"
            && matches!(a.as_str(), "cache.orgpartition" | "cache.sessionpartition")
}
fn invalid<T>(at: &Node, message: impl Into<String>) -> R<T> {
    exceptions::raise(at, "Cache.InvalidParamException", message)
}
fn text_arg<'a>(value: &'a Value, at: &Node) -> R<&'a str> {
    match value {
        Value::Str(s) => Ok(s),
        _ => invalid(at, "Cache key/partition must be a non-null String"),
    }
}
fn key_valid(key: &str, at: &Node) -> R<()> {
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric()) {
        return invalid(at, "Cache key must contain only alphanumeric characters");
    }
    if key.len() > 256 {
        return block(at, "Cache key exceeds the local 256-byte bound");
    }
    Ok(())
}
fn field<'a>(fields: &'a BTreeMap<String, Value>, key: &str) -> &'a Value {
    fields.get(key).unwrap_or(&Value::Null)
}
fn int(value: &Value) -> Option<i64> {
    match value {
        Value::Num(n) if n.q.is_integer() => n.q.to_integer().to_i64(),
        _ => None,
    }
}
#[derive(Clone)]
struct Partition {
    name: String,
    session: bool,
    capacity: usize,
}

impl Context<'_> {
    fn cache_partition(&self, name: Option<&str>, session: bool, at: &Node) -> R<Partition> {
        if self
            .program
            .environment
            .as_ref()
            .and_then(|e| e.capabilities.get("platformCache"))
            .and_then(serde_json::Value::as_bool)
            == Some(false)
        {
            return invalid(at, "Platform Cache is disabled by the explicit environment");
        }
        let entries = self
            .program
            .schema
            .metadata_records
            .iter()
            .filter(|r| r.object.eq_ignore_ascii_case("PlatformCachePartition"));
        let records = entries
            .filter(|r| {
                let namespace = match r.values.get("NamespacePrefix") {
                    Some(MetadataScalar::Text(s)) if !s.is_empty() => s.as_str(),
                    _ => "local",
                };
                if let Some(name) = name {
                    name == format!("{namespace}.{}", r.developer_name)
                } else {
                    r.values.get("IsDefaultPartition") == Some(&MetadataScalar::Boolean(true))
                }
            })
            .collect::<Vec<_>>();
        if records.len() != 1 {
            return invalid(
                at,
                "A unique configured cache partition is required; no default partition is inferred",
            );
        }
        let r = records[0];
        let namespace = match r.values.get("NamespacePrefix") {
            Some(MetadataScalar::Text(s)) if !s.is_empty() => s.as_str(),
            _ => "local",
        };
        if namespace != "local" {
            return block(
                at,
                "Cross-namespace cache visibility requires an explicit namespace policy",
            );
        }
        let allocation = if session {
            "AllocatedSessionCapacity"
        } else {
            "AllocatedCapacity"
        };
        let capacity = match r.values.get(allocation) {
            Some(MetadataScalar::Number(s)) => s
                .parse::<usize>()
                .ok()
                .and_then(|n| n.checked_mul(1024 * 1024)),
            None => Some(0),
            _ => None,
        }
        .ok_or_else(|| EvalError {
            exception: None,
            status: "blocked",
            diagnostic: at.diagnostic("APX_UNSUPPORTED", "Invalid cache capacity metadata"),
        })?;
        Ok(Partition {
            name: format!("{namespace}.{}", r.developer_name),
            session,
            capacity,
        })
    }
    fn cache_handle(partition: &Partition) -> Value {
        Value::Instance(Rc::new(RefCell::new(Instance {
            class: if partition.session {
                "Cache.SessionPartition"
            } else {
                "Cache.OrgPartition"
            }
            .into(),
            fields: BTreeMap::from([("\0partition".into(), Value::Str(partition.name.clone()))]),
            field_types: BTreeMap::new(),
        })))
    }
    fn cache_prefix(&self, partition: &Partition, at: &Node) -> R<String> {
        let scope = if partition.session {
            let session = self
                .program
                .environment
                .as_ref()
                .and_then(|e| e.capabilities.get("platformCacheSessionId"))
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty() && s.len() <= 256 && !s.contains('\0'));
            let Some(session) = session else {
                return block(
                    at,
                    "Session cache requires explicit capabilities.platformCacheSessionId",
                );
            };
            let Some(Value::Str(user)) = self.native_state.get("userinfo.userid") else {
                return block(at, "Session cache requires an explicit current user");
            };
            format!("session\0{session}\0{user}")
        } else {
            "org".into()
        };
        Ok(format!("{ENTRY}{scope}\0{}\0", partition.name))
    }
    fn cache_clock(&self, at: &Node) -> R<i64> {
        match self.native_state.get("clock.iso") {
            Some(Value::Str(s)) => match crate::dates::parse_datetime(s) {
                Ok(Value::Datetime(ms)) => Ok(ms),
                _ => block(at, "Cache requires a valid runtime clock"),
            },
            _ => block(at, "Cache runtime clock is unavailable"),
        }
    }
    fn cache_sweep(&mut self, now: i64) {
        self.native_state.retain(|key, value| {
            !key.starts_with(ENTRY)
                || match value {
                    Value::Instance(e) => int(field(&e.borrow().fields, "expires"))
                        .is_some_and(|expires| expires > now),
                    _ => false,
                }
        });
    }
    fn cache_sequence(&mut self) -> i64 {
        let sequence = self
            .native_state
            .get("cache.sequence")
            .and_then(int)
            .unwrap_or(0)
            + 1;
        self.native_state.insert(
            "cache.sequence".into(),
            Value::Num(Number::integer(sequence)),
        );
        sequence
    }
    fn cache_capacity_policy(&self, at: &Node) -> R<()> {
        if self
            .program
            .environment
            .as_ref()
            .and_then(|e| e.capabilities.get("platformCacheCapacityModel"))
            .and_then(serde_json::Value::as_str)
            != Some("apx-value-v1")
        {
            return block(at, "Non-empty cache capacity/eviction requires explicit capabilities.platformCacheCapacityModel=apx-value-v1; Salesforce serialization byte accounting is not implemented");
        }
        Ok(())
    }
    pub(super) fn cache_snapshot(
        &self,
        value: &Value,
        at: &Node,
        depth: usize,
        active: &mut BTreeSet<usize>,
        size: &mut usize,
    ) -> R<Value> {
        if depth > 64 {
            return block(at, "Cached value exceeds the local depth bound");
        }
        *size = size.saturating_add(match value {
            Value::Str(s) => s.len() + 16,
            Value::Blob(b) => b.len() + 16,
            Value::Num(n) => n.display().len() + 16,
            _ => 32,
        });
        if *size > HOST_VALUE_BYTES {
            return block(at, "Cached value exceeds the local 1 MiB bound");
        }
        let identity = match value {
            Value::List(v) | Value::Set(v) | Value::Iterable(v) => Some(Rc::as_ptr(v) as usize),
            Value::Map(v) => Some(Rc::as_ptr(v) as usize),
            Value::Record(v) => Some(Rc::as_ptr(v) as usize),
            Value::Instance(v) => Some(Rc::as_ptr(v) as usize),
            _ => None,
        };
        if let Some(id) = identity {
            if !active.insert(id) {
                return block(at, "Cyclic cached values are not supported");
            }
        }
        let copy = match value {
            Value::List(v) | Value::Set(v) => {
                let items = v
                    .borrow()
                    .iter()
                    .map(|v| self.cache_snapshot(v, at, depth + 1, active, size))
                    .collect::<R<Vec<_>>>()?;
                let is_set = matches!(value, Value::Set(_));
                let contract = if is_set {
                    self.set_types.borrow().get(&identity.unwrap()).cloned()
                } else {
                    self.list_types.borrow().get(&identity.unwrap()).cloned()
                };
                let copy = if is_set {
                    Value::Set(Rc::new(RefCell::new(items)))
                } else {
                    Value::list(items)
                };
                if let Some(t) = contract {
                    self.coerce(
                        copy,
                        &format!("{}<{t}>", if is_set { "set" } else { "list" }),
                        at,
                    )?
                } else {
                    copy
                }
            }
            Value::Map(v) => {
                let pairs = v
                    .borrow()
                    .iter()
                    .map(|(k, v)| {
                        Ok((
                            self.cache_snapshot(k, at, depth + 1, active, size)?,
                            self.cache_snapshot(v, at, depth + 1, active, size)?,
                        ))
                    })
                    .collect::<R<Vec<_>>>()?;
                let contract = self.map_types.borrow().get(&identity.unwrap()).cloned();
                let copy = Value::Map(Rc::new(RefCell::new(pairs)));
                if let Some((k, v)) = contract {
                    self.coerce(copy, &format!("map<{k},{v}>"), at)?
                } else {
                    copy
                }
            }
            Value::Record(r) => {
                let r = r.borrow();
                Value::Record(Rc::new(RefCell::new(Record {
                    typ: r.typ.clone(),
                    projected: r.projected,
                    fields: r
                        .fields
                        .iter()
                        .map(|(k, v)| {
                            Ok((
                                k.clone(),
                                self.cache_snapshot(v, at, depth + 1, active, size)?,
                            ))
                        })
                        .collect::<R<_>>()?,
                })))
            }
            Value::Instance(i) => {
                let i = i.borrow();
                let Some(class) = self.program.classes.get(&i.class.to_ascii_lowercase()) else {
                    return block(
                        at,
                        "Native platform instances cannot be cached by this evaluator",
                    );
                };
                if class.instance_fields.iter().any(|n| {
                    n.child("modifiers")
                        .is_some_and(|m| m.text().to_ascii_lowercase().contains("transient"))
                }) {
                    return block(
                        at,
                        "Cache serialization of transient fields is not implemented",
                    );
                }
                Value::Instance(Rc::new(RefCell::new(Instance {
                    class: i.class.clone(),
                    field_types: i.field_types.clone(),
                    fields: i
                        .fields
                        .iter()
                        .map(|(k, v)| {
                            Ok((
                                k.clone(),
                                self.cache_snapshot(v, at, depth + 1, active, size)?,
                            ))
                        })
                        .collect::<R<_>>()?,
                })))
            }
            Value::Iterable(_) | Value::Type(_) | Value::Namespace(_) | Value::Exception(_) => {
                return block(at, "This value type has no supported cache serialization")
            }
            _ => value.clone(),
        };
        if let Some(id) = identity {
            active.remove(&id);
        }
        self.charge(&copy, at)?;
        Ok(copy)
    }
    fn cache_read(&mut self, key: &str, at: &Node) -> R<Value> {
        let Some(Value::Instance(entry)) = self.native_state.get(key).cloned() else {
            return Ok(Value::Null);
        };
        let value = field(&entry.borrow().fields, "value").clone();
        let seq = self.cache_sequence();
        entry
            .borrow_mut()
            .fields
            .insert("used".into(), Value::Num(Number::integer(seq)));
        self.cache_snapshot(&value, at, 0, &mut BTreeSet::new(), &mut 0)
    }
    fn cache_write(
        &mut self,
        p: &Partition,
        prefix: &str,
        key: &str,
        value: &Value,
        ttl: i64,
        at: &Node,
    ) -> R<()> {
        let max = if p.session { 28_800 } else { 172_800 };
        if ttl < 300 || ttl > max {
            return block(at, format!("Cache TTL outside the documented 300..{max} second range requires qualified boundary behavior"));
        }
        if matches!(value, Value::Null) {
            return invalid(at, "Cache values cannot be null");
        }
        let mut size = key.len();
        let value = self.cache_snapshot(value, at, 0, &mut BTreeSet::new(), &mut size)?;
        let key = format!("{prefix}{key}");
        self.native_state.remove(&key);
        if p.capacity == 0 {
            return Ok(());
        }
        if size > p.capacity {
            self.cache_capacity_policy(at)?;
            return Ok(());
        }
        let mut entries = self
            .native_state
            .iter()
            .filter_map(|(key, value)| {
                if key.starts_with(prefix) {
                    if let Value::Instance(e) = value {
                        let e = e.borrow();
                        return Some((
                            key.clone(),
                            int(field(&e.fields, "size")).unwrap_or(0) as usize,
                            int(field(&e.fields, "used")).unwrap_or(0),
                        ));
                    }
                    None
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|e| e.2);
        let mut used = entries.iter().map(|e| e.1).sum::<usize>();
        if used.saturating_add(size) > p.capacity {
            self.cache_capacity_policy(at)?;
        }
        for (key, bytes, _) in entries {
            if used.saturating_add(size) <= p.capacity {
                break;
            }
            self.native_state.remove(&key);
            used -= bytes;
        }
        if self
            .native_state
            .keys()
            .filter(|k| k.starts_with(ENTRY))
            .count()
            >= HOST_ENTRIES
        {
            return block(at, "Cache exceeds the local 10000-entry bound");
        }
        let expires = self
            .cache_clock(at)?
            .checked_add(ttl * 1000)
            .ok_or_else(|| EvalError {
                exception: None,
                status: "blocked",
                diagnostic: at.diagnostic("APX_UNSUPPORTED", "Cache expiry overflow"),
            })?;
        let used = self.cache_sequence();
        self.native_state.insert(
            key,
            Value::Instance(Rc::new(RefCell::new(Instance {
                class: "\0CacheEntry".into(),
                field_types: BTreeMap::new(),
                fields: BTreeMap::from([
                    ("value".into(), value),
                    ("expires".into(), Value::Num(Number::integer(expires))),
                    ("size".into(), Value::Num(Number::integer(size as i64))),
                    ("used".into(), Value::Num(Number::integer(used))),
                ]),
            }))),
        );
        Ok(())
    }
    pub(super) fn cache_namespace(
        &mut self,
        namespace: &str,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        let namespace = namespace.to_ascii_lowercase();
        if !matches!(namespace.as_str(), "cache.org" | "cache.session") {
            return Ok(None);
        }
        let session = namespace == "cache.session";
        if name == "getpartition" {
            let [value] = args else {
                return invalid(at, "getPartition requires one name");
            };
            return Ok(Some(Self::cache_handle(&self.cache_partition(
                Some(text_arg(value, at)?),
                session,
                at,
            )?)));
        }
        let mut args = args.to_vec();
        let index = if args.first().is_some_and(|v| matches!(v, Value::Type(_))) {
            1
        } else {
            0
        };
        let p = if matches!(name, "get" | "put" | "contains" | "remove") {
            let Some(Value::Str(key)) = args.get(index) else {
                return block(
                    at,
                    "Static cache operations currently require a single String key",
                );
            };
            let parts = key.split('.').collect::<Vec<_>>();
            if parts.len() == 3 {
                let p =
                    self.cache_partition(Some(&format!("{}.{}", parts[0], parts[1])), session, at)?;
                args[index] = Value::Str(parts[2].into());
                p
            } else if parts.len() == 1 {
                self.cache_partition(None, session, at)?
            } else {
                return invalid(
                    at,
                    "Cache keys must be unqualified or namespace.partition.key",
                );
            }
        } else {
            self.cache_partition(None, session, at)?
        };
        self.cache_operation(&p, name, &args, at).map(Some)
    }
    pub(super) fn cache_method(
        &mut self,
        receiver: &Value,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        let Value::Instance(i) = receiver else {
            return Ok(None);
        };
        let (class, partition) = {
            let i = i.borrow();
            (
                i.class.to_ascii_lowercase(),
                field(&i.fields, "\0partition").clone(),
            )
        };
        if !cache_is_type(&class) {
            return Ok(None);
        }
        let p = self.cache_partition(
            Some(text_arg(&partition, at)?),
            class == "cache.sessionpartition",
            at,
        )?;
        self.cache_operation(&p, name, args, at).map(Some)
    }
    fn cache_operation(
        &mut self,
        p: &Partition,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Value> {
        if matches!(
            self.native_state.get("test.seealldata"),
            Some(Value::Bool(true))
        ) {
            return block(
                at,
                "Platform Cache operations are unavailable in SeeAllData=true tests",
            );
        }
        let prefix = self.cache_prefix(p, at)?;
        self.cache_sweep(self.cache_clock(at)?);
        match (name, args) {
            ("getname", []) => Ok(Value::Str(p.name.clone())),
            ("getkeys", []) => {
                let values = self
                    .native_state
                    .keys()
                    .filter_map(|key| key.strip_prefix(&prefix).map(|k| Value::Str(k.into())))
                    .collect();
                self.coerce(Value::Set(Rc::new(RefCell::new(values))), "set<string>", at)
            }
            ("getnumkeys", []) => Ok(Value::Num(Number::integer(
                self.native_state
                    .keys()
                    .filter(|k| k.starts_with(&prefix))
                    .count() as i64,
            ))),
            ("getcapacity", []) => {
                let used = self
                    .native_state
                    .iter()
                    .filter_map(|(k, v)| {
                        if k.starts_with(&prefix) {
                            if let Value::Instance(i) = v {
                                return int(field(&i.borrow().fields, "size"));
                            }
                            None
                        } else {
                            None
                        }
                    })
                    .sum::<i64>();
                if used > 0 {
                    self.cache_capacity_policy(at)?;
                }
                Ok(Value::Num(Number {
                    q: if p.capacity == 0 {
                        BigRational::zero()
                    } else {
                        BigRational::new((used * 100).into(), p.capacity.into())
                    },
                    kind: NumKind::Decimal,
                }))
            }
            ("get" | "contains", [Value::Set(keys)]) => {
                let keys = keys.borrow().clone();
                let pairs = keys
                    .into_iter()
                    .map(|key| Ok((key.clone(), self.cache_operation(p, name, &[key], at)?)))
                    .collect::<R<Vec<_>>>()?;
                self.coerce(
                    Value::Map(Rc::new(RefCell::new(pairs))),
                    if name == "get" {
                        "map<string,object>"
                    } else {
                        "map<string,boolean>"
                    },
                    at,
                )
            }
            ("get" | "contains" | "remove", [key]) => {
                let key = text_arg(key, at)?;
                key_valid(key, at)?;
                let full = format!("{prefix}{key}");
                match name {
                    "get" => self.cache_read(&full, at),
                    "contains" => Ok(Value::Bool(self.native_state.contains_key(&full))),
                    _ => Ok(Value::Bool(self.native_state.remove(&full).is_some())),
                }
            }
            ("get" | "remove", [Value::Type(class), key]) => {
                let key = text_arg(key, at)?;
                key_valid(key, at)?;
                let Some(class) = self.resolve_class(class) else {
                    return invalid(at, "Unknown CacheBuilder class");
                };
                if !self.class_assignable(&class, "cache.cachebuilder") {
                    return invalid(at, "Builder class must implement Cache.CacheBuilder");
                }
                let public_name = self.program.classes[&class].name.clone();
                let builder_key = format!("{public_name}_B_{key}");
                let full = format!("{prefix}{builder_key}");
                if name == "remove" {
                    return Ok(Value::Bool(self.native_state.remove(&full).is_some()));
                }
                let cached = self.cache_read(&full, at)?;
                if !matches!(cached, Value::Null) {
                    return Ok(cached);
                }
                let active = format!("cache.loading\0{full}");
                if self.native_state.contains_key(&active) {
                    return block(at, "Recursive CacheBuilder loading is not supported");
                }
                self.native_state.insert(active.clone(), Value::Bool(true));
                let loaded = (|| {
                    let instance = self.construct_instance(&class, vec![], at)?;
                    let Value::Instance(instance) = instance else {
                        unreachable!()
                    };
                    self.call_instance(instance, "doload", vec![Value::Str(key.into())], at)
                })();
                self.native_state.remove(&active);
                let loaded = loaded?;
                if !matches!(loaded, Value::Null) {
                    self.cache_write(
                        p,
                        &prefix,
                        &builder_key,
                        &loaded,
                        if p.session { 28_800 } else { 86_400 },
                        at,
                    )?;
                }
                Ok(loaded)
            }
            ("put", [key, value]) | ("put", [key, value, Value::Num(_)]) => {
                let key = text_arg(key, at)?;
                key_valid(key, at)?;
                let ttl = if args.len() == 3 {
                    int(&args[2]).ok_or_else(|| EvalError {
                        exception: None,
                        status: "failed",
                        diagnostic: at.diagnostic("APX_RUNTIME", "Cache TTL must be an integer"),
                    })?
                } else if p.session {
                    28_800
                } else {
                    86_400
                };
                self.cache_write(p, &prefix, key, value, ttl, at)?;
                Ok(Value::Null)
            }
            _ => block(
                at,
                format!(
                    "Cache {}.{name} overload is not implemented",
                    if p.session {
                        "SessionPartition"
                    } else {
                        "OrgPartition"
                    }
                ),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(source: &str, default: bool, capacity: usize) -> (Program, Node) {
        let n = crate::syntax::parse(source, "cache.cls").unwrap();
        let mut p = Program::default();
        crate::syntax::declarations(&n, &mut p, &mut vec![]);
        p.schema
            .metadata_records
            .push(crate::schema::MetadataRecord {
                object: "PlatformCachePartition".into(),
                developer_name: "Example".into(),
                label: "Example".into(),
                protected: false,
                values: BTreeMap::from([
                    (
                        "NamespacePrefix".into(),
                        MetadataScalar::Text(String::new()),
                    ),
                    (
                        "IsDefaultPartition".into(),
                        MetadataScalar::Boolean(default),
                    ),
                    (
                        "AllocatedCapacity".into(),
                        MetadataScalar::Number(capacity.to_string()),
                    ),
                    (
                        "AllocatedSessionCapacity".into(),
                        MetadataScalar::Number(capacity.to_string()),
                    ),
                ]),
            });
        let environment = serde_json::json!({"schemaVersion":1,"clock":{"now":"2026-01-15T12:00:00Z"},"currentUserId":"005000000000001AAA","records":[{"object":"User","fields":{"Id":"005000000000001AAA","LastName":"Local"}}],"capabilities":{"platformCache":true,"platformCacheSessionId":"explicit-session","platformCacheCapacityModel":"apx-value-v1"}});
        p.environment =
            Some(crate::environment::Environment::parse(&environment, &p.schema, "test").unwrap());
        (p, n)
    }
    fn partition(c: &mut Context<'_>, session: bool, n: &Node) -> Value {
        c.cache_namespace(
            if session {
                "cache.session"
            } else {
                "cache.org"
            },
            "getpartition",
            &[Value::Str("local.Example".into())],
            n,
        )
        .unwrap()
        .unwrap()
    }
    #[test]
    fn metadata_controls_default_zero_capacity_and_missing_partitions() {
        let (p, n) = fixture("class C{}", false, 0);
        let mut c = Context::new(&p);
        assert!(c
            .cache_namespace("cache.org", "get", &[Value::Str("Key".into())], &n)
            .unwrap_err()
            .exception
            .is_some());
        assert!(c
            .cache_namespace(
                "cache.org",
                "getpartition",
                &[Value::Str("local.Other".into())],
                &n
            )
            .is_err());
        let partition = partition(&mut c, false, &n);
        c.cache_method(
            &partition,
            "put",
            &[Value::Str("Key".into()), Value::Str("value".into())],
            &n,
        )
        .unwrap();
        assert!(matches!(
            c.cache_method(&partition, "get", &[Value::Str("Key".into())], &n)
                .unwrap(),
            Some(Value::Null)
        ));
        let (p, n) = fixture("class C{}", true, 1);
        let mut c = Context::new(&p);
        c.cache_namespace(
            "cache.org",
            "put",
            &[Value::Str("Key".into()), Value::Str("yes".into())],
            &n,
        )
        .unwrap();
        assert_eq!(
            c.cache_namespace(
                "cache.org",
                "get",
                &[Value::Str("local.Example.Key".into())],
                &n
            )
            .unwrap()
            .unwrap()
            .display(),
            "yes"
        );
    }
    #[test]
    fn cache_copies_nested_values_and_expires_at_the_runtime_clock() {
        let (p, n) = fixture("class C{}", false, 1);
        let mut c = Context::new(&p);
        let p = partition(&mut c, false, &n);
        let list = c
            .coerce(
                Value::list(vec![Value::Str("before".into())]),
                "list<string>",
                &n,
            )
            .unwrap();
        c.cache_method(
            &p,
            "put",
            &[
                Value::Str("Key".into()),
                list.clone(),
                Value::Num(Number::integer(300)),
            ],
            &n,
        )
        .unwrap();
        let Value::List(list) = list else {
            unreachable!()
        };
        list.borrow_mut()[0] = Value::Str("changed input".into());
        let copy = c
            .cache_method(&p, "get", &[Value::Str("Key".into())], &n)
            .unwrap()
            .unwrap();
        assert_eq!(copy.display(), "[before]");
        let Value::List(copy) = copy else {
            unreachable!()
        };
        copy.borrow_mut()[0] = Value::Str("changed output".into());
        assert_eq!(
            c.cache_method(&p, "get", &[Value::Str("Key".into())], &n)
                .unwrap()
                .unwrap()
                .display(),
            "[before]"
        );
        c.native_state.insert(
            "clock.iso".into(),
            Value::Str("2026-01-15T12:04:59Z".into()),
        );
        assert_eq!(
            c.cache_method(&p, "contains", &[Value::Str("Key".into())], &n)
                .unwrap()
                .unwrap()
                .display(),
            "true"
        );
        c.native_state.insert(
            "clock.iso".into(),
            Value::Str("2026-01-15T12:05:00Z".into()),
        );
        assert_eq!(
            c.cache_method(&p, "getnumkeys", &[], &n)
                .unwrap()
                .unwrap()
                .display(),
            "0"
        );
    }
    #[test]
    fn session_user_and_test_context_isolation_are_real() {
        let (p, n) = fixture("class C{}", false, 1);
        let mut a = Context::new(&p);
        let s = partition(&mut a, true, &n);
        let o = partition(&mut a, false, &n);
        a.cache_method(
            &s,
            "put",
            &[Value::Str("Key".into()), Value::Str("private".into())],
            &n,
        )
        .unwrap();
        assert!(matches!(
            a.cache_method(&o, "get", &[Value::Str("Key".into())], &n)
                .unwrap(),
            Some(Value::Null)
        ));
        a.native_state.insert(
            "userinfo.userid".into(),
            Value::Str("005000000000002AAA".into()),
        );
        assert!(matches!(
            a.cache_method(&s, "get", &[Value::Str("Key".into())], &n)
                .unwrap(),
            Some(Value::Null)
        ));
        a.native_state.insert(
            "userinfo.userid".into(),
            Value::Str("005000000000001AAA".into()),
        );
        assert_eq!(
            a.cache_method(&s, "get", &[Value::Str("Key".into())], &n)
                .unwrap()
                .unwrap()
                .display(),
            "private"
        );
        let mut b = Context::new(&p);
        assert!(matches!(
            b.cache_method(&s, "get", &[Value::Str("Key".into())], &n)
                .unwrap(),
            Some(Value::Null)
        ));
        let mut no_session = p.clone();
        no_session.environment.as_mut().unwrap().capabilities["platformCacheSessionId"] =
            serde_json::Value::Null;
        let mut c = Context::new(&no_session);
        assert_eq!(
            c.cache_method(&s, "getkeys", &[], &n).unwrap_err().status,
            "blocked"
        );
    }
    #[test]
    fn builders_execute_once_per_key_and_again_after_explicit_removal() {
        let (p,n)=fixture("public class Builder implements Cache.CacheBuilder { public static Integer count=0; public Object doLoad(String key){ count++; return key+count; } }",false,1);
        let mut c = Context::new(&p);
        let p = partition(&mut c, false, &n);
        let args = [Value::Type("Builder".into()), Value::Str("Key".into())];
        assert_eq!(
            c.cache_method(&p, "get", &args, &n)
                .unwrap()
                .unwrap()
                .display(),
            "Key1"
        );
        assert_eq!(
            c.cache_method(&p, "get", &args, &n)
                .unwrap()
                .unwrap()
                .display(),
            "Key1"
        );
        assert_eq!(
            c.cache_method(&p, "getkeys", &[], &n)
                .unwrap()
                .unwrap()
                .display(),
            "[Builder_B_Key]"
        );
        assert_eq!(
            c.cache_method(&p, "remove", &args, &n)
                .unwrap()
                .unwrap()
                .display(),
            "true"
        );
        assert_eq!(
            c.cache_method(&p, "get", &args, &n)
                .unwrap()
                .unwrap()
                .display(),
            "Key2"
        );
    }
    #[test]
    fn invalid_unknown_and_unserializable_operations_never_succeed() {
        let (p, n) = fixture("class C{}", false, 1);
        let mut c = Context::new(&p);
        let p = partition(&mut c, false, &n);
        assert!(c
            .cache_method(&p, "put", &[Value::Null, Value::Str("x".into())], &n)
            .unwrap_err()
            .exception
            .is_some());
        assert!(c
            .cache_method(&p, "put", &[Value::Str("Key".into()), Value::Null], &n)
            .unwrap_err()
            .exception
            .is_some());
        assert_eq!(
            c.cache_method(
                &p,
                "put",
                &[
                    Value::Str("Key".into()),
                    Value::Str("x".into()),
                    Value::Num(Number::integer(1))
                ],
                &n
            )
            .unwrap_err()
            .status,
            "blocked"
        );
        assert_eq!(
            c.cache_method(&p, "getavgg ettime", &[], &n)
                .unwrap_err()
                .status,
            "blocked"
        );
        let cyclic = Value::list(vec![]);
        let Value::List(items) = &cyclic else {
            unreachable!()
        };
        items.borrow_mut().push(cyclic.clone());
        assert_eq!(
            c.cache_method(&p, "put", &[Value::Str("Key".into()), cyclic], &n)
                .unwrap_err()
                .status,
            "blocked"
        );
    }
    #[test]
    fn capacity_requires_declared_model_and_evicts_least_recently_used() {
        let (mut p, n) = fixture("class C{}", false, 1);
        p.environment.as_mut().unwrap().capabilities["platformCacheCapacityModel"] =
            serde_json::Value::Null;
        let mut c = Context::new(&p);
        let p = partition(&mut c, false, &n);
        c.cache_method(
            &p,
            "put",
            &[Value::Str("A".into()), Value::Str("x".into())],
            &n,
        )
        .unwrap();
        assert_eq!(
            c.cache_method(&p, "getcapacity", &[], &n)
                .unwrap_err()
                .status,
            "blocked"
        );
        let (p, n) = fixture("class C{}", false, 1);
        let mut c = Context::new(&p);
        let p = partition(&mut c, false, &n);
        let large = Value::Str("x".repeat(400_000));
        for key in ["A", "B"] {
            c.cache_method(&p, "put", &[Value::Str(key.into()), large.clone()], &n)
                .unwrap();
        }
        c.cache_method(&p, "get", &[Value::Str("A".into())], &n)
            .unwrap();
        c.cache_method(&p, "put", &[Value::Str("C".into()), large], &n)
            .unwrap();
        assert_eq!(
            c.cache_method(&p, "getkeys", &[], &n)
                .unwrap()
                .unwrap()
                .display(),
            "[A, C]"
        );
    }
}
