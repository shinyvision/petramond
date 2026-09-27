//! What each row of the content list says: the packs this game has (loaded
//! or refused, shipped or installed), the site's listing, and the queue and
//! pending changes folded into one row state with its copy.
//!
//! A row's state is two orthogonal facts plus a job: whether it loaded, what
//! the catalogue says about it, and what the queue is doing. Precedence: a
//! job, then a pending change, then an available update (an update may be the
//! fix for a refusal), then a refusal, then the plain catalogue status.

use std::time::Instant;

use petramond::content::{InstallRecord, Kind, ListingRow, Tier};

use crate::app::content::{ContentSession, Pending, PendingKind, Phase};

/// A pack this game has, loaded or refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct Local {
    /// Stable key: the pack id, or the directory name when it has none.
    pub(in crate::app) key: String,
    /// The directory name (what a removal is filed under; an install is
    /// filed under the pack id).
    pub(in crate::app) dir: String,
    pub(in crate::app) id: Option<String>,
    pub(in crate::app) name: String,
    pub(in crate::app) summary: String,
    pub(in crate::app) description: String,
    pub(in crate::app) version: Option<String>,
    pub(in crate::app) tier: Tier,
    /// Why discovery refused it; `None` = loaded.
    pub(in crate::app) refusal: Option<String>,
    /// Its install record, when valid.
    pub(in crate::app) record: Option<InstallRecord>,
    pub(in crate::app) touches_world: bool,
    pub(in crate::app) dependencies: Vec<String>,
}

impl Local {
    pub(in crate::app) fn shipped(&self) -> bool {
        self.tier == Tier::ContentPack
    }
}

/// Every pack discovery found, admitted or not.
pub(in crate::app) fn locals_from_discovery(dirs: &petramond::content::Dirs) -> Vec<Local> {
    use petramond::content::records;
    use petramond_world::assets::{self, Pack, PackHeader, PackOrigin};
    let record = |id: Option<&str>| records::load(dirs, id?).filter(|r| records::valid(dirs, r));
    let local = |pack: &Pack, refusal: Option<String>| {
        let dir = crate::app::content::dir_name(&pack.dir);
        Local {
            key: pack.id.clone().unwrap_or_else(|| dir.clone()),
            dir,
            id: pack.id.clone(),
            name: pack.name.clone(),
            summary: pack
                .summary
                .clone()
                .unwrap_or_else(|| pack.description.clone()),
            description: pack.description.clone(),
            version: pack.version.clone(),
            tier: petramond::content::tier_in(pack, dirs),
            refusal,
            record: match pack.origin {
                PackOrigin::Installed => record(pack.id.as_deref()),
                PackOrigin::Shipped => None,
            },
            touches_world: pack.touches_world,
            dependencies: pack.dependencies.clone(),
        }
    };
    let loaded = assets::packs().iter().map(|pack| local(pack, None));
    let refused = assets::refused().iter().map(|refused| {
        let header = refused.header.clone().unwrap_or_else(|| PackHeader {
            name: crate::app::content::dir_name(&refused.dir),
            id: None,
            version: None,
            description: String::new(),
            summary: None,
            icon: None,
            dependencies: Vec::new(),
            touches_world: true,
        });
        let pack = Pack {
            dir: refused.dir.clone(),
            header,
            origin: refused.origin,
            wasm: None,
            client_wasm: None,
            integrations: Vec::new(),
            launch: None,
        };
        local(&pack, Some(refused.reason.clone()))
    });
    loaded.chain(refused).collect()
}

/// One place in the list, fixed between rebuilds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) enum Slot {
    /// The Installed tab has nothing to show.
    NothingInstalled,
    /// The id filter's banner ("Show all").
    FilterBanner,
    /// Installs are off (a `PETRAMOND_MODS` override).
    InstallsOff,
    /// What the listing request came to, when that is not rows.
    ListingStatus,
    /// Filtered ids petramond.com does not have.
    NotListed,
    /// A pack this game has, by index into the view's locals.
    Local(usize),
    /// A listing row, by pack id.
    Listed(String),
}

