//! Synchronous Test.stopTest job drain, not a production scheduler.
//! Jobs execute actual Apex in fresh limit/context frames with committed data.
use super::*;
use crate::record_store::{DataValue, RecordData};
use chrono::Datelike;
use num_traits::ToPrimitive;

#[derive(Clone, Debug)]
pub(super) enum JobKind {
    Queueable(Value),
    Future {
        class: String,
        method: String,
        args: Vec<Value>,
        callouts: bool,
    },
    Scheduled(Value),
    Batch {
        instance: Value,
        scope: usize,
    },
}
#[derive(Clone, Debug)]
pub(super) struct AsyncJob {
    pub id: String,
    pub kind: JobKind,
    pub at: Node,
    pub user: Option<Value>,
}
pub(super) fn async_is_type(typ: &str) -> bool {
    matches!(
        typ.trim_start_matches("system."),
        "queueablecontext"
            | "schedulablecontext"
            | "database.batchablecontext"
            | "database.querylocator"
    )
}
fn native(class: &str, fields: BTreeMap<String, Value>) -> Value {
    Value::Instance(Rc::new(RefCell::new(Instance {
        class: class.into(),
        fields,
        field_types: BTreeMap::new(),
    })))
}
fn store_error(at: &Node, error: crate::record_store::StoreError) -> EvalError {
    EvalError {
        exception: None,
        status: if error.unsupported {
            "blocked"
        } else {
            "failed"
        },
        diagnostic: at.diagnostic("APX_ASYNC", error.to_string()),
    }
}
fn integer(value: &Value) -> Option<i64> {
    match value {
        Value::Num(n) if n.q.is_integer() => n.q.to_integer().to_i64(),
        _ => None,
    }
}

