//! Local controls for the portable oracle fixtures. This never contacts an org.
use std::{fs, path::PathBuf, process::Command};

#[test]
fn comparison_fixtures_exercise_the_intended_failures_and_positive_controls() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/salesforce-comparison");
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
    for scenario in manifest["scenarios"].as_array().unwrap() {
        let id = scenario["id"].as_str().unwrap();
        let project = root.join(scenario["project"].as_str().unwrap());
        let local = &scenario["local"];
        let mut command = Command::new(env!("CARGO_BIN_EXE_apx"));
        command.arg(local["command"].as_str().unwrap());
        for path in local["paths"].as_array().unwrap() {
            command.arg(project.join(path.as_str().unwrap()));
        }
        command.args(["--project", project.to_str().unwrap(), "--json"]);
        if let Some(filter) = local["filter"].as_str() {
            command.args(["--filter", filter]);
        }
        if let Some(environment) = local["environment"].as_str() {
            command.arg("--environment").arg(project.join(environment));
        }
        let output = command.output().unwrap();
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| panic!("{id}: {e}: {}", String::from_utf8_lossy(&output.stderr)));
        let expected = &scenario["expected"];
        if expected["compile"] == "fail" {
            assert_eq!(output.status.code(), Some(2), "{id}: {report}");
            assert_eq!(report["summary"]["passed"], 0, "{id}");
            assert_eq!(report["summary"]["failed"], 0, "{id}");
        } else {
            assert_eq!(report["tests"].as_array().unwrap().len(), 1, "{id}");
            let test = &report["tests"][0];
            assert_eq!(test["id"], local["filter"], "{id}");
            if expected["tests"] == "pass" {
                assert_eq!(output.status.code(), Some(4), "{id}: {report}");
                assert_eq!(test["status"], "passed", "{id}: {report}");
                assert!(test["assertions"]["executed"].as_u64().unwrap() > 0, "{id}");
            } else {
                assert_eq!(output.status.code(), Some(1), "{id}: {report}");
                assert_eq!(test["status"], "failed", "{id}: {report}");
            }
        }
        if let Some(failure) = expected.get("failure") {
            let diagnostics = report["diagnostics"].as_array().unwrap();
            let matching = diagnostics.iter().any(|d| {
                let message = d["message"].as_str().unwrap_or("").to_ascii_lowercase();
                failure["messageIncludes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|part| message.contains(&part.as_str().unwrap().to_ascii_lowercase()))
                    && (failure["category"] != "governor" || d["code"] == "APX_GOVERNOR")
            });
            assert!(matching, "{id}: unexpected failure: {diagnostics:?}");
        }
    }
}
