use apx_engine::{
    runtime::Context,
    syntax::{declarations, parse, Program},
};

fn program(source: &str) -> Program {
    let tree = parse(source, "fixture.cls").expect("fixture syntax");
    let mut p = Program::default();
    let mut ds = vec![];
    declarations(&tree, &mut p, &mut ds);
    assert!(ds.is_empty(), "{ds:?}");
    p
}
fn evaluate(body: &str) -> Result<(), String> {
    let p = program(&format!(
        "@IsTest class Probe {{ @IsTest static void sample() {{ {body} }} }}"
    ));
    let m = &p.classes["probe"].methods[0];
    Context::new(&p)
        .call("Probe", "sample", vec![], &m.declaration)
        .map(|_| ())
        .map_err(|e| format!("{}:{}", e.status, e.diagnostic.message))
}
fn passes(s: &str) {
    assert!(evaluate(s).is_ok(), "{:?}", evaluate(s));
}
fn fails(s: &str, category: &str, fragment: &str) {
    let e = evaluate(s).unwrap_err();
    assert!(e.starts_with(category) && e.contains(fragment), "{e}");
}

#[test]
fn decimal_addition_is_exact() {
    passes("Decimal x=0.1; Decimal y=0.2; System.assertEquals(0.3,x+y);");
}
#[test]
fn large_decimal_is_not_float() {
    passes("Decimal x=9007199254740993.0; System.assertEquals(9007199254740994.0,x+1);");
}
#[test]
fn integral_division_truncates() {
    passes("System.assertEquals(2,5/2);");
}
#[test]
fn finite_decimal_division() {
    passes("Decimal a=1; Decimal b=4; System.assertEquals(0.25,a/b);");
}
#[test]
fn unqualified_rounding_blocks() {
    fails(
        "Decimal a=1; Decimal b=3; Decimal c=a/b;",
        "blocked",
        "precision policy",
    );
}
#[test]
fn division_zero_fails() {
    fails("Integer x=1/0;", "failed", "zero");
}
#[test]
fn wrong_assertion_fails() {
    fails(
        "System.assertEquals(4,2+3);",
        "failed",
        "expected 4, actual 5",
    );
}
#[test]
fn boolean_short_circuit() {
    passes("Boolean x=false && 1/0==0; System.assertEquals(false,x);");
}
#[test]
fn if_else() {
    passes("Integer x=0; if(true){x=2;}else{x=3;} System.assertEquals(2,x);");
}
#[test]
fn for_loop() {
    passes("Integer x=0; for(Integer i=0;i<5;i++){x+=i;} System.assertEquals(10,x);");
}
#[test]
fn while_loop() {
    passes("Integer i=0; while(i<3){i++;} System.assertEquals(3,i);");
}
#[test]
fn break_and_continue() {
    passes("Integer x=0; for(Integer i=0;i<9;i++){if(i==2){continue;} if(i==4){break;} x+=i;} System.assertEquals(4,x);");
}
#[test]
fn assignment_preserves_type() {
    fails("Integer x=1; x='bad';", "failed", "Integer");
}
#[test]
fn unknown_type_blocks() {
    fails("MissingType x=null;", "blocked", "Type MissingType");
}
#[test]
fn unknown_native_blocks() {
    fails(
        "System.unimplemented('x');",
        "blocked",
        "system.unimplemented",
    );
}
#[test]
fn debug_arguments_execute_and_logs_are_captured() {
    let p=program("class Probe{static void sample(){Integer n=1;System.debug(LoggingLevel.INFO,n++);System.assertEquals(2,n);}}");
    let mut context = Context::new(&p);
    context
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap();
    assert_eq!(context.debug_logs, vec!["1"]);
}
#[test]
fn cpu_observation_blocks() {
    fails("Integer n=Limits.getCpuTime();", "blocked", "CPU/heap");
}
#[test]
fn collection_mutation() {
    passes("List<Integer> xs=new List<Integer>(); xs.add(2); xs.add(3); System.assertEquals(2,xs.size()); System.assertEquals(3,xs.get(1));");
}
#[test]
fn collection_type_rejected() {
    fails(
        "List<Integer> xs=new List<Integer>(); xs.add('bad');",
        "failed",
        "integer",
    );
}
#[test]
fn collection_index_type_rejected() {
    fails(
        "List<Integer> xs=new List<Integer>{1}; xs[0]='bad';",
        "failed",
        "integer",
    );
}
#[test]
fn collection_set_type_rejected() {
    fails(
        "List<Integer> xs=new List<Integer>{1}; xs.set(0,'bad');",
        "failed",
        "integer",
    );
}
#[test]
fn recursive_list_mutation_blocks() {
    fails(
        "List<Object> xs=new List<Object>(); xs.add(xs);",
        "blocked",
        "Nested/reference",
    );
}
#[test]
fn foreach() {
    passes("List<Integer> xs=new List<Integer>{1,2,3}; Integer total=0; for(Integer x:xs){total+=x;} System.assertEquals(6,total);");
}
#[test]
fn strings() {
    passes("System.assertEquals(3,'abc'.length()); System.assert('abc'.contains('b')); System.assertEquals('ABC','abc'.toUpperCase());");
}
#[test]
fn comparison_uses_apex_ascii_case() {
    passes("System.assert('abc'=='ABC'); System.assertEquals(false,'abc'.equals('ABC'));");
}
#[test]
fn object_insert_query() {
    passes("Account a=new Account(Name='Acme'); insert a; System.assert(a.Id!=null); Account b=[SELECT Id,Name FROM Account]; System.assertEquals('Acme',b.Name); System.assertEquals(1,[SELECT COUNT() FROM Account]);");
}
#[test]
fn unqueried_field_fails() {
    fails("Account a=new Account(Name='A'); insert a; Account b=[SELECT Id FROM Account]; String name=b.Name;","failed","without querying");
}
#[test]
fn required_field_fails() {
    fails(
        "Account a=new Account(); insert a;",
        "failed",
        "Required field",
    );
}
#[test]
fn dynamic_object_field_blocks() {
    fails(
        "Account a=new Account(Name='A'); String x=a.Unknown__c;",
        "blocked",
        "Unknown SObject field",
    );
}
#[test]
fn update_missing_record_is_a_dml_error() {
    fails(
        "Account a=new Account(Name='A'); update a;",
        "failed",
        "MISSING_ARGUMENT",
    );
}
#[test]
fn query_where_bind() {
    passes("Account a=new Account(Name='A'); Account b=new Account(Name='B'); insert a; insert b; String target='B'; Account c=[SELECT Name FROM Account WHERE Name = :target]; System.assertEquals('B',c.Name);");
}
#[test]
fn query_zero_limit() {
    passes("Account a=new Account(Name='A'); insert a; List<Account> xs=[SELECT Id FROM Account LIMIT 0]; System.assertEquals(0,xs.size());");
}
#[test]
fn compound_where_evaluates_each_predicate() {
    passes("List<Account> records=new List<Account>{new Account(Name='A'),new Account(Name='B'),new Account(Name='C')};insert records;System.assertEquals(2,[SELECT COUNT() FROM Account WHERE Name='A' OR Name='B']);System.assertEquals(1,[SELECT COUNT() FROM Account WHERE Name!='A' AND Name!='B']);");
}
#[test]
fn indexed_assignment_evaluates_location_once_before_rhs() {
    passes("List<Integer> xs=new List<Integer>{1,2}; Integer i=0; xs[i++]+=1;System.assertEquals(1,i);System.assertEquals(2,xs[0]);i=0;xs[i++]=i;System.assertEquals(1,xs[0]);");
}
#[test]
fn query_order_and_limit_are_applied() {
    passes("List<Account> records=new List<Account>{new Account(Name='B'),new Account(Name='A')};insert records;List<Account> rows=[SELECT Name FROM Account ORDER BY Name LIMIT 1];System.assertEquals(1,rows.size());System.assertEquals('A',rows[0].Name);");
}
#[test]
fn test_budget_frame() {
    passes("Account a=new Account(Name='A'); insert a; Test.startTest(); System.assertEquals(0,Limits.getDmlStatements()); Test.stopTest(); System.assertEquals(1,Limits.getDmlStatements());");
}
#[test]
fn trigger_mutation() {
    let mut p=program("@IsTest class Probe { @IsTest static void sample(){Account a=new Account(Name='A');insert a;Account b=[SELECT Description FROM Account];System.assertEquals('changed',b.Description);} }");
    let trigger=parse("trigger Stamp on Account (before insert) { for(Account a:Trigger.new){a.Description='changed';} }","Stamp.trigger").unwrap();
    let mut ds = vec![];
    declarations(&trigger, &mut p, &mut ds);
    let m = &p.classes["probe"].methods[0];
    let r = Context::new(&p).call("Probe", "sample", vec![], &m.declaration);
    assert!(r.is_ok(), "{r:?}");
}
#[test]
fn contexts_isolate_records() {
    let p=program("class Probe{static void add(){Account a=new Account(Name='A');insert a;}static void empty(){System.assertEquals(0,[SELECT COUNT() FROM Account]);}}");
    let c = &p.classes["probe"];
    Context::new(&p)
        .call("Probe", "add", vec![], &c.declaration)
        .unwrap();
    Context::new(&p)
        .call("Probe", "empty", vec![], &c.declaration)
        .unwrap();
}
#[test]
fn statics_are_fresh() {
    let p=program("class Probe{static Integer count=0;static void sample(){count++;System.assertEquals(1,count);}}");
    for _ in 0..2 {
        Context::new(&p)
            .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
            .unwrap();
    }
}
#[test]
fn inheritance_and_virtual_dispatch_execute() {
    let p = program("virtual class Parent{public virtual Integer count(){return 1;}} class Child extends Parent{public override Integer count(){return 2;}} class Probe{static void sample(){Parent p=new Child();System.assertEquals(2,p.count());}}");
    Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap();
}
#[test]
fn malformed_source_rejected() {
    assert!(parse("class Broken { void x( {", "bad.cls").is_err());
}
#[test]
fn duplicate_declarations_rejected() {
    let tree = parse("class X{} class x{}", "a.cls").unwrap();
    let mut ds = vec![];
    declarations(&tree, &mut Program::default(), &mut ds);
    assert_eq!(ds[0].code, "APX_DUPLICATE_TYPE");
}
#[test]
fn hostile_nesting_rejected() {
    let source = format!(
        "class X{{void a(){{Integer x={}1{};}}}}",
        "(".repeat(200),
        ")".repeat(200)
    );
    assert!(parse(&source, "deep.cls").is_err());
}
#[test]
fn metadata_blocks_record_execution() {
    let mut p =
        program("class Probe{static void sample(){Account a=new Account(Name='A');insert a;}}");
    p.schema.blockers.push(apx_engine::schema::MetadataBlocker {
        path: "Flow metadata".into(),
        object: Some("Account".into()),
        capability: "dml".into(),
        reason: "Active Flow metadata requires execution".into(),
    });
    let e = Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap_err();
    assert_eq!(e.status, "blocked");
    assert!(e.diagnostic.message.contains("metadata"));
}

