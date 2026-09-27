//! Mod files: a client mod's own files in its storage buckets, named and laid
//! out as it likes, appended to, rewritten in place and read back in ranges.
//!
//! `Pack` files live in `<data>/client_mod_data/packs/<mod_id>/files/`, `World`
//! files in the presented session's bucket beside the KV blobs, under
//! `<mod_id>/files/`. A path is one or more `/`-separated segments, each a
//! name every OS the game ships on can store ([`file_path_problem`]); there
//! is no length rule, and a name the OS still refuses comes back in its words.

use serde::{Deserialize, Serialize};

use crate::ClientStorageScope;

/// One `[offset, len]` range of one mod file.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientFileRange {
    pub scope: ClientStorageScope,
    pub path: String,
    pub offset: u64,
    pub len: u64,
}

/// Many `[offset, len]` ranges of one mod file, in absolute file offsets.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientFileRanges {
    pub scope: ClientStorageScope,
    pub path: String,
    pub ranges: Vec<[u64; 2]>,
}

/// How a file ticket finished (`ClientFilePoll`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ClientFileAnswer {
    /// A write, sync, rename or delete finished. `range`: where an append, a
    /// write or an engine record landed (`None` for a sync, a rename or a
    /// delete). `envelope`: a state record's envelope. Both absolute
    /// `[offset, len]`.
    Done {
        range: Option<[u64; 2]>,
        envelope: Option<[u64; 2]>,
    },
    /// A read's bytes: those that exist in the range asked for.
    Read(#[serde(with = "serde_bytes")] Vec<u8>),
    /// One page of a listing, sorted by name; `more` = continue after the
    /// last name.
    Listing {
        entries: Vec<ClientFileEntry>,
        more: bool,
    },
    /// A folder choice ([`HostCall::ClientFolderChoose`](crate::HostCall::ClientFolderChoose)):
    /// the folder now chosen, or `None` = the player cancelled and the slot
    /// keeps what it had.
    Folder(Option<ClientFolderInfo>),
}

/// A chosen folder, as the player reads it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientFolderInfo {
    /// The folder's path, for showing where files go.
    pub label: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientFileEntry {
    pub name: String,
    pub dir: bool,
    pub len: u64,
    pub modified_unix_ms: u64,
}

/// One file, as `ClientFileStat` answers it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientFileInfo {
    /// Bytes the file will hold once every queued write whose size is known
    /// has landed.
    pub len: u64,
    /// Bytes on disk now. `len - written` is the file's backlog.
    pub written: u64,
    pub modified_unix_ms: u64,
    /// The most recent write failure, cleared by a later successful write.
    pub error: Option<String>,
}

/// Stems no Windows filesystem stores as a plain file name.
const DEVICE_STEMS: [&str; 24] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9", "conin$",
    "conout$",
];

/// Why `segment` cannot be one name of a mod file path; `None` = it can.
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

/// Why `path` cannot name a mod file or directory; `None` = it can. A path is
/// one or more `/`-separated segments, each passing [`file_segment_problem`].
pub fn file_path_problem(path: &str) -> Option<String> {
    path.split('/').find_map(file_segment_problem)
}
