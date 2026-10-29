//! Content identity of bytes actually consumed by one evaluator run.
//! Readers record the bytes they parse, not a separate pre-read hash. Directory
//! inventories precede project scanning; final verification establishes that all
//! consumed bytes and input names still describe one filesystem state.
use crate::model::{InputDigest, InputDirectoryDigest};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

const MAX_BYTES: usize = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;
const IGNORED: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".sf",
    ".sfdx",
    ".apx",
    "dist",
];

#[derive(Default)]
struct Capture {
    files: BTreeMap<String, (String, usize)>,
    directories: BTreeMap<String, (String, bool)>,
    bytes: usize,
    entries: usize,
    changed: bool,
}
thread_local! { static ACTIVE: RefCell<Option<Capture>> = const { RefCell::new(None) }; }

pub struct Session {
    previous: Option<Capture>,
}
pub struct Manifest {
    pub files: Vec<InputDigest>,
    pub directories: Vec<InputDirectoryDigest>,
    pub fingerprint: String,
    pub changed: Option<String>,
}

/// Unlike PathBuf::push on a Windows verbatim base, retain `..` until each
/// preceding component has been checked for aliases by ensure_regular_path.
pub fn lexical_join(base: &Path, child: &Path) -> PathBuf {
    #[cfg(windows)]
    let native_child = {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        std::ffi::OsString::from_wide(
            &child
                .as_os_str()
                .encode_wide()
                .map(|c| {
                    if c == u16::from(b'/') {
                        u16::from(b'\\')
                    } else {
                        c
                    }
                })
                .collect::<Vec<_>>(),
        )
    };
    #[cfg(windows)]
    let child = Path::new(&native_child);
    if child.is_absolute()
        || matches!(
            child.components().next(),
            Some(std::path::Component::Prefix(_))
        )
    {
        return child.to_path_buf();
    }
    if child.has_root() {
        let mut result = std::ffi::OsString::new();
        if let Some(std::path::Component::Prefix(prefix)) = base.components().next() {
            result.push(prefix.as_os_str());
        }
        result.push(child.as_os_str());
        return result.into();
    }
    let mut result = base.as_os_str().to_owned();
    result.push(std::path::MAIN_SEPARATOR.to_string());
    result.push(child.as_os_str());
    result.into()
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// Snapshot identities do not model aliases. Reject them before opening or
/// canonicalizing a consumed path, including links in any ancestor component.
/// This also rejects Windows junctions and other reparse points; they must not
/// silently become an ordinary canonical target in the input manifest.
pub fn ensure_regular_path(path: &Path) -> io::Result<()> {
    // Do not normalize `..` before inspecting preceding components: a path
    // through `link/../file` must not hide the link during normalization.
    if !path.is_absolute()
        && matches!(
            path.components().next(),
            Some(std::path::Component::Prefix(_))
        )
    {
        return Err(invalid(
            "Drive-relative consumed input paths are unsupported",
        ));
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        lexical_join(&std::env::current_dir()?, path)
    };
    let mut component_path = PathBuf::new();
    for component in absolute.components() {
        component_path.push(component.as_os_str());
        if matches!(component, std::path::Component::Prefix(_)) {
            continue;
        }
        let metadata = fs::symlink_metadata(&component_path)?;
        let alias = metadata.file_type().is_symlink();
        #[cfg(windows)]
        let alias = {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
            alias || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        };
        if alias {
            return Err(invalid(format!(
                "Symbolic links and reparse points are unsupported consumed inputs: {}",
                component_path.display()
            )));
        }
        if !metadata.is_file() && !metadata.is_dir() {
            return Err(invalid(format!(
                "Consumed inputs must be regular files or directories: {}",
                component_path.display()
            )));
        }
    }
    Ok(())
}

pub fn fingerprint(files: &[InputDigest], directories: &[InputDirectoryDigest]) -> String {
    #[derive(Serialize)]
    struct Identity<'a> {
        files: &'a [InputDigest],
        directories: &'a [InputDirectoryDigest],
    }
    hash(
        &serde_json::to_vec(&Identity { files, directories })
            .expect("input identity serialization"),
    )
}

#[derive(Serialize)]
struct Entry {
    name: String,
    kind: &'static str,
}
fn directory(path: &Path, include_ignored: bool) -> io::Result<(String, Vec<PathBuf>, usize)> {
    ensure_regular_path(path)?;
    let mut entries = Vec::new();
    let mut children = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid("Input filename is not Unicode"))?;
        if !include_ignored && IGNORED.contains(&name.as_str()) {
            continue;
        }
        if entries.len() >= MAX_ENTRIES {
            return Err(invalid("Input directory exceeds 100000 entries"));
        }
        let typ = entry.file_type()?;
        let kind = if typ.is_symlink() {
            "symlink"
        } else if typ.is_dir() {
            children.push(entry.path());
            "directory"
        } else if typ.is_file() {
            "file"
        } else {
            "other"
        };
        entries.push(Entry { name, kind });
    }
    entries.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
    Ok((
        hash(&serde_json::to_vec(&entries).expect("directory identity serialization")),
        children,
        entries.len(),
    ))
}