#[test]
fn cross_project_selection_rejected() {
    let here = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let other = here
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures/demo/force-app/main/default/classes/Calculator.cls");
    let e = apx_engine::project::load(
        Some(here.to_str().unwrap()),
        &[other.to_string_lossy().into_owned()],
    );
    assert!(matches!(e,Err(ref d) if d.code=="APX_PATH"));
}

#[test]
fn decimal_array_keeps_element_promotion() {
    passes("List<Decimal> xs=new Decimal[]{1}; System.assertEquals(0.5,xs[0]/2);");
}
#[test]
fn record_constructor_type_checked() {
    fails(
        "Account a=new Account(Name='A',NumberOfEmployees='wrong');insert a;",
        "failed",
        "Integer",
    );
}
#[test]
fn record_assignment_type_checked() {
    fails(
        "Account a=new Account(Name='A');a.NumberOfEmployees='wrong';",
        "failed",
        "Integer",
    );
}
#[test]
fn private_method_cross_class_blocks() {
    let p=program("class Hidden{private static Integer secret(){return 7;}}class Probe{static void sample(){System.assertEquals(7,Hidden.secret());}}");
    let e = Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap_err();
    assert!(e.diagnostic.message.contains("Non-public"));
}

#[test]
fn query_ascii_equality_is_insensitive() {
    passes("Account a=new Account(Name='Acme');insert a;System.assertEquals(1,[SELECT COUNT() FROM Account WHERE Name='acme']);");
}
#[test]
fn limit_overflow_is_rejected() {
    fails(
        "List<Account> xs=[SELECT Id FROM Account LIMIT 99999999999999999999999999999];",
        "failed",
        "out of range",
    );
}
#[test]
fn record_equality_compares_fields_and_identity_separately() {
    passes("Account a=new Account(Name='A');Account b=new Account(Name='A');System.assertEquals(a,b);System.assert(a==b);System.assert(a!==b);b.Name='B';System.assertNotEquals(a,b);");
}
#[test]
fn invented_system_assert_alias_blocks() {
    fails("System.areEqual(1,1);", "blocked", "system.areequal");
}
#[test]
fn duplicate_record_insert_rejected() {
    fails(
        "Account a=new Account(Name='A');List<Account> xs=new List<Account>{a,a};insert xs;",
        "failed",
        "Duplicate record identity",
    );
}
#[test]
fn trigger_does_not_resolve_caller_method() {
    let mut p=program("class Probe{private static void callerOnly(){}static void sample(){Account a=new Account(Name='A');insert a;}}");
    let t = parse(
        "trigger Wrong on Account(before insert){callerOnly();}",
        "Wrong.trigger",
    )
    .unwrap();
    declarations(&t, &mut p, &mut vec![]);
    let e = Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap_err();
    assert_eq!(e.status, "blocked");
}

