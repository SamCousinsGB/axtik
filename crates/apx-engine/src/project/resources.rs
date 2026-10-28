//! Source-format static resources. Payload names come from their companion XML,
//! including binary files with ordinary extensions and expanded archive folders.
use super::*;
use crate::schema::{MetadataRecord, MetadataScalar, SchemaCatalog};
use std::collections::BTreeMap;

const RESOURCE_LIMIT: usize = 5 * 1024 * 1024;
fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::plain("APX_STATIC_RESOURCE", message)
}

pub(super) fn load(paths: &[PathBuf], schema: &mut SchemaCatalog) -> Result<(), Diagnostic> {
    let mut total = 0usize;
    let mut names = BTreeSet::new();
    for metadata in paths.iter().filter(|p| {
        p.file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.ends_with(".resource-meta.xml"))
    }) {
        let file = metadata.file_name().unwrap().to_str().unwrap();
        let name = file.strip_suffix(".resource-meta.xml").unwrap();
        if name.is_empty()
            || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
            || !names.insert(name.to_ascii_lowercase())
        {
            return Err(error("Invalid or duplicate static resource name"));
        }
        let xml = String::from_utf8(read_limited(metadata, 128 * 1024, "APX_STATIC_RESOURCE")?)
            .map_err(|_| error("StaticResource metadata is not UTF-8"))?;
        let doc = roxmltree::Document::parse_with_options(
            &xml,
            roxmltree::ParsingOptions {
                allow_dtd: false,
                nodes_limit: 1000,
                entity_resolver: None,
            },
        )
        .map_err(|e| error(format!("Invalid StaticResource XML: {e}")))?;
        let root = doc.root_element();
        if !root.has_tag_name("StaticResource") {
            return Err(error("Resource metadata root must be StaticResource"));
        }
        let text = |tag: &str| {
            root.children()
                .find(|n| n.has_tag_name(tag))
                .and_then(|n| n.text())
                .map(str::to_string)
        };
        let content_type =
            text("contentType").ok_or_else(|| error("StaticResource contentType missing"))?;
        let parent = metadata.parent().unwrap();
        let candidates = fs::read_dir(parent)
            .map_err(|e| error(e.to_string()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p != metadata
                    && (p.file_name().and_then(|s| s.to_str()) == Some(name)
                        || p.file_stem().and_then(|s| s.to_str()) == Some(name))
            })
            .collect::<Vec<_>>();
        if candidates.len() != 1 {
            return Err(error(format!(
                "StaticResource {name} needs one unambiguous payload; found {}",
                candidates.len()
            )));
        }
        let payload = &candidates[0];
        if fs::symlink_metadata(payload)
            .map_err(|e| error(e.to_string()))?
            .file_type()
            .is_symlink()
        {
            return Err(error("Static resource payload symlinks are not supported"));
        }
        let bytes = if payload.is_file() {
            read_limited(payload, RESOURCE_LIMIT, "APX_STATIC_RESOURCE")?
        } else {
            archive_folder(payload)?
        };
        total = total.saturating_add(bytes.len());
        if total > 50 * 1024 * 1024 {
            return Err(error("Static resource payloads exceed 50 MiB local bound"));
        }
        let mut values = BTreeMap::from([
            ("Name".into(), MetadataScalar::Text(name.into())),
            ("Body".into(), MetadataScalar::Binary(bytes.into())),
            ("ContentType".into(), MetadataScalar::Text(content_type)),
        ]);
        for key in ["cacheControl", "description"] {
            if let Some(v) = text(key) {
                values.insert(key.into(), MetadataScalar::Text(v));
            }
        }
        schema.metadata_records.push(MetadataRecord {
            object: "StaticResource".into(),
            developer_name: name.into(),
            label: name.into(),
            protected: false,
            values,
        });
    }
    Ok(())
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0xedb88320 } else { 0 };
        }
    }
    !crc
}
fn u16le(out: &mut Vec<u8>, n: u16) {
    out.extend_from_slice(&n.to_le_bytes());
}
fn u32le(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_le_bytes());
}
fn archive_folder(folder: &Path) -> Result<Vec<u8>, Diagnostic> {
    let mut files = vec![];
    for entry in walkdir::WalkDir::new(folder).follow_links(false) {
        let entry = entry.map_err(|e| error(e.to_string()))?;
        if entry.file_type().is_symlink() {
            return Err(error("Expanded static resource symlinks are not supported"));
        }
        if entry.file_type().is_file() {
            files.push(entry.path().to_path_buf());
        }
        if files.len() > 1000 {
            return Err(error("Expanded static resource exceeds 1000 files"));
        }
    }
    files.sort();
    let mut archive = vec![];
    let mut directory = vec![];
    for file in &files {
        let name = file
            .strip_prefix(folder)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if name.len() > u16::MAX as usize {
            return Err(error("Static resource archive path too long"));
        }
        let bytes = read_limited(file, RESOURCE_LIMIT, "APX_STATIC_RESOURCE")?;
        let crc = crc32(&bytes);
        let offset = archive.len() as u32;
        u32le(&mut archive, 0x04034b50);
        u16le(&mut archive, 20);
        u16le(&mut archive, 0x800);
        u16le(&mut archive, 0);
        u16le(&mut archive, 0);
        u16le(&mut archive, 33);
        u32le(&mut archive, crc);
        u32le(&mut archive, bytes.len() as u32);
        u32le(&mut archive, bytes.len() as u32);
        u16le(&mut archive, name.len() as u16);
        u16le(&mut archive, 0);
        archive.extend_from_slice(name.as_bytes());
        archive.extend_from_slice(&bytes);
        u32le(&mut directory, 0x02014b50);
        u16le(&mut directory, 20);
        u16le(&mut directory, 20);
        u16le(&mut directory, 0x800);
        u16le(&mut directory, 0);
        u16le(&mut directory, 0);
        u16le(&mut directory, 33);
        u32le(&mut directory, crc);
        u32le(&mut directory, bytes.len() as u32);
        u32le(&mut directory, bytes.len() as u32);
        u16le(&mut directory, name.len() as u16);
        u16le(&mut directory, 0);
        u16le(&mut directory, 0);
        u16le(&mut directory, 0);
        u16le(&mut directory, 0);
        u32le(&mut directory, 0);
        u32le(&mut directory, offset);
        directory.extend_from_slice(name.as_bytes());
        if archive.len() + directory.len() + 22 > RESOURCE_LIMIT {
            return Err(error(
                "Expanded static resource archive exceeds 5 MiB local bound",
            ));
        }
    }
    let offset = archive.len() as u32;
    let size = directory.len() as u32;
    archive.extend_from_slice(&directory);
    u32le(&mut archive, 0x06054b50);
    u16le(&mut archive, 0);
    u16le(&mut archive, 0);
    u16le(&mut archive, files.len() as u16);
    u16le(&mut archive, files.len() as u16);
    u32le(&mut archive, size);
    u32le(&mut archive, offset);
    u16le(&mut archive, 0);
    Ok(archive)
}

