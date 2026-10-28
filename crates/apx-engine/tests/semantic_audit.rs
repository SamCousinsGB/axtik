//! Independent probes for recently implemented semantics and false-green edges.
use apx_engine::{
    runtime::{Context, EvalError},
    syntax::{declarations, parse, Program},
};
fn run(source: &str) -> Result<(), EvalError> {
    let tree = parse(source, "audit.cls").unwrap();
    let mut p = Program::default();
    let mut ds = vec![];
    declarations(&tree, &mut p, &mut ds);
    assert!(ds.is_empty(), "{ds:?}");
    Context::new(&p)
        .call("probe", "run", vec![], &tree)
        .map(|_| ())
}

#[test]
fn an_active_property_cannot_bypass_a_private_accessor_through_another_class() {
    let result=run("class Box{public Integer value{private get{return Helper.read(this);}set;}public Integer expose(){return value;}}class Helper{public static Integer read(Box b){return b.value;}}class Probe{static void run(){new Box().expose();}}");
    assert!(
        result.is_err(),
        "An active accessor must not expose its private value to an external helper"
    );
}
#[test]
fn custom_equals_is_executed_and_identity_comparison_has_no_side_effect() {
    run("class Token{public static Integer calls=0;public Integer value;public Token(Integer n){value=n;}public Boolean equals(Object other){calls++;return value==((Token)other).value;}}class Probe{static void run(){Token a=new Token(3);Token b=new Token(3);System.assert(a!==b);System.assertEquals(0,Token.calls);System.assertEquals(a,b);System.assertEquals(1,Token.calls);System.assertNotEquals(new List<Token>{a},new List<Token>{new Token(4)});System.assertEquals(2,Token.calls);}}").unwrap();
}
#[test]
fn private_auto_setter_and_getter_remain_restricted() {
    assert!(run("class Box{public Integer value{get;private set;}}class Probe{static void run(){new Box().value=9;}}").is_err());
    assert!(run("class Box{public Integer value{private get;set;}}class Probe{static void run(){Integer value=new Box().value;}}").is_err());
}
#[test]
fn custom_getter_side_effects_happen_once_per_access() {
    run("class Box{public Integer calls=0;public Integer value{get{calls++;return calls;}}}class Probe{static void run(){Box b=new Box();System.assertEquals(1,b.value);System.assertEquals(2,b.value);System.assertEquals(2,b.calls);}}").unwrap();
}
#[test]
fn nested_string_collection_equality_is_case_sensitive() {
    run("class Probe{static void run(){System.assert('a'=='A');System.assertNotEquals(new List<String>{'a'},new List<String>{'A'});System.assertNotEquals(new Map<String,List<String>>{'x'=>new List<String>{'a'}},new Map<String,List<String>>{'x'=>new List<String>{'A'}});}}").unwrap();
}

#[test]
fn list_membership_uses_user_equals_without_holding_mutable_collection_borrows() {
    run("class Token{public static Integer calls=0;public Integer value;public Token(Integer n){value=n;}public Boolean equals(Object other){calls++;return value==((Token)other).value;}}class Probe{static void run(){List<Token> items=new List<Token>{new Token(3)};System.assert(items.contains(new Token(3)));System.assertEquals(0,items.indexOf(new Token(3)));System.assertEquals(2,Token.calls);System.assertEquals(-1,items.indexOf(new Token(4)));}}").unwrap();
}

#[test]
fn full_regex_match_must_consider_longer_alternatives() {
    run("class Probe{static void run(){System.assert(Pattern.matches('a|ab','ab'));System.assert(!Pattern.matches('a|ab','abc'));Matcher m=Pattern.compile('(a|ab)').matcher('ab');System.assert(m.matches());System.assertEquals('ab',m.group(1));}}").unwrap();
}
#[test]
fn failed_full_or_prefix_match_does_not_disable_a_later_find() {
    run("class Probe{static void run(){Matcher m=Pattern.compile('a').matcher('ba');System.assert(!m.matches());System.assert(m.find());System.assertEquals(1,m.start());m.reset();System.assert(!m.lookingAt());System.assert(m.find());}}").unwrap();
}
#[test]
fn mixed_unsupported_regex_flags_do_not_inherit_an_allowed_groups_exemption() {
    let result = run("class Probe{static void run(){Pattern.compile('(?:a)(?U).*');}}");
    assert!(
        result.is_err(),
        "Unknown/Java-incompatible flags must remain unsupported even beside an allowed group"
    );
}

