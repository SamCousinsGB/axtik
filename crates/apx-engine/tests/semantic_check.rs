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
fn invented_date_time_apis_are_unsupported_even_in_unexecuted_code() {
    for statement in [
        "Integer n=Date.today().invented();",
        "Integer n=Datetime.now().invented();",
        "Integer n=Time.newInstance(1,2,3,4).invented();",
        "Date d=Date.now();",
        "Datetime d=Datetime.today();",
        "Integer n=Date.today().inventedField;",
        "Integer n=Date.inventedField;",
        "Datetime d=Datetime.now().addMilliseconds(1);",
    ] {
        rejects(&format!("@IsTest class Probe {{ static void neverCalled() {{ if(false){{{statement}}} }} @IsTest static void test(){{System.assert(true);}} }}"), &["APX_COMPILE_UNSUPPORTED"]);
    }
}

#[test]
fn date_time_contracts_check_arity_types_and_receiver_kind() {
    for statement in [
        "Date.newInstance(2024,1);",
        "Date.newInstance('2024',1,2);",
        "Date.today(1);",
        "Date.today().addDays(1L);",
        "Date.today().daysBetween(Datetime.now());",
        "Date.isLeapYear('2024');",
        "Datetime.newInstanceGmt(1L);",
        "Datetime.newInstanceGmt(Date.today(),Date.today());",
        "Datetime.now().formatGmt();",
        "Datetime.now().format(42);",
        "Time.newInstance(1,2,3);",
        "Time.newInstance(1,2,3,4).addHours('1');",
    ] {
        rejects(
            &format!("class Probe {{ void unused(){{if(false){{{statement}}}}} }}"),
            &["APX_BIND_METHOD"],
        );
    }
    for statement in ["Date.addDays(1);", "Datetime.getTime();", "Time.hour();"] {
        rejects(
            &format!("class Probe {{ void unused(){{{statement}}} }}"),
            &["APX_STATIC_CONTEXT"],
        );
    }
    // Static-through-instance dispatch is not borrowed from Java as an implicit
    // language rule. It must remain explicit until independently established.
    rejects(
        "class Probe {void unused(){Date.today().today();}}",
        &["APX_COMPILE_UNSUPPORTED"],
    );
}

#[test]
fn date_time_results_have_precise_types_including_boolean_long_and_date() {
    accepts(
        r#"class Probe {void unused(){
        Date d=System.Date.newInstance(2024,2,29); Date parsed=Date.valueOf('2024-02-29');
        Date boxed=Date.valueOf((Object)d); Date locale=Date.parse('2/29/2024');
        Boolean leap=Date.isLeapYear(2024); Integer days=Date.daysInMonth(2024,2);
        Date next=d.addDays(1).addMonths(1).addYears(1).toStartOfMonth().toStartOfWeek();
        Integer delta=d.daysBetween(next); Integer months=d.monthsBetween(next); Boolean same=d.isSameDay(next);
        Integer y=d.year(); Integer m=d.month(); Integer day=d.day(); Integer ordinal=d.dayOfYear(); String display=d.format();
        Time t=Time.newInstance(1,2,3,4).addHours(1).addMinutes(2).addSeconds(3).addMilliseconds(4);
        Integer hour=t.hour(); Integer minute=t.minute(); Integer second=t.second(); Integer ms=t.millisecond();
        Datetime dt=Datetime.newInstanceGmt(d,t); Datetime local=Datetime.newInstance(d,t);
        Datetime midnight=Datetime.newInstance(2024,2,29); Datetime midnightGmt=Datetime.newInstanceGmt(2024,2,29);
        Datetime clock=Datetime.newInstanceGmt(2024,2,29,1,2,3); Datetime localClock=Datetime.newInstance(2024,2,29,1,2,3);
        Long epoch=dt.getTime(); Datetime fromEpoch=Datetime.newInstance(epoch); Datetime fromInt=Datetime.newInstance(1);
        Datetime converted=Datetime.valueOf((Object)dt); Datetime gmt=Datetime.valueOfGmt('2024-02-29 01:02:03');
        Datetime adjusted=dt.addDays(1).addHours(1).addMinutes(1).addSeconds(1).addMonths(1).addYears(1);
        Date datePart=dt.dateGmt(); Time timePart=dt.timeGmt(); Boolean equal=dt.isSameDay(adjusted);
        String formatted=dt.format('yyyy-MM-dd','UTC'); String utc=dt.formatGmt('yyyy');
        Integer component=dt.yearGmt()+dt.monthGmt()+dt.dayGmt()+dt.hourGmt()+dt.minuteGmt()+dt.secondGmt()+dt.millisecondGmt();
    }}"#,
    );
    for statement in [
        "Integer wrong=Date.isLeapYear(2024);",
        "Integer wrong=Date.today().toStartOfMonth();",
        "Integer wrong=Datetime.now().getTime();",
        "Date wrong=Datetime.now().timeGmt();",
        "Time wrong=Datetime.now().dateGmt();",
    ] {
        rejects(
            &format!("class Probe {{ void unused(){{{statement}}} }}"),
            &["APX_TYPE_MISMATCH"],
        );
    }
}

