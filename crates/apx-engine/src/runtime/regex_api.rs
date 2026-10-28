//! Linear-time, explicitly bounded ASCII subset of the Apex Pattern/Matcher API.
use super::*;
use num_traits::ToPrimitive;

pub(super) fn is_type(typ: &str) -> bool {
    matches!(
        typ.to_ascii_lowercase().trim_start_matches("system."),
        "pattern" | "matcher"
    )
}
fn compile(source: &str, at: &Node) -> R<regex::Regex> {
    crate::regex_subset::compile(source, false).map_err(|message| EvalError {
        exception: None,
        status: "blocked",
        diagnostic: at.diagnostic("APX_UNSUPPORTED", message),
    })
}
fn object(class: &str, fields: BTreeMap<String, Value>) -> Value {
    Value::Instance(Rc::new(RefCell::new(Instance {
        class: class.into(),
        fields,
        field_types: BTreeMap::new(),
    })))
}
impl Context<'_> {
    pub(super) fn regex_namespace(
        &mut self,
        ns: &str,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        if ns.trim_start_matches("system.") != "pattern" {
            return Ok(None);
        }
        match (name, args) {
            ("compile", [Value::Str(source)]) => {
                compile(source, at)?;
                Ok(Some(object(
                    "pattern",
                    BTreeMap::from([("\0source".into(), Value::Str(source.clone()))]),
                )))
            }
            ("matches", [Value::Str(source), Value::Str(input)]) => {
                let pattern = self
                    .regex_namespace("pattern", "compile", &[Value::Str(source.clone())], at)?
                    .unwrap();
                let matcher = self
                    .regex_method(&pattern, "matcher", &[Value::Str(input.clone())], at)?
                    .unwrap();
                self.regex_method(&matcher, "matches", &[], at)
            }
            _ => block(at, "Pattern overload is not implemented"),
        }
    }
    pub(super) fn regex_method(
        &mut self,
        receiver: &Value,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        let Value::Instance(instance) = receiver else {
            return Ok(None);
        };
        let class = instance.borrow().class.clone();
        if !is_type(&class) {
            return Ok(None);
        }
        let Value::Str(source) = instance.borrow().fields["\0source"].clone() else {
            return block(at, "Invalid regex state");
        };
        if class == "pattern" {
            return match (name, args) {
                ("pattern", []) => Ok(Some(Value::Str(source))),
                ("matcher", [Value::Str(input)]) => {
                    crate::regex_subset::validate_input(&source, input).map_err(|message| {
                        EvalError {
                            exception: None,
                            status: "blocked",
                            diagnostic: at.diagnostic("APX_UNSUPPORTED", message),
                        }
                    })?;
                    let matcher = object(
                        "matcher",
                        BTreeMap::from([
                            ("\0source".into(), Value::Str(source)),
                            ("\0input".into(), Value::Str(input.clone())),
                            ("\0first".into(), Value::Num(Number::integer(-1))),
                            ("\0last".into(), Value::Num(Number::integer(0))),
                            ("\0groups".into(), Value::Null),
                        ]),
                    );
                    self.charge(&Value::Str(input.clone()), at)?;
                    Ok(Some(matcher))
                }
                _ => block(at, "Pattern method is not implemented"),
            };
        }
        let pattern = compile(&source, at)?;
        let Value::Str(input) = instance.borrow().fields["\0input"].clone() else {
            return block(at, "Invalid matcher input state");
        };
        self.charge(&Value::Str(input.clone()), at)?;
        if ["find", "matches", "lookingat"].contains(&name) {
            if !args.is_empty() {
                return block(at, "Indexed Matcher.find overload is not implemented");
            }
            let start = if name == "find" {
                let state = instance.borrow();
                let integer = |key: &str| match &state.fields[key] {
                    Value::Num(n) => n.q.to_integer().to_i64().unwrap_or(-1),
                    _ => -1,
                };
                let last = integer("\0last");
                let next = last + i64::from(integer("\0first") == last);
                next.max(0) as usize
            } else {
                0
            };
            // Anchoring must participate in matching: filtering an unanchored
            // first alternative would incorrectly reject `a|ab` against `ab`.
            let anchored;
            let selected = if name == "matches" {
                anchored =
                    crate::regex_subset::compile(&source, true).map_err(|message| EvalError {
                        exception: None,
                        status: "blocked",
                        diagnostic: at.diagnostic("APX_UNSUPPORTED", message),
                    })?;
                &anchored
            } else {
                &pattern
            };
            let captures = if start <= input.len() {
                selected.captures_at(&input, start)
            } else {
                None
            };
            let matched = captures.filter(|caps| {
                let m = caps.get(0).unwrap();
                name == "find"
                    || (m.start() == 0 && (name == "lookingat" || m.end() == input.len()))
            });
            let mut state = instance.borrow_mut();
            if let Some(captures) = matched {
                let bytes = captures.iter().flatten().map(|c| c.len()).sum::<usize>();
                if bytes > 1_048_576 {
                    return block(at, "Regex capture expansion exceeds 1 MiB host bound");
                }
                for capture in captures.iter().flatten() {
                    self.charge(&Value::Str(capture.as_str().into()), at)?;
                }
                let full = captures.get(0).unwrap();
                let first = full.start();
                let last = full.end();
                let groups = captures
                    .iter()
                    .map(|c| {
                        c.map(|c| Value::Str(c.as_str().into()))
                            .unwrap_or(Value::Null)
                    })
                    .collect();
                let starts = captures
                    .iter()
                    .map(|c| Value::Num(Number::integer(c.map(|c| c.start() as i64).unwrap_or(-1))))
                    .collect();
                let ends = captures
                    .iter()
                    .map(|c| Value::Num(Number::integer(c.map(|c| c.end() as i64).unwrap_or(-1))))
                    .collect();
                state.fields.insert("\0groups".into(), Value::list(groups));
                state.fields.insert("\0starts".into(), Value::list(starts));
                state.fields.insert("\0ends".into(), Value::list(ends));
                state
                    .fields
                    .insert("\0first".into(), Value::Num(Number::integer(first as i64)));
                state
                    .fields
                    .insert("\0last".into(), Value::Num(Number::integer(last as i64)));
                return Ok(Some(Value::Bool(true)));
            }
            state.fields.insert("\0groups".into(), Value::Null);
            // A failed search keeps the previous end position. A search beyond
            // the region retains the zero-width start too, keeping it exhausted.
            if start <= input.len() {
                state
                    .fields
                    .insert("\0first".into(), Value::Num(Number::integer(-1)));
            }
            return Ok(Some(Value::Bool(false)));
        }
        if name == "groupcount" && args.is_empty() {
            return Ok(Some(Value::Num(Number::integer(
                pattern.captures_len() as i64 - 1,
            ))));
        }
        if name == "reset" && args.is_empty() {
            instance
                .borrow_mut()
                .fields
                .insert("\0groups".into(), Value::Null);
            instance
                .borrow_mut()
                .fields
                .insert("\0first".into(), Value::Num(Number::integer(-1)));
            instance
                .borrow_mut()
                .fields
                .insert("\0last".into(), Value::Num(Number::integer(0)));
            return Ok(Some(receiver.clone()));
        }
        if ["group", "start", "end"].contains(&name) {
            let index = match args {
                [] => 0,
                [Value::Num(n)] if n.q.is_integer() => {
                    n.q.to_integer().to_usize().unwrap_or(usize::MAX)
                }
                _ => return block(at, "Matcher capture index must be Integer"),
            };
            let state = instance.borrow();
            if !matches!(state.fields["\0groups"], Value::List(_)) {
                return exceptions::raise(at, "System.StringException", "No match found");
            }
            let key = match name {
                "start" => "\0starts",
                "end" => "\0ends",
                _ => "\0groups",
            };
            let Value::List(groups) = &state.fields[key] else {
                return block(at, "Missing matcher captures");
            };
            let result = groups.borrow().get(index).cloned();
            return match result {
                Some(v) => Ok(Some(v)),
                None => exceptions::raise(
                    at,
                    "System.StringException",
                    "No group with the requested index",
                ),
            };
        }
        block(at, format!("Matcher.{name} is not implemented"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captures_follow_input_and_find_progresses_after_empty_matches() {
        let n = crate::syntax::parse("class Host{}", "pattern.cls").unwrap();
        let p = Program::default();
        let mut c = Context::new(&p);
        let pattern = c
            .regex_namespace(
                "pattern",
                "compile",
                &[Value::Str("(pear|apple)".into())],
                &n,
            )
            .unwrap()
            .unwrap();
        let m = c
            .regex_method(&pattern, "matcher", &[Value::Str("pear apple".into())], &n)
            .unwrap()
            .unwrap();
        for word in ["pear", "apple"] {
            assert!(matches!(
                c.regex_method(&m, "find", &[], &n).unwrap(),
                Some(Value::Bool(true))
            ));
            assert_eq!(
                c.regex_method(&m, "group", &[Value::Num(Number::integer(1))], &n)
                    .unwrap()
                    .unwrap()
                    .display(),
                word
            );
        }
        assert!(matches!(
            c.regex_method(&m, "find", &[], &n).unwrap(),
            Some(Value::Bool(false))
        ));
        assert!(c
            .regex_method(&m, "group", &[], &n)
            .unwrap_err()
            .exception
            .is_some());
        let p = c
            .regex_namespace("pattern", "compile", &[Value::Str("a*".into())], &n)
            .unwrap()
            .unwrap();
        let m = c
            .regex_method(&p, "matcher", &[Value::Str("b".into())], &n)
            .unwrap()
            .unwrap();
        assert!(matches!(
            c.regex_method(&m, "find", &[], &n).unwrap(),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            c.regex_method(&m, "find", &[], &n).unwrap(),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            c.regex_method(&m, "find", &[], &n).unwrap(),
            Some(Value::Bool(false))
        ));
        assert!(compile("(?=lookahead)", &n)
            .unwrap_err()
            .exception
            .is_none());
    }
}
