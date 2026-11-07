use crate::{
    model::{Format, Kind, Preview, Record},
    store::Result,
};
use sha2::{Digest, Sha256};
use std::{
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
    ptr,
};
use windows_sys::Win32::{
    Foundation::*,
    System::{DataExchange::*, Memory::*},
    UI::Shell::DragQueryFileW,
};

pub const TEXT: u32 = 13;
pub const FILES: u32 = 15;
pub const DIB: u32 = 8;
pub const DIBV5: u32 = 17;
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub fn registered(s: &str) -> u32 {
    unsafe { RegisterClipboardFormatW(wide(s).as_ptr()) }
}

#[derive(Clone)]
pub struct Capture {
    pub formats: Vec<Format>,
    pub sources: Vec<PathBuf>,
}
struct Open;
impl Open {
    fn new(owner: HWND) -> Result<Self> {
        for _ in 0..10 {
            if unsafe { OpenClipboard(owner) } != 0 {
                return Ok(Self);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Err("Clipboard is busy; try again.".into())
    }
}
impl Drop for Open {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}

fn bytes(id: u32) -> Option<Vec<u8>> {
    unsafe {
        let handle = GetClipboardData(id);
        if handle.is_null() {
            return None;
        }
        let size = GlobalSize(handle);
        if size == 0 {
            return None;
        }
        let data = GlobalLock(handle);
        if data.is_null() {
            return None;
        }
        let result = std::slice::from_raw_parts(data.cast::<u8>(), size).to_vec();
        GlobalUnlock(handle);
        Some(result)
    }
}

pub fn read(owner: HWND, respect_privacy: bool) -> Result<Option<Capture>> {
    let _open = Open::new(owner)?;
    if respect_privacy {
        for name in [
            "ExcludeClipboardContentFromMonitorProcessing",
            "Clipboard Viewer Ignore",
        ] {
            if unsafe { IsClipboardFormatAvailable(registered(name)) } != 0 {
                return Ok(None);
            }
        }
        if bytes(registered("CanIncludeInClipboardHistory"))
            .is_some_and(|v| v.len() >= 4 && v[..4] == [0, 0, 0, 0])
        {
            return Ok(None);
        }
    }
    let mut formats = vec![];
    // PNG first: preserve alpha before trying Windows' synthesized DIB.
    for name in [
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
    ] {
        let id = registered(name);
        if let Some(data) = bytes(id) {
            formats.push(Format {
                id,
                name: Some(name.into()),
                data,
            });
        }
    }
    for id in [TEXT, DIBV5, DIB, 1, 7, 16] {
        if let Some(data) = bytes(id) {
            formats.push(Format {
                id,
                name: None,
                data,
            });
        }
    }
    let mut sources = vec![];
    unsafe {
        let drop = GetClipboardData(FILES);
        if !drop.is_null() {
            let count = DragQueryFileW(drop, u32::MAX, ptr::null_mut(), 0);
            for n in 0..count {
                let len = DragQueryFileW(drop, n, ptr::null_mut(), 0);
                let mut buffer = vec![0u16; len as usize + 1];
                DragQueryFileW(drop, n, buffer.as_mut_ptr(), len + 1);
                sources.push(PathBuf::from(std::ffi::OsString::from_wide(
                    &buffer[..len as usize],
                )));
            }
        }
    }
    if formats.is_empty() && sources.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Capture { formats, sources }))
    }
}

/// Inspect and clear file references under a managed staging directory while
/// holding the clipboard lock, so no owner can race the check and deletion.
pub fn release_staged_paths(prefix: &std::path::Path) -> Result<()> {
    let _open = Open::new(ptr::null_mut())?;
    unsafe {
        let drop = GetClipboardData(FILES);
        if !drop.is_null() {
            let count = DragQueryFileW(drop, u32::MAX, ptr::null_mut(), 0);
            for n in 0..count {
                let len = DragQueryFileW(drop, n, ptr::null_mut(), 0);
                let mut buffer = vec![0u16; len as usize + 1];
                DragQueryFileW(drop, n, buffer.as_mut_ptr(), len + 1);
                let path = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..len as usize]));
                if path.starts_with(prefix) {
                    if EmptyClipboard() == 0 {
                        return Err("Cannot release clipboard file references".into());
                    }
                    break;
                }
            }
        }
    }
    Ok(())
}