#[test]
fn qualified_system_assert_namespace() {
    passes("System.Assert.areEqual('hello world','hello world'); System.Assert.isTrue('HELLO'.equalsIgnoreCase('hello'));");
}
#[test]
fn iterable_list_binding_and_record_iteration() {
    passes("List<Account> accounts=new List<Account>{new Account(NumberOfEmployees=2),new Account(NumberOfEmployees=3),new Account(NumberOfEmployees=null)}; Iterable<Account> sequence=accounts; Integer sum=0; for(Account a:sequence){if(a.NumberOfEmployees!=null){sum+=a.NumberOfEmployees;}} System.Assert.areEqual(5,sum);");
}
#[test]
fn iterable_alias_mutation_rejected() {
    for mutation in [
        "xs.add(3)",
        "xs.clear()",
        "xs.set(0,3)",
        "xs.remove(0)",
        "xs[0]=3",
    ] {
        fails(&format!("List<Integer> xs=new List<Integer>{{1,2}}; Iterable<Integer> seq=xs; for(Integer n:seq){{{mutation};}}"),"failed","during iteration");
    }
}
#[test]
fn iteration_lock_released_after_break_and_nested_loop() {
    passes("List<Integer> xs=new List<Integer>{1}; Iterable<Integer> seq=xs; for(Integer a:seq){for(Integer b:seq){break;}break;}xs.add(2);System.assertEquals(2,xs.size());");
}
#[test]
fn iterable_type_contract_enforced() {
    fails(
        "List<Integer> xs=new List<Integer>{1};Iterable<String> seq=xs;",
        "blocked",
        "exactly match",
    );
    fails(
        "List<Integer> xs=new List<Integer>{1};Iterable<Integer> seq=xs;seq.add(2);",
        "blocked",
        "not implemented",
    );
}
#[test]
fn unused_instance_and_nested_definitions_do_not_block_static_execution() {
    let p=program("class Probe{private String unused='ignored';Probe(){missing();}class Inner{String value;}interface Contract{}enum Choice{ONE,TWO}static Integer sample(){return 7;}}");
    let value = Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap();
    assert_eq!(value.display(), "7");
}
#[test]
fn deferred_nested_shadow_does_not_resolve_to_platform_type() {
    let p = program("class Probe{class System{}static void sample(){System.assert(true);}}");
    let e = Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap_err();
    assert_eq!(e.status, "blocked");
    assert!(e.diagnostic.message.contains("probe.system"));
}
#[test]
fn unused_async_method_is_safe_but_submission_requires_test_frame() {
    let p=program("class Probe{@future static void later(){}static Integer sample(){return 7;}static void bad(){later();}}");
    assert!(Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .is_ok());
    let e = Context::new(&p)
        .call("Probe", "bad", vec![], &p.classes["probe"].declaration)
        .unwrap_err();
    assert_eq!(e.status, "blocked");
    assert!(e
        .diagnostic
        .message
        .contains("outside Test.startTest/stopTest"));
}
#[test]
fn unsupported_behavior_inside_static_initializer_still_blocks() {
    let p = program("class Probe{static{missing();}static void sample(){}}");
    let e = Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap_err();
    assert!(e.diagnostic.message.contains("missing"));
}