#[test]
fn adjacent_native_catalogues_cannot_guess_a_return_type_for_invented_methods() {
    for statement in [
        "String s=EncodingUtil.invented('x');",
        "Integer n=Limits.getInventedLimit();",
        "Integer n=Math.invented(1);",
        "Boolean b=Account.SObjectType.getDescribe().isInvented();",
        "Boolean b=Account.Name.getDescribe().isInvented();",
        "Integer n=Math.inventedConstant;",
    ] {
        rejects(
            &format!("class Probe {{ void unused(){{if(false){{{statement}}}}} }}"),
            &["APX_COMPILE_UNSUPPORTED"],
        );
    }
    for statement in [
        "EncodingUtil.base64Encode('not a blob');",
        "EncodingUtil.urlEncode('missing encoding');",
        "Limits.getQueries(1);",
        "Math.random(1);",
        "Math.mod(1.2,2);",
        "Account.SObjectType.getDescribe().isCreateable('wrong');",
        "Account.Name.getDescribe().getLength(1);",
    ] {
        rejects(
            &format!("class Probe {{ void unused(){{{statement}}} }}"),
            &["APX_BIND_METHOD"],
        );
    }
}

#[test]
fn adjacent_native_contracts_preserve_valid_calls_and_exact_result_types() {
    accepts(
        r#"class Probe {void unused(){
        Blob decoded=EncodingUtil.base64Decode('eA=='); String text=EncodingUtil.base64Encode(decoded);
        Blob bytes=EncodingUtil.convertFromHex('ff'); String hex=EncodingUtil.convertToHex(bytes);
        String url=EncodingUtil.urlDecode(EncodingUtil.urlEncode('a b','UTF-8'),'UTF-8');
        Integer limits=Limits.getQueries()+Limits.getLimitQueries()+Limits.getCpuTime()+Limits.getLimitHeapSize();
        Integer abs=Math.abs(-1); Long longer=Math.abs(1L); Decimal exact=Math.abs(1.2);
        Double doubleValue=(Double)1.2; Double floating=Math.sqrt(doubleValue); Decimal decimalRoot=Math.sqrt(1.2);
        Double power=Math.pow(2,3); Double random=Math.random(); Integer rounded=Math.round(doubleValue);
        Long roundedLong=Math.roundToLong(1.2); Long max=Math.max(1,2L); Integer remainder=Math.mod(5,2);
        Double pi=Math.PI; Double e=System.Math.E;
        Boolean allowed=Account.SObjectType.getDescribe().isCreateable();
        Boolean accessible=Account.Name.getDescribe().isAccessible(); Integer length=Account.Name.getDescribe().getLength();
        List<Schema.SObjectType> targets=Account.Id.getDescribe().getReferenceTo();
        Map<Id,Schema.RecordTypeInfo> recordTypes=Account.SObjectType.getDescribe().getRecordTypeInfosById();
    }}"#,
    );
    for statement in [
        "Integer bad=Math.random();",
        "Integer bad=Math.pow(2,3);",
        "Integer bad=Math.roundToLong(1.2);",
        "Integer bad=Account.Name.getDescribe().isAccessible();",
    ] {
        rejects(
            &format!("class Probe {{ void unused(){{{statement}}} }}"),
            &["APX_TYPE_MISMATCH"],
        );
    }
}

#[test]
fn unknown_argument_types_and_ambiguous_numeric_promotions_remain_explicit() {
    rejects(
        "class Probe {void unused(){Date.newInstance(JSON.deserializeUntyped('1'),1,1);}}",
        &["APX_COMPILE_UNSUPPORTED"],
    );
    rejects(
        "class Probe {void unused(){Math.sqrt(1);}}",
        &["APX_COMPILE_UNSUPPORTED"],
    );
    accepts("class Probe {void unused(){Date d=Date.newInstance(null,null,null);Date parsed=Date.valueOf(null);}}");
    accepts("class Math {public static String PI='source member';} class Probe {void unused(){String value=Math.PI;}}");
}

#[test]
fn complete_engine_pipeline_cannot_pass_an_assertion_beside_invalid_dead_code() {
    let directory = std::env::temp_dir().canonicalize().unwrap().join(format!(
        "apx-native-compile-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(directory.clone());
    let source = directory.join("Probe.cls");
    std::fs::write(&source,"@IsTest class Probe {@IsTest static void test(){if(false){Integer ignored=Date.today().invented();}System.assert(true);}}").unwrap();
    for command in ["check", "validate", "test"] {
        let report = apx_engine::run(&apx_engine::Options {
            command: command.into(),
            project: Some(directory.to_string_lossy().into()),
            paths: vec![source.to_string_lossy().into()],
            ..Default::default()
        });
        assert_ne!(
            report.exit_code, 0,
            "{command} must reject unresolved native calls"
        );
        assert_ne!(report.status, "preview_passed");
        assert_eq!(report.summary.passed, 0);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.code == "APX_COMPILE_UNSUPPORTED"),
            "{:?}",
            report.diagnostics
        );
        assert!(report
            .tests
            .iter()
            .all(|test| test.assertions.executed == 0));
    }
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
