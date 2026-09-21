//! The personal schematic library on disk: its index, thumbnails for the
//! cards on screen, and the save / delete / load jobs — one at a time, on the
//! background pool, never on the frame thread.

use crate::game::Game;
use petramond::schematic::{archive::Thumbnail, library, Schematic};
use petramond::worker::JobPool;
use petramond_render::job::Job;
use std::{collections::VecDeque, path::PathBuf, sync::Arc};

/// Thumbnails kept beyond the cards on screen, so scrolling back is instant.
const THUMBNAILS_OFF_SCREEN: usize = 16;

enum Edit {
    Delete(PathBuf),
    Load(PathBuf),
}

enum Completed {
    Listed(Vec<library::Entry>),
    Saved(library::Entry),
    Deleted(PathBuf),
    Loaded(PathBuf, Result<Arc<Schematic>, String>),
    Thumbnail(library::Entry, Result<Thumbnail, String>),
}

/// What a finished job means to the rest of the game.
pub(super) enum LibraryEvent {
    /// The load the player last asked for finished.
    Loaded(Result<Arc<Schematic>, String>),
    /// A save or delete went through.
    Changed,
    Failed(String),
}

struct Cached {
    path: PathBuf,
    revision: u32,
    /// `None` for an archive whose image failed to decode: not retried.
    image: Option<Thumbnail>,
}

impl Cached {
    fn is(&self, entry: &library::Entry) -> bool {
        self.path == entry.path && self.revision == entry.header.revision
    }
}

#[derive(Default)]
pub struct SchematicLibrary {
    entries: Vec<library::Entry>,
    listed: bool,
    job: Option<Job<Result<Completed, String>>>,
    saves: VecDeque<Arc<Schematic>>,
    edits: VecDeque<Edit>,
    /// The load whose result becomes the placement preview.
    requested_load: Option<PathBuf>,
    /// The archive the current paste preview came from.
    previewed: Option<PathBuf>,
    /// A save finished since [`take_saved`](Self::take_saved) last asked.
    saved: bool,
    wanted: VecDeque<library::Entry>,
    /// Least recently shown first.
    thumbnails: VecDeque<Cached>,
    on_screen: usize,
}

impl SchematicLibrary {
    pub fn entries(&self) -> &[library::Entry] {
        &self.entries
    }

    #[cfg(test)]
    pub fn entries_mut(&mut self) -> &mut Vec<library::Entry> {
        &mut self.entries
    }

    /// Queue a captured schematic; the app renders its thumbnail and starts
    /// the save once the worker is free.
    pub(super) fn queue_save(&mut self, schematic: Arc<Schematic>) {
        self.saves.push_back(schematic);
    }

    /// The next capture waiting for its thumbnail, once no job is running.
    pub fn pending_save(&self) -> Option<Arc<Schematic>> {
        self.job
            .is_none()
            .then(|| self.saves.front().cloned())
            .flatten()
    }

    /// Render the thumbnail and publish the complete archive in one job.
    pub fn start_save(
        &mut self,
        jobs: &JobPool,
        schematic: Arc<Schematic>,
        thumbnail: impl FnOnce() -> Result<Vec<u8>, String> + Send + 'static,
    ) {
        self.saves.pop_front();
        self.job = Some(Job::spawn(jobs, move || {
            let path = library::save(&library::directory(), &schematic, &thumbnail()?)?;
            library::inspect(&path).map(Completed::Saved)
        }));
    }

    pub fn abandon_save(&mut self) {
        self.saves.pop_front();
    }

    /// Whether a save was published since this last asked (an edge).
    pub fn take_saved(&mut self) -> bool {
        std::mem::take(&mut self.saved)
    }

    pub fn delete(&mut self, path: PathBuf) {
        self.edits.push_back(Edit::Delete(path));
    }

    pub(super) fn request_load(&mut self, path: PathBuf) {
        self.cancel_load();
        self.requested_load = Some(path.clone());
        self.edits.push_back(Edit::Load(path));
    }

    /// Forget a load that has not become a preview yet.
    pub fn cancel_load(&mut self) {
        self.requested_load = None;
        self.edits.retain(|e| !matches!(e, Edit::Load(_)));
    }

    pub(super) fn previewed(&self) -> Option<&PathBuf> {
        self.previewed.as_ref()
    }

    pub(super) fn set_previewed(&mut self, path: Option<PathBuf>) {
        self.previewed = path;
    }

    /// Ask for the images of the cards on screen, every one of them.
    pub fn request_thumbnails(&mut self, visible: &[usize]) {
        self.wanted.clear();
        self.on_screen = visible.len();
        for &index in visible {
            let Some(entry) = self.entries.get(index) else {
                continue;
            };
            if let Some(i) = self.thumbnails.iter().position(|c| c.is(entry)) {
                let cached = self.thumbnails.remove(i).unwrap();
                self.thumbnails.push_back(cached);
            } else {
                self.wanted.push_back(entry.clone());
            }
        }
    }

    pub fn thumbnail(&self, entry: &library::Entry) -> Option<&Thumbnail> {
        self.thumbnails.iter().find(|c| c.is(entry))?.image.as_ref()
    }

