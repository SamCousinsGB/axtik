//! Local test event bus. Publication, delivery, failure and callbacks are distinct.
use super::*;
use crate::record_store::{DataValue, SaveResult};
use rand::RngCore;

#[derive(Clone, Debug)]
pub(super) struct EventBatch {
    identity: String,
    records: Vec<Rc<RefCell<Record>>>,
    callback: Option<Value>,
    after_commit: bool,
    at: Node,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{FieldKind, FieldSchema};
    fn program(source: &str) -> (Program, Node) {
        let node = crate::syntax::parse(source, "events.cls").unwrap();
        let mut p = Program::default();
        crate::syntax::declarations(&node, &mut p, &mut vec![]);
        for (name, behavior) in [
            ("Notice__e", "PublishAfterCommit"),
            ("Immediate__e", "PublishImmediately"),
        ] {
            let mut event = p.schema.objects["account"].clone();
            event.name = name.into();
            event.platform_event = true;
            event.fields.clear();
            event.key_prefix = "e00".into();
            event.publish_behavior = behavior.into();
            let mut message = FieldSchema::new("Message__c", FieldKind::Text);
            message.required = true;
            event.fields.insert("message__c".into(), message);
            let mut id = FieldSchema::new("EventUuid", FieldKind::Text);
            id.writable = false;
            event.fields.insert("eventuuid".into(), id);
            p.schema.objects.insert(name.to_ascii_lowercase(), event);
        }
        let fixture = serde_json::json!({"schemaVersion":1,"clock":{"now":"2026-01-15T12:00:00Z"},"currentUserId":"005000000000001AAA","records":[
          {"object":"User","fields":{"Id":"005000000000001AAA","LastName":"Original","IsActive":true}},
          {"object":"User","fields":{"Id":"005000000000005AAA","LastName":"Automation","IsActive":true}}
        ],"capabilities":{"automatedProcessUserId":"005000000000005AAA"}});
        p.environment =
            Some(crate::environment::Environment::parse(&fixture, &p.schema, "test").unwrap());
        (p, node)
    }
    #[test]
    fn publication_snapshots_payload_and_delivers_actual_trigger_and_callback() {
        let (p, n) = program(
            r#"
          trigger NoticeTrigger on Notice__e(after insert) {
            Assert.areEqual(null,Trigger.newMap);
            for(Notice__e e:Trigger.new) { insert new Account(Name=e.Message__c); }
          }
          public class Callback implements EventBus.EventPublishSuccessCallback {
            public void onSuccess(EventBus.SuccessResult result) {
              Assert.areEqual(1,result.getEventUuids().size());
              Assert.areEqual('005000000000005AAA',UserInfo.getUserId());
              insert new Account(Name='callback');
            }
          }
          @IsTest class Check {public static void run(){
            Test.startTest();
            Notice__e event=new Notice__e(Message__c='original');
            Database.SaveResult result=EventBus.publish(event,new Callback());
            Assert.isTrue(result.isSuccess()); event.Message__c='mutated';
            Assert.areEqual(0,[SELECT COUNT() FROM Account]);
            Test.stopTest();
            Assert.areEqual(1,[SELECT COUNT() FROM Account WHERE Name='original']);
            Assert.areEqual(0,[SELECT COUNT() FROM Account WHERE Name='mutated']);
            Assert.areEqual(1,[SELECT COUNT() FROM Account WHERE Name='callback']);
            Assert.areEqual('005000000000001AAA',UserInfo.getUserId());
          }}
        "#,
        );
        let mut c = Context::new(&p);
        c.call("check", "run", vec![], &n).unwrap();
        c.assert_events_drained(&n).unwrap();
    }
    #[test]
    fn forced_failure_calls_failure_handler_without_delivery_or_success_callback() {
        let (p, n) = program(
            r#"
          trigger NoticeTrigger on Notice__e(after insert) { Assert.fail('Failed events cannot be delivered'); }
          public class Callback implements EventBus.EventPublishFailureCallback,EventBus.EventPublishSuccessCallback {
            public void onFailure(EventBus.FailureResult result){insert new Account(Name='failure');}
            public void onSuccess(EventBus.SuccessResult result){Assert.fail('Failure cannot be success');}
          }
          @IsTest class Check {public static void run(){
            Test.startTest();EventBus.publish(new Notice__e(Message__c='data'),new Callback());
            Test.getEventBus().fail();Assert.areEqual(1,[SELECT COUNT() FROM Account WHERE Name='failure']);
            Test.stopTest();Assert.areEqual(1,[SELECT COUNT() FROM Account]);
          }}
        "#,
        );
        Context::new(&p).call("check", "run", vec![], &n).unwrap();
    }
    #[test]
    fn invalid_payload_is_a_failed_save_result_and_uuid_provenance_is_enforced() {
        let (p, n) = program("class C{}");
        let mut c = Context::new(&p);
        let invalid = Value::Record(Rc::new(RefCell::new(Record {
            typ: "Notice__e".into(),
            fields: BTreeMap::new(),
            projected: false,
        })));
        let result = c
            .events_namespace("eventbus", "publish", &[invalid], &n)
            .unwrap()
            .unwrap();
        let Value::Instance(result) = result else {
            panic!()
        };
        assert!(matches!(
            result.borrow().fields["success"],
            Value::Bool(false)
        ));
        assert!(c.event_state.pending.is_empty());
        let fake = Value::Record(Rc::new(RefCell::new(Record {
            typ: "Notice__e".into(),
            fields: BTreeMap::from([
                ("message__c".into(), Value::Str("data".into())),
                (
                    "eventuuid".into(),
                    Value::Str("00000000-0000-4000-8000-000000000000".into()),
                ),
            ]),
            projected: false,
        })));
        assert_eq!(
            c.events_namespace("eventbus", "publish", &[fake], &n)
                .unwrap_err()
                .status,
            "blocked"
        );
    }
    #[test]
    fn future_published_events_are_delivered_after_async_jobs() {
        let (p, n) = program(
            r#"
          trigger NoticeTrigger on Notice__e(after insert) {for(Notice__e e:Trigger.new){insert new Account(Name=e.Message__c);}}
          public class Producer { @Future public static void send(){EventBus.publish(new Notice__e(Message__c='from future'));} }
          @IsTest class Check {public static void run(){Test.startTest();Producer.send();Test.stopTest();Assert.areEqual(1,[SELECT COUNT() FROM Account WHERE Name='from future']);}}
        "#,
        );
        let mut c = Context::new(&p);
        c.call("check", "run", vec![], &n).unwrap();
        c.assert_async_drained(&n).unwrap();
        c.assert_events_drained(&n).unwrap();
    }
    #[test]
    fn rollback_removes_after_commit_publications_but_preserves_immediate_ones() {
        let (p, n) = program("class C{}");
        let mut c = Context::new(&p);
        c.events_savepoint(9);
        for typ in ["Notice__e", "Immediate__e"] {
            let event = Value::Record(Rc::new(RefCell::new(Record {
                typ: typ.into(),
                fields: BTreeMap::from([("message__c".into(), Value::Str("data".into()))]),
                projected: false,
            })));
            c.events_namespace("eventbus", "publish", &[event], &n)
                .unwrap();
        }
        assert_eq!(c.event_state.pending.len(), 2);
        c.events_rollback(9, &n).unwrap();
        assert_eq!(c.event_state.pending.len(), 1);
        assert!(!c.event_state.pending[0].after_commit);
    }
}
#[derive(Default, Debug)]
pub(super) struct EventState {
    pending: Vec<EventBatch>,
    snapshots: BTreeMap<u64, (Vec<EventBatch>, u64)>,
    delivered: u64,
    immediate_calls: usize,
}
pub(super) fn event_uuid() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| "Host randomness unavailable for event UUID".to_string())?;
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    let h = hex::encode(bytes);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    ))
}
fn uuid(at: &Node) -> R<String> {
    event_uuid().map_err(|e| EvalError {
        exception: None,
        status: "blocked",
        diagnostic: at.diagnostic("APX_RANDOM", e),
    })
}
pub(super) fn is_type(typ: &str) -> bool {
    matches!(
        typ.to_ascii_lowercase().as_str(),
        "eventbus.testbroker" | "eventbus.successresult" | "eventbus.failureresult"
    )
}
fn native(class: &str, fields: BTreeMap<String, Value>) -> Value {
    Value::Instance(Rc::new(RefCell::new(Instance {
        class: class.into(),
        fields,
        field_types: BTreeMap::new(),
    })))
}

