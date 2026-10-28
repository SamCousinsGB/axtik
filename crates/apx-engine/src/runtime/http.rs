//! In-memory HTTP request/response values and test-mock dispatch. Never opens a socket.
use super::*;
use num_traits::ToPrimitive;

fn canonical(typ: &str) -> String {
    typ.to_ascii_lowercase()
        .trim_start_matches("system.")
        .to_string()
}
pub(super) fn http_is_type(typ: &str) -> bool {
    [
        "http",
        "httprequest",
        "httpresponse",
        "restrequest",
        "restresponse",
    ]
    .contains(&canonical(typ).as_str())
}
const HEADERS: &str = "\0headers";
const BODY: &str = "\0body";
const MOCK: &str = "http.calloutMock";
impl Context<'_> {
    pub(super) fn http_construct(&self, typ: &str, args: &[Value], at: &Node) -> R<Option<Value>> {
        let typ = canonical(typ);
        if !http_is_type(&typ) {
            return Ok(None);
        }
        if !args.is_empty() {
            return block(at, "HTTP native constructors take no arguments");
        }
        let mut instance = Instance {
            class: typ.clone(),
            fields: BTreeMap::new(),
            field_types: BTreeMap::new(),
        };
        instance.fields.insert(
            HEADERS.into(),
            self.coerce(
                Value::Map(Rc::new(RefCell::new(vec![]))),
                "map<string,string>",
                at,
            )?,
        );
        instance.fields.insert(BODY.into(), Value::Null);
        if typ == "restrequest" {
            for (name, kind) in [
                ("requesturi", "String"),
                ("httpmethod", "String"),
                ("requestbody", "Blob"),
                ("remoteaddress", "String"),
            ] {
                instance.fields.insert(name.into(), Value::Null);
                instance.field_types.insert(name.into(), kind.into());
            }
            for name in ["headers", "params"] {
                instance.fields.insert(
                    name.into(),
                    self.coerce(
                        Value::Map(Rc::new(RefCell::new(vec![]))),
                        "map<string,string>",
                        at,
                    )?,
                );
                instance
                    .field_types
                    .insert(name.into(), "map<string,string>".into());
            }
        }
        if typ == "restresponse" {
            instance
                .fields
                .insert("statuscode".into(), Value::Num(Number::integer(200)));
            instance
                .field_types
                .insert("statuscode".into(), "Integer".into());
            instance.fields.insert("responsebody".into(), Value::Null);
            instance
                .field_types
                .insert("responsebody".into(), "Blob".into());
            instance.fields.insert(
                "headers".into(),
                self.coerce(
                    Value::Map(Rc::new(RefCell::new(vec![]))),
                    "map<string,string>",
                    at,
                )?,
            );
            instance
                .field_types
                .insert("headers".into(), "map<string,string>".into());
        }
        Ok(Some(Value::Instance(Rc::new(RefCell::new(instance)))))
    }
    pub(super) fn http_namespace(
        &mut self,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        if name != "setmock" {
            return Ok(None);
        }
        let [Value::Type(interface), Value::Instance(mock)] = args else {
            return block(
                at,
                "Test.setMock requires an interface type and a mock instance",
            );
        };
        if canonical(interface) != "httpcalloutmock" {
            return block(at, format!("Mock interface {interface} is not implemented"));
        }
        if !self.class_assignable(&mock.borrow().class, "HttpCalloutMock") {
            return block(at, "Mock must implement HttpCalloutMock");
        }
        self.native_state
            .insert(MOCK.into(), Value::Instance(mock.clone()));
        Ok(Some(Value::Null))
    }
    pub(super) fn http_context_get(&self, name: &str, at: &Node) -> R<Value> {
        if !["request", "response"].contains(&name) {
            return block(at, "Unknown RestContext field");
        }
        Ok(self
            .native_state
            .get(&format!("restcontext.{name}"))
            .cloned()
            .unwrap_or(Value::Null))
    }
    pub(super) fn http_context_set(&mut self, name: &str, value: Value, at: &Node) -> R<()> {
        let typ = match name {
            "request" => "restrequest",
            "response" => "restresponse",
            _ => return block(at, "Unknown RestContext field"),
        };
        if !matches!(&value, Value::Null)
            && !matches!(&value,Value::Instance(i) if canonical(&i.borrow().class)==typ)
        {
            return block(at, "RestContext assignment has incompatible type");
        }
        self.native_state
            .insert(format!("restcontext.{name}"), value);
        Ok(())
    }
    pub(super) fn http_method(
        &mut self,
        receiver: &Value,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        let Value::Instance(instance) = receiver else {
            return Ok(None);
        };
        let typ = canonical(&instance.borrow().class);
        if !http_is_type(&typ) {
            return Ok(None);
        }
        if typ == "http" {
            if name != "send" || args.len() != 1 {
                return block(at, "Unsupported Http method signature");
            }
            if matches!(
                self.native_state.get("async.callouts"),
                Some(Value::Bool(false))
            ) {
                return exceptions::raise(
                    at,
                    "System.CalloutException",
                    "Callout not allowed from this asynchronous context",
                );
            }
            if !matches!(&args[0],Value::Instance(i) if canonical(&i.borrow().class)=="httprequest")
            {
                return block(at, "Http.send requires HttpRequest");
            }
            if self.callouts >= 100 {
                return block(at, "Callout governor limit exceeded (uncatchable)");
            }
            self.callouts += 1;
            let Some(Value::Instance(mock)) = self.native_state.get(MOCK).cloned() else {
                return exceptions::raise(at,"System.CalloutException","Methods defined as TestMethod do not support Web service callouts without an HttpCalloutMock");
            };
            let result = self.call_instance(mock, "respond", vec![args[0].clone()], at)?;
            if !matches!(&result,Value::Instance(i) if canonical(&i.borrow().class)=="httpresponse")
            {
                return exceptions::raise(
                    at,
                    "System.CalloutException",
                    "HttpCalloutMock returned an invalid response",
                );
            }
            return Ok(Some(result));
        }
        let allowed: &[&str] = match typ.as_str() {
            "httprequest" => &[
                "setheader",
                "getheader",
                "getheaderkeys",
                "setbody",
                "setbodyasblob",
                "getbody",
                "getbodyasblob",
                "setendpoint",
                "getendpoint",
                "setmethod",
                "getmethod",
                "settimeout",
                "gettimeout",
                "setcompressed",
                "getcompressed",
                "setclientcertificatename",
            ],
            "httpresponse" => &[
                "setheader",
                "getheader",
                "getheaderkeys",
                "setbody",
                "setbodyasblob",
                "getbody",
                "getbodyasblob",
                "setstatuscode",
                "getstatuscode",
                "setstatus",
                "getstatus",
            ],
            "restrequest" => &["addheader", "addparameter"],
            "restresponse" => &["addheader"],
            _ => &[],
        };
        if !allowed.contains(&name) {
            return block(at, format!("{typ}.{name} is not implemented"));
        }
        if name == "setheader" || name == "addheader" {
            let [Value::Str(key), Value::Str(value)] = args else {
                return block(at, "HTTP header setter requires two Strings");
            };
            if key.contains(['\r', '\n']) || value.contains(['\r', '\n']) {
                return exceptions::raise(
                    at,
                    "System.CalloutException",
                    "Invalid newline in HTTP header",
                );
            }
            let keyfield = if typ.starts_with("rest") {
                "headers"
            } else {
                HEADERS
            };
            let Value::Map(headers) = instance.borrow().fields[keyfield].clone() else {
                return block(at, "Invalid native header state");
            };
            let mut headers = headers.borrow_mut();
            if let Some((_, old)) = headers
                .iter_mut()
                .find(|(k, _)| matches!(k,Value::Str(s) if s.eq_ignore_ascii_case(key)))
            {
                *old = Value::Str(value.clone());
            } else {
                headers.push((Value::Str(key.clone()), Value::Str(value.clone())));
            }
            return Ok(Some(Value::Null));
        }
        if name == "getheader" {
            let [Value::Str(key)] = args else {
                return block(at, "HTTP header getter requires one String");
            };
            let keyfield = if typ.starts_with("rest") {
                "headers"
            } else {
                HEADERS
            };
            let Value::Map(headers) = instance.borrow().fields[keyfield].clone() else {
                return block(at, "Invalid native header state");
            };
            let value = headers
                .borrow()
                .iter()
                .find(|(k, _)| matches!(k,Value::Str(s) if s.eq_ignore_ascii_case(key)))
                .map(|(_, v)| v.clone())
                .unwrap_or(Value::Null);
            return Ok(Some(value));
        }
        if name == "getheaderkeys" && args.is_empty() {
            let Value::Map(headers) = instance.borrow().fields[HEADERS].clone() else {
                return block(at, "Invalid native header state");
            };
            return Ok(Some(self.coerce(
                Value::list(headers.borrow().iter().map(|(k, _)| k.clone()).collect()),
                "list<string>",
                at,
            )?));
        }
        if name == "addparameter" && typ == "restrequest" {
            let [Value::Str(key), Value::Str(value)] = args else {
                return block(at, "RestRequest.addParameter requires two Strings");
            };
            let Value::Map(params) = instance.borrow().fields["params"].clone() else {
                return block(at, "Invalid native params state");
            };
            let mut params = params.borrow_mut();
            if let Some((_, v)) = params
                .iter_mut()
                .find(|(k, _)| matches!(k,Value::Str(k) if k==key))
            {
                *v = Value::Str(value.clone());
            } else {
                params.push((Value::Str(key.clone()), Value::Str(value.clone())));
            }
            return Ok(Some(Value::Null));
        }
        if !["httprequest", "httpresponse"].contains(&typ.as_str()) {
            return block(at, format!("{typ}.{name} is not implemented"));
        }
        let mut instance = instance.borrow_mut();
        let result = match (name, args) {
            ("setbody", [Value::Str(s)]) => {
                if s.len() > 1_048_576 {
                    return block(at, "HTTP body exceeds bounded 1 MiB host allocation");
                }
                instance
                    .fields
                    .insert(BODY.into(), Value::Blob(s.as_bytes().to_vec()));
                Value::Null
            }
            ("setbodyasblob", [Value::Blob(b)]) => {
                if b.len() > 1_048_576 {
                    return block(at, "HTTP body exceeds bounded 1 MiB host allocation");
                }
                instance.fields.insert(BODY.into(), Value::Blob(b.clone()));
                Value::Null
            }
            ("getbodyasblob", []) => instance.fields[BODY].clone(),
            ("getbody", []) => match &instance.fields[BODY] {
                Value::Null => Value::Null,
                Value::Blob(b) => {
                    Value::Str(String::from_utf8(b.clone()).map_err(|_| EvalError {
                        exception: Some(Value::Exception(Rc::new(RefCell::new(ExceptionValue {
                            typ: "System.StringException".into(),
                            message: "BLOB is not a valid UTF-8 string".into(),
                            stack: format!("APX local {}:{}", at.path, at.line()),
                        })))),
                        status: "failed",
                        diagnostic: at.diagnostic("APX_HTTP", "BLOB is not a valid UTF-8 string"),
                    })?)
                }
                _ => return block(at, "Invalid native body state"),
            },
            (
                "setendpoint" | "setmethod" | "setstatus" | "setclientcertificatename",
                [Value::Str(s)],
            ) => {
                let key = format!("\0{}", &name[3..]);
                instance.fields.insert(key, Value::Str(s.clone()));
                Value::Null
            }
            ("getendpoint" | "getmethod" | "getstatus", []) => instance
                .fields
                .get(&format!("\0{}", &name[3..]))
                .cloned()
                .unwrap_or(Value::Null),
            ("setstatuscode" | "settimeout", [Value::Num(n)]) => {
                let Some(v) = n.q.to_integer().to_i64().filter(|_| n.q.is_integer()) else {
                    return exceptions::raise(
                        at,
                        "System.TypeException",
                        "HTTP integer parameter expected",
                    );
                };
                if name == "settimeout" && !(1..=120_000).contains(&v) {
                    return exceptions::raise(
                        at,
                        "System.CalloutException",
                        "Timeout must be between 1 and 120000 milliseconds",
                    );
                }
                instance
                    .fields
                    .insert(format!("\0{}", &name[3..]), Value::Num(Number::integer(v)));
                Value::Null
            }
            ("getstatuscode", []) => instance
                .fields
                .get("\0statuscode")
                .cloned()
                .unwrap_or(Value::Num(Number::integer(0))),
            ("gettimeout", []) => instance
                .fields
                .get("\0timeout")
                .cloned()
                .unwrap_or(Value::Num(Number::integer(10_000))),
            ("setcompressed", [Value::Bool(v)]) => {
                instance
                    .fields
                    .insert("\0compressed".into(), Value::Bool(*v));
                Value::Null
            }
            ("getcompressed", []) => instance
                .fields
                .get("\0compressed")
                .cloned()
                .unwrap_or(Value::Bool(false)),
            _ => return block(at, format!("{typ}.{name} signature is not implemented")),
        };
        Ok(Some(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (Program, Node) {
        let n = crate::syntax::parse("class Host {}", "host.cls").unwrap();
        (Program::default(), n)
    }
    #[test]
    fn headers_are_case_insensitive_and_body_bytes_roundtrip() {
        let (p, n) = setup();
        let mut c = Context::new(&p);
        let r = c.http_construct("HttpResponse", &[], &n).unwrap().unwrap();
        c.http_method(
            &r,
            "setheader",
            &[
                Value::Str("Content-Type".into()),
                Value::Str("application/json".into()),
            ],
            &n,
        )
        .unwrap();
        assert_eq!(
            c.http_method(&r, "getheader", &[Value::Str("content-type".into())], &n)
                .unwrap()
                .unwrap()
                .display(),
            "application/json"
        );
        c.http_method(&r, "setbodyasblob", &[Value::Blob(vec![0, 255])], &n)
            .unwrap();
        assert!(
            matches!(c.http_method(&r,"getbodyasblob",&[],&n).unwrap(),Some(Value::Blob(v))if v==vec![0,255])
        );
        assert!(c.http_method(&r, "getbody", &[], &n).is_err());
    }
    #[test]
    fn rest_context_is_isolated_and_typed() {
        let (p, n) = setup();
        let mut a = Context::new(&p);
        let b = Context::new(&p);
        let r = a.http_construct("RestRequest", &[], &n).unwrap().unwrap();
        a.http_context_set("request", r, &n).unwrap();
        assert!(matches!(
            b.http_context_get("request", &n).unwrap(),
            Value::Null
        ));
        assert!(a
            .http_context_set("response", Value::Str("bad".into()), &n)
            .is_err());
    }
    #[test]
    fn no_mock_never_triggers_real_network() {
        let (p, n) = setup();
        let mut c = Context::new(&p);
        let client = c.http_construct("Http", &[], &n).unwrap().unwrap();
        let request = c.http_construct("HttpRequest", &[], &n).unwrap().unwrap();
        let err = c.http_method(&client, "send", &[request], &n).unwrap_err();
        assert!(err.exception.is_some());
        assert_eq!(c.callouts, 1);
    }
    #[test]
    fn native_methods_cannot_leak_between_request_and_response() {
        let (p, n) = setup();
        let mut c = Context::new(&p);
        let request = c.http_construct("HttpRequest", &[], &n).unwrap().unwrap();
        let response = c.http_construct("HttpResponse", &[], &n).unwrap().unwrap();
        assert_eq!(
            c.http_method(
                &request,
                "setstatuscode",
                &[Value::Num(Number::integer(201))],
                &n
            )
            .unwrap_err()
            .status,
            "blocked"
        );
        assert_eq!(
            c.http_method(
                &response,
                "setendpoint",
                &[Value::Str("https://example.invalid".into())],
                &n
            )
            .unwrap_err()
            .status,
            "blocked"
        );
    }
}