/// What a message stamp's button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum MessageAction {
    SignIn,
    CancelLoad,
    Retry,
    ShowAll,
    Browse,
}

impl MessageAction {
    pub(in crate::app) fn text(self) -> &'static str {
        match self {
            MessageAction::SignIn => "Sign In",
            MessageAction::CancelLoad => "Cancel",
            MessageAction::Retry => "Retry",
            MessageAction::ShowAll => "Show all",
            MessageAction::Browse => "Browse",
        }
    }

    pub(in crate::app) fn tip(self) -> &'static str {
        match self {
            MessageAction::SignIn => "Sign in to petramond.com",
            MessageAction::CancelLoad => "Cancel (F5 tries again)",
            MessageAction::Retry => "Retry (F5)",
            MessageAction::ShowAll => "Show every pack",
            MessageAction::Browse => "Find addons and mods on petramond.com",
        }
    }
}

/// A message stamp's content.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) struct Message {
    pub(in crate::app) text: String,
    pub(in crate::app) palette: &'static str,
    pub(in crate::app) busy: bool,
    pub(in crate::app) action: Option<MessageAction>,
}

impl Message {
    fn new(text: impl Into<String>, palette: &'static str) -> Self {
        Self {
            text: text.into(),
            palette,
            busy: false,
            action: None,
        }
    }

    fn with(mut self, action: MessageAction) -> Self {
        self.action = Some(action);
        self
    }
}

pub(in crate::app) const MUTED: &str = "text_muted";
pub(in crate::app) const ACCENT: &str = "accent";
pub(in crate::app) const WARN: &str = "warn";
pub(in crate::app) const DANGER: &str = "danger";

/// Names in tooltips and confirm copy are cut to this many characters.
pub(in crate::app) const NAME_CAP: usize = 32;

/// `text` cut to `max` characters, with an ellipsis when cut.
pub(in crate::app) fn cap(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// At most three names, then "and N more".
pub(in crate::app) fn name_list(names: &[String]) -> String {
    let shown: Vec<String> = names.iter().take(3).map(|n| cap(n, NAME_CAP)).collect();
    let mut out = match shown.len() {
        0 => String::new(),
        1 => shown[0].clone(),
        _ if names.len() > 3 => shown.join(", "),
        n => format!("{} and {}", shown[..n - 1].join(", "), shown[n - 1]),
    };
    if names.len() > 3 {
        out.push_str(&format!(" and {} more", names.len() - 3));
    }
    out
}

/// A size as the rows show it: KB below a megabyte, MB with one decimal
/// above, so the longest progress line still fits the smallest viewport.
pub(in crate::app) fn size(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if bytes < MB {
        format!("{} KB", (bytes / 1024).max(1))
    } else {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    }
}

/// `done` of `total` in `total`'s unit: "198/470 KB", "1.9/20.0 MB".
fn size_of(done: u64, total: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if total < MB {
        format!("{}/{}", done / 1024, size(total))
    } else {
        format!("{:.1}/{}", done as f64 / MB as f64, size(total))
    }
}

fn version_badge(version: &str) -> String {
    format!("v{}", cap(version, 12))
}

/// The listing-state message, when the Browse tab has one.
pub(in crate::app) fn listing_message(
    session: &ContentSession,
    available: usize,
) -> Option<Message> {
    use crate::app::content::ListingState as L;
    Some(match &session.listing {
        L::SignedOut { expired: false } => {
            Message::new("Sign in to petramond.com to get addons and mods.", MUTED)
                .with(MessageAction::SignIn)
        }
        L::SignedOut { expired: true } => {
            Message::new("Your sign-in expired. Sign in again.", WARN).with(MessageAction::SignIn)
        }
        L::Loading if !session.rows_known => Message {
            busy: true,
            ..Message::new("Loading…", MUTED).with(MessageAction::CancelLoad)
        },
        L::Failed(why) => Message::new(format!("Could not reach petramond.com.\n{why}"), DANGER)
            .with(MessageAction::Retry),
        L::Cancelled => Message::new("Cancelled.", MUTED).with(MessageAction::Retry),
        L::Ready | L::Loading if available > 0 => return None,
        L::Ready | L::Loading => Message::new("Nothing on petramond.com yet.", MUTED),
    })
}

pub(in crate::app) fn nothing_installed_message() -> Message {
    Message::new("No addons or mods installed yet.", MUTED).with(MessageAction::Browse)
}

pub(in crate::app) fn installs_off_message() -> Message {
    Message::new("Installing is off while PETRAMOND_MODS is set.", WARN)
}

pub(in crate::app) fn filter_message() -> Message {
    Message::new("Showing what this world needs.", MUTED).with(MessageAction::ShowAll)
}

pub(in crate::app) fn not_listed_message(ids: &[String]) -> Message {
    Message::new(format!("Not on petramond.com: {}", name_list(ids)), MUTED)
}

/// The entry's action column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) enum Action {
    None,
    Get,
    Update,
    Retry,
    Replace,
    Undo,
    /// A job runs: the gauge, at this fraction.
    Busy(f32),
}

