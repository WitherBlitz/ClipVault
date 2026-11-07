use crate::model::{Record, StoredFile};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashSet, VecDeque},
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    ptr,
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{Foundation::LocalFree, Security::Cryptography::*};

pub type Result<T> = std::result::Result<T, String>;
pub struct Store {
    pub entries: VecDeque<Record>,
    pub warnings: Vec<String>,
    root: PathBuf,
}
impl Store {
    pub fn open(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(root.join("items")).map_err(err)?;
        fs::create_dir_all(root.join("staging")).map_err(err)?;
        let mut store = Self {
            entries: VecDeque::new(),
            warnings: vec![],
            root,
        };
        for item in fs::read_dir(store.root.join("items")).map_err(err)? {
            let path = item.map_err(err)?.path();
            if path.extension().is_some_and(|v| v == "cv") {
                let result = fs::read(&path)
                    .map_err(err)
                    .and_then(|b| unprotect(&b))
                    .and_then(|b| bincode::deserialize::<Record>(&b).map_err(err));
                match result {
                    Ok(record)
                        if path.file_stem().is_some_and(|v| v == record.id.as_str())
                            && safe_id(&record.id) =>
                    {
                        store.entries.push_back(record)
                    }
                    _ => store
                        .warnings
                        .push("An unreadable history entry was skipped (kept on disk).".into()),
                }
            }
        }
        store
            .entries
            .make_contiguous()
            .sort_by(|a, b| b.id.cmp(&a.id));
        store.trim()?;
        let mut kept: HashSet<_> = store.entries.iter().map(|v| v.id.clone()).collect();
        // An unreadable metadata file may still have recoverable encrypted payloads.
        for item in fs::read_dir(store.root.join("items")).map_err(err)? {
            let path = item.map_err(err)?.path();
            if path.extension().is_some_and(|v| v == "cv") {
                kept.insert(path.file_stem().unwrap().to_string_lossy().into_owned());
            }
        }
        for item in fs::read_dir(store.root.join("items")).map_err(err)? {
            let path = item.map_err(err)?.path();
            if path.is_dir()
                && !kept.contains(&path.file_name().unwrap().to_string_lossy().into_owned())
            {
                fs::remove_dir_all(path).map_err(err)?;
            } else if path.extension().is_some_and(|v| v == "tmp") {
                fs::remove_file(path).map_err(err)?;
            }
        }
        for item in fs::read_dir(store.root.join("staging")).map_err(err)? {
            let path = item.map_err(err)?.path();
            if path.is_dir()
                && !kept.contains(&path.file_name().unwrap().to_string_lossy().into_owned())
            {
                fs::remove_dir_all(path).map_err(err)?;
            }
        }
        Ok(store)
    }
    pub fn insert(&mut self, mut record: Record, sources: &[PathBuf]) -> Result<bool> {
        if sources.is_empty()
            && self
                .entries
                .front()
                .is_some_and(|v| v.fingerprint == record.fingerprint)
        {
            return Ok(false);
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(err)?;
        record.created = now.as_secs();
        record.id = format!("{:032x}", now.as_nanos());
        let folder = self.root.join("items").join(&record.id);
        let result = (|| {
            if !sources.is_empty() {
                fs::create_dir(&folder).map_err(err)?;
                for (index, source) in sources.iter().enumerate() {
                    let name = source.file_name().ok_or("Cannot snapshot a drive root")?;
                    let relative = PathBuf::from(index.to_string()).join(name);
                    record.roots.push(relative.clone());
                    snapshot(source, &relative, &folder, &mut record.files)?;
                }
            }
            let encrypted = protect(&bincode::serialize(&record).map_err(err)?)?;
            let tmp = self.root.join("items").join(format!("{}.tmp", record.id));
            let mut file = File::create(&tmp).map_err(err)?;
            file.write_all(&encrypted).map_err(err)?;
            file.sync_all().map_err(err)?;
            fs::rename(
                tmp,
                self.root.join("items").join(format!("{}.cv", record.id)),
            )
            .map_err(err)?;
            Ok(())
        })();
        if let Err(e) = result {
            let _ = fs::remove_dir_all(folder);
            return Err(e);
        }
        self.entries.push_front(record);
        if let Err(error) = self.trim() {
            let new = self.entries.pop_front().unwrap();
            self.remove_payload(&new.id)?;
            return Err(error);
        }
        Ok(true)
    }
    fn trim(&mut self) -> Result<()> {
        while self.entries.len() > 100 {
            let record = self.entries.back().unwrap();
            self.remove_payload(&record.id)?;
            self.entries.pop_back();
        }
        Ok(())
    }
    fn remove_payload(&self, id: &str) -> Result<()> {
        let staging = self.root.join("staging").join(id);
        if staging.exists() {
            crate::clipboard::release_staged_paths(&staging)?;
        }
        // Delete staging first: if a consumer has a file open, preserve the
        // encrypted source and metadata and report the failure.
        for path in [staging, self.root.join("items").join(id)] {
            if path.exists() {
                fs::remove_dir_all(path).map_err(err)?;
            }
        }
        let path = self.root.join("items").join(format!("{id}.cv"));
        if path.exists() {
            fs::remove_file(path).map_err(err)?;
        }
        Ok(())
    }
    pub fn clear(&mut self) -> Result<()> {
        while let Some(record) = self.entries.back() {
            self.remove_payload(&record.id)?;
            self.entries.pop_back();
        }
        Ok(())
    }
    pub fn materialize(&self, id: &str) -> Result<Vec<PathBuf>> {
        let record = self
            .entries
            .iter()
            .find(|r| r.id == id)
            .ok_or("History entry no longer exists")?;
        let output = self.root.join("staging").join(id);
        let ready = output.join(".ready");
        if record.roots.is_empty() {
            return Ok(vec![]);
        }
        let intact = ready.exists()
            && record.files.iter().all(|file| {
                if !safe_relative(&file.relative) {
                    return false;
                }
                let path = output.join(&file.relative);
                if file.blob.is_none() {
                    path.is_dir()
                } else {
                    file_digest(&path)
                        .is_ok_and(|(len, hash)| len == file.length && hash == file.digest)
                }
            });
        if !intact {
            if output.exists() {
                fs::remove_dir_all(&output).map_err(err)?;
            }
            fs::create_dir_all(&output).map_err(err)?;
            for file in &record.files {
                if !safe_relative(&file.relative) {
                    return Err("Invalid snapshot path".into());
                }
                let target = output.join(&file.relative);
                if let Some(blob) = &file.blob {
                    if !safe_id(blob) {
                        return Err("Invalid snapshot blob".into());
                    }
                    fs::create_dir_all(target.parent().unwrap()).map_err(err)?;
                    let (length, digest) =
                        decrypt_file(&self.root.join("items").join(id).join(blob), &target)?;
                    if length != file.length || digest != file.digest {
                        let _ = fs::remove_file(&target);
                        return Err("File snapshot integrity check failed".into());
                    }
                } else {
                    fs::create_dir_all(target).map_err(err)?;
                }
            }
            File::create(ready).map_err(err)?;
        }
        record
            .roots
            .iter()
            .map(|p| {
                if safe_relative(p) {
                    Ok(output.join(p))
                } else {
                    Err("Invalid snapshot root".into())
                }
            })
            .collect()
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|v| v.is_ascii_hexdigit())
}
fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.components().all(|v| matches!(v, Component::Normal(_)))
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn snapshot(
    source: &Path,
    relative: &Path,
    folder: &Path,
    files: &mut Vec<StoredFile>,
) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    let metadata = fs::symlink_metadata(source)
        .map_err(|e| format!("Cannot read {}: {e}", source.display()))?;
    // Do not follow junctions or symlinks into unrelated directories or cycles.
    if metadata.file_attributes() & 0x400 != 0 {
        return Err(format!(
            "Cannot snapshot a link or junction: {}",
            source.display()
        ));
    }
    if metadata.is_dir() {
        files.push(StoredFile {
            relative: relative.into(),
            blob: None,
            length: 0,
            digest: vec![],
        });
        for child in fs::read_dir(source).map_err(err)? {
            let child = child.map_err(err)?;
            snapshot(
                &child.path(),
                &relative.join(child.file_name()),
                folder,
                files,
            )?;
        }
    } else {
        let blob = format!("{:016x}", files.len());
        let (length, digest) = encrypt_file(source, &folder.join(&blob))?;
        if length != metadata.len() {
            return Err("File changed while its snapshot was being saved".into());
        }
        files.push(StoredFile {
            relative: relative.into(),
            blob: Some(blob),
            length,
            digest,
        });
    }
    Ok(())
}

