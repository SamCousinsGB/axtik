use apx_engine::{
    model::{Diagnostic, Report},
    run, Options,
};
fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|x| x == "--help" || x == "-h") || args.is_empty() {
        println!("APX {} - experimental offline Apex engine\n\napx check|validate|test [classes or folders...] [--project ROOT] [--filter Class.method] [--environment FILE] [--json]\n\ncheck: syntax/declarations and test discovery (not full type validation)\nvalidate/test: execute supported local preview; preview success returns 4\nNo org connection or telemetry. Unsupported behavior blocks.", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args[0] == "--version" || args[0] == "version" {
        println!("apx {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let mut o = Options {
        command: args[0].clone(),
        ..Default::default()
    };
    let mut error = None;
    let mut i = 1;
    if !["check", "validate", "test"].contains(&o.command.as_str()) {
        error = Some(format!("Unknown command {}", o.command))
    }
    while i < args.len() {
        match args[i].as_str() {
            "--json" => o.json = true,
            "--project" | "--filter" | "--source-dir" | "--environment" => {
                let key = &args[i];
                i += 1;
                if let Some(v) = args.get(i) {
                    match key.as_str() {
                        "--project" if o.project.is_some() => {
                            error = Some("--project may only be provided once".into())
                        }
                        "--filter" if o.filter.is_some() => {
                            error = Some("--filter may only be provided once; use one exact identity or wildcard".into())
                        }
                        "--project" => o.project = Some(v.clone()),
                        "--environment" if o.environment.is_some()=>error=Some("--environment may only be provided once".into()),
                        "--environment" => o.environment=Some(v.clone()),
                        "--filter" => o.filter = Some(v.clone()),
                        _ => o.paths.push(v.clone()),
                    }
                } else {
                    error = Some(format!("Missing value for {key}"))
                }
            }
            "--" => {
                o.paths.extend(args.iter().skip(i + 1).cloned());
                break;
            }
            s if s.starts_with('-') => error = Some(format!("Unknown option {s}")),
            _ => o.paths.push(args[i].clone()),
        }
        i += 1
    }
    let report = if let Some(e) = error {
        let mut r = Report::new(&o.command);
        r.status = "failed".into();
        r.exit_code = 2;
        r.diagnostics.push(Diagnostic::plain("APX_ARGUMENT", e));
        r
    } else {
        match std::panic::catch_unwind(|| run(&o)) {
            Ok(r) => r,
            Err(_) => {
                let mut r = Report::new(&o.command);
                r.diagnostics.push(Diagnostic::plain(
                    "APX_ENGINE_FAULT",
                    "Engine panic; no result can be trusted",
                ));
                r
            }
        }
    };
    if o.json {
        println!(
            "{}",
            serde_json::to_string(&report).expect("report serialization")
        )
    } else {
        println!(
            "APX {} | {} | {}",
            report.engine_version, report.status, report.assurance
        );
        for d in &report.diagnostics {
            eprintln!(
                "{}:{}:{} [{}] {}",
                d.path, d.line, d.column, d.code, d.message
            )
        }
        for t in &report.tests {
            println!(
                "{} {} ({:.2} ms): {}",
                t.status, t.id, t.duration_ms, t.message
            )
        }
        println!(
            "{} files ({} selected), {} tests; {} passed, {} failed, {} blocked; {:.2} ms",
            report.summary.files,
            report.summary.selected_files,
            report.summary.tests,
            report.summary.passed,
            report.summary.failed,
            report.summary.blocked,
            report.summary.duration_ms
        );
        if report.command == "check" {
            println!("Scope: syntax and declarations only; runtime, full typing and deployment are not certified.")
        }
    }
    std::process::exit(report.exit_code)
}