impl<'a> Context<'a> {
    pub(super) fn initialize_event_uuid(
        &mut self,
        record: &Rc<RefCell<Record>>,
        at: &Node,
    ) -> R<()> {
        let id = uuid(at)?;
        record
            .borrow_mut()
            .fields
            .insert("eventuuid".into(), Value::Str(id.clone()));
        self.native_state.insert(
            format!("event.initialized.{id}"),
            Value::Record(record.clone()),
        );
        Ok(())
    }
    pub(super) fn events_namespace(
        &mut self,
        ns: &str,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        if ns.trim_start_matches("system.") == "test" && name == "geteventbus" && args.is_empty() {
            return Ok(Some(native("eventbus.testbroker", BTreeMap::new())));
        }
        if ns.trim_start_matches("system.") == "limits" {
            return Ok(match (name, args) {
                ("getpublishimmediatedml", []) => Some(Value::Num(Number::integer(
                    self.event_state.immediate_calls as i64,
                ))),
                ("getlimitpublishimmediatedml", []) => Some(Value::Num(Number::integer(150))),
                _ => None,
            });
        }
        if ns != "eventbus" {
            return Ok(None);
        }
        if name != "publish" || args.is_empty() || args.len() > 2 {
            return block(at, "EventBus supports publish(eventOrList[,callback])");
        }
        self.ensure_data_store(at)?;
        let (input, single) = match &args[0] {
            Value::Record(r) => (vec![r.clone()], true),
            Value::List(rows) => (
                rows.borrow()
                    .iter()
                    .map(|v| match v {
                        Value::Record(r) => Ok(r.clone()),
                        _ => block(at, "EventBus.publish list must contain event SObjects"),
                    })
                    .collect::<R<Vec<_>>>()?,
                false,
            ),
            _ => return block(at, "EventBus.publish requires an event SObject or list"),
        };
        if input.len() > 10_000
            || self
                .event_state
                .pending
                .iter()
                .map(|b| b.records.len())
                .sum::<usize>()
                .saturating_add(input.len())
                > 10_000
        {
            return block(at, "Pending local event bus exceeds 10000-event bound");
        }
        let callback = if let Some(value) = args.get(1) {
            let Value::Instance(i) = value else {
                return block(at, "Event publish callback requires an Apex class instance");
            };
            if !self.class_assignable(&i.borrow().class, "eventbus.eventpublishsuccesscallback")
                && !self.class_assignable(&i.borrow().class, "eventbus.eventpublishfailurecallback")
            {
                return block(
                    at,
                    "Callback does not implement an EventBus publish callback interface",
                );
            }
            Some(self.cache_snapshot(value, at, 0, &mut BTreeSet::new(), &mut 0)?)
        } else {
            None
        };
        let mut records = vec![];
        let mut results = vec![];
        let mut mode = None;
        for record in input {
            let typ = record.borrow().typ.clone();
            let Some(schema) = self.program.schema.object(&typ) else {
                return block(at, "Unknown event schema");
            };
            if !schema.platform_event {
                return block(at, "EventBus.publish only accepts platform events");
            }
            let after_commit = match schema.publish_behavior.as_str() {
                "PublishImmediately" => false,
                "PublishAfterCommit" => true,
                _ => {
                    return block(
                        at,
                        "Platform event publishBehavior is absent or unsupported",
                    )
                }
            };
            if mode.is_some_and(|old| old != after_commit) {
                return block(
                    at,
                    "Mixed publication behavior in one event list is not implemented",
                );
            }
            mode = Some(after_commit);
            if let Some(Value::Str(id)) = record.borrow().fields.get("eventuuid") {
                if !matches!(self.native_state.get(&format!("event.initialized.{id}")), Some(Value::Record(original)) if Rc::ptr_eq(original,&record))
                {
                    return block(
                        at,
                        "EventUuid provenance must come from sObjectType.newSObject(null,true)",
                    );
                }
            }
            let DataValue::Record(payload) = data::to_data(&Value::Record(record.clone()), at, 0)?
            else {
                unreachable!()
            };
            let validation = self.data_store.as_ref().unwrap().validate_event(&payload);
            let result = match validation {
                Ok(mut payload) => {
                    if payload.get("EventUuid").is_none_or(DataValue::is_null) {
                        payload.set("EventUuid", DataValue::Text(uuid(at)?));
                    }
                    let Value::Record(record) =
                        data::from_data(&DataValue::Record(Box::new(payload)), at, 0)?
                    else {
                        unreachable!()
                    };
                    record.borrow_mut().projected = false;
                    records.push(record);
                    SaveResult {
                        success: true,
                        id: None,
                        created: false,
                        errors: vec![],
                    }
                }
                Err(error) if error.unsupported => return block(at, error.to_string()),
                Err(error) => SaveResult {
                    success: false,
                    id: None,
                    created: false,
                    errors: vec![error],
                },
            };
            let value = self.save_result_value(&result, "Database.SaveResult", at)?;
            if let Value::Instance(i) = &value {
                i.borrow_mut()
                    .fields
                    .insert("\0eventresult".into(), Value::Bool(true));
            }
            results.push(value);
        }
        if mode == Some(true) {
            self.dml += 1;
            if self.dml > 150 {
                return block(at, "Platform publish exceeded DML statement limit");
            }
        } else if mode == Some(false) {
            self.event_state.immediate_calls += 1;
            if self.event_state.immediate_calls > 150 {
                return block(at, "Publish Immediately call limit exceeded");
            }
        }
        if !records.is_empty() {
            self.event_state.pending.push(EventBatch {
                identity: uuid(at)?,
                records,
                callback,
                after_commit: mode == Some(true),
                at: at.clone(),
            });
        }
        Ok(Some(if single {
            results.remove(0)
        } else {
            self.coerce(Value::list(results), "List<Database.SaveResult>", at)?
        }))
    }
    pub(super) fn events_method(
        &mut self,
        receiver: &Value,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        let Value::Instance(i) = receiver else {
            return Ok(None);
        };
        let class = i.borrow().class.clone();
        if class == "database.saveresult"
            && i.borrow().fields.contains_key("\0eventresult")
            && name == "getid"
        {
            return block(
                at,
                "Platform-event SaveResult ID semantics are not qualified",
            );
        }
        if !is_type(&class) {
            return Ok(None);
        }
        if !args.is_empty() {
            return block(at, "Event bus result/broker methods take no arguments");
        }
        if class == "eventbus.testbroker" {
            match name {
                "deliver" => self.events_drain(at, false)?,
                "fail" => self.events_drain(at, true)?,
                _ => return block(at, "Unknown test event bus method"),
            }
            return Ok(Some(Value::Null));
        }
        if name == "geteventuuids" {
            return Ok(Some(i.borrow().fields["\0uuids"].clone()));
        }
        block(at, "Event publish result method is not implemented")
    }
    pub(super) fn events_savepoint(&mut self, id: u64) {
        self.event_state.snapshots.insert(
            id,
            (self.event_state.pending.clone(), self.event_state.delivered),
        );
    }
    pub(super) fn events_rollback(&mut self, id: u64, at: &Node) -> R<()> {
        let Some((saved, delivered)) = self.event_state.snapshots.get(&id).cloned() else {
            return block(at, "Missing event queue savepoint");
        };
        if self.event_state.delivered != delivered {
            return block(at,"Rollback across delivered event transactions requires cross-transaction merge semantics");
        }
        let ids = saved
            .iter()
            .map(|b| b.identity.clone())
            .collect::<BTreeSet<_>>();
        let immediate = self
            .event_state
            .pending
            .iter()
            .filter(|b| !b.after_commit && !ids.contains(&b.identity))
            .cloned()
            .collect::<Vec<_>>();
        self.event_state.pending = saved;
        self.event_state.pending.extend(immediate);
        Ok(())
    }
    pub(super) fn events_release(&mut self, id: u64) {
        self.event_state.snapshots.remove(&id);
    }
    pub(super) fn events_absorb(&mut self, other: EventState) {
        self.event_state.pending.extend(other.pending);
        self.event_state.delivered += other.delivered;
    }
    pub fn assert_events_drained(&self, at: &Node) -> R<()> {
        if self.event_state.pending.is_empty() {
            Ok(())
        } else {
            block(at,"Pending platform events were not delivered; call Test.getEventBus().deliver() for downstream events")
        }
    }