#[test]
fn matcher_continuation_matches_independent_jdk_zero_width_and_failure_probes() {
    // Independent JDK 21 probe evidence is preserved in corpus/evidence.
    run("class Probe{static void run(){Matcher m=Pattern.compile('^').matcher('b');System.assert(m.find());System.assertEquals(0,m.start());System.assert(!m.find());System.assert(m.find());System.assertEquals(0,m.start());System.assert(!m.find());System.assert(m.find());m=Pattern.compile('a*').matcher('b');System.assert(m.find());System.assertEquals(0,m.start());System.assert(m.find());System.assertEquals(1,m.start());System.assert(!m.find());System.assert(!m.find());System.assert(!m.find());m=Pattern.compile('a*').matcher('ba');System.assert(m.find());System.assert(!m.matches());System.assert(m.find());System.assertEquals(0,m.start());System.assert(m.find());System.assertEquals(1,m.start());System.assertEquals(2,m.end());m=Pattern.compile('a').matcher('baba');System.assert(m.find());System.assertEquals(1,m.start());System.assert(!m.matches());System.assert(m.find());System.assertEquals(3,m.start());}}").unwrap();
}

#[test]
fn java_line_terminators_cannot_produce_a_false_regex_pass() {
    for source in [
        "class Probe{static void run(){System.assert(!Pattern.matches('.', '\\r'));}}",
        "class Probe{static void run(){System.assert(Pattern.compile('a$').matcher('a\\n').find());}}",
    ] {
        // Exact Java behavior or an explicit unsupported result are acceptable;
        // a completed, incorrect assertion is not.
        if let Err(error) = run(source) {
            assert_eq!(error.status, "blocked", "{error:?}");
        }
    }
}

#[test]
fn string_regex_edges_are_exact_or_explicitly_unsupported() {
    use apx_engine::{platform, value::Value};
    for (input, pattern, expected) in [
        ("a", "a*", "XX"),
        ("abc", "\\b", "XabcX"),
        ("a\n", "a$", "X\n"),
        ("\r", ".", "\r"),
        ("abb", "(?:a)(?U)b+", "X"),
    ] {
        match platform::instance(
            &Value::Str(input.into()),
            "replaceall",
            &[Value::Str(pattern.into()), Value::Str("X".into())],
        ) {
            Ok(Some(value)) => assert_eq!(value.display(), expected),
            Err(error) => assert!(error.unsupported, "{error:?}"),
            _ => panic!("replaceAll must produce a result or explicit unsupported error"),
        }
    }
    let result = platform::instance(
        &Value::Str("abc".into()),
        "split",
        &[Value::Str("\\b".into())],
    );
    assert!(result.is_err_and(|error| error.unsupported));
}

#[test]
fn invalid_ciphertexts_fail_without_host_panics_and_changed_plaintext_changes_ciphertext() {
    use apx_engine::{platform, value::Value};
    let key = Value::Blob(vec![7; 16]);
    let iv = Value::Blob(vec![8; 16]);
    let cipher = |data: Vec<u8>| {
        platform::invoke(
            "crypto",
            "encrypt",
            &[
                Value::Str("AES128".into()),
                key.clone(),
                iv.clone(),
                Value::Blob(data),
            ],
        )
        .unwrap()
        .unwrap()
    };
    let original = cipher(b"original".to_vec());
    let changed = cipher(b"changed".to_vec());
    assert!(!original.equal(&changed));
    for length in [0, 1, 15, 17, 31] {
        let result = platform::invoke(
            "crypto",
            "decrypt",
            &[
                Value::Str("AES128".into()),
                key.clone(),
                iv.clone(),
                Value::Blob(vec![0; length]),
            ],
        );
        assert!(result.is_err(), "Malformed length {length} must fail");
    }
    assert!(platform::invoke(
        "crypto",
        "encrypt",
        &[
            Value::Str("AES128".into()),
            Value::Blob(vec![0; 15]),
            iv,
            Value::Blob(vec![])
        ]
    )
    .is_err());
}