#[test]
fn static_initializers_execute_in_source_order_once() {
    let p=program("class Probe{static Integer x=1;static{x+=2;}static Integer y=x+3;static void sample(){System.assertEquals(3,x);System.assertEquals(6,y);}}");
    Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap();
}
#[test]
fn nested_instance_constructor_fields_and_overloads() {
    let p=program("class Probe{class Box{public Integer number;public Box(Integer n){this.number=n;}public void change(Integer n){number=n;}public void change(String s){number=Integer.valueOf(s);}}static void sample(){Box b=new Box(3);b.change('4');System.assertEquals(4,b.number);b.change(8);System.assertEquals(8,b.number);}}");
    Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap();
}
#[test]
fn nested_enum_values_and_switch() {
    let p=program("class Probe{enum Mode{RED,BLUE}static void sample(){Mode m=Mode.BLUE;Integer found=0;switch on m{when RED{found=1;}when BLUE{found=2;}when else{found=3;}}System.assertEquals(2,found);System.assertEquals('BLUE',m.name());System.assertEquals(1,m.ordinal());System.assertEquals(2,Mode.values().size());}}");
    Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap();
}
#[test]
fn map_nested_generics_and_mutation() {
    passes("Map<String,List<Integer>> m=new Map<String,List<Integer>>{'a'=>new List<Integer>{1,2}};m.get('a').add(3);System.assertEquals(3,m.get('a').size());System.assert(m.containsKey('a'));System.assertEquals(1,m.keySet().size());m.remove('a');System.assert(m.isEmpty());");
}
#[test]
fn set_uniqueness_and_iteration() {
    passes("Set<String> s=new Set<String>();System.assert(s.add('a'));System.assert(!s.add('a'));s.add('b');Integer n=0;for(String v:s){n++;}System.assertEquals(2,n);System.assert(s.remove('a'));System.assert(!s.contains('a'));");
}
#[test]
fn empty_collection_initializers_preserve_declared_kind() {
    passes("List<SObject> records=new List<SObject>{};Set<String> names=new Set<String>{};Map<String,Object> values=new Map<String,Object>{};System.assertEquals(0,records.size());System.assertEquals(0,names.size());System.assertEquals(0,values.size());");
}
#[test]
fn map_cycle_rejected() {
    fails(
        "Map<String,Object> m=new Map<String,Object>();m.put('self',m);",
        "blocked",
        "cyclic",
    );
}
#[test]
fn exception_matching_and_finally_order() {
    passes("Integer n=0;try{throw new IllegalArgumentException('bad');}catch(StringException wrong){n=1;}catch(Exception expected){System.assertEquals('bad',expected.getMessage());n=2;}finally{n+=3;}System.assertEquals(5,n);");
}
#[test]
fn unsupported_operation_cannot_be_caught_or_masked_by_finally() {
    fails(
        "try{Database.unimplemented();}catch(Exception e){}finally{System.assert(false);}",
        "blocked",
        "Unknown class",
    );
}
#[test]
fn hostile_minimum_exponent_is_rejected_without_overflow() {
    assert!(apx_engine::value::Number::parse("1e-2147483648").is_err());
}
#[test]
fn constructor_delegation_and_parent_order() {
    let p=program("class Parent{public Integer parentValue;public Parent(Integer n){parentValue=n;}}class Child extends Parent{public Integer value=3;public Child(){this(7);}public Child(Integer n){super(n+1);value+=n;}}class Probe{static void sample(){Child c=new Child();System.assertEquals(8,c.parentValue);System.assertEquals(10,c.value);}}");
    Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap();
}
#[test]
fn string_value_of_null_preserves_null() {
    passes("Object empty=null;System.assertEquals(null,String.valueOf(empty));");
}
#[test]
fn invalid_cast_throws_type_exception_with_source_type() {
    passes("Object item=new Account();Boolean caught=false;try{Datetime bad=(Datetime)item;}catch(TypeException e){caught=true;System.assertEquals('Invalid conversion from runtime type Account to Datetime',e.getMessage());}System.assert(caught);");
}
#[test]
fn nested_exception_uses_enclosing_scope_and_matches_qualified_catch() {
    let p=program("class Outer{public class SpecialException extends Exception{}public class Inner{public void run(){throw new SpecialException('problem');}}}class Probe{static void sample(){Boolean caught=false;try{new Outer.Inner().run();}catch(Outer.SpecialException e){caught=true;System.assertEquals('problem',e.getMessage());}System.assert(caught);}}");
    Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap();
}
#[test]
fn soql_for_loop_fetches_record_batches_of_two_hundred() {
    passes("List<Account> rows=new List<Account>();for(Integer i=0;i<401;i++){rows.add(new Account(Name='A'+i));}insert rows;Integer batches=0;Integer count=0;for(List<Account> batch:[SELECT Id FROM Account]){batches++;count+=batch.size();}System.assertEquals(3,batches);System.assertEquals(401,count);");
}
#[test]
fn record_map_constructor_and_list_type_token() {
    passes("Account a=new Account(Name='A');insert a;List<Account> xs=new List<Account>{a};Map<Id,Account> m=new Map<Id,Account>(xs);System.assertEquals(a,m.get(a.Id));System.assertEquals(Account.SObjectType,xs.getSObjectType());");
}
#[test]
fn number_parse_error_is_catchable() {
    passes("Boolean caught=false;try{Integer n=Integer.valueOf('invalid');}catch(TypeException e){caught=true;}System.assert(caught);");
}
#[test]
fn safe_navigation_short_circuits_calls_and_null_numeric_ordering() {
    passes("String missing=null;Integer i=0;System.assertEquals(null,missing?.substring(i++));System.assertEquals(0,i);System.assert(!(missing?.length()>2));Account a=null;System.assertEquals(null,a?.Name);");
}
#[test]
fn collection_string_equality_is_case_sensitive() {
    passes("System.assert('a'=='A');System.assertNotEquals(new List<String>{'a'},new List<String>{'A'});System.assert(new List<String>{'a'}!=new List<String>{'A'});");
}
#[test]
fn inherited_private_fields_cannot_be_read_unqualified() {
    let p=program("virtual class Base{private Integer secret=3;}class Child extends Base{public Integer peek(){return secret;}}class Probe{static void sample(){new Child().peek();}}");
    let e = Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap_err();
    assert_eq!(e.status, "blocked");
    assert!(e.diagnostic.message.contains("Non-public"));
}
#[test]
fn nested_class_literal_resolves_type_despite_same_named_variable() {
    let p=program("class Container{public class Item{}}class Probe{static void sample(){Container container=new Container();System.assertEquals('Container.Item',Container.Item.class.getName());}}");
    Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap();
}
#[test]
fn declared_record_relationships_are_typed_and_accessible() {
    passes("Contact c=new Contact(Account=new Account(Name='Related'));System.assertEquals('Related',c.Account.Name);c.Account=new Account(Name='Updated');System.assertEquals('Updated',c.Account.Name);");
}
#[test]
fn unknown_type_literals_do_not_create_real_types() {
    fails(
        "Type missing=List<AbsentType>.class;",
        "blocked",
        "Unknown Type literal",
    );
}
#[test]
fn double_rounding_is_distinct_from_exact_decimal_math() {
    passes("Double d=(Double)150;System.assertEquals(150,d);Double x=0.1D;Double y=0.2D;System.assert(x+y!=0.3D);Double half=(Double)1/(Double)2;System.assert(half==0.5D);");
    fails("Double x=(Double)1/(Double)0;", "blocked", "Non-finite");
}
#[test]
fn dynamic_generic_type_constructs_a_typed_map() {
    passes("Type t=Type.forName('Map<Id,Account>');Map<Id,SObject> rows=(Map<Id,SObject>)t.newInstance();Account a=new Account(Name='A');insert a;rows.put(a.Id,a);System.assertEquals(a,rows.get(a.Id));");
}
#[test]
fn nested_collection_covariance_preserves_narrow_mutation_contract() {
    passes("Map<String,List<Contact>> narrow=new Map<String,List<Contact>>{'a'=>new List<Contact>()};Map<String,List<SObject>> wide=(Map<String,List<SObject>>)narrow;wide.get('a').add(new Contact(LastName='C'));System.assertEquals(1,narrow.get('a').size());");
    fails("Map<String,List<Contact>> narrow=new Map<String,List<Contact>>{'a'=>new List<Contact>()};Map<String,List<SObject>> wide=(Map<String,List<SObject>>)narrow;wide.get('a').add(new Account(Name='Wrong'));","failed","Expected contact");
}
#[test]
fn id_assignment_validates_and_normalizes_checksum_form() {
    passes("Id value='001000000000001';System.assertEquals('001000000000001AAA',String.valueOf(value));System.assert(value=='001000000000001');Boolean caught=false;try{Id invalid='not-an-id';}catch(StringException e){caught=true;}System.assert(caught);");
    fails(
        "Id invalid=(Id)JSON.deserialize('\"not-an-id\"',Id.class);",
        "failed",
        "Invalid id",
    );
}
#[test]
fn governor_query_limit_cannot_be_swallowed_by_catch() {
    fails("try{for(Integer i=0;i<101;i++){List<Account> rows=[SELECT Id FROM Account];}}catch(Exception e){}","blocked","Uncatchable Apex governor limit");
}
#[test]
fn aliased_graph_display_has_an_expansion_budget() {
    use apx_engine::value::Value;
    let leaf = Value::list(vec![Value::Str("x".repeat(4096))]);
    let repeated = Value::list(vec![leaf; 1024]);
    assert!(repeated
        .display_checked()
        .unwrap_err()
        .contains("expansion bound"));
}
#[test]
fn property_getters_setters_and_auto_accessors_execute() {
    let p=program("class Box{private Integer backing=1;public Integer number{get{return backing*2;}set{backing=value+1;}}public Integer automatic{get;private set;}public Box(){automatic=4;}public static Integer shared{get;set;}}class Probe{static void sample(){Box b=new Box();System.assertEquals(2,b.number);b.number=3;System.assertEquals(8,b.number);b.number+=2;System.assertEquals(22,b.number);System.assertEquals(4,b.automatic);Box.shared=7;System.assertEquals(7,Box.shared);}}");
    Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap();
}
#[test]
fn readonly_property_assignment_is_rejected() {
    let p=program("class Box{public Integer number{get{return 1;}}}class Probe{static void sample(){new Box().number=2;}}");
    let e = Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap_err();
    assert!(e.diagnostic.message.contains("no setter"));
}
#[test]
fn recursive_calls_reach_semantic_guard_on_bounded_worker_stack() {
    let worker = std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let p = program("class Probe{static void sample(){sample();}}");
            let error = Context::new(&p)
                .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
                .unwrap_err();
            assert_eq!(error.status, "blocked");
            assert!(error.diagnostic.message.contains("call-depth"));
        })
        .unwrap();
    worker.join().unwrap();
}

