//! The mod files a presentation reads from, as the engine addresses them.
//!
//! Every read goes through the mod file store, so it lands after exactly
//! the queued writes that change its bytes. Every file there has an
//! INCARNATION a rename moves and a delete ends, and a positioned write
//! announces the bytes it changes; a piece range is `(incarnation, offset,
//! len)`, so what a presentation keeps decoded can never outlive its bytes.

use std::sync::mpsc::Receiver;

use crate::modding::client::files::{self, FileRef};

/// One file as a presentation reads it.
#[derive(Clone, Debug)]
pub struct SourceFile {
    pub file: FileRef,
    pub incarnation: u64,
    /// How the mod named it, for errors a player reads.
    pub label: String,
}

impl SourceFile {
    pub fn new(file: FileRef) -> Self {
        Self {
            incarnation: file.incarnation(),
            label: file.rel().to_owned(),
            file,
        }
    }
}

/// Byte ranges in one file, each `[offset, len]` in absolute file offsets.
#[derive(Clone, Debug)]
pub struct FileRanges {
    pub file: SourceFile,
    pub ranges: Vec<[u64; 2]>,
}

/// A change to bytes a presentation may hold decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileChange {
    /// `[start, end)` of this incarnation changed.
    Bytes {
        incarnation: u64,
        start: u64,
        end: u64,
    },
    /// The incarnation ended: its file was deleted, or renamed over.
    Ended { incarnation: u64, label: String },
}

/// A presentation's subscription to the file store's changes.
pub struct FileChanges(Receiver<files::FileChange>);

impl FileChanges {
    pub fn subscribe() -> Self {
        Self(files::subscribe())
    }

    pub fn take(&self) -> Vec<FileChange> {
        let mut out = Vec::new();
        while let Ok(change) = self.0.try_recv() {
            match change {
                files::FileChange::Overwritten {
                    incarnation,
                    range: [start, end],
                } => out.push(FileChange::Bytes {
                    incarnation,
                    start,
                    end,
                }),
                files::FileChange::Ended { incarnations, path } => out.extend(
                    incarnations
                        .into_iter()
                        .map(|incarnation| FileChange::Ended {
                            incarnation,
                            label: path.clone(),
                        }),
                ),
            }
        }
        out
    }
}

/// Read `[offset, offset + len)` of `file` through the store; `done` runs
/// on its I/O thread with exactly those bytes, or why not.
pub fn read(
    file: &SourceFile,
    offset: u64,
    len: u64,
    done: impl FnOnce(Result<Vec<u8>, String>) + Send + 'static,
) {
    files::read(&file.file, offset, len, move |result| {
        done(result.and_then(|bytes| {
            if bytes.len() as u64 == len {
                Ok(bytes)
            } else {
                Err(format!(
                    "the file ends {} bytes into a range of {len}",
                    bytes.len()
                ))
            }
        }))
    });
}