impl<'a> Context<'a> {
    fn async_snapshot(&self, value: &Value, at: &Node) -> R<Value> {
        self.cache_snapshot(value, at, 0, &mut BTreeSet::new(), &mut 0)
    }
    fn async_register_job(
        &mut self,
        kind: JobKind,
        job_type: &str,
        method: Option<&str>,
        at: &Node,
    ) -> R<String> {
        if self.test_frame.is_none() {
            return block(
                at,
                "Async submission outside Test.startTest/stopTest is not implemented",
            );
        }
        if self.native_state.contains_key("async.kind") {
            return block(
                at,
                "Nested asynchronous test submission requires qualified chaining semantics",
            );
        }
        if self.async_jobs.len() >= 50 {
            return block(at, "Asynchronous submission limit exceeded (50 jobs)");
        }
        self.ensure_data_store(at)?;
        let mut record = RecordData::new("AsyncApexJob")
            .with("Status", DataValue::Text("Queued".into()))
            .with("JobType", DataValue::Text(job_type.into()))
            .with("JobItemsProcessed", DataValue::integer(0))
            .with("TotalJobItems", DataValue::integer(1))
            .with("NumberOfErrors", DataValue::integer(0));
        if let Some(method) = method {
            record.set("MethodName", DataValue::Text(method.into()));
        }
        let id = self
            .data_store
            .as_mut()
            .unwrap()
            .upsert_platform_record(&record)
            .map_err(|e| store_error(at, e))?;
        self.async_sequence += 1;
        self.async_jobs.push(AsyncJob {
            id: id.clone(),
            kind,
            at: at.clone(),
            user: self.native_state.get("userinfo.userid").cloned(),
        });
        Ok(id)
    }
    pub(super) fn async_enqueue_future(
        &mut self,
        class: &str,
        method: &Method,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        let modifiers = method
            .declaration
            .child("modifiers")
            .map(|n| n.text().to_ascii_lowercase())
            .unwrap_or_default();
        if !modifiers.contains("@future") {
            return Ok(None);
        }
        let key = format!(
            "{}.{}",
            class.to_ascii_lowercase(),
            method.name.to_ascii_lowercase()
        );
        if matches!(self.native_state.get("async.invokeFuture"),Some(Value::Str(s)) if s==&key) {
            self.native_state.remove("async.invokeFuture");
            return Ok(None);
        }
        if !method.static_method || !method.return_type.eq_ignore_ascii_case("void") {
            return fail(at, "@future methods must be static and return void");
        }
        fn primitive(v: &Value) -> bool {
            match v {
                Value::Null
                | Value::Bool(_)
                | Value::Num(_)
                | Value::Str(_)
                | Value::Date(_)
                | Value::Datetime(_)
                | Value::Time(_)
                | Value::Blob(_) => true,
                Value::List(v) | Value::Set(v) => v
                    .borrow()
                    .iter()
                    .all(|v| !matches!(v, Value::List(_) | Value::Set(_)) && primitive(v)),
                _ => false,
            }
        }
        if !args.iter().all(primitive) {
            return fail(
                at,
                "@future arguments must be primitives or collections of primitives",
            );
        }
        let args = args
            .iter()
            .map(|v| self.async_snapshot(v, at))
            .collect::<R<_>>()?;
        self.async_register_job(
            JobKind::Future {
                class: class.into(),
                method: method.name.clone(),
                args,
                callouts: modifiers.replace(' ', "").contains("callout=true"),
            },
            "Future",
            Some(&method.name),
            at,
        )?;
        Ok(Some(Value::Null))
    }
    pub(super) fn async_namespace(
        &mut self,
        namespace: &str,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        let ns = namespace.to_ascii_lowercase();
        if ns == "system" {
            match (name, args) {
                ("enqueuejob", [value @ Value::Instance(instance)]) => {
                    if !self.class_assignable(&instance.borrow().class, "queueable") {
                        return fail(at, "enqueueJob requires a Queueable class");
                    }
                    let snapshot = self.async_snapshot(value, at)?;
                    return Ok(Some(Value::Str(self.async_register_job(
                        JobKind::Queueable(snapshot),
                        "Queueable",
                        None,
                        at,
                    )?)));
                }
                (
                    "schedule",
                    [Value::Str(label), Value::Str(expression), value @ Value::Instance(instance)],
                ) => {
                    if !self.class_assignable(&instance.borrow().class, "schedulable") {
                        return fail(at, "schedule requires a Schedulable class");
                    }
                    if label.is_empty() || label.len() > 80 {
                        return fail(at, "Scheduled job name must contain 1..80 characters");
                    }
                    self.ensure_data_store(at)?;
                    let user = self.native_state.get("userinfo.userid").and_then(|v| {
                        if let Value::Str(s) = v {
                            Some(s.as_str())
                        } else {
                            None
                        }
                    });
                    let timezone = user
                        .and_then(|id| self.data_store.as_ref().unwrap().find("User", id))
                        .and_then(|r| r.get("TimeZoneSidKey"))
                        .and_then(DataValue::text);
                    if !matches!(timezone, Some("GMT" | "UTC")) {
                        return block(
                            at,
                            "Scheduling currently requires an explicit GMT/UTC user timezone",
                        );
                    }
                    if self
                        .native_state
                        .contains_key(&format!("async.scheduleName.{label}"))
                    {
                        return exceptions::raise(
                            at,
                            "System.AsyncException",
                            "A scheduled job already uses this name",
                        );
                    }
                    let now = match self.native_state.get("clock.iso") {
                        Some(Value::Str(s)) => s.as_str(),
                        _ => return block(at, "Scheduling requires explicit runtime clock"),
                    };
                    let next = next_fire(expression, now).map_err(|message| EvalError {
                        exception: None,
                        status: "blocked",
                        diagnostic: at.diagnostic("APX_CRON", message),
                    })?;
                    let snapshot = self.async_snapshot(value, at)?;
                    let job = self.async_register_job(
                        JobKind::Scheduled(snapshot),
                        "ScheduledApex",
                        None,
                        at,
                    )?;
                    let trigger = RecordData::new("CronTrigger")
                        .with("CronExpression", DataValue::Text(expression.clone()))
                        .with("TimesTriggered", DataValue::integer(0))
                        .with("NextFireTime", DataValue::DateTime(next))
                        .with("State", DataValue::Text("WAITING".into()))
                        .with("TimeZoneSidKey", DataValue::Text("GMT".into()));
                    let detail = RecordData::new("CronJobDetail")
                        .with("Name", DataValue::Text(label.clone()))
                        .with("JobType", DataValue::Text("7".into()));
                    let detail_id = self
                        .data_store
                        .as_mut()
                        .unwrap()
                        .upsert_platform_record(&detail)
                        .map_err(|e| store_error(at, e))?;
                    let trigger = trigger.with("CronJobDetailId", DataValue::Text(detail_id));
                    let id = self
                        .data_store
                        .as_mut()
                        .unwrap()
                        .upsert_platform_record(&trigger)
                        .map_err(|e| store_error(at, e))?;
                    self.native_state
                        .insert(format!("async.cron.{job}"), Value::Str(id.clone()));
                    self.native_state.insert(
                        format!("async.scheduleName.{label}"),
                        Value::Str(id.clone()),
                    );
                    return Ok(Some(Value::Str(id)));
                }
                ("isfuture" | "isqueueable" | "isbatch" | "isscheduled", []) => {
                    let expected = match name {
                        "isfuture" => "FUTURE",
                        "isqueueable" => "QUEUEABLE",
                        "isbatch" => "BATCH_APEX",
                        _ => "SCHEDULED",
                    };
                    return Ok(Some(Value::Bool(
                        matches!(self.native_state.get("async.kind"),Some(Value::Str(s)) if s==expected),
                    )));
                }
                ("enqueuejob" | "schedule", _) => {
                    return block(at, format!("Unsupported System.{name} overload"))
                }
                _ => {}
            }
        }
        if ns == "database" {
            match name {
                "executebatch" => {
                    let (value, scope) = match args {
                        [value] => (value, 200),
                        [value, size] => {
                            let Some(size) = integer(size).filter(|n| (1..=2000).contains(n))
                            else {
                                return fail(at, "Batch scope must be 1..2000");
                            };
                            (value, size as usize)
                        }
                        _ => {
                            return block(
                                at,
                                "executeBatch requires an instance and optional scope",
                            )
                        }
                    };
                    let Value::Instance(instance) = value else {
                        return fail(at, "executeBatch requires a Batchable class");
                    };
                    let class = instance.borrow().class.to_ascii_lowercase();
                    if !self.program.classes.get(&class).is_some_and(|c| {
                        c.interfaces.iter().any(|i| {
                            i.to_ascii_lowercase()
                                .replace(' ', "")
                                .starts_with("database.batchable<")
                        })
                    }) {
                        return fail(at, "executeBatch requires Database.Batchable<T>");
                    }
                    let instance = self.async_snapshot(value, at)?;
                    return Ok(Some(Value::Str(self.async_register_job(
                        JobKind::Batch { instance, scope },
                        "BatchApex",
                        None,
                        at,
                    )?)));
                }
                "getquerylocator" => {
                    let rows = match args {
                        [Value::Str(_)] => self
                            .data_method(&Value::Namespace("database".into()), "query", args, at)?
                            .ok_or_else(|| EvalError {
                                exception: None,
                                status: "blocked",
                                diagnostic: at
                                    .diagnostic("APX_QUERY", "Query locator source unavailable"),
                            })?,
                        [value @ Value::List(_)] => self.async_snapshot(value, at)?,
                        _ => return block(at, "Unsupported getQueryLocator overload"),
                    };
                    if !matches!(rows, Value::List(_)) {
                        return fail(at, "QueryLocator requires a record query");
                    }
                    return Ok(Some(native(
                        "database.querylocator",
                        BTreeMap::from([("rows".into(), rows)]),
                    )));
                }
                _ => {}
            }
        }
        if ns == "limits" && args.is_empty() {
            match name {
                "getqueueablejobs" => {
                    return Ok(Some(Value::Num(Number::integer(
                        self.async_jobs
                            .iter()
                            .filter(|j| matches!(j.kind, JobKind::Queueable(_)))
                            .count() as i64,
                    ))))
                }
                "getlimitqueueablejobs" => {
                    return Ok(Some(Value::Num(Number::integer(
                        if self.native_state.contains_key("async.kind") {
                            1
                        } else {
                            50
                        },
                    ))))
                }
                "getfuturecalls" => {
                    return Ok(Some(Value::Num(Number::integer(
                        self.async_jobs
                            .iter()
                            .filter(|j| matches!(j.kind, JobKind::Future { .. }))
                            .count() as i64,
                    ))))
                }
                "getlimitfuturecalls" => return Ok(Some(Value::Num(Number::integer(50)))),
                _ => {}
            }
        }
        Ok(None)
    }
    pub(super) fn async_method(
        &mut self,
        receiver: &Value,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        let Value::Instance(i) = receiver else {
            return Ok(None);
        };
        let i = i.borrow();
        if !async_is_type(&i.class) {
            return Ok(None);
        }
        if !args.is_empty() {
            return fail(at, "Async context getters take no arguments");
        }
        match (i.class.trim_start_matches("system."), name) {
            ("queueablecontext" | "database.batchablecontext", "getjobid")
            | ("schedulablecontext", "gettriggerid") => {
                Ok(Some(i.fields.get("id").cloned().unwrap_or(Value::Null)))
            }
            _ => block(at, format!("{}.{} is not implemented", i.class, name)),
        }
    }
    fn async_child(&self, job: &AsyncJob, kind: &str, callouts: bool) -> Context<'a> {
        let mut child = Context::new(self.program);
        child.assertions = self.assertions.clone();
        child.install_data_store(self.data_store.as_ref().map(|s| s.fork_transaction()));
        child.statics = self.statics.clone();
        child.static_types = self.static_types.clone();
        child.initialized = self.initialized.clone();
        *child.list_types.borrow_mut() = self.list_types.borrow().clone();
        *child.set_types.borrow_mut() = self.set_types.borrow().clone();
        *child.map_types.borrow_mut() = self.map_types.borrow().clone();
        *child.list_owners.borrow_mut() = self.list_owners.borrow().clone();
        *child.set_owners.borrow_mut() = self.set_owners.borrow().clone();
        *child.map_owners.borrow_mut() = self.map_owners.borrow().clone();
        child.fuel = self.fuel;
        child.allocated.set(self.allocated.get());
        for (key, value) in &self.native_state {
            if key == "clock.iso" || key == "test.seealldata" || key == "http.calloutMock" {
                child.native_state.insert(key.clone(), value.clone());
            }
        }
        if self.native_state.contains_key("search.fixedIds")
            || self
                .native_state
                .contains_key("search.unqualifiedAsyncFixed")
        {
            child
                .native_state
                .insert("search.unqualifiedAsyncFixed".into(), Value::Bool(true));
        }
        if let Some(user) = &job.user {
            child
                .native_state
                .insert("userinfo.userid".into(), user.clone());
            if let (Some(store), Value::Str(user)) = (&mut child.data_store, user) {
                store.current_user = Some(user.clone());
            }
        }
        child
            .native_state
            .insert("request.quiddity".into(), Value::Str(kind.into()));
        child
            .native_state
            .insert("async.kind".into(), Value::Str(kind.into()));
        child
            .native_state
            .insert("async.callouts".into(), Value::Bool(callouts));
        child
    }
    fn async_call_callback(
        &mut self,
        instance: &Value,
        method: &str,
        args: Vec<Value>,
        at: &Node,
    ) -> R<Value> {
        let Value::Instance(instance) = instance else {
            return fail(at, "Async receiver is not a class instance");
        };
        let class = instance.borrow().class.to_ascii_lowercase();
        let static_method = self.program.classes.get(&class).is_some_and(|c| {
            c.methods.iter().any(|m| {
                m.name.eq_ignore_ascii_case(method)
                    && m.params.len() == args.len()
                    && m.static_method
            })
        });
        if static_method {
            self.call(&class, method, args, at)
        } else {
            self.call_instance(instance.clone(), method, args, at)
        }
    }
    fn async_execute(&mut self, job: &AsyncJob) -> R<()> {
        let (kind, callouts) = match &job.kind {
            JobKind::Future { callouts, .. } => ("FUTURE", *callouts),
            JobKind::Scheduled(_) => ("SCHEDULED", false),
            JobKind::Queueable(Value::Instance(i)) => (
                "QUEUEABLE",
                self.class_assignable(&i.borrow().class, "database.allowscallouts"),
            ),
            JobKind::Batch {
                instance: Value::Instance(i),
                ..
            } => (
                "BATCH_APEX",
                self.class_assignable(&i.borrow().class, "database.allowscallouts"),
            ),
            _ => return block(&job.at, "Invalid queued job payload"),
        };
        let mut child = self.async_child(job, kind, callouts);
        let result = (|| {
            match &job.kind {
                JobKind::Future {
                    class,
                    method,
                    args,
                    ..
                } => {
                    child.native_state.insert(
                        "async.invokeFuture".into(),
                        Value::Str(format!(
                            "{}.{}",
                            class.to_ascii_lowercase(),
                            method.to_ascii_lowercase()
                        )),
                    );
                    child.call(class, method, args.clone(), &job.at)?;
                }
                JobKind::Queueable(instance) => {
                    let context = native(
                        "queueablecontext",
                        BTreeMap::from([("id".into(), Value::Str(job.id.clone()))]),
                    );
                    child.async_call_callback(instance, "execute", vec![context], &job.at)?;
                }
                JobKind::Scheduled(instance) => {
                    let trigger = self
                        .native_state
                        .get(&format!("async.cron.{}", job.id))
                        .cloned()
                        .unwrap_or(Value::Null);
                    let context = native(
                        "schedulablecontext",
                        BTreeMap::from([("id".into(), trigger)]),
                    );
                    child.async_call_callback(instance, "execute", vec![context], &job.at)?;
                }
                JobKind::Batch { instance, scope } => {
                    let original = child.async_snapshot(instance, &job.at)?;
                    let context = native(
                        "database.batchablecontext",
                        BTreeMap::from([("id".into(), Value::Str(job.id.clone()))]),
                    );
                    let started = child.async_call_callback(
                        instance,
                        "start",
                        vec![context.clone()],
                        &job.at,
                    )?;
                    let rows =
                        match started {
                            Value::List(rows) => rows,
                            Value::Instance(locator)
                                if locator.borrow().class == "database.querylocator" =>
                            {
                                let value = locator
                                    .borrow()
                                    .fields
                                    .get("rows")
                                    .cloned()
                                    .unwrap_or(Value::Null);
                                let Value::List(rows) = value else {
                                    return block(&job.at, "QueryLocator has no rows");
                                };
                                rows
                            }
                            _ => return block(
                                &job.at,
                                "Batch start must return QueryLocator or supported List iterable",
                            ),
                        };
                    if rows.borrow().len() > *scope {
                        return block(&job.at,"A test may execute only one batch scope; reduce records or increase scope");
                    }
                    let stateful = if let Value::Instance(i) = instance {
                        child.class_assignable(&i.borrow().class, "database.stateful")
                    } else {
                        false
                    };
                    if !rows.borrow().is_empty() {
                        child.queries = 0;
                        child.dml = 0;
                        child.callouts = 0;
                        if let Some(s) = &mut child.data_store {
                            s.reset_limits();
                        }
                        let execute_instance = if stateful {
                            instance.clone()
                        } else {
                            child.async_snapshot(&original, &job.at)?
                        };
                        child.async_call_callback(
                            &execute_instance,
                            "execute",
                            vec![context.clone(), Value::List(rows)],
                            &job.at,
                        )?;
                    }
                    child.queries = 0;
                    child.dml = 0;
                    child.callouts = 0;
                    if let Some(s) = &mut child.data_store {
                        s.reset_limits();
                    }
                    let finish_instance = if stateful {
                        instance.clone()
                    } else {
                        child.async_snapshot(&original, &job.at)?
                    };
                    child.async_call_callback(
                        &finish_instance,
                        "finish",
                        vec![context],
                        &job.at,
                    )?;
                }
            }
            Ok(())
        })();
        self.fuel = child.fuel;
        self.allocated.set(child.allocated.get());
        self.debug_logs.extend(child.debug_logs);
        self.statics = child.statics;
        self.static_types = child.static_types;
        self.initialized = child.initialized;
        *self.list_types.borrow_mut() = child.list_types.into_inner();
        *self.set_types.borrow_mut() = child.set_types.into_inner();
        *self.map_types.borrow_mut() = child.map_types.into_inner();
        *self.list_owners.borrow_mut() = child.list_owners.into_inner();
        *self.set_owners.borrow_mut() = child.set_owners.into_inner();
        *self.map_owners.borrow_mut() = child.map_owners.into_inner();
        if result.is_ok() {
            if let Some(store) = child.data_store {
                self.data_store.as_mut().unwrap().merge_committed(store);
            }
            self.events_absorb(child.event_state);
        }
        let record = RecordData::new("AsyncApexJob")
            .with("Id", DataValue::Text(job.id.clone()))
            .with(
                "Status",
                DataValue::Text(
                    if result.is_ok() {
                        "Completed"
                    } else {
                        "Failed"
                    }
                    .into(),
                ),
            )
            .with(
                "NumberOfErrors",
                DataValue::integer(if result.is_ok() { 0 } else { 1 }),
            )
            .with(
                "JobItemsProcessed",
                DataValue::integer(if result.is_ok() { 1 } else { 0 }),
            )
            .with(
                "CompletedDate",
                DataValue::DateTime(self.data_store.as_ref().unwrap().now.clone()),
            );
        self.data_store
            .as_mut()
            .unwrap()
            .upsert_platform_record(&record)
            .map_err(|e| store_error(&job.at, e))?;
        result
    }
    pub(super) fn async_stop_test(&mut self, _at: &Node) -> R<()> {
        let jobs = std::mem::take(&mut self.async_jobs);
        let mut error = None;
        for job in jobs {
            if let Err(e) = self.async_execute(&job) {
                if error.is_none() {
                    error = Some(e);
                }
            }
        }
        match error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    pub fn assert_async_drained(&self, at: &Node) -> R<()> {
        if !self.async_jobs.is_empty() {
            return block(at,"Test completed with queued asynchronous jobs that were never executed; call Test.stopTest()");
        }
        Ok(())
    }
}

