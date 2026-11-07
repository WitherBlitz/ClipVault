use crate::{
    model::{Kind, Preview, Record},
    store::{Result, Store, protect, unprotect},
};
use bincode::Options;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const MAX_META: usize = 512 * 1024 * 1024;
const CHUNK: usize = 1024 * 1024;
const MAGIC: &[u8; 8] = b"CVXFER01";
static NEXT: AtomicU64 = AtomicU64::new(0);
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EntrySummary {
    pub id: String,
    pub created: u64,
    pub title: String,
    pub detail: String,
    pub kind: Kind,
    pub preview: Option<Preview>,
}
pub fn summaries(store: &Store) -> Vec<EntrySummary> {
    store
        .entries
        .iter()
        .map(|r| EntrySummary {
            id: r.id.clone(),
            created: r.created,
            title: r.title.clone(),
            detail: r.detail.clone(),
            kind: r.kind.clone(),
            preview: r.preview.clone(),
        })
        .collect()
}
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
fn safe_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path.components().all(|c| match c {
            Component::Normal(s) => {
                let n = s.to_string_lossy();
                let base = n.split('.').next().unwrap_or("").to_ascii_uppercase();
                let reserved = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                    || (base.len() == 4
                        && (base.starts_with("COM") || base.starts_with("LPT"))
                        && matches!(base.as_bytes()[3], b'1'..=b'9'));
                !reserved
                    && !n.chars().any(|c| c.is_control() || "<>:\"|?*".contains(c))
                    && !n.ends_with('.')
                    && !n.ends_with(' ')
            }
            _ => false,
        })
}
fn validate(record: &Record, id: &str) -> Result<()> {
    if !safe_id(id) || record.id != id {
        return Err("Invalid entry ID".into());
    }
    let mut format_ids = HashSet::new();
    let mut format_names = HashSet::new();
    for format in &record.formats {
        if !format_ids.insert(format.id) {
            return Err("Duplicate clipboard format".into());
        }
        match format.name.as_deref() {
            None if matches!(format.id, 1 | 7 | 8 | 13 | 16 | 17) => {}
            Some(name)
                if (0xc000..=0xffff).contains(&format.id)
                    && matches!(
                        name,
                        "PNG"
                            | "image/png"
                            | "JFIF"
                            | "image/jpeg"
                            | "GIF"
                            | "image/gif"
                            | "image/webp"
                            | "TIFF"
                            | "HTML Format"
                            | "Rich Text Format"
                            | "Rich Text Format Without Objects"
                    )
                    && format_names.insert(name) => {}
            _ => return Err("Invalid clipboard format".into()),
        }
    }
    if let Some(p) = &record.preview {
        let len = (p.width as usize)
            .checked_mul(p.height as usize)
            .and_then(|n| n.checked_mul(4));
        if p.width == 0
            || p.height == 0
            || p.width > 80
            || p.height > 80
            || len != Some(p.rgba.len())
        {
            return Err("Invalid preview".into());
        }
    }
    let mut paths = HashSet::new();
    let mut blobs = HashSet::new();
    let mut windows_paths = HashSet::new();
    let mut file_paths = HashSet::new();
    if record.files.len() > 10000 || record.roots.len() > 10000 {
        return Err("Too many snapshot paths".into());
    }
    for f in &record.files {
        if !safe_path(&f.relative)
            || !paths.insert(f.relative.clone())
            || !windows_paths.insert(f.relative.to_string_lossy().to_lowercase())
        {
            return Err("Invalid snapshot path".into());
        }
        if let Some(blob) = &f.blob {
            file_paths.insert(f.relative.clone());
            if !safe_id(blob) || !blobs.insert(blob.to_ascii_lowercase()) || f.digest.len() != 32 {
                return Err("Invalid snapshot file".into());
            }
        } else if f.length != 0 || !f.digest.is_empty() {
            return Err("Invalid snapshot directory".into());
        }
    }
    let mut roots = HashSet::new();
    for root in &record.roots {
        if !safe_path(root) || !paths.contains(root) {
            return Err("Invalid snapshot root".into());
        }
        if !roots.insert(root.clone()) {
            return Err("Duplicate snapshot root".into());
        }
    }
    for f in &record.files {
        if !f.relative.ancestors().any(|p| roots.contains(p)) {
            return Err("File outside snapshot roots".into());
        }
        let mut parent = f.relative.parent();
        while let Some(p) = parent {
            if p.as_os_str().is_empty() {
                break;
            }
            if file_paths.contains(p) {
                return Err("File used as directory".into());
            }
            parent = p.parent();
        }
    }
    Ok(())
}
fn read_u32(reader: &mut impl Read) -> Result<usize> {
    let mut b = [0; 4];
    reader.read_exact(&mut b).map_err(error)?;
    Ok(u32::from_le_bytes(b) as usize)
}
fn write_chunk(writer: &mut impl Write, data: &[u8]) -> Result<()> {
    writer
        .write_all(&(data.len() as u32).to_le_bytes())
        .map_err(error)?;
    writer.write_all(data).map_err(error)
}
pub fn write_entry(root: &Path, id: &str, writer: &mut impl Write) -> Result<()> {
    if !safe_id(id) {
        return Err("Invalid entry ID".into());
    }
    let encrypted = fs::read(root.join("items").join(format!("{id}.cv"))).map_err(error)?;
    if encrypted.len() > MAX_META * 2 {
        return Err("Metadata too large".into());
    }
    let meta = unprotect(&encrypted)?;
    if meta.len() > MAX_META {
        return Err("Metadata too large".into());
    }
    let record: Record = bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(MAX_META as u64)
        .reject_trailing_bytes()
        .deserialize(&meta)
        .map_err(error)?;
    validate(&record, id)?;
    writer.write_all(MAGIC).map_err(error)?;
    write_chunk(writer, &meta)?;
    writer.write_all(&Sha256::digest(&meta)).map_err(error)?;
    for f in &record.files {
        if let Some(blob) = &f.blob {
            let mut input = File::open(root.join("items").join(id).join(blob)).map_err(error)?;
            let mut digest = Sha256::new();
            let mut length = 0u64;
            loop {
                let mut first = [0; 1];
                if input.read(&mut first).map_err(error)? == 0 {
                    break;
                }
                let mut size = [0; 4];
                size[0] = first[0];
                input.read_exact(&mut size[1..]).map_err(error)?;
                let n = u32::from_le_bytes(size) as usize;
                if n == 0 || n > CHUNK * 2 {
                    return Err("Invalid encrypted chunk".into());
                }
                let mut data = vec![0; n];
                input.read_exact(&mut data).map_err(error)?;
                let plain = unprotect(&data)?;
                if plain.is_empty() || plain.len() > CHUNK {
                    return Err("Invalid chunk".into());
                }
                length = length
                    .checked_add(plain.len() as u64)
                    .ok_or("File too large")?;
                if length > f.length {
                    return Err("File length mismatch".into());
                }
                digest.update(&plain);
                write_chunk(writer, &plain)?;
            }
            if length != f.length || digest.finalize().as_slice() != f.digest {
                return Err("File integrity check failed".into());
            }
            write_chunk(writer, &[])?;
        }
    }
    Ok(())
}
pub fn read_entry(
    cache_root: &Path,
    expected_id: &str,
    reader: &mut impl Read,
) -> Result<(Record, Vec<PathBuf>)> {
    if !safe_id(expected_id) {
        return Err("Invalid entry ID".into());
    }
    let mut magic = [0; 8];
    reader.read_exact(&mut magic).map_err(error)?;
    if &magic != MAGIC {
        return Err("Invalid transfer version".into());
    }
    let len = read_u32(reader)?;
    if len == 0 || len > MAX_META {
        return Err("Invalid metadata size".into());
    }
    let mut meta = vec![0; len];
    reader.read_exact(&mut meta).map_err(error)?;
    let mut hash = [0; 32];
    reader.read_exact(&mut hash).map_err(error)?;
    if Sha256::digest(&meta).as_slice() != hash {
        return Err("Metadata integrity check failed".into());
    }
    let record: Record = bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(MAX_META as u64)
        .reject_trailing_bytes()
        .deserialize(&meta)
        .map_err(error)?;
    validate(&record, expected_id)?;
    fs::create_dir_all(cache_root.join("items")).map_err(error)?;
    fs::create_dir_all(cache_root.join("staging")).map_err(error)?;
    let tx = loop {
        let p = cache_root.join(format!(
            ".transfer-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&p) {
            Ok(()) => break p,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(error(e)),
        }
    };
    let mut preserve_transaction = false;
    let result = (|| {
        let folder = tx.join("new");
        fs::create_dir(&folder).map_err(error)?;
        for f in &record.files {
            if let Some(blob) = &f.blob {
                let mut output = File::create(folder.join(blob)).map_err(error)?;
                let mut digest = Sha256::new();
                let mut length = 0u64;
                loop {
                    let n = read_u32(reader)?;
                    if n == 0 {
                        break;
                    }
                    if n > CHUNK {
                        return Err("Invalid chunk size".into());
                    }
                    length = length.checked_add(n as u64).ok_or("File too large")?;
                    if length > f.length {
                        return Err("File length mismatch".into());
                    }
                    let mut data = vec![0; n];
                    reader.read_exact(&mut data).map_err(error)?;
                    digest.update(&data);
                    let encrypted = protect(&data)?;
                    output
                        .write_all(&(encrypted.len() as u32).to_le_bytes())
                        .map_err(error)?;
                    output.write_all(&encrypted).map_err(error)?;
                }
                if length != f.length || digest.finalize().as_slice() != f.digest {
                    return Err("File integrity check failed".into());
                }
                output.sync_all().map_err(error)?;
            }
        }
        let new_meta = tx.join("new.cv");
        let mut out = File::create(&new_meta).map_err(error)?;
        out.write_all(&protect(&meta)?).map_err(error)?;
        out.sync_all().map_err(error)?;
        let items = cache_root.join("items");
        let dest = items.join(expected_id);
        let dest_meta = items.join(format!("{expected_id}.cv"));
        let old = tx.join("old");
        let old_meta = tx.join("old.cv");
        let had_dir = dest.exists();
        let had_meta = dest_meta.exists();
        if had_dir {
            fs::rename(&dest, &old).map_err(error)?;
        }
        if had_meta && let Err(e) = fs::rename(&dest_meta, &old_meta) {
            if had_dir {
                preserve_transaction = fs::rename(&old, &dest).is_err();
            }
            return Err(error(e));
        }
        let commit = (|| {
            fs::rename(&folder, &dest).map_err(error)?;
            fs::rename(&new_meta, &dest_meta).map_err(error)?;
            let paths = Store::materialize_record(cache_root.to_path_buf(), record.clone())?;
            Store::trim_cache(cache_root.to_path_buf(), expected_id)?;
            Ok(paths)
        })();
        match commit {
            Ok(paths) => Ok((record, paths)),
            Err(e) => {
                let _ = fs::remove_file(&dest_meta);
                let _ = fs::remove_dir_all(&dest);
                if had_dir {
                    preserve_transaction |= fs::rename(&old, &dest).is_err();
                }
                if had_meta {
                    preserve_transaction |= fs::rename(&old_meta, &dest_meta).is_err();
                }
                Err(e)
            }
        }
    })();
    if !preserve_transaction {
        let _ = fs::remove_dir_all(&tx);
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    fn packet(record: &Record) -> Vec<u8> {
        let meta = bincode::serialize(record).unwrap();
        let mut bytes = MAGIC.to_vec();
        write_chunk(&mut bytes, &meta).unwrap();
        bytes.extend_from_slice(&Sha256::digest(&meta));
        bytes
    }
    #[test]
    fn import_rejects_untrusted_clipboard_formats() {
        use crate::model::Format;
        let dir = tempfile::tempdir().unwrap();
        let format = |id, name: Option<&str>| Format {
            id,
            name: name.map(str::to_owned),
            data: vec![1],
        };
        let cases = vec![
            vec![format(15, None)],
            vec![format(2, None)],
            vec![format(0x200, None)],
            vec![format(0x10000, None)],
            vec![format(0x10000, Some("PNG"))],
            vec![format(42, Some("PNG"))],
            vec![format(0xc000, Some("PNG\0evil"))],
            vec![format(0xc000, Some("Unknown"))],
            vec![format(13, None), format(13, None)],
            vec![format(0xc000, Some("PNG")), format(0xc001, Some("PNG"))],
            vec![format(0xc000, Some("PNG")), format(0xc000, Some("JFIF"))],
        ];
        for formats in cases {
            let mut record = Record::text("untrusted");
            record.id = "a".into();
            record.formats = formats;
            assert!(
                read_entry(
                    &dir.path().join("cache"),
                    "a",
                    &mut packet(&record).as_slice()
                )
                .is_err(),
                "accepted {:?}",
                record.formats
            );
        }
    }
    #[test]
    fn import_accepts_all_captured_clipboard_formats() {
        let dir = tempfile::tempdir().unwrap();
        let mut record = Record::text("supported");
        record.id = "a".into();
        for id in [1, 7, 8, 13, 16, 17] {
            record.formats.push(crate::model::Format {
                id,
                name: None,
                data: vec![1],
            });
        }
        for (index, name) in [
            "PNG",
            "image/png",
            "JFIF",
            "image/jpeg",
            "GIF",
            "image/gif",
            "image/webp",
            "TIFF",
            "HTML Format",
            "Rich Text Format",
            "Rich Text Format Without Objects",
        ]
        .iter()
        .enumerate()
        {
            record.formats.push(crate::model::Format {
                id: 0xc000 + index as u32,
                name: Some((*name).into()),
                data: vec![2],
            });
        }
        let (received, _) = read_entry(
            &dir.path().join("cache"),
            "a",
            &mut packet(&record).as_slice(),
        )
        .unwrap();
        assert_eq!(received.formats.len(), 17);
    }
    #[test]
    fn text_roundtrip_preserves_source_record() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("source")).unwrap();
        store.insert(Record::text("hello"), &[]).unwrap();
        let id = store.entries[0].id.clone();
        let mut bytes = Vec::new();
        write_entry(store.root(), &id, &mut bytes).unwrap();
        let (received, paths) =
            read_entry(&dir.path().join("cache"), &id, &mut bytes.as_slice()).unwrap();
        assert_eq!(received.id, id);
        assert_eq!(received.title, "hello");
        assert!(paths.is_empty());
    }
    #[test]
    fn files_roundtrip_after_source_deleted_and_rejects_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("folder");
        std::fs::create_dir_all(source.join("empty")).unwrap();
        std::fs::write(source.join("binary"), [0, 255, 4]).unwrap();
        let mut store = Store::open(dir.path().join("source")).unwrap();
        let mut record = Record::text("files");
        record.kind = Kind::Files;
        store.insert(record, std::slice::from_ref(&source)).unwrap();
        let id = store.entries[0].id.clone();
        std::fs::remove_dir_all(source).unwrap();
        let mut bytes = Vec::new();
        write_entry(store.root(), &id, &mut bytes).unwrap();
        let cache = dir.path().join("cache");
        let (_, paths) = read_entry(&cache, &id, &mut bytes.as_slice()).unwrap();
        assert_eq!(std::fs::read(paths[0].join("binary")).unwrap(), [0, 255, 4]);
        assert!(paths[0].join("empty").is_dir());
        assert!(read_entry(&cache, &id, &mut &bytes[..bytes.len() - 1]).is_err());
        let mut damaged = bytes.clone();
        let pos = damaged.len() - 5;
        damaged[pos] ^= 1;
        assert!(read_entry(&cache, &id, &mut damaged.as_slice()).is_err());
        let paths = Store::open(cache).unwrap().materialize(&id).unwrap();
        assert_eq!(std::fs::read(paths[0].join("binary")).unwrap(), [0, 255, 4]);
    }
    #[test]
    fn image_roundtrip_preserves_formats_and_preview() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("source")).unwrap();
        let mut record = Record::text("image");
        record.kind = Kind::Image;
        record.formats.push(crate::model::Format {
            id: 0xc123,
            name: Some("PNG".into()),
            data: vec![0, 1, 255],
        });
        record.preview = Some(Preview {
            width: 1,
            height: 1,
            rgba: vec![1, 2, 3, 4],
        });
        store.insert(record, &[]).unwrap();
        let id = store.entries[0].id.clone();
        let mut bytes = Vec::new();
        write_entry(store.root(), &id, &mut bytes).unwrap();
        let (result, _) =
            read_entry(&dir.path().join("cache"), &id, &mut bytes.as_slice()).unwrap();
        assert_eq!(result.formats[0].data, [0, 1, 255]);
        assert_eq!(result.formats[0].id, 0xc123);
        assert_eq!(result.formats[0].name.as_deref(), Some("PNG"));
        assert_eq!(result.preview.unwrap().rgba, [1, 2, 3, 4]);
    }
    #[test]
    fn large_multiformat_image_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("source")).unwrap();
        let mut record = Record::text("large image");
        record.kind = Kind::Image;
        for (id, name, byte) in [(8, None, 1), (17, None, 2), (0xc123, Some("PNG".into()), 3)] {
            record.formats.push(crate::model::Format {
                id,
                name,
                data: vec![byte; 12 * 1024 * 1024],
            });
        }
        store.insert(record, &[]).unwrap();
        let id = store.entries[0].id.clone();
        let mut bytes = Vec::new();
        write_entry(store.root(), &id, &mut bytes).unwrap();
        let (received, _) =
            read_entry(&dir.path().join("cache"), &id, &mut bytes.as_slice()).unwrap();
        assert_eq!(received.formats.len(), 3);
        for (index, format) in received.formats.iter().enumerate() {
            assert_eq!(format.data, vec![(index + 1) as u8; 12 * 1024 * 1024]);
        }
    }
    #[test]
    fn rejects_oversized_preview_dimensions() {
        let mut record = Record::text("image");
        record.id = "a".into();
        record.preview = Some(Preview {
            width: 81,
            height: 1,
            rgba: vec![0; 81 * 4],
        });
        assert!(validate(&record, "a").is_err());
    }
    #[test]
    fn rejects_windows_device_names_and_traversal() {
        for path in ["0/CON.txt", "../escape", "0/file:stream"] {
            let mut record = Record::text("malicious");
            record.id = "a".into();
            record.roots.push(PathBuf::from(path));
            record.files.push(crate::model::StoredFile {
                relative: PathBuf::from(path),
                blob: None,
                length: 0,
                digest: vec![],
            });
            assert!(validate(&record, "a").is_err(), "accepted {path}");
        }
    }
    #[test]
    fn cache_retains_selected_old_entry_at_capacity() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let cache = dir.path().join("cache");
        let mut store = Store::open(source.clone()).unwrap();
        store.insert(Record::text("selected"), &[]).unwrap();
        let id = store.entries[0].id.clone();
        let mut bytes = Vec::new();
        write_entry(&source, &id, &mut bytes).unwrap();
        fs::create_dir_all(cache.join("items")).unwrap();
        for n in 0..100 {
            let mut r = Record::text("cached");
            r.id = format!("f{n:031x}");
            fs::write(
                cache.join("items").join(format!("{}.cv", r.id)),
                protect(&bincode::serialize(&r).unwrap()).unwrap(),
            )
            .unwrap();
        }
        read_entry(&cache, &id, &mut bytes.as_slice()).unwrap();
        let reopened = Store::open(cache).unwrap();
        assert_eq!(reopened.entries.len(), 100);
        assert!(reopened.entries.iter().any(|r| r.id == id));
    }
}
