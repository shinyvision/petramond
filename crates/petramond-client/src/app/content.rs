//! The content library's state the app keeps for the whole process: the
//! listing petramond.com last gave, the download queue, the icons, what waits
//! for a restart, what the startup apply did (shown once — a release build
//! may have no console), and the start route a relaunch came back with.
//!
//! It lives on the App, not on the browser screen: downloads outlive the
//! screen, and the title shows their progress. Every network call runs on a
//! worker thread under a generation guard; nothing on the frame speaks HTTP.

mod icons;
mod jobs;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use petramond::content::install::{self, Op};
use petramond::content::{ApplyReport, Dirs, ListingRow};
use petramond::service::ServiceError;

pub(super) use icons::{dir_name, pack_icon_name, pack_icons, site_icon_name};
pub(super) use jobs::Phase;

use super::shell_docs::ContentView;
use super::{App, AppScreen, ExitKind};

/// Where a relaunch asked to land (`PETRAMOND_START`), honoured once and
/// never passed on.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct StartRoute {
    pub screen: String,
    /// Where that screen's Back leads, when not the title: `connect:<addr>`
    /// or `world:<save dir>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back: Option<String>,
}

impl StartRoute {
    /// The route this process was started with, taken out of the environment
    /// so nothing this process starts inherits it.
    pub fn take_from_env() -> Option<Self> {
        if cfg!(test) {
            return None;
        }
        let raw = std::env::var("PETRAMOND_START").ok()?;
        std::env::remove_var("PETRAMOND_START");
        match serde_json::from_str(&raw) {
            Ok(route) => Some(route),
            Err(e) => {
                log::warn!("PETRAMOND_START is not a start route ({e}): {raw}");
                None
            }
        }
    }

    /// The browser, with Back leading to `back`.
    pub(super) fn content(back: Option<String>) -> Self {
        Self {
            screen: "content".to_owned(),
            back,
        }
    }

    pub(super) fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

type ListingAnswer = Result<Vec<ListingRow>, ServiceError>;

/// What the listing request last came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ListingState {
    /// Nobody is signed in (or the sign-in just expired): the listing needs
    /// a bearer.
    SignedOut {
        expired: bool,
    },
    Loading,
    Ready,
    Failed(String),
    /// The player abandoned the request.
    Cancelled,
}

/// A change staged for the next start, as the browser shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Pending {
    pub(super) dir: String,
    pub(super) change: PendingKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PendingKind {
    Install {
        name: String,
        version: String,
        /// Read from the staged files, which admission already accepted.
        touches_world: bool,
        dependencies: Vec<String>,
    },
    Remove,
}

pub(super) struct ContentSession {
    pub(super) dirs: Dirs,
    /// The shipped packs' ids: never installed over, never listed.
    pub(super) shipped: BTreeSet<String>,
    /// Whether discovery reads the installed root (not under a
    /// `PETRAMOND_MODS` override): an install that could never load is never
    /// staged.
    pub(super) installs_enabled: bool,
    pub(super) listing: ListingState,
    /// The last good listing, kept for the process so a reopen draws at once.
    pub(super) rows: Vec<ListingRow>,
    /// `rows` came from a successful listing (some time this process).
    pub(super) rows_known: bool,
    /// When the running listing request started, for the sweeping gauge.
    pub(super) listing_started: f64,
    listing_gen: u64,
    listing_rx: Option<Receiver<(u64, ListingAnswer)>>,
    pub(super) jobs: jobs::Jobs,
    /// Why a download failed, by pack id, until retried or refreshed.
    pub(super) failed: BTreeMap<String, String>,
    pub(super) pending: Vec<Pending>,
    pub(super) icons: icons::Icons,
    /// Quit or restart the moment the queue drains with every job staged.
    pub(super) exit_when_idle: Option<ExitKind>,
    /// The open browser's own state (`None` while it is closed).
    pub(super) view: Option<ContentView>,
    /// Network workers run only outside tests: the suite never touches
    /// petramond.com.
    network: bool,
}