// Bounded numeric Quartz subset: lists, ranges, steps, *, ?, optional year.
// Named months/days and L/W/# modifiers stay explicit unsupported syntax.
fn cron_values(text: &str, min: u32, max: u32) -> Result<BTreeSet<u32>, String> {
    let mut values = BTreeSet::new();
    for part in text.split(',') {
        let (base, step) = if let Some((base, step)) = part.split_once('/') {
            (base, step.parse::<u32>().map_err(|_| "Invalid cron step")?)
        } else {
            (part, 1)
        };
        if step == 0 || step > max + 1 {
            return Err("Invalid cron step".into());
        }
        let (a, b) = if base == "*" || base == "?" {
            (min, max)
        } else if let Some((a, b)) = base.split_once('-') {
            (
                a.parse::<u32>().map_err(|_| "Unsupported cron token")?,
                b.parse::<u32>().map_err(|_| "Unsupported cron token")?,
            )
        } else {
            let a = base.parse::<u32>().map_err(|_| "Unsupported cron token")?;
            (a, if part.contains('/') { max } else { a })
        };
        if a < min || b > max || a > b {
            return Err("Cron field out of range".into());
        }
        values.extend((a..=b).step_by(step as usize));
    }
    if values.is_empty() {
        return Err("Empty cron field".into());
    }
    Ok(values)
}
fn next_fire(expression: &str, now: &str) -> Result<String, String> {
    if expression.len() > 512 {
        return Err("Cron expression exceeds local bound".into());
    }
    let parts = expression.split_whitespace().collect::<Vec<_>>();
    if !(6..=7).contains(&parts.len()) {
        return Err("Cron requires six or seven fields".into());
    }
    if (parts[3] == "?") == (parts[5] == "?") {
        return Err("One of day-of-month/day-of-week must be ?".into());
    }
    if parts
        .iter()
        .enumerate()
        .any(|(index, p)| index != 3 && index != 5 && p.contains('?'))
    {
        return Err("? is only valid in day-of-month/day-of-week".into());
    }
    let seconds = cron_values(parts[0], 0, 59)?;
    let minutes = cron_values(parts[1], 0, 59)?;
    let hours = cron_values(parts[2], 0, 23)?;
    let days = cron_values(parts[3], 1, 31)?;
    let months = cron_values(parts[4], 1, 12)?;
    let weekdays = cron_values(parts[5], 1, 7)?;
    let years = cron_values(parts.get(6).copied().unwrap_or("*"), 1970, 2199)?;
    let now = chrono::DateTime::parse_from_rfc3339(now)
        .map_err(|_| "Invalid scheduler clock")?
        .with_timezone(&chrono::Utc);
    let mut day = now.date_naive();
    for _ in 0..=366 * 174 {
        if years.contains(&(day.year() as u32))
            && months.contains(&day.month())
            && days.contains(&day.day())
            && weekdays.contains(&(day.weekday().num_days_from_sunday() + 1))
        {
            for h in &hours {
                for m in &minutes {
                    for s in &seconds {
                        let value = day
                            .and_hms_opt(*h, *m, *s)
                            .ok_or("Invalid schedule date")?
                            .and_utc();
                        if value > now {
                            return Ok(value.format("%Y-%m-%dT%H:%M:%SZ").to_string());
                        }
                    }
                }
            }
        }
        day = day.succ_opt().ok_or("Schedule date overflow")?;
        if day.year() > 2199 {
            break;
        }
    }
    Err("No future firing within the local scheduler bound".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn program(source: &str) -> (Program, Node) {
        let n = crate::syntax::parse(source, "async.cls").unwrap();
        let mut p = Program::default();
        crate::syntax::declarations(&n, &mut p, &mut vec![]);
        let environment = serde_json::json!({"schemaVersion":1,"clock":{"now":"2026-01-15T12:00:00Z"},"currentUserId":"005000000000001AAA","records":[{"object":"User","fields":{"Id":"005000000000001AAA","LastName":"Local","TimeZoneSidKey":"GMT"}}]});
        p.environment =
            Some(crate::environment::Environment::parse(&environment, &p.schema, "test").unwrap());
        (p, n)
    }
    #[test]
    fn queueable_is_deferred_snapshotted_and_executes_actual_dml() {
        let (p,n)=program("public class Work implements Queueable { public String label; public Work(String s){label=s;} public void execute(QueueableContext c){System.assert(System.isQueueable());System.assertEquals(0,Limits.getQueries());System.assertNotEquals(null,c.getJobId());insert new Account(Name=label);} } @IsTest class Host { static void run(){Test.startTest();Work w=new Work('submitted');Id job=System.enqueueJob(w);w.label='mutated';System.assertEquals(0,[SELECT COUNT() FROM Account]);Test.stopTest();System.assertEquals('submitted',[SELECT Name FROM Account].Name);System.assertEquals('Completed',[SELECT Status FROM AsyncApexJob WHERE Id=:job].Status);} }");
        Context::new(&p).call("host", "run", vec![], &n).unwrap();
    }
    #[test]
    fn future_runs_only_at_stop_and_test_statics_are_observable() {
        let (p,n)=program("public class Work {public static Integer value=7; @future public static void later(Integer n){System.assert(System.isFuture());value+=n;insert new Account(Name='future');}} @IsTest class Host {static void run(){Test.startTest();Work.later(4);System.assertEquals(7,Work.value);Test.stopTest();System.assertEquals(11,Work.value);System.assertEquals(1,[SELECT COUNT() FROM Account]);}} ");
        Context::new(&p).call("host", "run", vec![], &n).unwrap();
    }
    #[test]
    fn job_failure_is_not_swallowed_and_data_is_rolled_back() {
        let (p,n)=program("public class Work implements Queueable{public void execute(QueueableContext c){insert new Account(Name='must rollback');System.assert(false,'job mutation detected');}} @IsTest class Host{static void run(){Test.startTest();System.enqueueJob(new Work());}}");
        let mut c = Context::new(&p);
        c.call("host", "run", vec![], &n).unwrap();
        let error = c.async_stop_test(&n).unwrap_err();
        assert_eq!(error.status, "failed");
        assert!(error.diagnostic.message.contains("job mutation detected"));
        let store = c.data_store.unwrap();
        assert!(store.records("Account", false).is_empty());
        assert_eq!(
            store.records("AsyncApexJob", false)[0].get("Status"),
            Some(&DataValue::Text("Failed".into()))
        );
    }
    #[test]
    fn stateful_batch_executes_start_scope_finish_and_records_results() {
        let (p,n)=program("public class Work implements Database.Batchable<SObject>,Database.Stateful{public Integer seen=0;public static Integer result=0;public Database.QueryLocator start(Database.BatchableContext c){return Database.getQueryLocator('SELECT Id,Name FROM Account');}public void execute(Database.BatchableContext c,List<Account> rows){seen+=rows.size();for(Account a:rows){a.Name='changed';}update rows;}public void finish(Database.BatchableContext c){result=seen;}} @IsTest class Host{static void run(){insert new Account(Name='before');Test.startTest();Database.executeBatch(new Work(),2);Test.stopTest();System.assertEquals(1,Work.result);System.assertEquals('changed',[SELECT Name FROM Account].Name);}}");
        Context::new(&p).call("host", "run", vec![], &n).unwrap();
    }
    #[test]
    fn schedule_has_real_record_and_callback_without_advancing_production_clock() {
        let (p,n)=program("public class Work implements Schedulable{public static Integer calls=0;public void execute(SchedulableContext c){System.assert(System.isScheduled());System.assertNotEquals(null,c.getTriggerId());calls++;}} @IsTest class Host{static void run(){Test.startTest();Id job=System.schedule('declared job','0 0 0 28 5 ? 2099',new Work());System.assertEquals(0,Work.calls);Test.stopTest();System.assertEquals(1,Work.calls);CronTrigger row=[SELECT CronExpression,TimesTriggered FROM CronTrigger WHERE Id=:job];System.assertEquals(0,row.TimesTriggered);System.assertEquals('0 0 0 28 5 ? 2099',row.CronExpression);}}");
        Context::new(&p).call("host", "run", vec![], &n).unwrap();
    }
    #[test]
    fn cron_source_values_and_unsupported_tokens_are_not_ignored() {
        assert_eq!(
            next_fire("0 0 0 28 5 ? 2099", "2026-01-15T12:00:00Z").unwrap(),
            "2099-05-28T00:00:00Z"
        );
        assert_eq!(
            next_fire("0 15 9 ? * 2-6", "2026-01-16T10:00:00Z").unwrap(),
            "2026-01-19T09:15:00Z"
        );
        assert_ne!(
            next_fire("0 0 0 29 5 ? 2099", "2026-01-15T12:00:00Z").unwrap(),
            "2099-05-28T00:00:00Z"
        );
        assert!(next_fire("0 0 0 L * ?", "2026-01-15T12:00:00Z").is_err());
        assert!(next_fire("0 0 0 ? * ?", "2026-01-15T12:00:00Z").is_err());
        assert!(next_fire("? 0 0 * * ?", "2026-01-15T12:00:00Z").is_err());
    }
    #[test]
    fn non_stateful_batch_does_not_retain_instance_changes_between_callbacks() {
        let (p,n)=program("public class Work implements Database.Batchable<SObject>{public Integer seen=0;public static Integer result=0;public List<Account> start(Database.BatchableContext c){seen=5;return [SELECT Id FROM Account];}public void execute(Database.BatchableContext c,List<Account> rows){System.assertEquals(0,seen);seen=9;}public void finish(Database.BatchableContext c){result=seen;}} @IsTest class Host{static void run(){insert new Account(Name='A');Test.startTest();Database.executeBatch(new Work());Test.stopTest();System.assertEquals(0,Work.result);}}");
        Context::new(&p).call("host", "run", vec![], &n).unwrap();
    }
    #[test]
    fn unfinished_jobs_are_visible_to_the_completion_gate() {
        let (p,n)=program("public class Work implements Queueable{public void execute(QueueableContext c){}} @IsTest class Host{static void run(){Test.startTest();System.enqueueJob(new Work());}}");
        let mut c = Context::new(&p);
        c.call("host", "run", vec![], &n).unwrap();
        assert_eq!(c.assert_async_drained(&n).unwrap_err().status, "blocked");
        c.async_stop_test(&n).unwrap();
        c.assert_async_drained(&n).unwrap();
    }
}