pub fn record_read(path: &Path, bytes: &[u8]) -> io::Result<()> {
    ensure_regular_path(path)?;
    ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        let Some(capture) = active.as_mut() else {
            return Ok(());
        };
        let path = crate::project::normalized(&path.canonicalize()?);
        let digest = hash(bytes);
        if let Some((before, _)) = capture.files.get(&path) {
            if before != &digest {
                capture.changed = true;
            }
        } else {
            capture.bytes = capture
                .bytes
                .checked_add(bytes.len())
                .ok_or_else(|| invalid("Input byte budget overflow"))?;
            if capture.bytes > MAX_BYTES {
                return Err(invalid("Consumed inputs exceed 256 MiB"));
            }
            capture.files.insert(path, (digest, bytes.len()));
        }
        Ok(())
    })
}

/// Record shallow names before scanning; recurse only through real directories.
pub fn observe_directory(path: &Path, recursive: bool) -> io::Result<()> {
    observe(path, recursive, false)
}
pub fn observe_resource_directory(path: &Path) -> io::Result<()> {
    observe(path, true, true)
}
fn observe(path: &Path, recursive: bool, include_ignored: bool) -> io::Result<()> {
    if !ACTIVE.with(|a| a.borrow().is_some()) {
        return Ok(());
    }
    let mut todo = vec![path.to_path_buf()];
    while let Some(path) = todo.pop() {
        ensure_regular_path(&path)?;
        let normalized = crate::project::normalized(&path.canonicalize()?);
        if !recursive
            && ACTIVE.with(|a| {
                a.borrow()
                    .as_ref()
                    .is_some_and(|c| c.directories.contains_key(&normalized))
            })
        {
            continue;
        }
        let (digest, children, count) = directory(&path, include_ignored)?;
        ACTIVE.with(|a| -> io::Result<()> {
            let mut a = a.borrow_mut();
            let capture = a.as_mut().expect("active input capture");
            if let Some((expected, previous_policy)) = capture.directories.get(&normalized).cloned()
            {
                if previous_policy == include_ignored {
                    if expected != digest {
                        capture.changed = true;
                    }
                } else if include_ignored {
                    if !directory(&path, false).is_ok_and(|(now, _, _)| now == expected) {
                        capture.changed = true;
                    }
                    capture.entries = capture
                        .entries
                        .checked_add(count)
                        .ok_or_else(|| invalid("Input entry budget overflow"))?;
                    if capture.entries > MAX_ENTRIES {
                        return Err(invalid("Input directory inventories exceed 100000 entries"));
                    }
                    capture.directories.insert(normalized, (digest, true));
                }
            } else {
                capture.entries = capture
                    .entries
                    .checked_add(count)
                    .ok_or_else(|| invalid("Input entry budget overflow"))?;
                if capture.entries > MAX_ENTRIES {
                    return Err(invalid("Input directory inventories exceed 100000 entries"));
                }
                capture
                    .directories
                    .insert(normalized, (digest, include_ignored));
            }
            Ok(())
        })?;
        if recursive {
            todo.extend(children);
        }
    }
    Ok(())
}

impl Session {
    pub fn begin() -> Self {
        Self {
            previous: ACTIVE.with(|a| a.replace(Some(Capture::default()))),
        }
    }
    pub fn finish(&self) -> Manifest {
        ACTIVE.with(|a| {
            let a = a.borrow();
            let capture = a.as_ref().expect("active input capture");
            let files = capture
                .files
                .iter()
                .map(|(path, (sha256, _))| InputDigest {
                    path: path.clone(),
                    sha256: sha256.clone(),
                })
                .collect::<Vec<_>>();
            let directories = capture
                .directories
                .iter()
                .map(|(path, (sha256, include_ignored))| InputDirectoryDigest {
                    path: path.clone(),
                    sha256: sha256.clone(),
                    include_ignored: *include_ignored,
                })
                .collect::<Vec<_>>();
            let mut changed = capture
                .changed
                .then(|| "An input was read with different contents during the run".to_string());
            for (path, (expected, size)) in &capture.files {
                if changed.is_some() {
                    break;
                }
                let checked = (|| -> io::Result<bool> {
                    ensure_regular_path(Path::new(path))?;
                    let mut bytes = Vec::new();
                    fs::File::open(path)?
                        .take(*size as u64 + 1)
                        .read_to_end(&mut bytes)?;
                    Ok(bytes.len() == *size && hash(&bytes) == *expected)
                })();
                if !matches!(checked, Ok(true)) {
                    changed.get_or_insert_with(|| {
                        format!("Consumed input changed or disappeared: {path}")
                    });
                }
            }
            for (path, (expected, include_ignored)) in &capture.directories {
                if changed.is_some() {
                    break;
                }
                if !directory(Path::new(path), *include_ignored)
                    .is_ok_and(|(actual, _, _)| actual == *expected)
                {
                    changed.get_or_insert_with(|| {
                        format!("Input directory changed or disappeared: {path}")
                    });
                }
            }
            let fingerprint = fingerprint(&files, &directories);
            Manifest {
                files,
                directories,
                fingerprint,
                changed,
            }
        })
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        ACTIVE.with(|a| {
            a.replace(self.previous.take());
        });
    }
}