impl Default for ContentSession {
    fn default() -> Self {
        // The suite never reads the player's own content state.
        let dirs = if cfg!(test) {
            let scratch = std::env::temp_dir().join("petramond-content-unused");
            Dirs {
                mods: scratch.join("mods"),
                content: scratch.join("content"),
            }
        } else {
            Dirs::installed()
        };
        Self::new(dirs, !cfg!(test))
    }
}

impl ContentSession {
    pub(super) fn new(dirs: Dirs, network: bool) -> Self {
        let mut session = Self {
            dirs,
            shipped: BTreeSet::new(),
            installs_enabled: false,
            listing: ListingState::SignedOut { expired: false },
            rows: Vec::new(),
            rows_known: false,
            listing_started: 0.0,
            listing_gen: 0,
            listing_rx: None,
            jobs: jobs::Jobs::new(network),
            failed: BTreeMap::new(),
            pending: Vec::new(),
            icons: icons::Icons::new(network),
            exit_when_idle: None,
            view: None,
            network,
        };
        session.reload_pending();
        session
    }

    /// Re-read what waits for a restart. Called where it can have changed:
    /// the browser opening, a job staging, an undo or a delete.
    pub(super) fn reload_pending(&mut self) {
        self.pending = install::pending(&self.dirs)
            .into_iter()
            .map(|change| Pending {
                dir: change.dir.clone(),
                change: match change.op {
                    Op::Install { staged, record } => {
                        let header =
                            petramond_world::assets::admit_pack_dir(&self.dirs.mods.join(&staged))
                                .ok();
                        PendingKind::Install {
                            name: record.name.clone(),
                            version: record.version.clone(),
                            touches_world: header.as_ref().is_none_or(|h| h.touches_world),
                            dependencies: header.map(|h| h.dependencies).unwrap_or_default(),
                        }
                    }
                    Op::Remove => PendingKind::Remove,
                },
            })
            .collect();
    }

    pub(super) fn pending_for(&self, dir: &str) -> Option<&Pending> {
        self.pending.iter().find(|p| p.dir == dir)
    }

    /// The pending change a row stands for: the one filed under its folder,
    /// or an install filed under its pack id `key` (an install lands at
    /// `mods/<id>` over whichever folder holds that id, so a folder named
    /// otherwise is still the pack it replaces).
    pub(super) fn pending_of(&self, dir: &str, key: &str) -> Option<&Pending> {
        self.pending_for(dir).or_else(|| {
            self.pending_for(key)
                .filter(|p| matches!(p.change, PendingKind::Install { .. }))
        })
    }

    /// The listing's row for `mod_id`, when the listing can be trusted to
    /// say what petramond.com has (a successful fetch, shown or refreshing).
    pub(super) fn listed(&self, mod_id: &str) -> Option<&ListingRow> {
        self.listing_trusted()
            .then(|| self.rows.iter().find(|r| r.mod_id == mod_id))
            .flatten()
    }

    pub(super) fn listing_trusted(&self) -> bool {
        self.rows_known && matches!(self.listing, ListingState::Ready | ListingState::Loading)
    }

    /// Start fetching the listing (abandoning any request already running).
    pub(super) fn fetch_listing(&mut self, now: f64) {
        self.listing_gen += 1;
        self.listing_rx = None;
        self.listing = ListingState::Loading;
        self.listing_started = now;
        if !self.network {
            return;
        }
        let gen = self.listing_gen;
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("petramond-content-listing".to_owned())
            .spawn(move || {
                let _ = tx.send((gen, petramond::content::api::listing()));
            });
        match spawned {
            Ok(_) => self.listing_rx = Some(rx),
            Err(e) => self.listing = ListingState::Failed(format!("could not start: {e}")),
        }
    }

    /// Abandon the running listing request: its answer becomes stale.
    pub(super) fn cancel_listing(&mut self) {
        if self.listing == ListingState::Loading {
            self.listing_gen += 1;
            self.listing_rx = None;
            self.listing = ListingState::Cancelled;
        }
    }

