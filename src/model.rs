use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Kind {
    Text,
    Image,
    Files,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Format {
    pub id: u32,
    pub name: Option<String>,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preview {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredFile {
    pub relative: PathBuf,
    pub blob: Option<String>,
    pub length: u64,
    pub digest: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub created: u64,
    pub kind: Kind,
    pub title: String,
    pub detail: String,
    pub formats: Vec<Format>,
    pub preview: Option<Preview>,
    pub files: Vec<StoredFile>,
    pub roots: Vec<PathBuf>,
    pub fingerprint: Vec<u8>,
}

impl Record {
    pub fn text(value: &str) -> Self {
        Self {
            id: String::new(),
            created: 0,
            kind: Kind::Text,
            title: value.into(),
            detail: String::new(),
            formats: vec![],
            preview: None,
            files: vec![],
            roots: vec![],
            fingerprint: value.as_bytes().to_vec(),
        }
    }
}

pub fn page_range(page: usize, count: usize) -> std::ops::Range<usize> {
    let start = (page.min(9) * 10).min(count);
    start..(start + 10).min(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pages_cover_exactly_ten_items_and_never_exceed_history() {
        assert_eq!(page_range(0, 100), 0..10);
        assert_eq!(page_range(9, 100), 90..100);
        assert_eq!(page_range(1, 13), 10..13);
        assert_eq!(page_range(9, 3), 3..3);
        assert_eq!(page_range(100, 100), 90..100);
    }
}