#[test]
fn super_dispatch_bypasses_override() {
    let p=program("virtual class Base{public virtual Integer value(){return 3;}}class Child extends Base{public override Integer value(){return super.value()+4;}}class Probe{static void sample(){Base c=new Child();System.assertEquals(7,c.value());}}");
    Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap();
}
#[test]
fn custom_iterable_dispatches_iterator_and_restores_loop_scope() {
    let p=program("class Sequence implements Iterable<Integer>{public Iterator<Integer> iterator(){return new Cursor();}public class Cursor implements Iterator<Integer>{Integer i=0;public Boolean hasNext(){return i<3;}public Integer next(){return ++i;}}}class Probe{static void sample(){Integer sum=0;for(Integer n:new Sequence()){if(n==2){continue;}sum+=n;}System.assertEquals(4,sum);for(Integer n:new Sequence()){sum+=n;break;}System.assertEquals(5,sum);}}");
    Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap();
}
#[test]
fn static_private_field_read_is_checked() {
    let p=program("class Hidden{private static Integer secret=1;}class Probe{static void sample(){Integer n=Hidden.secret;}}");
    let e = Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap_err();
    assert_eq!(e.status, "blocked");
    assert!(e.diagnostic.message.contains("Non-public field"));
}

#[test]
fn iterable_compares_underlying_list_values() {
    passes("List<Integer> xs=new List<Integer>{1};Iterable<Integer> seq=xs;System.assert(seq==seq);System.assert(seq===xs);System.assertEquals(new List<Integer>{1},seq);System.assertNotEquals(new List<Integer>{2},seq);");
}

