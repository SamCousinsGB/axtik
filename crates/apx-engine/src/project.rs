use crate::{
    model::{Diagnostic, FileInfo},
    syntax::{self, Program},
};
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};
mod resources;
pub fn normalized(p: &Path) -> String {
    p.to_string_lossy()
        .strip_prefix(r"\\?\")
        .unwrap_or(&p.to_string_lossy())
        .to_string()
}
fn apex(p: &Path) -> bool {
    p.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("cls") || s.eq_ignore_ascii_case("trigger"))
}
fn scan(
    path: &Path,
    files: &mut BTreeSet<PathBuf>,
    metadata: &mut BTreeSet<PathBuf>,
) -> Result<(), Diagnostic> {
    crate::inputs::ensure_regular_path(path).map_err(|e| Diagnostic::plain("APX_INPUT_PATH", e.to_string()))?;
    if path.is_file() {
        if let Some(parent) = path.parent() {
            crate::inputs::observe_directory(parent, false).map_err(|e| Diagnostic::plain("APX_INPUT_SNAPSHOT", e.to_string()))?;
        }
        if apex(path) {
            files.insert(path.to_path_buf());
        }
        return Ok(());
    }
    crate::inputs::observe_directory(path, true).map_err(|e| Diagnostic::plain("APX_INPUT_SNAPSHOT", e.to_string()))?;
    let mut entries = 0usize;
    for ent in walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            !matches!(
                e.file_name().to_str(),
                Some(".git" | "node_modules" | "target" | ".sf" | ".sfdx" | ".apx" | "dist")
            )
        })
    {
        entries += 1;
        if entries > 100_000 {
            return Err(Diagnostic::plain(
                "APX_SCAN_BUDGET",
                "Directory traversal exceeds 100000 entries",
            ));
        }
        let e = ent.map_err(|e| Diagnostic::plain("APX_READ", e.to_string()))?;
        if e.file_type().is_symlink() {
            continue;
        }
        if !e.file_type().is_file() {
            continue;
        }
        let p = e.path();
        if apex(p) {
            files.insert(p.to_path_buf());
            if files.len() > 5000 {
                return Err(Diagnostic::plain(
                    "APX_FILE_LIMIT",
                    "Project exceeds 5000 Apex files",
                ));
            }
        } else if let Some(n) = p.file_name().and_then(|x| x.to_str()) {
            let lower = n.to_ascii_lowercase();
            let ext = p
                .extension()
                .and_then(|x| x.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let companion =
                lower.ends_with(".cls-meta.xml") || lower.ends_with(".trigger-meta.xml");
            let inert = [
                "json", "md", "txt", "js", "ts", "css", "html", "svg", "png", "jpg", "jpeg", "gif",
                "map",
            ]
            .contains(&ext.as_str())
                || n.starts_with('.');
            if !companion && !inert {
                metadata.insert(p.to_path_buf());
            }
        }
    }
    Ok(())
}
pub struct Loaded {
    pub root: PathBuf,
    pub program: Program,
    pub files: Vec<FileInfo>,
    pub diagnostics: Vec<Diagnostic>,
}
fn read_limited(path: &Path, max: usize, code: &str) -> Result<Vec<u8>, Diagnostic> {
    crate::inputs::ensure_regular_path(path).map_err(|e| Diagnostic::plain("APX_INPUT_PATH", e.to_string()))?;
    let file = fs::File::open(path)
        .map_err(|e| Diagnostic::plain("APX_READ", format!("{}: {e}", path.display())))?;
    if file
        .metadata()
        .map_err(|e| Diagnostic::plain("APX_READ", e.to_string()))?
        .len()
        > max as u64
    {
        return Err(Diagnostic::plain(
            code,
            format!("{} exceeds {} byte input bound", path.display(), max),
        ));
    }
    let mut bytes = Vec::new();
    file.take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Diagnostic::plain("APX_READ", e.to_string()))?;
    if bytes.len() > max {
        return Err(Diagnostic::plain(
            code,
            "Input grew beyond its bounded read limit",
        ));
    }
    crate::inputs::record_read(path, &bytes).map_err(|e| Diagnostic::plain("APX_INPUT_SNAPSHOT", e.to_string()))?;
    Ok(bytes)
}
pub fn load(root: Option<&str>, inputs: &[String]) -> Result<Loaded, Diagnostic> {
    let cwd = std::env::current_dir().map_err(|e| Diagnostic::plain("APX_PATH", e.to_string()))?;
    let start = if let Some(r) = root {
        PathBuf::from(r)
    } else if let Some(s) = inputs.first().filter(|s| Path::new(s.as_str()).exists()) {
        let p = PathBuf::from(s);
        if p.is_file() {
            p.parent().unwrap_or(&cwd).to_path_buf()
        } else {
            p
        }
    } else {
        cwd.clone()
    };
    crate::inputs::ensure_regular_path(&start).map_err(|e| Diagnostic::plain("APX_INPUT_PATH", e.to_string()))?;
    let mut project = start
        .canonicalize()
        .map_err(|e| Diagnostic::plain("APX_PATH", format!("{}: {e}", start.display())))?;
    if !project.is_dir() {
        return Err(Diagnostic::plain(
            "APX_PATH",
            "Project root must be a directory",
        ));
    }
    if root.is_none() {
        if let Some(p) = project
            .ancestors()
            .find(|p| p.join("sfdx-project.json").is_file())
        {
            project = p.to_path_buf()
        }
    }
    let mut roots = vec![];
    crate::inputs::observe_directory(&project, false).map_err(|e| Diagnostic::plain("APX_INPUT_SNAPSHOT", e.to_string()))?;
    let sf = project.join("sfdx-project.json");
    if sf.exists() {
        let bytes = read_limited(&sf, 1024 * 1024, "APX_CONFIG")?;
        let json: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| Diagnostic::plain("APX_CONFIG", format!("sfdx-project.json: {e}")))?;
        let packages = json
            .get("packageDirectories")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                Diagnostic::plain(
                    "APX_CONFIG",
                    "sfdx-project.json requires packageDirectories array",
                )
            })?;
        for p in packages {
            let name = p.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                Diagnostic::plain("APX_CONFIG", "Each packageDirectory needs a path")
            })?;
            let package_path = crate::inputs::lexical_join(&project, Path::new(name));
            crate::inputs::ensure_regular_path(&package_path).map_err(|e| Diagnostic::plain("APX_INPUT_PATH", e.to_string()))?;
            let path = package_path
                .canonicalize()
                .map_err(|e| Diagnostic::plain("APX_CONFIG", format!("Package {name}: {e}")))?;
            if !path.starts_with(&project) {
                return Err(Diagnostic::plain(
                    "APX_CONFIG",
                    "Package directory outside project is not supported",
                ));
            }
            roots.push(path)
        }
    } else {
        roots.push(project.clone())
    }
    let mut all = BTreeSet::new();
    let mut meta = BTreeSet::new();
    for p in &roots {
        scan(p, &mut all, &mut meta)?
    }
    let mut selected = BTreeSet::new();
    if inputs.is_empty() {
        selected = all.clone()
    } else {
        for input in inputs {
            let mut p = PathBuf::from(input);
            if !p.exists() {
                p = crate::inputs::lexical_join(&project, Path::new(input))
            }
            if p.exists() {
                crate::inputs::ensure_regular_path(&p).map_err(|e| Diagnostic::plain("APX_INPUT_PATH", e.to_string()))?;
                let p = p
                    .canonicalize()
                    .map_err(|e| Diagnostic::plain("APX_PATH", e.to_string()))?;
                if !p.starts_with(&project) {
                    return Err(Diagnostic::plain("APX_PATH", "Selected source is outside the resolved project root; validate separate projects in separate runs"));
                }
                scan(&p, &mut selected, &mut meta)?;
            } else {
                let matched = all
                    .iter()
                    .filter(|p| {
                        p.file_stem()
                            .and_then(|s| s.to_str())
                            .is_some_and(|s| s.eq_ignore_ascii_case(input.trim_end_matches(".cls")))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if matched.is_empty() {
                    return Err(Diagnostic::plain(
                        "APX_PATH",
                        format!("No class or path matches {input}"),
                    ));
                }
                selected.extend(matched);
            }
        }
    }
    all.extend(selected.clone());
    if selected.is_empty() {
        return Err(Diagnostic::plain(
            "APX_EMPTY_SELECTION",
            "No .cls or .trigger files selected",
        ));
    }
    let mut program = Program::default();
    let mut diagnostics = vec![];
    let mut infos = vec![];
    let mut total = 0usize;
    for path in all {
        let bytes = read_limited(&path, 2 * 1024 * 1024, "APX_SOURCE_BUDGET")?;
        total += bytes.len();
        if bytes.len() > 2 * 1024 * 1024 || total > 32 * 1024 * 1024 {
            return Err(Diagnostic::plain(
                "APX_SOURCE_BUDGET",
                "Source exceeds preview bound: 2 MiB per file / 32 MiB project",
            ));
        }
        let src = String::from_utf8(bytes).map_err(|_| {
            Diagnostic::plain("APX_ENCODING", format!("{} is not UTF-8", path.display()))
        })?;
        let name = normalized(&path);
        let companion = PathBuf::from(format!("{}-meta.xml", path.to_string_lossy()));
        if companion.is_file() {
            let bytes = read_limited(&companion, 128 * 1024, "APX_METADATA")?;
            let text = String::from_utf8(bytes).map_err(|_| {
                Diagnostic::plain("APX_METADATA", "Apex companion metadata is not UTF-8")
            })?;
            let doc = roxmltree::Document::parse_with_options(
                &text,
                roxmltree::ParsingOptions {
                    allow_dtd: false,
                    nodes_limit: 1000,
                    entity_resolver: None,
                },
            )
            .map_err(|e| {
                Diagnostic::plain("APX_METADATA", format!("{}: {e}", companion.display()))
            })?;
            if let Some(v) = doc
                .descendants()
                .find(|n| n.has_tag_name("apiVersion"))
                .and_then(|n| n.text())
            {
                let version = v
                    .split('.')
                    .next()
                    .and_then(|s| s.parse::<u32>().ok())
                    .filter(|v| *v > 0 && *v <= 1000)
                    .ok_or_else(|| Diagnostic::plain("APX_METADATA", "Invalid Apex apiVersion"))?;
                program.api_versions.insert(name.clone(), version);
            }
        }
        let classes = match syntax::parse(&src, &name) {
            Ok(tree) => syntax::declarations(&tree, &mut program, &mut diagnostics),
            Err(mut ds) => {
                diagnostics.append(&mut ds);
                vec![]
            }
        };
        infos.push(FileInfo {
            path: name,
            selected: selected.contains(&path),
            classes,
        });
    }
    let metadata_paths = meta.into_iter().collect::<Vec<_>>();
    let mut dw_bytes = 0usize;
    for path in metadata_paths.iter().filter(|p| {
        p.extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("dwl"))
    }) {
        let bytes = read_limited(path, 1024 * 1024, "APX_DATAWEAVE")?;
        dw_bytes += bytes.len();
        if dw_bytes > 16 * 1024 * 1024 {
            return Err(Diagnostic::plain(
                "APX_DATAWEAVE",
                "DataWeave sources exceed16MiB",
            ));
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| Diagnostic::plain("APX_DATAWEAVE", "Invalid DataWeave script name"))?
            .to_ascii_lowercase();
        let source = String::from_utf8(bytes)
            .map_err(|_| Diagnostic::plain("APX_DATAWEAVE", "DataWeave script is not UTF-8"))?;
        if program
            .dataweave_scripts
            .insert(name.clone(), source)
            .is_some()
        {
            return Err(Diagnostic::plain(
                "APX_DATAWEAVE",
                format!("Duplicate DataWeave script {name}"),
            ));
        }
    }
    program.schema = crate::schema::SchemaCatalog::load_files(&metadata_paths)
        .map_err(|e| Diagnostic::plain("APX_METADATA", e.to_string()))?;
    resources::load(&metadata_paths, &mut program.schema)?;
    for class in program
        .classes
        .values()
        .filter(|class| class.parent.is_none())
    {
        let mut values = std::collections::BTreeMap::from([
            (
                "LengthWithoutComments".into(),
                crate::schema::MetadataScalar::Number(
                    resources::source_size_without_comments(
                        &class.declaration.source,
                        &class.declaration.path,
                    )?
                    .to_string(),
                ),
            ),
            (
                "Name".into(),
                crate::schema::MetadataScalar::Text(class.name.clone()),
            ),
            (
                "NamespacePrefix".into(),
                crate::schema::MetadataScalar::Null,
            ),
            (
                "Status".into(),
                crate::schema::MetadataScalar::Text("Active".into()),
            ),
            (
                "Body".into(),
                crate::schema::MetadataScalar::Text(class.declaration.source.to_string()),
            ),
        ]);
        if let Some(version) = program.api_versions.get(class.declaration.path.as_ref()) {
            values.insert(
                "ApiVersion".into(),
                crate::schema::MetadataScalar::Number(version.to_string()),
            );
        }
        program
            .schema
            .metadata_records
            .push(crate::schema::MetadataRecord {
                object: "ApexClass".into(),
                developer_name: class.name.clone(),
                label: class.name.clone(),
                protected: false,
                values,
            });
    }
    // Preserve the old gate until each runtime operation consumes catalog blockers.
    program.automation_blockers = metadata_paths
        .into_iter()
        .take(8)
        .map(|p| normalized(&p))
        .collect();
    Ok(Loaded {
        root: project,
        program,
        files: infos,
        diagnostics,
    })
}