    /// Settle the finished job, then start the next. `browsing` lists the
    /// directory the first time someone can see the library.
    pub(super) fn poll(&mut self, jobs: &JobPool, browsing: bool) -> Option<LibraryEvent> {
        let event = Job::finish(&mut self.job).and_then(|outcome| {
            let result = outcome.unwrap_or_else(|_| Err("Schematic worker failed".into()));
            self.settle(result)
        });
        if self.job.is_none() {
            self.start_next(jobs, browsing);
        }
        event
    }

    fn settle(&mut self, result: Result<Completed, String>) -> Option<LibraryEvent> {
        match result {
            Ok(Completed::Listed(entries)) => self.entries = entries,
            Ok(Completed::Saved(entry)) => {
                self.entries.push(entry);
                library::sort(&mut self.entries);
                self.saved = true;
                return Some(LibraryEvent::Changed);
            }
            Ok(Completed::Deleted(path)) => {
                self.entries.retain(|e| e.path != path);
                self.thumbnails.retain(|e| e.path != path);
                self.wanted.retain(|e| e.path != path);
                return Some(LibraryEvent::Changed);
            }
            Ok(Completed::Loaded(path, result)) => {
                if self.requested_load.as_ref() == Some(&path) {
                    self.requested_load = None;
                    if result.is_ok() {
                        self.previewed = Some(path);
                    }
                    return Some(LibraryEvent::Loaded(result));
                }
            }
            Ok(Completed::Thumbnail(entry, result)) => {
                if let Err(error) = &result {
                    log::warn!("schematic thumbnail {}: {error}", entry.path.display());
                }
                self.thumbnails.retain(|e| e.path != entry.path);
                self.thumbnails.push_back(Cached {
                    path: entry.path,
                    revision: entry.header.revision,
                    image: result.ok(),
                });
                let keep = self.on_screen + THUMBNAILS_OFF_SCREEN;
                while self.thumbnails.len() > keep {
                    self.thumbnails.pop_front();
                }
            }
            Err(error) => return Some(LibraryEvent::Failed(error)),
        }
        None
    }

    fn start_next(&mut self, jobs: &JobPool, browsing: bool) {
        if let Some(edit) = self.edits.pop_front() {
            self.job = Some(Job::spawn(jobs, move || match edit {
                Edit::Delete(path) => {
                    library::delete(&path)?;
                    Ok(Completed::Deleted(path))
                }
                Edit::Load(path) => {
                    let result = library::read(&path).and_then(|schematic| {
                        schematic.validate()?;
                        Ok(Arc::new(schematic))
                    });
                    Ok(Completed::Loaded(path, result))
                }
            }));
        } else if browsing && !self.listed {
            self.listed = true;
            self.job = Some(Job::spawn(jobs, || {
                library::list(&library::directory()).map(Completed::Listed)
            }));
        } else if self.saves.is_empty() {
            while let Some(entry) = self.wanted.pop_front() {
                if self.thumbnails.iter().any(|c| c.is(&entry)) {
                    continue;
                }
                self.job = Some(Job::spawn(jobs, move || {
                    let image = library::thumbnail(&entry);
                    Ok(Completed::Thumbnail(entry, image))
                }));
                break;
            }
        }
    }
}

impl Game {
    /// Advance the library one step and apply what finished.
    pub fn poll_schematic_library(&mut self) {
        let browsing = self.creative_mode() || self.schematic_choice_open();
        match self.schematic_library.poll(&self.jobs, browsing) {
            Some(LibraryEvent::Loaded(result)) if self.creative_mode() => match result {
                Ok(schematic) => {
                    self.world_tools.cancel_all();
                    self.schematic_preview.begin_paste(schematic);
                    self.paste_preview_ready = true;
                }
                Err(error) => self.notice = error,
            },
            // A paste preview never goes up outside creative mode.
            Some(LibraryEvent::Loaded(_)) => self.schematic_library.set_previewed(None),
            Some(LibraryEvent::Changed) => self.notice.clear(),
            Some(LibraryEvent::Failed(error)) => self.notice = error,
            None => {}
        }
    }

    /// Load library entry `index` to paste it; the preview goes up when the
    /// archive has been read.
    pub fn begin_schematic_paste(&mut self, index: usize) {
        let Some(entry) = self.schematic_library.entries().get(index) else {
            return;
        };
        let path = entry.path.clone();
        self.world_tools.selection.cancel_extrusion();
        self.paste_preview_ready = false;
        self.schematic_library.request_load(path);
        self.notice.clear();
    }

    /// Whether a requested paste preview went up since this last asked.
    pub fn take_paste_preview_ready(&mut self) -> bool {
        std::mem::take(&mut self.paste_preview_ready)
    }

    /// Forget a paste that was asked for but is not up yet.
    pub fn cancel_pending_paste(&mut self) {
        self.paste_preview_ready = false;
        self.schematic_library.cancel_load();
    }

    /// Delete `entry`, taking down a paste preview that came from it.
    pub fn delete_schematic(&mut self, entry: library::Entry) {
        if self.schematic_library.previewed() == Some(&entry.path) {
            self.cancel_world_tools();
        }
        self.schematic_library.delete(entry.path);
        self.notice.clear();
    }
}