fn protect(data: &[u8]) -> Result<Vec<u8>> {
    crypt(data, false)
}
fn unprotect(data: &[u8]) -> Result<Vec<u8>> {
    crypt(data, true)
}
fn crypt(data: &[u8], decrypt: bool) -> Result<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(data.len()).map_err(err)?,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output: CRYPT_INTEGER_BLOB = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        if decrypt {
            CryptUnprotectData(
                &input,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptProtectData(
                &input,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 {
        return Err(format!(
            "Windows encryption failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let result =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(result)
}

fn encrypt_file(source: &Path, target: &Path) -> Result<(u64, Vec<u8>)> {
    let mut input = File::open(source).map_err(err)?;
    let mut output = File::create(target).map_err(err)?;
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut digest = Sha256::new();
    let mut length = 0;
    loop {
        let n = input.read(&mut buffer).map_err(err)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
        length += n as u64;
        let encrypted = protect(&buffer[..n])?;
        output
            .write_all(&(encrypted.len() as u32).to_le_bytes())
            .map_err(err)?;
        output.write_all(&encrypted).map_err(err)?;
    }
    output.sync_all().map_err(err)?;
    Ok((length, digest.finalize().to_vec()))
}

fn decrypt_file(source: &Path, target: &Path) -> Result<(u64, Vec<u8>)> {
    let mut input = File::open(source).map_err(err)?;
    let mut output = File::create(target).map_err(err)?;
    let mut digest = Sha256::new();
    let mut length = 0;
    loop {
        let mut size = [0u8; 4];
        let n = input.read(&mut size[..1]).map_err(err)?;
        if n == 0 {
            break;
        }
        input.read_exact(&mut size[1..]).map_err(err)?;
        let size = u32::from_le_bytes(size) as usize;
        if size > 2 * 1024 * 1024 {
            return Err("Damaged file snapshot".into());
        }
        let mut encrypted = vec![0u8; size];
        input.read_exact(&mut encrypted).map_err(err)?;
        let decrypted = unprotect(&encrypted)?;
        length += decrypted.len() as u64;
        digest.update(&decrypted);
        output.write_all(&decrypted).map_err(err)?;
    }
    output.sync_all().map_err(err)?;
    Ok((length, digest.finalize().to_vec()))
}

fn file_digest(path: &Path) -> Result<(u64, Vec<u8>)> {
    let mut file = File::open(path).map_err(err)?;
    let mut digest = Sha256::new();
    let mut length = 0;
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let n = file.read(&mut buffer).map_err(err)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
        length += n as u64;
    }
    Ok((length, digest.finalize().to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn locked_staging_rejects_new_entry_without_losing_old_snapshot() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("file.bin");
        fs::write(&source, b"keep me").unwrap();
        let root = dir.path().join("vault");
        let mut store = Store::open(root.clone()).unwrap();
        let mut record = Record::text("file");
        record.kind = Kind::Files;
        store.insert(record, &[source]).unwrap();
        let id = store.entries[0].id.clone();
        let paths = store.materialize(&id).unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&paths[0])
            .unwrap();
        for n in 0..99 {
            store
                .insert(Record::text(&format!("entry-{n}")), &[])
                .unwrap();
        }
        assert!(store.insert(Record::text("over capacity"), &[]).is_err());
        assert_eq!(store.entries.len(), 100);
        assert_eq!(store.entries[99].id, id);
        assert!(
            root.join("items")
                .join(&id)
                .join("0000000000000000")
                .exists()
        );
        drop(lock);
        drop(store);
        assert_eq!(Store::open(root).unwrap().entries.len(), 100);
    }
    use crate::model::Kind;
    use std::fs;
    #[test]
    fn retention_is_newest_first_and_survives_restart_encrypted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("vault");
        let mut store = Store::open(root.clone()).unwrap();
        for n in 0..101 {
            store
                .insert(Record::text(&format!("secret-{n}")), &[])
                .unwrap();
        }
        assert_eq!(store.entries.len(), 100);
        assert_eq!(store.entries[0].title, "secret-100");
        assert_eq!(store.entries[99].title, "secret-1");
        drop(store);
        let store = Store::open(root.clone()).unwrap();
        assert_eq!(store.entries.len(), 100);
        let bytes = fs::read(
            root.join("items")
                .join(format!("{}.cv", store.entries[0].id)),
        )
        .unwrap();
        assert!(!bytes.windows(10).any(|b| b == b"secret-100"));
    }
    #[test]
    fn binary_file_and_folder_survive_source_deletion_and_restart() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        fs::create_dir_all(source.join("sub")).unwrap();
        fs::write(
            source.join("sub").join("anything.weird"),
            [0, 255, 13, 0, 42],
        )
        .unwrap();
        fs::create_dir(source.join("empty")).unwrap();
        let root = dir.path().join("vault");
        let mut store = Store::open(root.clone()).unwrap();
        let mut record = Record::text("files");
        record.kind = Kind::Files;
        store.insert(record, std::slice::from_ref(&source)).unwrap();
        fs::remove_dir_all(&source).unwrap();
        let store = Store::open(root).unwrap();
        let restored = store.materialize(&store.entries[0].id).unwrap();
        assert_eq!(
            fs::read(restored[0].join("sub/anything.weird")).unwrap(),
            [0, 255, 13, 0, 42]
        );
        assert!(restored[0].join("empty").is_dir());
    }
    #[test]
    fn duplicate_text_does_not_consume_a_slot_and_corruption_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().into()).unwrap();
        assert!(store.insert(Record::text("你好 🦀"), &[]).unwrap());
        assert!(!store.insert(Record::text("你好 🦀"), &[]).unwrap());
        assert_eq!(store.entries.len(), 1);
        fs::write(dir.path().join("items/broken.cv"), b"broken").unwrap();
        let store = Store::open(dir.path().into()).unwrap();
        assert_eq!(store.entries[0].title, "你好 🦀");
        assert_eq!(store.warnings.len(), 1);
    }
    #[test]
    fn unreadable_metadata_preserves_its_encrypted_file_payload() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("file.bin");
        fs::write(&source, b"recoverable bytes").unwrap();
        let root = dir.path().join("vault");
        let mut store = Store::open(root.clone()).unwrap();
        let mut record = Record::text("file");
        record.kind = Kind::Files;
        store.insert(record, &[source]).unwrap();
        let id = store.entries[0].id.clone();
        fs::write(root.join("items").join(format!("{id}.cv")), b"damaged").unwrap();
        drop(store);
        let store = Store::open(root.clone()).unwrap();
        assert_eq!(store.warnings.len(), 1);
        assert!(
            root.join("items")
                .join(id)
                .join("0000000000000000")
                .exists()
        );
    }
    #[test]
    fn truncated_encrypted_file_cannot_be_marked_ready() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("file.bin");
        fs::write(&source, vec![42; 2 * 1024 * 1024]).unwrap();
        let root = dir.path().join("vault");
        let mut store = Store::open(root.clone()).unwrap();
        let mut record = Record::text("file");
        record.kind = Kind::Files;
        store.insert(record, &[source]).unwrap();
        let id = store.entries[0].id.clone();
        let blob = root.join("items").join(&id).join("0000000000000000");
        let original = fs::read(&blob).unwrap();
        let first = u32::from_le_bytes(original[..4].try_into().unwrap()) as usize + 4;
        for truncated in [&original[..first], &original[..0]] {
            fs::write(&blob, truncated).unwrap();
            assert!(
                store.materialize(&id).is_err(),
                "truncated snapshot must fail"
            );
            assert!(!root.join("staging").join(&id).join(".ready").exists());
        }
    }
}