    /// A listing answer, however it arrived (the listing worker, or a
    /// download that re-fetched it).
    pub(super) fn listing_arrived(&mut self, answer: Result<Vec<ListingRow>, ServiceError>) {
        match answer {
            Ok(rows) => {
                let rows: Vec<ListingRow> = rows
                    .into_iter()
                    .filter(|r| !self.shipped.contains(&r.mod_id))
                    .collect();
                self.rows = rows;
                self.rows_known = true;
                self.listing = ListingState::Ready;
                self.icons.want(&self.rows);
                if let Some(view) = self.view.as_mut() {
                    view.listing_landed(&self.rows);
                }
            }
            Err(ServiceError::SignInRequired(_)) => self.signed_out(true),
            Err(e) => self.listing = ListingState::Failed(e.message()),
        }
    }

    /// The stored sign-in is gone: the listing needs a new one, and every
    /// queued download goes back to Get.
    pub(super) fn signed_out(&mut self, expired: bool) {
        self.listing_gen += 1;
        self.listing_rx = None;
        self.listing = ListingState::SignedOut { expired };
        self.jobs.cancel_all();
        self.exit_when_idle = None;
    }

    /// Queue a download of `row`, unless it could never install.
    pub(super) fn get(&mut self, row: &ListingRow) -> Result<(), String> {
        if !self.installs_enabled {
            return Err("Installing is off while PETRAMOND_MODS is set.".to_owned());
        }
        if self.shipped.contains(&row.mod_id) {
            return Err("That id belongs to a pack that ships with Petramond.".to_owned());
        }
        if self.pending_for(&row.mod_id).is_some() || self.jobs.get(&row.mod_id).is_some() {
            return Err("A change to this pack is already waiting.".to_owned());
        }
        self.failed.remove(&row.mod_id);
        self.jobs
            .enqueue(row.clone(), self.dirs.clone(), self.shipped.clone());
        Ok(())
    }

    /// Withdraw the pending change to `dir`.
    pub(super) fn undo(&mut self, dir: &str) {
        install::undo(&self.dirs, dir);
        self.reload_pending();
    }

    /// Stage removing the installed directory `dir`.
    pub(super) fn remove(&mut self, dir: &str) -> Result<(), String> {
        let staged = install::stage_remove(&self.dirs, dir);
        self.reload_pending();
        staged
    }

    /// Drain the listing worker, the download queue and the icon worker.
    fn poll(&mut self) -> Vec<jobs::Event> {
        let mut events = Vec::new();
        if let Some(rx) = self.listing_rx.as_ref() {
            match rx.try_recv() {
                Ok((gen, answer)) => {
                    self.listing_rx = None;
                    if gen == self.listing_gen {
                        // A dead sign-in reads the same whoever found it: the
                        // app must re-read the cleared credential, or the
                        // browser sees the stale one and fetches again.
                        if matches!(answer, Err(ServiceError::SignInRequired(_))) {
                            events.push(jobs::Event::SignedOut);
                        } else {
                            self.listing_arrived(answer);
                        }
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.listing_rx = None;
                    self.listing = ListingState::Failed("the request stopped".to_owned());
                }
            }
        }
        self.icons.poll();
        events.extend(self.jobs.poll());
        events
    }
}

/// Whether an installed pack is a Petramond Addon, by the one classifier.
/// Records only change at startup, so each pack is classified once.
pub(super) fn is_addon(pack: &petramond_world::assets::Pack) -> bool {
    use std::sync::OnceLock;
    static ADDONS: OnceLock<BTreeSet<std::path::PathBuf>> = OnceLock::new();
    ADDONS
        .get_or_init(|| {
            petramond_world::assets::packs()
                .iter()
                .filter(|p| petramond::content::tier(p) == petramond::content::Tier::Addon)
                .map(|p| p.dir.clone())
                .collect()
        })
        .contains(&pack.dir)
}

impl App {
    /// What the startup apply of pending content changes did.
    pub fn set_content_report(&mut self, report: ApplyReport) {
        self.content_report = report;
    }

