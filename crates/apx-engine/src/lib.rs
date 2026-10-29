pub mod access;
pub mod dataweave;
pub mod dates;
pub mod environment;
pub mod inputs;
pub mod model;
pub mod platform;
pub mod project;
pub mod record_store;
pub mod regex_subset;
pub mod runtime;
pub mod schema;
pub mod semantic;
pub mod soql;
pub mod syntax;
pub mod value;
use model::{Diagnostic, Report, TestResult};
use std::{collections::BTreeSet, time::Instant};

#[derive(Default, Debug)]
pub struct Options {
    pub command: String,
    pub project: Option<String>,
    pub paths: Vec<String>,
    pub filter: Option<String>,
    pub environment: Option<String>,
    pub json: bool,
}
fn matches_filter(id: &str, filter: &str) -> bool {
    let pat = format!("(?i)^{}$", regex::escape(filter).replace(r"\*", ".*"));
    regex::Regex::new(&pat).is_ok_and(|r| r.is_match(id))
}
fn annotation_see_all_data(node: &syntax::Node) -> Option<bool> {
    let modifiers = node
        .named()
        .filter(|n| n.kind == "modifiers" || n.kind == "annotation")
        .map(|n| n.text())
        .collect::<Vec<_>>()
        .join(" ");
    let re = regex::Regex::new(r"(?i)\bseealldata\s*=\s*(true|false)\b").unwrap();
    re.captures(&modifiers)
        .map(|c| c[1].eq_ignore_ascii_case("true"))
}
pub fn run(options: &Options) -> Report {
    // Windows executable stacks default to 1 MiB. The bounded tree evaluator
    // needs space for nested trigger/call frames without removing its own limits.
    std::thread::scope(|scope| {
        match std::thread::Builder::new()
            .name("apx-evaluator".into())
            .stack_size(16 * 1024 * 1024)
            .spawn_scoped(scope, || run_inner(options))
        {
            Ok(worker) => match worker.join() {
                Ok(report) => report,
                Err(_) => {
                    let mut report = Report::new(&options.command);
                    report
                        .diagnostics
                        .push(Diagnostic::plain("APX_ENGINE", "Evaluator worker panicked"));
                    report
                }
            },
            Err(error) => {
                let mut report = Report::new(&options.command);
                report.diagnostics.push(Diagnostic::plain(
                    "APX_ENGINE",
                    format!("Unable to start evaluator worker: {error}"),
                ));
                report
            }
        }
    })
}
fn run_inner(options: &Options) -> Report {
    let started = Instant::now();
    let capture = inputs::Session::begin();
    let mut report = run_loaded(options);
    let manifest = capture.finish();
    report.inputs = manifest.files;
    report.input_directories = manifest.directories;
    report.input_fingerprint = manifest.fingerprint;
    if let Some(message) = manifest.changed {
        report.status = "blocked".into();
        report.exit_code = 4;
        report.diagnostics.clear();
        report.diagnostics.push(Diagnostic::plain("APX_INPUT_CHANGED", format!("{message}. Results are invalid; rerun against a stable project.")));
        for test in &mut report.tests {
            test.status = "blocked".into();
            test.message = "Inputs changed during the run; result invalidated".into();
        }
        report.summary.passed = 0;
        report.summary.failed = 0;
        report.summary.errored = 0;
        report.summary.blocked = report.tests.len();
    }
    report.summary.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}