    pub(super) fn events_drain(&mut self, at: &Node, failed: bool) -> R<()> {
        if self.event_state.pending.is_empty() {
            return Ok(());
        }
        let user = self
            .program
            .environment
            .as_ref()
            .and_then(|e| e.capabilities.get("automatedProcessUserId"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let Some(user) = user else {
            return block(at, "Event delivery requires explicit Automated Process user identity in the environment");
        };
        self.ensure_data_store(at)?;
        if self
            .data_store
            .as_ref()
            .unwrap()
            .find("User", &user)
            .is_none()
        {
            return block(
                at,
                "Configured Automated Process user is absent from environment records",
            );
        }
        // Only this wave is delivered. Events emitted downstream require another deliver().
        let wave = std::mem::take(&mut self.event_state.pending);
        for batch in wave {
            if !failed {
                let mut objects = BTreeMap::<String, Vec<Rc<RefCell<Record>>>>::new();
                for record in &batch.records {
                    objects
                        .entry(record.borrow().typ.clone())
                        .or_default()
                        .push(record.clone());
                }
                for (object, records) in objects {
                    for chunk in records.chunks(200) {
                        self.events_frame(&user, |child| {
                            child.fire_data_triggers(&object, "after insert", chunk, &[], &batch.at)
                        })?;
                    }
                }
            }
            if let Some(Value::Instance(callback)) = &batch.callback {
                let interface = if failed {
                    "eventbus.eventpublishfailurecallback"
                } else {
                    "eventbus.eventpublishsuccesscallback"
                };
                if self.class_assignable(&callback.borrow().class, interface) {
                    self.events_frame(&user, |child| {
                        let ids = batch
                            .records
                            .iter()
                            .map(|r| r.borrow().fields["eventuuid"].clone())
                            .collect();
                        let ids = child.coerce(Value::list(ids), "List<String>", &batch.at)?;
                        let result = native(
                            if failed {
                                "eventbus.failureresult"
                            } else {
                                "eventbus.successresult"
                            },
                            BTreeMap::from([("\0uuids".into(), ids)]),
                        );
                        child.call_instance(
                            callback.clone(),
                            if failed { "onfailure" } else { "onsuccess" },
                            vec![result],
                            &batch.at,
                        )?;
                        Ok(())
                    })?;
                }
            }
        }
        Ok(())
    }
    fn events_frame<F>(&mut self, user: &str, execute: F) -> R<()>
    where
        F: FnOnce(&mut Context<'a>) -> R<()>,
    {
        let parent_limits = self.data_store.as_ref().map(|s| {
            (
                s.dml_statements,
                s.dml_rows,
                s.query_rows,
                s.current_user.clone(),
            )
        });
        let mut child = Context::new(self.program);
        child.assertions = self.assertions.clone();
        if self.native_state.contains_key("search.fixedIds")
            || self
                .native_state
                .contains_key("search.unqualifiedAsyncFixed")
        {
            child
                .native_state
                .insert("search.unqualifiedAsyncFixed".into(), Value::Bool(true));
        }
        child.install_data_store(
            self.data_store
                .as_ref()
                .map(|store| store.fork_transaction()),
        );
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
        for key in ["clock.iso", "test.seealldata"] {
            if let Some(v) = self.native_state.get(key) {
                child.native_state.insert(key.into(), v.clone());
            }
        }
        child
            .native_state
            .insert("userinfo.userid".into(), Value::Str(user.into()));
        child.native_state.insert(
            "request.quiddity".into(),
            Value::Str("PLATFORM_EVENT".into()),
        );
        child
            .native_state
            .insert("event.frame".into(), Value::Bool(true));
        child.data_store.as_mut().unwrap().current_user = Some(user.into());
        let result = execute(&mut child);
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
        self.event_state.delivered += 1;
        if result.is_ok() {
            if let (Some(parent), Some(committed)) = (&mut self.data_store, child.data_store) {
                parent.merge_committed(committed);
            }
            self.events_absorb(child.event_state);
            if let (Some(store), Some((statements, rows, queries, user))) =
                (&mut self.data_store, parent_limits)
            {
                store.dml_statements = statements;
                store.dml_rows = rows;
                store.query_rows = queries;
                store.current_user = user;
            }
        }
        result
    }
}