impl Action {
    /// Enter's action on the selected row: never an undo.
    pub(in crate::app) fn is_primary(self) -> bool {
        matches!(
            self,
            Action::Get | Action::Update | Action::Retry | Action::Replace
        )
    }
}

/// Everything one entry stamp shows.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) struct Entry {
    pub(in crate::app) key: String,
    pub(in crate::app) dir: String,
    pub(in crate::app) name: String,
    pub(in crate::app) is_addon: bool,
    pub(in crate::app) version: Option<String>,
    pub(in crate::app) summary: String,
    pub(in crate::app) description: String,
    /// What installing it means for worlds, once it is staged.
    pub(in crate::app) world_note: Option<&'static str>,
    pub(in crate::app) detail: String,
    pub(in crate::app) detail_palette: &'static str,
    pub(in crate::app) detail_tip: String,
    pub(in crate::app) action: Action,
    pub(in crate::app) can_delete: bool,
    /// The row's listing row, when it has one (Get / Update / Replace).
    pub(in crate::app) listed: Option<ListingRow>,
    /// The action button's tooltip (without a key hint).
    pub(in crate::app) action_tip: String,
    pub(in crate::app) icon: Option<String>,
    pub(in crate::app) touches_world: bool,
}

pub(in crate::app) const WORLD_NOTE_TOUCHES: &str =
    "New worlds use it. Turn it on for an existing world in World Settings › Mods.";
pub(in crate::app) const WORLD_NOTE_PRESENTATION: &str = "Ready in every world after a restart.";

/// A running job's detail line, its tooltip, and the gauge's fraction.
pub(in crate::app) fn job_detail(
    phase: Phase,
    (done, total): (u64, u64),
    now: Instant,
) -> (String, &'static str, String, f32) {
    let fraction = if total == 0 {
        0.0
    } else {
        done as f32 / total as f32
    };
    match phase {
        Phase::Queued => (
            "Queued".into(),
            MUTED,
            "Waiting for the download before it".into(),
            0.0,
        ),
        Phase::Downloading => (
            format!(
                "{}% · {}",
                (fraction * 100.0).floor() as u32,
                size_of(done, total)
            ),
            MUTED,
            format!("Downloading: {} of {}", size(done), size(total)),
            fraction,
        ),
        Phase::Waiting { until } => (
            format!(
                "Retrying in {} s",
                until.saturating_duration_since(now).as_secs_f32().ceil() as u64
            ),
            WARN,
            "petramond.com is busy; the download retries by itself".into(),
            fraction,
        ),
        Phase::Checking => (
            "Checking…".into(),
            MUTED,
            "Checking and unpacking the download".into(),
            1.0,
        ),
    }
}

