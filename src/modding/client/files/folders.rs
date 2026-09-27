//! Folders the PLAYER chooses for a client mod ([`ClientStorageScope::Chosen`]):
//! the OS folder picker the app installs, and each mod's remembered choices.
//!
//! The choices live beside the mod's pack bucket, where its own file calls
//! cannot reach: a mod names a folder SLOT, never a path, so the only folders
//! it can write outside its bucket are ones a player picked for it.
//!
//! [`ClientStorageScope::Chosen`]: mod_api::ClientStorageScope::Chosen

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use mod_api::{ClientFileAnswer, ClientFolderInfo};

use super::TicketSink;

/// The remembered choices, one JSON map of slot → folder per pack bucket.
const CHOICES: &str = "chosen_folders.json";

/// One folder the player is asked for.
pub struct FolderRequest {
    pub title: String,
    /// Where the picker opens: the slot's folder, else the player's videos.
    pub start: Option<PathBuf>,
    /// Answered once, from any thread; dropped unanswered, it cancels.
    pub done: FolderAnswer,
}

/// What the player picked in one picker: [`answer`](Self::answer) it with
/// the folder, or `None` for a cancel.
pub struct FolderAnswer(Option<Box<dyn FnOnce(Option<PathBuf>) + Send>>);

impl FolderAnswer {
    pub fn answer(mut self, picked: Option<PathBuf>) {
        if let Some(done) = self.0.take() {
            done(picked);
        }
    }
}

impl Drop for FolderAnswer {
    fn drop(&mut self) {
        if let Some(done) = self.0.take() {
            done(None);
        }
    }
}

/// Shows the OS folder picker without blocking the caller.
pub type FolderChooser = Arc<dyn Fn(FolderRequest) + Send + Sync>;

static CHOOSER: Mutex<Option<FolderChooser>> = Mutex::new(None);

/// One picker at a time, whichever mod asked.
static OPEN: AtomicBool = AtomicBool::new(false);

/// The app's folder picker. Without one (a headless server, a test that sets
/// none) every choice is refused.
pub fn install_chooser(chooser: Option<FolderChooser>) {
    *CHOOSER.lock().unwrap_or_else(PoisonError::into_inner) = chooser;
}

/// What the player reads for `folder`.
pub fn label(folder: &Path) -> String {
    folder.display().to_string()
}

/// The folder chosen for `slot` in the pack bucket at `pack`, while it still
/// exists.
pub fn chosen(pack: &Path, slot: u32) -> Option<PathBuf> {
    read(pack).remove(&slot).filter(|dir| dir.is_dir())
}

pub fn info(pack: &Path, slot: u32) -> Option<ClientFolderInfo> {
    chosen(pack, slot).map(|dir| ClientFolderInfo { label: label(&dir) })
}

/// Ask the player for `slot`'s folder; `sink` answers with what they chose.
/// `Err` = refused before anything opened.
pub fn choose(pack: &Path, slot: u32, title: String, sink: TicketSink) -> Result<(), String> {
    let Some(chooser) = CHOOSER
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
    else {
        return Err("This build has no folder picker.".into());
    };
    if OPEN.swap(true, Ordering::AcqRel) {
        return Err("A folder picker is already open.".into());
    }
    let start = chosen(pack, slot).or_else(videos);
    let pack = pack.to_path_buf();
    chooser(FolderRequest {
        title,
        start,
        done: FolderAnswer(Some(Box::new(move |picked| {
            OPEN.store(false, Ordering::Release);
            let answer = match picked {
                None => Ok(ClientFileAnswer::Folder(None)),
                Some(dir) => remember(&pack, slot, &dir).map(|()| {
                    ClientFileAnswer::Folder(Some(ClientFolderInfo { label: label(&dir) }))
                }),
            };
            sink.finish(answer);
        }))),
    });
    Ok(())
}

/// The player's videos folder, else their home.
fn videos() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    let dirs = directories::UserDirs::new()?;
    Some(
        dirs.video_dir()
            .map_or_else(|| dirs.home_dir().to_path_buf(), Path::to_path_buf),
    )
}

fn read(pack: &Path) -> BTreeMap<u32, PathBuf> {
    std::fs::read(pack.join(CHOICES))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn remember(pack: &Path, slot: u32, dir: &Path) -> Result<(), String> {
    static WRITING: Mutex<()> = Mutex::new(());
    let _turn = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    let mut choices = read(pack);
    choices.insert(slot, dir.to_path_buf());
    let bytes = serde_json::to_vec_pretty(&choices).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(pack).map_err(|e| e.to_string())?;
    let tmp = pack.join(format!(".{CHOICES}.tmp"));
    std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, pack.join(CHOICES)))
        .map_err(|e| format!("the folder choice could not be kept: {e}"))
}