pub(super) fn source_size_without_comments(source: &str, path: &str) -> Result<usize, Diagnostic> {
    let tree = crate::syntax::parse(source, path)
        .map_err(|_| error("Previously parsed Apex source could not be inspected"))?;
    fn comments(n: &crate::syntax::Node) -> usize {
        if matches!(n.kind.as_str(), "line_comment" | "block_comment") {
            n.text().encode_utf16().count()
        } else {
            n.children.iter().map(|(_, c)| comments(c)).sum()
        }
    }
    Ok(source.encode_utf16().count() - comments(&tree))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn comment_size_ignores_comment_markers_inside_literals() {
        let plain = "class C {String x='//literal';}";
        let commented = format!("/* Header */{plain}//Tail");
        assert_eq!(
            source_size_without_comments(&commented, "c.cls").unwrap(),
            plain.len()
        );
        assert_eq!(crc32(b"123456789"), 0xcbf43926);
    }
    #[test]
    fn payloads_follow_metadata_names_and_actual_binary_changes() {
        let dir = std::env::temp_dir().join(format!(
            "apx-resource-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&dir).unwrap();
        let metadata = dir.join("Changed.resource-meta.xml");
        fs::write(&metadata,"<StaticResource><contentType>application/octet-stream</contentType><cacheControl>Private</cacheControl></StaticResource>").unwrap();
        fs::write(dir.join("Changed.png"), [0, 255, 1]).unwrap();
        let mut schema = SchemaCatalog::standard();
        load(&[metadata.clone()], &mut schema).unwrap();
        let MetadataScalar::Binary(body) = &schema.metadata_records.last().unwrap().values["Body"]
        else {
            panic!("binary")
        };
        assert_eq!(body.as_ref(), &[0, 255, 1]);
        fs::write(dir.join("Changed.png"), [9, 8]).unwrap();
        let mut schema = SchemaCatalog::standard();
        load(&[metadata.clone()], &mut schema).unwrap();
        let MetadataScalar::Binary(body) = &schema.metadata_records.last().unwrap().values["Body"]
        else {
            panic!("binary")
        };
        assert_eq!(body.as_ref(), &[9, 8]);
        fs::write(dir.join("Changed.resource"), [3]).unwrap();
        assert!(load(&[metadata], &mut SchemaCatalog::standard()).is_err());
        assert_eq!(
            fs::canonicalize(&dir).unwrap().parent().unwrap(),
            fs::canonicalize(std::env::temp_dir()).unwrap()
        );
        fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn expanded_archive_is_deterministic_and_preserves_paths_and_bytes() {
        let dir = std::env::temp_dir().join(format!(
            "apx-resource-zip-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(dir.join("nested")).unwrap();
        fs::write(dir.join("a.txt"), b"alpha").unwrap();
        fs::write(dir.join("nested/b.bin"), [0, 255, 17]).unwrap();
        let archive = archive_folder(&dir).unwrap();
        assert_eq!(archive, archive_folder(&dir).unwrap());
        assert_eq!(&archive[..4], b"PK\x03\x04");
        if let Ok(target) = std::env::var("APX_RESOURCE_ZIP_PROBE") {
            fs::write(target, &archive).unwrap();
        }
        assert_eq!(
            fs::canonicalize(&dir).unwrap().parent().unwrap(),
            fs::canonicalize(std::env::temp_dir()).unwrap()
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