    pub fn content_report(&self) -> &ApplyReport {
        &self.content_report
    }

    /// The start route this launch was given, once.
    pub fn take_start_route(&mut self) -> Option<StartRoute> {
        self.start_route.take()
    }

    /// Every frame, whatever the screen: downloads run on without the
    /// browser, and the title shows their progress.
    pub(super) fn poll_content(&mut self) {
        if let Some(route) = self.take_start_route() {
            if route.screen == "content" && self.session.is_none() {
                self.open_content(route.back, None);
            }
        }
        let mut staged = false;
        for event in self.content.poll() {
            match event {
                jobs::Event::Staged(id) => {
                    self.content.failed.remove(&id);
                    staged = true;
                }
                jobs::Event::Failed(id, why) => {
                    self.content.failed.insert(id, why);
                    self.content.exit_when_idle = None;
                }
                jobs::Event::Gone(id) => {
                    self.content
                        .failed
                        .insert(id, "No longer on petramond.com".to_owned());
                    self.content.exit_when_idle = None;
                    // Merged, so the failed row and its reason stay put.
                    let now = self.now();
                    self.content.fetch_listing(now);
                }
                jobs::Event::Listing(rows) => self.content.listing_arrived(Ok(rows)),
                jobs::Event::SignedOut => {
                    self.content.signed_out(true);
                    self.refresh_account_view();
                }
            }
        }
        if staged {
            self.content.reload_pending();
        }
        if self.content.jobs.is_empty() {
            if let Some(kind) = self.content.exit_when_idle.take() {
                self.exit_now(kind);
            }
        }
    }

    /// Open the browser. `back` is where its Back leads (see
    /// [`StartRoute::back`]); `filter` shows only those pack ids.
    pub(super) fn open_content(&mut self, back: Option<String>, filter: Option<Vec<String>>) {
        if !cfg!(test) {
            self.refresh_account_view();
            self.content.installs_enabled = petramond_world::assets::installed_root_active();
            if self.content.shipped.is_empty() {
                self.content.shipped = petramond_world::assets::shipped_pack_ids();
            }
        }
        self.content.reload_pending();
        let now = self.now();
        let signed_in = self.shell.account.saved.is_some();
        if !signed_in {
            self.content.signed_out(false);
        } else if !matches!(self.content.listing, ListingState::Loading) {
            self.content.fetch_listing(now);
        }
        // The last start's failures show once: here as failed rows.
        for (dir, why) in std::mem::take(&mut self.content_report.failed) {
            self.content.failed.insert(dir, why);
        }
        let mut view = ContentView::new(back, filter, !self.content.rows_known);
        view.locals = super::shell_docs::content_locals(&self.content.dirs);
        self.content.view = Some(view);
        self.content.icons.want(&self.content.rows.clone());
        self.set_screen(AppScreen::Content);
    }

    /// Leave the browser for where it was opened from. Downloads keep going.
    pub(super) fn close_content(&mut self) {
        let back = self.content.view.take().and_then(|v| v.back);
        self.content.cancel_listing();
        match back.as_deref().and_then(|b| b.split_once(':')) {
            Some(("connect", addr)) => {
                self.shell.connect.addr = addr.to_owned();
                self.reopen_connect_server();
            }
            Some(("world", dir)) => {
                self.shell.refresh_worlds();
                let index = self.shell.worlds().iter().position(|w| w.dir_name == dir);
                self.shell.select_world(index);
                self.set_screen(AppScreen::WorldSelect);
            }
            _ => self.set_screen(AppScreen::Title),
        }
    }

    /// Whether any download is queued or running.
    pub(super) fn content_jobs_running(&self) -> bool {
        !self.content.jobs.is_empty()
    }
}

#[cfg(test)]
impl ContentSession {
    /// A session over scratch directories, with no network and nothing
    /// shipped: tests arrange everything themselves.
    pub(super) fn for_test(dirs: Dirs) -> Self {
        let mut session = Self::new(dirs, false);
        session.installs_enabled = true;
        session
    }
}
