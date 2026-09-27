use serde::{Deserialize, Serialize};

use crate::ClientStorageScope;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientFileRange {
    pub scope: ClientStorageScope,
    pub path: String,
    pub offset: u64,
    pub len: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientFileRanges {
    pub scope: ClientStorageScope,
    pub path: String,
    pub ranges: Vec<[u64; 2]>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ClientFileAnswer {
    Done {
        range: Option<[u64; 2]>,
        envelope: Option<[u64; 2]>,
    },
    Read(#[serde(with = "serde_bytes")] Vec<u8>),
    Listing {
        entries: Vec<ClientFileEntry>,
        more: bool,
    },
    Folder(Option<ClientFolderInfo>),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientFolderInfo {
    pub label: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientFileEntry {
    pub name: String,
    pub dir: bool,
    pub len: u64,
    pub modified_unix_ms: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientFileInfo {
    pub len: u64,
    pub written: u64,
    pub modified_unix_ms: u64,
    pub error: Option<String>,
}

const DEVICE_STEMS: [&str; 24] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9", "conin$",
    "conout$",
];

pub fn file_segment_problem(segment: &str) -> Option<String> {
    if segment.is_empty() {
        return Some("an empty name".into());
    }
    if segment.starts_with('.') {
        return Some(format!("'{segment}' begins with '.'"));
    }
    if let Some(c) = segment
        .chars()
        .find(|&c| c <= '\u{1f}' || c == '\u{7f}' || "\\<>:\"|?*".contains(c))
    {
        return Some(format!("'{segment}' holds the character {c:?}"));
    }
    if segment.ends_with('.') || segment.ends_with(' ') {
        return Some(format!("'{segment}' ends in '.' or a space"));
    }
    let stem = segment.split('.').next().unwrap_or(segment);
    if DEVICE_STEMS.iter().any(|d| stem.eq_ignore_ascii_case(d)) {
        return Some(format!("'{segment}' is a reserved device name"));
    }
    None
}

pub fn file_path_problem(path: &str) -> Option<String> {
    path.split('/').find_map(file_segment_problem)
}