impl Entry {
    fn from_local(local: &Local) -> Self {
        Self {
            key: local.key.clone(),
            dir: local.dir.clone(),
            name: local.name.clone(),
            is_addon: local.tier == Tier::Addon,
            version: local.version.clone(),
            summary: local.summary.clone(),
            description: local.description.clone(),
            world_note: None,
            detail: String::new(),
            detail_palette: MUTED,
            detail_tip: String::new(),
            action: Action::None,
            can_delete: false,
            listed: None,
            action_tip: String::new(),
            icon: None,
            touches_world: local.touches_world,
        }
    }

    fn from_listing(row: &ListingRow) -> Self {
        Self {
            key: row.mod_id.clone(),
            dir: row.mod_id.clone(),
            name: row.name.clone(),
            is_addon: row.kind == Kind::Addon,
            version: Some(row.version.clone()).filter(|v| !v.is_empty()),
            summary: row.summary.clone(),
            description: row.description.clone(),
            world_note: None,
            detail: size(row.byte_size),
            detail_palette: MUTED,
            detail_tip: format!(
                "Download size: {} ({} bytes)",
                size(row.byte_size),
                row.byte_size
            ),
            action: Action::Get,
            can_delete: false,
            listed: Some(row.clone()),
            action_tip: format!("Get {}", cap(&row.name, NAME_CAP)),
            icon: None,
            touches_world: true,
        }
    }

    fn set(&mut self, detail: impl Into<String>, palette: &'static str, tip: impl Into<String>) {
        self.detail = detail.into();
        self.detail_palette = palette;
        self.detail_tip = tip.into();
    }
}

/// A pack this game has, as its row shows it now.
pub(in crate::app) fn local_entry(local: &Local, session: &ContentSession, now: Instant) -> Entry {
    let mut entry = Entry::from_local(local);
    let name = cap(&local.name, NAME_CAP);
    // Content packs are part of Petramond: never listed, never removable.
    if local.shipped() {
        return entry;
    }
    entry.can_delete = true;
    let listed = local
        .id
        .as_deref()
        .and_then(|id| session.listed(id))
        .cloned();
    let from_site = local.record.as_ref().filter(|r| r.content_id.is_some());
    if overlay(&mut entry, local.id.as_deref(), session, now) {
        // A pending install over a pack already here is an update.
        if let (
            Action::Undo,
            Some(Pending {
                change: PendingKind::Install { version, .. },
                ..
            }),
        ) = (entry.action, session.pending_of(&local.dir, &local.key))
        {
            entry.world_note = None;
            entry.set(
                "Updates on restart",
                ACCENT,
                format!("Updates to {} when you restart", version_badge(version)),
            );
            entry.action_tip = match &local.version {
                Some(old) => format!("Stay on {}", version_badge(old)),
                None => format!("Don't update {name}"),
            };
        }
        if entry.action == Action::Retry {
            entry.listed = listed;
        }
        return entry;
    }
    let update = from_site
        .zip(listed.as_ref())
        .filter(|(record, row)| record.archive_sha256 != row.sha256)
        .map(|(_, row)| row.clone());
    if let Some(row) = update {
        let new = version_badge(&row.version);
        match &local.refusal {
            None => entry.set(
                format!("{} · {new}", size(row.byte_size)),
                ACCENT,
                format!("Update to {new} ({})", size(row.byte_size)),
            ),
            Some(why) => entry.set(
                "Not loaded · update",
                DANGER,
                format!("Not loaded: {why}. {new} is on petramond.com and may fix it."),
            ),
        }
        entry.action = Action::Update;
        entry.action_tip = format!("Update {name} to {new}");
        entry.listed = Some(row);
        return entry;
    }
    if let Some(why) = &local.refusal {
        entry.set(format!("Not loaded: {why}"), DANGER, why.clone());
        return entry;
    }
    match &local.record {
        Some(record) if record.content_id.is_none() => entry.set(
            "Installed locally",
            MUTED,
            "Installed by make addons; never updated from petramond.com",
        ),
        Some(_) if session.listing_trusted() && listed.is_none() => entry.set(
            "No longer listed",
            MUTED,
            "Installed. No longer on petramond.com, so it gets no updates.",
        ),
        Some(record) => entry.set(
            format!("Installed · {}", size(record.archive_bytes)),
            MUTED,
            "",
        ),
        None => {
            entry.set("Installed by hand", MUTED, "");
            if let Some(row) = listed {
                entry.detail_tip = format!(
                    "petramond.com has {} {}",
                    cap(&row.name, NAME_CAP),
                    version_badge(&row.version)
                );
                entry.action = Action::Replace;
                entry.action_tip = format!("Replace your copy of {name}");
                entry.listed = Some(row);
            }
        }
    }
    entry
}

