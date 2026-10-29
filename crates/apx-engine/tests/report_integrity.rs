use apx_engine::{inputs::{self, Session}, model::AssertionCounts, run, Options};
use std::{fs, path::PathBuf, sync::atomic::{AtomicU64, Ordering}};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().canonicalize().unwrap().join(format!("axtik-report-{}-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),NEXT.fetch_add(1,Ordering::Relaxed)));
        fs::create_dir(&p).unwrap(); Self(p)
    }
    fn write(&self, name: &str, text: &str) -> PathBuf {
        let p = self.0.join(name); fs::create_dir_all(p.parent().unwrap()).unwrap(); fs::write(&p,text).unwrap(); p
    }
    fn run(&self, command: &str) -> apx_engine::model::Report {
        run(&Options { command:command.into(),project:Some(self.0.to_string_lossy().into()),..Default::default() })
    }
}
impl Drop for Fixture { fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); } }

#[test]
fn actual_assertion_verdicts_include_helpers_and_async_but_separate_setup() {
    let f=Fixture::new();
    f.write("Worker.cls", "public class Worker { @future public static void later() { System.assertEquals(2, 1+1); } }");
    f.write("Assertions.cls", "@IsTest class Assertions { @TestSetup static void setup(){System.assert(true);} static void helper(){Assert.isNull(null);} @IsTest static void positive(){System.assert(true); helper();} @IsTest static void empty(){if(false){System.assert(false);}} @IsTest static void negative(){Assert.isTrue(true); Assert.fail('intentional assertion failure');} @IsTest static void asynchronous(){Test.startTest(); Worker.later(); System.assert(true); Test.stopTest();} }");
    let report=f.run("validate");
    assert_eq!(report.schema_version,2);
    assert_eq!(report.exit_code,1,"{:?}",report.diagnostics);
    for (name, expected) in [("positive",AssertionCounts{executed:2,passed:2,failed:0}),("empty",AssertionCounts::default()),("negative",AssertionCounts{executed:2,passed:1,failed:1}),("asynchronous",AssertionCounts{executed:2,passed:2,failed:0})] {
        let result=report.tests.iter().find(|t|t.id.ends_with(&format!(".{name}"))).unwrap();
        assert_eq!(result.assertions,expected,"{name}: {}",result.message);
        assert_eq!(result.setup_assertions,AssertionCounts{executed:1,passed:1,failed:0});
    }
    assert_eq!(report.diagnostics.iter().filter(|d|d.code=="APX_NO_ASSERTIONS").count(),1);
    assert_eq!(f.run("check").tests.iter().map(|t|t.assertions.executed+t.setup_assertions.executed).sum::<usize>(),0);
}

#[test]
fn input_manifest_covers_consumed_payloads_metadata_and_configuration() {
    let f=Fixture::new();
    f.write("sfdx-project.json",r#"{"packageDirectories":[{"path":"force-app"}],"sourceApiVersion":"68.0"}"#);
    f.write("force-app/classes/Probe.cls","@IsTest class Probe { @IsTest static void testIt(){System.assert(true);} }");
    f.write("force-app/classes/Probe.cls-meta.xml","<ApexClass><apiVersion>68.0</apiVersion></ApexClass>");
    f.write("force-app/staticresources/Picture.resource-meta.xml","<StaticResource><contentType>image/png</contentType><cacheControl>Private</cacheControl></StaticResource>");
    let payload=f.write("force-app/staticresources/Picture.png","actual bytes");
    f.write("force-app/staticresources/Expanded.resource-meta.xml","<StaticResource><contentType>application/zip</contentType><cacheControl>Private</cacheControl></StaticResource>");
    let archived=f.write("force-app/staticresources/Expanded/node_modules/module.js","module contents");
    let report=f.run("check");
    assert_eq!(report.exit_code,0,"{:?}",report.diagnostics);
    for suffix in ["sfdx-project.json","Probe.cls","Probe.cls-meta.xml","Picture.resource-meta.xml","Picture.png","Expanded.resource-meta.xml","module.js"] {
        assert!(report.inputs.iter().any(|i|i.path.ends_with(suffix)),"missing {suffix}: {:?}",report.inputs);
    }
    assert!(report.input_directories.iter().any(|d|d.include_ignored && d.path.ends_with("node_modules")));
    assert_eq!(report.input_fingerprint,inputs::fingerprint(&report.inputs,&report.input_directories));
    fs::write(payload,"changed bytes").unwrap();
    let next=f.run("check");
    assert_ne!(report.input_fingerprint,next.input_fingerprint);
    fs::write(archived.with_file_name("another.js"),"new consumed entry").unwrap();
    assert_ne!(next.input_fingerprint,f.run("check").input_fingerprint);
}

#[test]
fn capture_rejects_changed_deleted_added_and_transiently_read_inputs() {
    let f=Fixture::new();
    let file=f.write("payload.csv","first");
    {
        let capture=Session::begin(); inputs::observe_directory(&f.0,true).unwrap();
        inputs::record_read(&file,b"first").unwrap();
        fs::write(&file,"second").unwrap();
        assert!(capture.finish().changed.is_some());
    }
    {
        let capture=Session::begin(); inputs::observe_directory(&f.0,true).unwrap();
        inputs::record_read(&file,b"second").unwrap();
        f.write("new.cls","class NewClass {}");
        assert!(capture.finish().changed.is_some());
    }
    {
        let capture=Session::begin(); inputs::record_read(&file,b"second").unwrap();
        fs::write(&file,"transient").unwrap(); inputs::record_read(&file,b"transient").unwrap();
        fs::write(&file,"second").unwrap();
        assert!(capture.finish().changed.is_some(),"ABA changes across actual reads must be detected");
    }
    {
        let capture=Session::begin(); inputs::record_read(&file,b"second").unwrap(); fs::remove_file(file).unwrap();
        assert!(capture.finish().changed.is_some());
    }
}

#[test]
fn independent_runs_and_failed_loads_do_not_leak_input_capture() {
    let a=Fixture::new(); a.write("A.cls","class A {}");
    let b=Fixture::new(); b.write("B.cls","class B { void broken( }");
    let first=a.run("check");
    let bad=b.run("check");
    assert_eq!(bad.exit_code,2);
    assert!(bad.inputs.iter().all(|i|!i.path.ends_with("A.cls")));
    assert!(bad.inputs.iter().any(|i|i.path.ends_with("B.cls")));
    assert_eq!(first.input_fingerprint,a.run("check").input_fingerprint);
}