fn allocate(data: &[u8]) -> Result<HGLOBAL> {
    unsafe {
        let handle = GlobalAlloc(GMEM_MOVEABLE, data.len().max(1));
        if handle.is_null() {
            return Err("Cannot allocate clipboard memory".into());
        }
        let destination = GlobalLock(handle);
        if destination.is_null() {
            GlobalFree(handle);
            return Err("Cannot lock clipboard memory".into());
        }
        ptr::copy_nonoverlapping(data.as_ptr(), destination.cast(), data.len());
        GlobalUnlock(handle);
        Ok(handle)
    }
}

pub fn write(owner: HWND, formats: &[Format], paths: &[PathBuf]) -> Result<u32> {
    let mut payload: Vec<(u32, Vec<u8>)> = formats
        .iter()
        .map(|v| {
            (
                v.name.as_deref().map(registered).unwrap_or(v.id),
                v.data.clone(),
            )
        })
        .collect();
    if !paths.is_empty() {
        let mut drop = vec![0u8; 20];
        drop[..4].copy_from_slice(&20u32.to_le_bytes());
        drop[16..20].copy_from_slice(&1u32.to_le_bytes());
        for path in paths {
            for c in path.as_os_str().encode_wide().chain(Some(0)) {
                drop.extend_from_slice(&c.to_le_bytes());
            }
        }
        drop.extend_from_slice(&[0, 0]);
        payload.push((FILES, drop));
        payload.push((
            registered("Preferred DropEffect"),
            1u32.to_le_bytes().to_vec(),
        ));
    }
    // Prepare every memory block before touching the existing clipboard.
    let mut prepared = vec![];
    for (id, data) in payload {
        match allocate(&data) {
            Ok(handle) => prepared.push((id, handle)),
            Err(e) => {
                for (_, h) in prepared {
                    unsafe {
                        GlobalFree(h);
                    }
                }
                return Err(e);
            }
        }
    }
    let result = (|| {
        let _open = Open::new(owner)?;
        if unsafe { EmptyClipboard() } == 0 {
            return Err("Cannot replace the clipboard".into());
        }
        for (id, handle) in &mut prepared {
            if unsafe { SetClipboardData(*id, *handle) }.is_null() {
                return Err(format!("Cannot restore clipboard format {id}"));
            }
            *handle = ptr::null_mut(); // ownership transferred to Windows
        }
        Ok(unsafe { GetClipboardSequenceNumber() })
    })();
    for (_, handle) in prepared {
        if !handle.is_null() {
            unsafe {
                GlobalFree(handle);
            }
        }
    }
    result
}

impl Capture {
    pub fn into_record(self) -> Record {
        let mut digest = Sha256::new();
        for f in &self.formats {
            digest.update(f.id.to_le_bytes());
            digest.update((f.data.len() as u64).to_le_bytes());
            digest.update(&f.data);
        }
        let mut record = Record::text("");
        record.fingerprint = digest.finalize().to_vec();
        if !self.sources.is_empty() {
            record.kind = Kind::Files;
            record.title = self
                .sources
                .iter()
                .filter_map(|v| v.file_name())
                .map(|v| v.to_string_lossy())
                .collect::<Vec<_>>()
                .join(", ");
            record.detail = format!(
                "{} file/folder{} · saved copy",
                self.sources.len(),
                if self.sources.len() == 1 { "" } else { "s" }
            );
        } else {
            let preview = self.formats.iter().find_map(decode_image);
            let image_format = self.formats.iter().any(|f| {
                f.id == DIB
                    || f.id == DIBV5
                    || f.name.as_deref().is_some_and(|n| {
                        [
                            "PNG",
                            "image/png",
                            "JFIF",
                            "image/jpeg",
                            "GIF",
                            "image/gif",
                            "image/webp",
                            "TIFF",
                        ]
                        .contains(&n)
                    })
            });
            if image_format {
                record.kind = Kind::Image;
                record.title = "Copied image".into();
                record.detail = preview
                    .as_ref()
                    .map(|v| format!("{} × {} pixels", v.0, v.1))
                    .unwrap_or_else(|| "Original image formats preserved".into());
                record.preview = preview.map(|(_, _, v)| v);
            } else {
                let text = self
                    .formats
                    .iter()
                    .find(|f| f.id == TEXT)
                    .map(|f| {
                        let units: Vec<_> = f
                            .data
                            .chunks_exact(2)
                            .map(|b| u16::from_le_bytes([b[0], b[1]]))
                            .take_while(|v| *v != 0)
                            .collect();
                        String::from_utf16_lossy(&units)
                    })
                    .unwrap_or_else(|| "Rich text".into());
                record.title = text.chars().take(1000).collect();
                record.detail = format!("{} characters", text.chars().count());
            }
        }
        record.formats = self.formats;
        record
    }
}

