use apx_engine::{inputs, run, Options};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // The fixture itself has a physical location even on hosts whose system
        // temporary directory is reached through an OS-provided alias.
        let parent = std::env::temp_dir().canonicalize().unwrap();
        let path = parent.join(format!(
            "axtik-path-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn write(&self, name: &str, content: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }
    fn check(&self) -> apx_engine::model::Report {
        check_project(&self.0)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn check_project(path: &Path) -> apx_engine::model::Report {
    run(&Options {
        command: "check".into(),
        project: Some(path.to_string_lossy().into()),
        ..Default::default()
    })
}
fn rejected(report: &apx_engine::model::Report) {
    assert_ne!(report.exit_code, 0, "{report:?}");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("links and reparse points")),
        "{report:?}"
    );
    assert_eq!(report.summary.passed, 0);
}

#[test]
fn physical_paths_and_parent_components_still_work() {
    let f = Fixture::new();
    let file = f.write("project/Probe.cls", "class Probe {}");
    fs::create_dir(f.0.join("project/child")).unwrap();
    inputs::ensure_regular_path(&file).unwrap();
    inputs::ensure_regular_path(&f.0.join("project/child/../Probe.cls")).unwrap();
    assert_eq!(f.check().exit_code, 0);
}

#[cfg(unix)]
#[test]
fn retargeted_consumed_file_symlinks_are_rejected_before_capture_or_read() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let a = f.write("first.json", r#"{"packageDirectories":[{"path":"src"}]}"#);
    let b = f.write(
        "second.json",
        r#"{"packageDirectories":[{"path":"other"}]}"#,
    );
    f.write("src/Probe.cls", "class Probe {}");
    f.write("other/Changed.cls", "class Changed {}");
    let alias = f.0.join("sfdx-project.json");
    symlink(&a, &alias).unwrap();
    let session = inputs::Session::begin();
    assert!(inputs::record_read(&alias, &fs::read(&a).unwrap()).is_err());
    rejected(&f.check());
    fs::remove_file(&alias).unwrap();
    symlink(&b, &alias).unwrap();
    assert!(inputs::record_read(&alias, &fs::read(&b).unwrap()).is_err());
    rejected(&f.check());
    assert!(
        session.finish().files.is_empty(),
        "rejected alias must not become an accepted canonical identity"
    );
}

#[cfg(unix)]
#[test]
fn file_replaced_with_symlink_after_read_invalidates_the_snapshot() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let consumed = f.write("data.json", "same bytes");
    let target = f.write("target.json", "same bytes");
    let session = inputs::Session::begin();
    inputs::record_read(&consumed, b"same bytes").unwrap();
    fs::remove_file(&consumed).unwrap();
    symlink(target, &consumed).unwrap();
    assert!(
        session.finish().changed.is_some(),
        "even equal bytes cannot qualify a newly introduced alias"
    );
}

#[cfg(unix)]
#[test]
fn ancestor_aliases_companions_and_environment_reads_cannot_bypass_path_guards() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    f.write("real/Probe.cls", "class Probe {}");
    let link = f.0.join("alias");
    symlink(f.0.join("real"), &link).unwrap();
    rejected(&check_project(&link));
    assert!(inputs::ensure_regular_path(&inputs::lexical_join(
        &link,
        Path::new("../real/Probe.cls")
    ))
    .is_err());
    let metadata = f.write(
        "metadata.xml",
        "<ApexClass><apiVersion>68.0</apiVersion></ApexClass>",
    );
    symlink(metadata, f.0.join("real/Probe.cls-meta.xml")).unwrap();
    rejected(&check_project(&f.0.join("real")));
    let fixture = f.write("fixture.json", "{}");
    let fixture_alias = f.0.join("fixture-alias.json");
    symlink(fixture, &fixture_alias).unwrap();
    let error = apx_engine::environment::Environment::load(
        &fixture_alias,
        &apx_engine::schema::SchemaCatalog::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("links and reparse points"));
}

#[cfg(windows)]
#[test]
fn windows_directory_junctions_are_rejected_before_canonicalization() {
    use std::os::windows::process::CommandExt;
    let f = Fixture::new();
    f.write("real/Probe.cls", "class Probe {}");
    let junction = f.0.join("alias");
    let target = f.0.join("real");
    let result = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&junction)
        .arg(&target)
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "junction creation failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let direct = inputs::ensure_regular_path(&junction.join("Probe.cls"));
    let parent = inputs::ensure_regular_path(&inputs::lexical_join(
        &junction,
        Path::new("../real/Probe.cls"),
    ));
    f.write(
        "sfdx-project.json",
        r#"{"packageDirectories":[{"path":"alias/../real"}]}"#,
    );
    let disguised_package = check_project(&f.0);
    let report = check_project(&junction);
    // Remove only the junction itself before recursive fixture cleanup.
    fs::remove_dir(&junction).unwrap();
    assert!(direct.is_err());
    assert!(parent.is_err());
    rejected(&report);
    rejected(&disguised_package);
    assert!(target.join("Probe.cls").is_file());
}
