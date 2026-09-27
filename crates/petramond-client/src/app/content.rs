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

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct StartRoute {
    pub screen: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back: Option<String>,
}

impl StartRoute {
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

    #[cfg(test)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ListingState {
    SignedOut { expired: bool },
    Loading,
    Ready,
    Failed(String),
    Cancelled,
}

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
        touches_world: bool,
        dependencies: Vec<String>,
    },
    Remove,
}

pub(super) struct ContentSession {
    pub(super) dirs: Dirs,
    pub(super) shipped: BTreeSet<String>,
    pub(super) installs_enabled: bool,
    pub(super) listing: ListingState,
    pub(super) rows: Vec<ListingRow>,
    pub(super) rows_known: bool,
    pub(super) listing_started: f64,
    listing_gen: u64,
    listing_rx: Option<Receiver<(u64, ListingAnswer)>>,
    pub(super) jobs: jobs::Jobs,
    pub(super) failed: BTreeMap<String, String>,
    pub(super) pending: Vec<Pending>,
    pub(super) icons: icons::Icons,
    pub(super) exit_when_idle: Option<ExitKind>,
    pub(super) view: Option<ContentView>,
    network: bool,
}

impl Default for ContentSession {
    fn default() -> Self {
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

    pub(super) fn pending_of(&self, dir: &str, key: &str) -> Option<&Pending> {
        self.pending_for(dir).or_else(|| {
            self.pending_for(key)
                .filter(|p| matches!(p.change, PendingKind::Install { .. }))
        })
    }

    pub(super) fn listed(&self, mod_id: &str) -> Option<&ListingRow> {
        self.listing_trusted()
            .then(|| self.rows.iter().find(|r| r.mod_id == mod_id))
            .flatten()
    }

    pub(super) fn listing_trusted(&self) -> bool {
        self.rows_known && matches!(self.listing, ListingState::Ready | ListingState::Loading)
    }

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

    pub(super) fn cancel_listing(&mut self) {
        if self.listing == ListingState::Loading {
            self.listing_gen += 1;
            self.listing_rx = None;
            self.listing = ListingState::Cancelled;
        }
    }

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

    pub(super) fn signed_out(&mut self, expired: bool) {
        self.listing_gen += 1;
        self.listing_rx = None;
        self.listing = ListingState::SignedOut { expired };
        self.jobs.cancel_all();
        self.exit_when_idle = None;
    }

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

    pub(super) fn undo(&mut self, dir: &str) {
        install::undo(&self.dirs, dir);
        self.reload_pending();
    }

    pub(super) fn remove(&mut self, dir: &str) -> Result<(), String> {
        let staged = install::stage_remove(&self.dirs, dir);
        self.reload_pending();
        staged
    }

    fn poll(&mut self) -> Vec<jobs::Event> {
        let mut events = Vec::new();
        if let Some(rx) = self.listing_rx.as_ref() {
            match rx.try_recv() {
                Ok((gen, answer)) => {
                    self.listing_rx = None;
                    if gen == self.listing_gen {
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

pub(super) fn is_addon(pack: &petramond_world::assets::Pack) -> bool {
    petramond::content::tier(pack) == petramond::content::Tier::Addon
}

impl App {
    pub(super) fn request_content_apply(&mut self) {
        if self.session.is_none() && self.launched.is_none() && !self.content.pending.is_empty() {
            self.content_apply_requested = true;
        }
    }

    pub fn take_content_apply_requested(&mut self) -> bool {
        std::mem::take(&mut self.content_apply_requested)
    }

    pub fn content_applied(&mut self, report: ApplyReport) {
        let changed = !report.applied.is_empty();
        self.content_report = report;
        self.content.reload_pending();
        for (dir, why) in &self.content_report.failed {
            self.content.failed.insert(dir.clone(), why.clone());
        }
        if changed {
            self.retained_section_cache = None;
            if let Some(view) = self.content.view.as_mut() {
                view.locals = super::shell_docs::content_locals(&self.content.dirs);
                view.request_rebuild();
            }
            self.ui = super::ui_runtime::AppUi::new();
            self.hud_ui = super::ui_runtime::AppUi::new();
        }
    }

    pub fn set_content_report(&mut self, report: ApplyReport) {
        self.content_report = report;
    }

    pub fn content_report(&self) -> &ApplyReport {
        &self.content_report
    }

    pub fn take_start_route(&mut self) -> Option<StartRoute> {
        self.start_route.take()
    }

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
        for (dir, why) in std::mem::take(&mut self.content_report.failed) {
            self.content.failed.insert(dir, why);
        }
        let mut view = ContentView::new(back, filter, !self.content.rows_known);
        view.locals = super::shell_docs::content_locals(&self.content.dirs);
        self.content.view = Some(view);
        self.content.icons.want(&self.content.rows.clone());
        self.set_screen(AppScreen::Content);
    }

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

    pub(super) fn content_jobs_running(&self) -> bool {
        !self.content.jobs.is_empty()
    }
}

#[cfg(test)]
impl ContentSession {
    pub(super) fn for_test(dirs: Dirs) -> Self {
        let mut session = Self::new(dirs, false);
        session.installs_enabled = true;
        session
    }
}
