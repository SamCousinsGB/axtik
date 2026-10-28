//! Apex Stub API: a proxy routes declared methods to the supplied Apex provider.
use super::*;

impl Context<'_> {
    pub(super) fn stub_namespace(
        &mut self,
        ns: &str,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        if ns.trim_start_matches("system.") != "test" || name != "createstub" {
            return Ok(None);
        }
        let [Value::Type(typ), Value::Instance(provider)] = args else {
            return exceptions::raise(
                at,
                "System.TypeException",
                "Test.createStub requires an Apex Type and StubProvider",
            );
        };
        let provider_class = provider.borrow().class.clone();
        if !self.class_assignable(&provider_class, "system.stubprovider")
            && !self.class_assignable(&provider_class, "stubprovider")
        {
            return exceptions::raise(
                at,
                "System.TypeException",
                "Provider must implement System.StubProvider",
            );
        }
        let Some(class) = self.resolve_class(typ) else {
            return exceptions::raise(
                at,
                "System.TypeException",
                "Test.createStub requires a user-defined Apex type",
            );
        };
        let declaration = &self.program.classes[&class];
        if declaration.parent.is_some() {
            return exceptions::raise(
                at,
                "System.TypeException",
                "Test.createStub() cannot be called with inner Apex classes",
            );
        }
        if !["class_declaration", "interface_declaration"].contains(&declaration.kind.as_str()) {
            return exceptions::raise(
                at,
                "System.TypeException",
                "Test.createStub requires a class or interface",
            );
        }
        if declaration
            .interfaces
            .iter()
            .any(|i| i.to_ascii_lowercase().starts_with("database.batchable"))
        {
            return exceptions::raise(
                at,
                "System.TypeException",
                "Batchable classes cannot be stubbed",
            );
        }
        if !declaration.constructors.is_empty()
            && declaration.constructors.iter().all(|c| {
                !c.declaration.child("modifiers").is_some_and(|m| {
                    m.text().split_whitespace().any(|s| {
                        s.eq_ignore_ascii_case("public") || s.eq_ignore_ascii_case("global")
                    })
                })
            })
        {
            return exceptions::raise(
                at,
                "System.TypeException",
                "Classes with only private constructors cannot be stubbed",
            );
        }
        // Instance constructors and field initializers do not execute on a stub.
        self.initialize(&class, at)?;
        let value = Value::Instance(Rc::new(RefCell::new(Instance {
            class,
            fields: BTreeMap::from([("\0stubprovider".into(), Value::Instance(provider.clone()))]),
            field_types: BTreeMap::new(),
        })));
        self.charge(&value, at)?;
        Ok(Some(value))
    }

    pub(super) fn stub_call(
        &mut self,
        proxy: Rc<RefCell<Instance>>,
        name: &str,
        args: &[Value],
        at: &Node,
    ) -> R<Option<Value>> {
        let provider = proxy.borrow().fields.get("\0stubprovider").cloned();
        let Some(Value::Instance(provider)) = provider else {
            return Ok(None);
        };
        let class = proxy.borrow().class.clone();
        let (owner, method) = self.find_method(&class, name, args, false, at)?;
        let modifiers = method
            .declaration
            .child("modifiers")
            .map(|n| n.text().to_ascii_lowercase())
            .unwrap_or_default();
        let interface = self.program.classes[&owner].kind == "interface_declaration";
        if !interface
            && !modifiers
                .split_whitespace()
                .any(|s| s == "public" || s == "global")
        {
            return block(at, "Stub API cannot intercept private or protected methods");
        }
        if method
            .params
            .iter()
            .map(|(_, t)| t)
            .chain(std::iter::once(&method.return_type))
            .any(|t| t.to_ascii_lowercase().contains("iterator<"))
        {
            return block(
                at,
                "Stub API does not support Iterator parameter or return types",
            );
        }
        self.check_method_access(&owner, &method, at)?;
        let param_types = method
            .params
            .iter()
            .map(|(_, t)| {
                Value::Type(
                    self.resolve_class_from(t, &owner)
                        .unwrap_or_else(|| t.clone()),
                )
            })
            .collect();
        let param_nodes = method
            .declaration
            .field("parameters")
            .map(|p| p.named().collect::<Vec<_>>())
            .unwrap_or_default();
        let param_names = method
            .params
            .iter()
            .enumerate()
            .map(|(i, (name, _))| {
                Value::Str(
                    param_nodes
                        .get(i)
                        .and_then(|p| p.field("name"))
                        .map(|n| n.text().to_string())
                        .unwrap_or_else(|| name.clone()),
                )
            })
            .collect();
        let coerced = method
            .params
            .iter()
            .zip(args)
            .map(|((_, t), v)| {
                self.coerce(
                    v.clone(),
                    &self
                        .resolve_class_from(t, &owner)
                        .unwrap_or_else(|| t.clone()),
                    at,
                )
            })
            .collect::<R<Vec<_>>>()?;
        let return_type = self
            .resolve_class_from(&method.return_type, &owner)
            .unwrap_or_else(|| method.return_type.clone());
        let method_name = method
            .declaration
            .field("name")
            .map(|n| n.text().to_string())
            .unwrap_or_else(|| method.name.clone());
        let result = self.call_instance(
            provider,
            "handlemethodcall",
            vec![
                Value::Instance(proxy),
                Value::Str(method_name),
                Value::Type(return_type.clone()),
                self.coerce(Value::list(param_types), "List<System.Type>", at)?,
                self.coerce(Value::list(param_names), "List<String>", at)?,
                self.coerce(Value::list(coerced), "List<Object>", at)?,
            ],
            at,
        )?;
        if return_type.eq_ignore_ascii_case("void") {
            return Ok(Some(Value::Null));
        }
        self.coerce(result, &return_type, at).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(source: &str) -> R<Value> {
        let n = crate::syntax::parse(source, "stub.cls").unwrap();
        let mut p = Program::default();
        let mut diagnostics = vec![];
        crate::syntax::declarations(&n, &mut p, &mut diagnostics);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        Context::new(&p).call("check", "run", vec![], &n)
    }
    #[test]
    fn provider_receives_real_metadata_arguments_and_controls_return() {
        run(r#"
          public class Service {
            public Service() { Assert.fail('Constructor must be bypassed'); }
            public Integer calculate(Integer inputValue) { return 100; }
          }
          public class Provider implements System.StubProvider {
            public Object handleMethodCall(Object target, String name, Type resultType, List<Type> types, List<String> names, List<Object> args) {
              Assert.areEqual('calculate', name); Assert.areEqual(Integer.class, resultType);
              Assert.areEqual(Integer.class, types[0]); Assert.areEqual('inputValue', names[0]);
              Assert.areEqual(7, args[0]); return 49;
            }
          }
          @IsTest class Check { public static void run() {
            Service s = (Service) Test.createStub(Service.class, new Provider());
            Assert.areEqual(49, s.calculate(7));
          } }
        "#).unwrap();
    }
    #[test]
    fn provider_return_must_match_declared_type() {
        let error = run(r#"
          public class Service { public Integer calculate() { return 100; } }
          public class Provider implements System.StubProvider {
            public Object handleMethodCall(Object target, String name, Type t, List<Type> ts, List<String> ns, List<Object> args) { return 'wrong'; }
          }
          @IsTest class Check { public static void run() {
            Service s = (Service) Test.createStub(Service.class, new Provider()); s.calculate();
          } }
        "#).unwrap_err();
        assert!(
            error.diagnostic.message.contains("Expected Integer"),
            "{error:?}"
        );
    }
}
