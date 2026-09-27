use std::sync::mpsc::Receiver;

use crate::modding::client::files::{self, FileRef};

#[derive(Clone, Debug)]
pub struct SourceFile {
    pub file: FileRef,
    pub incarnation: u64,
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

#[derive(Clone, Debug)]
pub struct FileRanges {
    pub file: SourceFile,
    pub ranges: Vec<[u64; 2]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileChange {
    Bytes {
        incarnation: u64,
        start: u64,
        end: u64,
    },
    Ended {
        incarnation: u64,
        label: String,
    },
}

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
