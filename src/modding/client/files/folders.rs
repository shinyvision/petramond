use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use mod_api::{ClientFileAnswer, ClientFolderInfo};

use super::TicketSink;

const CHOICES: &str = "chosen_folders.json";

pub struct FolderRequest {
    pub title: String,
    pub start: Option<PathBuf>,
    pub done: FolderAnswer,
}

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

pub type FolderChooser = Arc<dyn Fn(FolderRequest) + Send + Sync>;

static CHOOSER: Mutex<Option<FolderChooser>> = Mutex::new(None);

static OPEN: AtomicBool = AtomicBool::new(false);

pub fn install_chooser(chooser: Option<FolderChooser>) {
    *CHOOSER.lock().unwrap_or_else(PoisonError::into_inner) = chooser;
}

pub fn label(folder: &Path) -> String {
    folder.display().to_string()
}

pub fn chosen(pack: &Path, slot: u32) -> Option<PathBuf> {
    read(pack).remove(&slot).filter(|dir| dir.is_dir())
}

pub fn info(pack: &Path, slot: u32) -> Option<ClientFolderInfo> {
    chosen(pack, slot).map(|dir| ClientFolderInfo { label: label(&dir) })
}

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
