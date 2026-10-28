//! Optional source integration check: run explicitly with APX_RECIPES_REPO set to
//! the unchanged pinned upstream clone. The ordinary unit suite needs no clone.
use apx_engine::dataweave::{execute, DwField, DwValue};
use std::{collections::BTreeMap, fs, path::PathBuf, process::Command};

#[test]
#[ignore = "requires APX_RECIPES_REPO pointing to the pinned official clone"]
fn every_pinned_dataweave_script_executes_its_actual_source() {
    let root = PathBuf::from(std::env::var("APX_RECIPES_REPO").expect("Set APX_RECIPES_REPO"));
    let revision = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(revision.status.success());
    assert_eq!(
        String::from_utf8(revision.stdout).unwrap().trim(),
        "b364267fc786a8a3ac4c3968467fcc4abd4adac6"
    );
    let clean = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["status", "--porcelain", "--untracked-files=all"])
        .output()
        .unwrap();
    assert!(
        clean.status.success() && clean.stdout.is_empty(),
        "Upstream source must be unchanged"
    );
    let mut scripts = fs::read_dir(root.join("force-app/main/default/dw"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "dwl"))
        .collect::<Vec<_>>();
    scripts.sort();
    assert_eq!(scripts.len(), 14);
    let json = |s: &str| DwValue::String(s.into());
    for file in scripts {
        let name = file.file_stem().unwrap().to_str().unwrap();
        let source = fs::read_to_string(&file).unwrap();
        let mut inputs=BTreeMap::from([
            ("payload".into(),json("[{\"isWinner\":true,\"marker\":\"selected\",\"currency\":\"USD\"},{\"isWinner\":false,\"marker\":\"omitted\"}]")),
            ("records".into(),json("[{\"first_name\":\"Independent\",\"last_name\":\"Fixture\",\"email\":\"different@example.invalid\"}]")),
            ("inputs".into(),json("[\"city\",\"child\",\"box\",\"deer\",\"person\",\"datum\"]")),
            ("products".into(),json("[{\"price\":12.5,\"properties\":{\"year\":2020,\"title\":\"A&B\",\"author\":[\"Different Author\"]}},{\"price\":99,\"properties\":{\"year\":1999,\"title\":\"excluded\",\"author\":[]}}]")),
            ("attributes".into(),json("{\"publishedAfter\":2010}")),
            ("exchangeRates".into(),json("{\"USD\":[{\"currency\":\"EUR\",\"ratio\":2.4}]}")),
        ]);
        if source.contains("application/csv") {
            let delimiter = if source.contains("separator=';'") {
                ';'
            } else {
                ','
            };
            let csv=format!("first_name{delimiter}last_name{delimiter}email\nIndependent{delimiter}Fixture{delimiter}different@example.invalid");
            for key in ["records", "payload"] {
                inputs.insert(key.into(), json(&csv));
            }
        }
        if source.contains("application/java") {
            inputs.insert(
                "records".into(),
                DwValue::Array(vec![DwValue::Object(vec![
                    DwField::new("FirstName", json("Independent")),
                    DwField::new("LastName", json("Fixture")),
                    DwField::new("CreatedDate", json("2026-03-04T21:07:08Z")),
                ])]),
            );
        }
        let result = execute(&source, &inputs);
        match name {
            "error" => {
                let e = result.unwrap_err();
                assert!(!e.unsupported && e.message.starts_with("Division by zero"));
            }
            "excelOutputError" => {
                let e = result.unwrap_err();
                assert!(
                    !e.unsupported
                        && e.message
                            .contains("Unknown content type `application/xlsx`")
                );
            }
            _ => {
                let result = result.unwrap_or_else(|e| panic!("{name}: {e:?}"));
                assert!(!result.text.is_empty(), "{name}");
                match name {
                    "helloWorld" => assert_eq!(result.text, "\"Hello World\""),
                    "multipleInputs" => {
                        assert!(result.text.contains("<price currency=\"EUR\">30</price>"));
                        assert!(result.text.contains("Different Author"));
                        assert!(!result.text.contains("excluded"));
                    }
                    "logFilter" => {
                        assert!(result.text.contains("selected"));
                        assert!(!result.text.contains("omitted"));
                        assert_eq!(result.logs.len(), 2);
                    }
                    "jsonDateFormat" => {
                        assert!(result.text.contains("09:07:08 PM, March 04, 2026"))
                    }
                    "pluralizeFunction" => {
                        assert!(result.text.contains("cities"));
                        assert!(result.text.contains("children"));
                    }
                    "reservedApexKeywords" => assert!(result.text.contains("currency_x")),
                    _ => assert!(result.text.contains("Independent"), "{name}"),
                }
            }
        }
        println!("Verified source: {name}");
    }
}