fn run_loaded(options: &Options) -> Report {
    let start = Instant::now();
    let mut report = Report::new(&options.command);
    let mut loaded = match project::load(options.project.as_deref(), &options.paths) {
        Ok(v) => v,
        Err(e) => {
            report.diagnostics.push(e);
            report.status = "failed".into();
            report.exit_code = 2;
            return report;
        }
    };
    if let Some(path) = &options.environment {
        match environment::Environment::load(std::path::Path::new(path), &loaded.program.schema) {
            Ok(environment) => loaded.program.environment = Some(environment),
            Err(e) => {
                report.status = "failed".into();
                report.exit_code = 2;
                report
                    .diagnostics
                    .push(Diagnostic::plain("APX_ENVIRONMENT", e.to_string()));
                return report;
            }
        }
    }
    report.project_root = project::normalized(&loaded.root);
    report.files = loaded.files;
    report.diagnostics = loaded.diagnostics;
    if !report.diagnostics.iter().any(|d| d.severity == "error") {
        report.diagnostics.extend(semantic::check(&loaded.program));
    }
    if let Some(environment) = &loaded.program.environment {
        report.limitations.push(format!("Explicit synthetic local environment: {} ({}). Fixture outcomes do not certify Salesforce behavior.",environment.name,environment.source));
    }
    report.summary.files = report.files.len();
    report.summary.selected_files = report.files.iter().filter(|f| f.selected).count();
    let selected = report
        .files
        .iter()
        .filter(|f| f.selected)
        .map(|f| f.path.clone())
        .collect::<BTreeSet<_>>();
    let mut alltests = vec![];
    for c in loaded.program.classes.values() {
        for m in &c.methods {
            if m.test {
                alltests.push((c.name.clone(), m.clone()))
            }
        }
    }
    let mut tests = alltests
        .iter()
        .filter(|(_, m)| selected.contains(m.declaration.path.as_ref()))
        .cloned()
        .collect::<Vec<_>>();
    if options.command == "validate" && tests.is_empty() && !alltests.is_empty() {
        tests = alltests;
        let mut d=Diagnostic::plain("APX_SELECTION_EXPANDED","No tests in selected source; conservatively validating with all discovered project tests. Dependency-based selection is not implemented.");
        d.severity = "information".into();
        report.diagnostics.push(d)
    }
    if let Some(filter) = &options.filter {
        tests.retain(|(c, m)| matches_filter(&format!("{c}.{}", m.name), filter))
    }
    for (c, m) in &tests {
        report.tests.push(TestResult {
            id: format!("{c}.{}", m.name),
            path: m.declaration.path.to_string(),
            line: m.declaration.line(),
            status: "not_run".into(),
            message: "Not executed (discovery)".into(),
            duration_ms: 0.0,
            assertions: Default::default(),
            setup_assertions: Default::default(),
        })
    }
    if report.diagnostics.iter().any(|d| d.severity == "error") {
        report.exit_code = 2;
        report.status = "failed".into();
        for t in &mut report.tests {
            t.status = "blocked".into();
            t.message = "Project contains syntax/declaration/semantic errors".into()
        }
    } else if options.command == "check" {
        report.exit_code = 0;
        report.status = "checked".into()
    } else if tests.is_empty() {
        report.exit_code = 4;
        report.status = "blocked".into();
        report.diagnostics.push(Diagnostic::plain("APX_NO_TESTS","No matching @IsTest/testMethod methods. Syntax checks alone do not validate runtime behavior. Select tests or use check explicitly."));
    } else {
        for (i, (c, m)) in tests.into_iter().enumerate() {
            if std::env::var_os("APX_TRACE_TEST").is_some() {
                eprintln!("APX test: {c}.{}", m.name);
            }
            let ts = Instant::now();
            let mut setup = runtime::Context::new(&loaded.program);
            let class = loaded.program.classes.get(&c.to_ascii_lowercase()).unwrap();
            let see_all_data = annotation_see_all_data(&m.declaration)
                .or_else(|| annotation_see_all_data(&class.declaration))
                .unwrap_or(false);
            setup
                .native_state
                .insert("test.seealldata".into(), value::Value::Bool(see_all_data));
            let mut assertions = model::AssertionCounts::default();
            let result: Result<value::Value, runtime::EvalError> = (|| {
                for s in class.methods.iter().filter(|m| m.setup) {
                    setup.call(&c, &s.name, vec![], &s.declaration)?;
                }
                let mut context = runtime::Context::new(&loaded.program);
                context
                    .native_state
                    .insert("test.seealldata".into(), value::Value::Bool(see_all_data));
                if let Some(clock) = setup.native_state.get("clock.iso") {
                    context
                        .native_state
                        .insert("clock.iso".into(), clock.clone());
                }
                context.install_data_store(setup.data_store.take());
                let result = (|| {
                    let result = context.call(&c, &m.name, vec![], &m.declaration)?;
                    context.assert_async_drained(&m.declaration)?;
                    context.assert_events_drained(&m.declaration)?;
                    Ok(result)
                })();
                assertions = context.assertion_counts();
                result
            })();
            let out = &mut report.tests[i];
            out.assertions = assertions;
            out.setup_assertions = setup.assertion_counts();
            out.duration_ms = ts.elapsed().as_secs_f64() * 1000.;
            match result {
                Ok(_) => {
                    out.status = "passed".into();
                    out.message =
                        "Passed in experimental local evaluator; not Salesforce-qualified".into();
                    if assertions.executed == 0 {
                        let mut warning = m.declaration.diagnostic("APX_NO_ASSERTIONS", "No test-body assertion verdicts were observed, including helpers and asynchronous callbacks. This is an execution-only result.");
                        warning.severity = "warning".into();
                        report.diagnostics.push(warning);
                        out.message.push_str("; no test-body assertions executed");
                    }
                }
                Err(e) => {
                    out.status = e.status.into();
                    out.message = e.diagnostic.message.clone();
                    report.diagnostics.push(e.diagnostic)
                }
            }
        }
        if report.tests.iter().any(|t| t.status == "errored") {
            report.exit_code = 3;
            report.status = "errored".into()
        } else if report.tests.iter().any(|t| t.status == "blocked") {
            report.exit_code = 4;
            report.status = "blocked".into()
        } else if report.tests.iter().any(|t| t.status == "failed") {
            report.exit_code = 1;
            report.status = "failed".into()
        } else {
            report.exit_code = 4;
            report.status = "preview_passed".into();
            let mut d=Diagnostic::plain("APX_PREVIEW","Tests passed in the local preview subset. This engine has not been qualified against Salesforce and cannot certify deployment or complete platform behavior.");
            d.severity = "warning".into();
            report.diagnostics.push(d)
        }
    }
    report.summary.tests = report.tests.len();
    report.summary.passed = report.tests.iter().filter(|t| t.status == "passed").count();
    report.summary.failed = report.tests.iter().filter(|t| t.status == "failed").count();
    report.summary.blocked = report
        .tests
        .iter()
        .filter(|t| t.status == "blocked")
        .count();
    report.summary.errored = report
        .tests
        .iter()
        .filter(|t| t.status == "errored")
        .count();
    report.summary.duration_ms = start.elapsed().as_secs_f64() * 1000.;
    report
}