fn decode_image(format: &Format) -> Option<(u32, u32, Preview)> {
    let data = if format.id == DIB || format.id == DIBV5 {
        dib_to_bmp(&format.data)?
    } else {
        if !format.name.as_deref().is_some_and(|n| {
            [
                "PNG",
                "image/png",
                "JFIF",
                "image/jpeg",
                "GIF",
                "image/gif",
                "image/webp",
                "TIFF",
            ]
            .contains(&n)
        }) {
            return None;
        }
        format.data.clone()
    };
    let mut reader = image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    let (w, h) = (image.width(), image.height());
    let thumb = if w <= 80 && h <= 80 {
        image.to_rgba8()
    } else {
        image.thumbnail(80, 80).to_rgba8()
    };
    Some((
        w,
        h,
        Preview {
            width: thumb.width(),
            height: thumb.height(),
            rgba: thumb.into_raw(),
        },
    ))
}

fn dib_to_bmp(dib: &[u8]) -> Option<Vec<u8>> {
    let u32_at = |offset: usize| -> Option<u32> {
        Some(u32::from_le_bytes(
            dib.get(offset..offset + 4)?.try_into().ok()?,
        ))
    };
    let header = u32_at(0)? as usize;
    if header < 40 || header > dib.len() {
        return None;
    }
    let bits = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?);
    let compression = u32_at(16)?;
    let colors = u32_at(32)? as usize;
    let palette = if colors > 0 {
        colors
    } else if bits <= 8 {
        1usize << bits
    } else {
        0
    };
    let masks = if header == 40 && compression == 3 {
        12
    } else if header == 40 && compression == 6 {
        16
    } else {
        0
    };
    let offset = 14usize
        .checked_add(header)?
        .checked_add(masks)?
        .checked_add(palette.checked_mul(4)?)?;
    if offset > dib.len() + 14 {
        return None;
    }
    let mut bmp = b"BM".to_vec();
    bmp.extend_from_slice(&u32::try_from(dib.len() + 14).ok()?.to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&u32::try_from(offset).ok()?.to_le_bytes());
    bmp.extend_from_slice(dib);
    Some(bmp)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dibv5_keeps_alpha_in_preview_and_original_payload() {
        let mut data = vec![0u8; 124];
        data[..4].copy_from_slice(&124u32.to_le_bytes());
        data[4..8].copy_from_slice(&1i32.to_le_bytes());
        data[8..12].copy_from_slice(&(-1i32).to_le_bytes());
        data[12..14].copy_from_slice(&1u16.to_le_bytes());
        data[14..16].copy_from_slice(&32u16.to_le_bytes());
        data[16..20].copy_from_slice(&3u32.to_le_bytes());
        data[20..24].copy_from_slice(&4u32.to_le_bytes());
        for (offset, mask) in [
            (40, 0x00ff0000u32),
            (44, 0x0000ff00),
            (48, 0x000000ff),
            (52, 0xff000000),
            (56, 0x73524742),
        ] {
            data[offset..offset + 4].copy_from_slice(&mask.to_le_bytes());
        }
        data.extend_from_slice(&[30, 20, 10, 128]);
        let record = Capture {
            formats: vec![Format {
                id: DIBV5,
                name: None,
                data: data.clone(),
            }],
            sources: vec![],
        }
        .into_record();
        assert_eq!(record.preview.unwrap().rgba, [10, 20, 30, 128]);
        assert_eq!(record.formats[0].data, data);
    }
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    struct ClipboardTest {
        owner: HWND,
        saved: Option<Capture>,
    }
    impl ClipboardTest {
        fn new() -> Self {
            let owner = unsafe {
                CreateWindowExW(
                    0,
                    wide("STATIC").as_ptr(),
                    wide("ClipVault clipboard test").as_ptr(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    HWND_MESSAGE,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null(),
                )
            };
            assert!(!owner.is_null());
            let saved = read(owner, false).unwrap();
            Self { owner, saved }
        }
    }
    impl Drop for ClipboardTest {
        fn drop(&mut self) {
            if let Some(saved) = &self.saved {
                let _ = write(self.owner, &saved.formats, &saved.sources);
            }
            unsafe {
                DestroyWindow(self.owner);
            }
        }
    }
    #[test]
    fn real_clipboard_unicode_html_alpha_image_files_and_privacy() {
        let test = ClipboardTest::new();
        let data: Vec<u8> = "Hello 你好 🦀\r\nline two\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let html = Format {
            id: registered("HTML Format"),
            name: Some("HTML Format".into()),
            data: b"Version:1.0\r\n<!--StartFragment--><b>hello</b><!--EndFragment-->\0".to_vec(),
        };
        write(
            test.owner,
            &[
                Format {
                    id: TEXT,
                    name: None,
                    data: data.clone(),
                },
                html.clone(),
            ],
            &[],
        )
        .unwrap();
        let capture = read(test.owner, true).unwrap().expect("text capture");
        assert!(
            capture
                .formats
                .iter()
                .any(|v| v.id == TEXT && v.data == data)
        );
        assert!(
            capture
                .formats
                .iter()
                .any(|v| v.name == html.name && v.data == html.data)
        );
        assert!(capture.into_record().title.starts_with("Hello 你好 🦀"));

        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 128, 0, 255, 0, 0])
            .unwrap()
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let png = png.into_inner();
        write(
            test.owner,
            &[Format {
                id: registered("PNG"),
                name: Some("PNG".into()),
                data: png.clone(),
            }],
            &[],
        )
        .unwrap();
        let record = read(test.owner, true).unwrap().unwrap().into_record();
        assert_eq!(record.kind, Kind::Image);
        assert_eq!(record.preview.unwrap().rgba, [255, 0, 0, 128, 0, 255, 0, 0]);
        assert_eq!(record.formats[0].data, png);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("any.extension");
        std::fs::write(&path, [0, 255, 42]).unwrap();
        write(test.owner, &[], std::slice::from_ref(&path)).unwrap();
        let capture = read(test.owner, true).unwrap().unwrap();
        assert_eq!(capture.sources, vec![path]);
        assert_eq!(capture.into_record().kind, Kind::Files);

        write(
            test.owner,
            &[
                Format {
                    id: TEXT,
                    name: None,
                    data,
                },
                Format {
                    id: registered("CanIncludeInClipboardHistory"),
                    name: Some("CanIncludeInClipboardHistory".into()),
                    data: vec![0, 0, 0, 0],
                },
            ],
            &[],
        )
        .unwrap();
        assert!(read(test.owner, true).unwrap().is_none());
    }
    #[test]
    fn dib_bitmap_preview_uses_correct_pixel_offset_and_orientation() {
        let mut bytes = vec![0u8; 40];
        bytes[0..4].copy_from_slice(&40u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&1i32.to_le_bytes());
        bytes[8..12].copy_from_slice(&2i32.to_le_bytes());
        bytes[12..14].copy_from_slice(&1u16.to_le_bytes());
        bytes[14..16].copy_from_slice(&24u16.to_le_bytes());
        bytes.extend_from_slice(&[255, 0, 0, 0, 0, 0, 255, 0]);
        let record = Capture {
            formats: vec![Format {
                id: DIB,
                name: None,
                data: bytes,
            }],
            sources: vec![],
        }
        .into_record();
        let preview = record.preview.unwrap();
        assert_eq!((preview.width, preview.height), (1, 2));
        assert_eq!(preview.rgba, [255, 0, 0, 255, 0, 0, 255, 255]);
    }
    #[test]
    fn eviction_clears_clipboard_reference_before_removing_staged_file() {
        let test = ClipboardTest::new();
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("original.bin");
        std::fs::write(&source, b"saved bytes").unwrap();
        let mut store = crate::store::Store::open(dir.path().join("vault")).unwrap();
        let mut record = Record::text("file");
        record.kind = Kind::Files;
        store.insert(record, &[source]).unwrap();
        let paths = store.materialize(&store.entries[0].id).unwrap();
        write(test.owner, &[], &paths).unwrap();
        for n in 0..100 {
            store
                .insert(Record::text(&format!("item-{n}")), &[])
                .unwrap();
        }
        assert_eq!(store.entries.len(), 100);
        assert!(!paths[0].exists());
        assert!(
            read(test.owner, false).unwrap().is_none(),
            "clipboard must not retain deleted staging paths"
        );
    }
}
