use apx_engine::{
    semantic,
    syntax::{declarations, parse, Program},
};

fn diagnostics(source: &str) -> Vec<apx_engine::model::Diagnostic> {
    let mut program = Program::default();
    let tree = parse(source, "semantic.cls").expect("valid fixture grammar");
    let mut errors = vec![];
    declarations(&tree, &mut program, &mut errors);
    assert!(errors.is_empty(), "{errors:?}");
    semantic::check(&program)
}
fn rejects(source: &str, codes: &[&str]) {
    let errors = diagnostics(source);
    for code in codes {
        assert!(
            errors.iter().any(|d| d.code == *code),
            "Missing {code}: {errors:?}"
        );
    }
}
fn accepts(source: &str) {
    let errors = diagnostics(source);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn checks_unreachable_branches_and_unselected_methods_without_execution() {
    rejects("@IsTest class Probe { static void neverCalled() { return 'bad'; } @IsTest static void test(){ if(false){ Integer wrong='text'; MissingType.missingMethod(); } System.assert(true); } }", &["APX_TYPE_MISMATCH","APX_BIND_NAME"]);
    rejects(
        "class Probe { static Integer hidden(){ if(true){return 1;}else{return 'wrong';} } }",
        &["APX_TYPE_MISMATCH"],
    );
}
#[test]
fn resolves_source_members_arguments_and_access_on_all_paths() {
    rejects("class Library { private static Integer hidden(String a){return 1;} public static Integer number(Integer a){return a;} } class Probe { void f(){ if(false){Library.hidden('x'); Library.number('wrong'); Library.unknown(); Library value=new Library(); value.absent=2;} } }", &["APX_MEMBER_ACCESS","APX_BIND_METHOD","APX_BIND_MEMBER"]);
}
#[test]
fn rejects_unknown_declared_types_nested_generics_and_bad_collection_elements() {
    rejects("class Probe { List<Map<String,Missing>> values; void f(Absent a){ List<Integer> bad=new List<Integer>{1,'x'}; Map<String,Integer> wrong=new Map<String,Integer>{'key'=>'text'}; } }", &["APX_BIND_TYPE","APX_TYPE_MISMATCH"]);
}
#[test]
fn checks_fields_accessors_constructor_initializers_and_trigger_bodies() {
    rejects("class Probe { Integer a='bad'; Integer count {get{return 'wrong';}set{value='bad';}} Probe(){Integer b='bad';} } trigger TestTrigger on Account(before insert){if(false){Integer c='bad';}}", &["APX_TYPE_MISMATCH"]);
}
#[test]
fn accepts_nested_types_inheritance_overloads_collections_and_shadowing() {
    accepts("public virtual class Base { public virtual Integer n(Integer x){return x;} } public class Child extends Base { public class Item { public String text; } public override Integer n(Integer x){return x+1;} public String n(String x){return x;} public void f(){Item item=new Item(); item.text='ok'; Map<String,List<Integer>> nested=new Map<String,List<Integer>>{'a'=>new List<Integer>{1,2}}; List<Integer> values=nested.get('a'); for(Integer value:values){Integer result=n(value);} String value=n('text');} }");
}
#[test]
fn respects_catch_for_and_branch_scopes_and_type_tokens() {
    accepts("@IsTest class Probe { @IsTest static void test(){ for(Integer i=0;i<3;i++){Integer j=i+1;} try {String x='a';} catch(Exception e){String message=e.getMessage();} Type token=Probe.class; if(true){String value='s';}else{Integer value=1;} System.Assert.isTrue(true); } }");
}
#[test]
fn keeps_query_assignment_and_record_field_types() {
    accepts("class Probe {void f(){ Account a=[SELECT Id,Name FROM Account LIMIT 1]; List<Account> rows=[SELECT Id,Name FROM Account]; for(Account row:rows){String name=row.Name;} for(List<Account> batch:[SELECT Id FROM Account]){Integer size=batch.size();} Account child=new Account(Name='name'); }}");
    rejects("class Probe { void f(){ if(false){ Account a=new Account(Name=42); Integer wrong=[SELECT Name FROM Account LIMIT 1].Name; } } }", &["APX_TYPE_MISMATCH"]);
}
#[test]
fn resolves_inherited_overloads_and_query_arguments_without_executing() {
    accepts("virtual class Base { public String pick(String text){return text;} } class Child extends Base { public Integer pick(Integer n){return n;} void query(List<Account> rows){} void f(){String s=pick('ok');Integer n=pick(1);query([SELECT Id FROM Account]);} }");
    rejects("class Probe {String pick(Object o){return 'x';}Integer pick(Integer n){return n;} void f(){if(false){String wrong=pick(1);}}}", &["APX_TYPE_MISMATCH"]);
    rejects("class Probe {String pick(String s){return s;}Integer pick(Integer n){return n;} void f(){if(false){Object wrong=pick(null);}}}", &["APX_BIND_METHOD"]);
}
#[test]
fn rejects_unreachable_invalid_collection_calls_native_members_and_static_access() {
    rejects("class Probe {Integer field; void instanceMethod(){} static void f(){if(false){Integer a=field;this.instanceMethod();instanceMethod();Probe.instanceMethod();List<Integer> nums=new List<Integer>();nums.add('bad');nums.get('bad');nums.missing();Map<String,Integer> map=new Map<String,Integer>();map.put('key','bad');String s='x';s.missing();s.substring('bad');System.missing();Integer.missing();Assert.missing();}}}", &["APX_STATIC_CONTEXT","APX_BIND_METHOD","APX_TYPE_MISMATCH"]);
}
#[test]
fn type_tokens_ignore_value_shadowing_and_standard_exception_subtypes_widen() {
    accepts("class Probe {class Nested{} static Probe probe; void use(Exception e){} void f(){Type token=Probe.Nested.class;Exception e=new DmlException();use(new QueryException());List<Account> rows=new List<SObject>();}}");
}
#[test]
fn checks_operator_operands_throw_and_void_returns_in_dead_code() {
    for source in [
        "class Probe {void f(){if(false){Integer n='bad'*2;}}}",
        "class Probe {void f(){if(false){String s='bad';s++;}}}",
        "class Probe {void f(){if(false){throw 'bad';}}}",
        "class Probe {void f(){if(false){return null;}}}",
        "class Probe {void f(){if(false){Boolean b=!1;}}}",
    ] {
        rejects(source, &["APX_TYPE_MISMATCH"]);
    }
    accepts("class Probe {Integer instanceField;static Integer shared;{instanceField=1;}static{shared=1;}void f(){Integer n=2*3+4;Boolean b=true&false;throw new DmlException();}}");
}
#[test]
#[ignore = "requires APX_RECIPES_REPO pointing to the pinned unchanged official clone"]
fn checks_the_complete_pinned_recipe_project_without_executing_tests() {
    let repo = std::env::var("APX_RECIPES_REPO").expect("APX_RECIPES_REPO");
    let project = apx_engine::project::load(Some(&repo), &[]).expect("load corpus");
    let errors = semantic::check(&project.program);
    for error in &errors {
        eprintln!(
            "{}:{} {} {}",
            error.path, error.line, error.code, error.message
        );
    }
    assert!(
        errors.is_empty(),
        "{} semantic errors in unchanged corpus",
        errors.len()
    );
}