/// A listing row not installed here, as its row shows it now.
pub(in crate::app) fn listed_entry(
    row: &ListingRow,
    session: &ContentSession,
    now: Instant,
) -> Entry {
    let mut entry = Entry::from_listing(row);
    overlay(&mut entry, Some(&row.mod_id), session, now);
    entry
}

/// Lay the row's job, pending change or failure over it, when it has one.
fn overlay(entry: &mut Entry, id: Option<&str>, session: &ContentSession, now: Instant) -> bool {
    let name = cap(&entry.name, NAME_CAP);
    if let Some(job) = id.and_then(|id| session.jobs.get(id)) {
        let (detail, palette, tip, fraction) = job_detail(job.phase, job.bytes(), now);
        entry.set(detail, palette, tip);
        entry.action = Action::Busy(fraction);
        entry.can_delete = false;
        return true;
    }
    if let Some(pending) = session.pending_of(&entry.dir, &entry.key) {
        entry.action = Action::Undo;
        entry.can_delete = false;
        match &pending.change {
            PendingKind::Install { touches_world, .. } => {
                entry.set(
                    "Installs on restart",
                    ACCENT,
                    "Installs when you restart Petramond",
                );
                entry.action_tip = format!("Don't install {name}");
                entry.touches_world = *touches_world;
                entry.world_note = Some(if *touches_world {
                    WORLD_NOTE_TOUCHES
                } else {
                    WORLD_NOTE_PRESENTATION
                });
            }
            PendingKind::Remove => {
                entry.set(
                    "Removed on restart",
                    WARN,
                    "Removed when you restart Petramond",
                );
                entry.action_tip = format!("Keep {name}");
            }
        }
        return true;
    }
    if let Some(why) = id.and_then(|id| session.failed.get(id)) {
        entry.set(format!("Failed: {why}"), DANGER, why.clone());
        entry.action = Action::Retry;
        entry.action_tip = format!("Try {name} again");
        return true;
    }
    false
}

/// What Restart now applies, naming at most three packs. `name_of` names
/// the pack installed under a pending change's folder name or pack id.
pub(in crate::app) fn restart_tip(
    session: &ContentSession,
    name_of: impl Fn(&str) -> Option<String>,
) -> String {
    let phrases: Vec<String> = session
        .pending
        .iter()
        .map(|p| {
            let name = name_of(&p.dir);
            match &p.change {
                PendingKind::Install { name: listed, .. } => match name {
                    Some(installed) => format!("update {}", cap(&installed, NAME_CAP)),
                    None => format!("install {}", cap(listed, NAME_CAP)),
                },
                PendingKind::Remove => {
                    format!("remove {}", cap(&name.unwrap_or(p.dir.clone()), NAME_CAP))
                }
            }
        })
        .collect();
    if phrases.is_empty() {
        return String::new();
    }
    let shown = &phrases[..phrases.len().min(3)];
    let mut tip = format!("Restart to {}", shown.join(", "));
    if phrases.len() > 3 {
        tip.push_str(&format!(" and {} more", phrases.len() - 3));
    }
    tip
}