#[test]
fn deep_collection_equality_and_user_equals_override() {
    passes("System.assertEquals(new Map<String,List<Integer>>{'x'=>new List<Integer>{1}},new Map<String,List<Integer>>{'x'=>new List<Integer>{1}});System.assertNotEquals(new Set<String>{'a'},new Set<String>{'b'});");
    let p=program("class Token{Integer value;public Token(Integer n){value=n;}public Boolean equals(Object other){return value==((Token)other).value;}}class Probe{static void sample(){Token a=new Token(3);Token b=new Token(3);System.assertEquals(a,b);System.assert(a!==b);System.assertNotEquals(a,new Token(4));}}");
    Context::new(&p)
        .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
        .unwrap();
}
#[test]
fn nested_types_do_not_fall_back_to_builtin_coercion_or_construction() {
    for body in [
        "String s='wrong';",
        "Object a=new Account();",
        "List<Account> xs=new List<Account>();",
    ] {
        let p = program(&format!(
            "class Probe{{class String{{}}class Account{{}}static void sample(){{{body}}}}}"
        ));
        let e = Context::new(&p)
            .call("Probe", "sample", vec![], &p.classes["probe"].declaration)
            .unwrap_err();
        assert_eq!(e.status, "blocked");
        assert!(e.diagnostic.message.contains("nested/instance"));
    }
}
